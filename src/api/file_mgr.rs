//! File management — upload, folders, versioning.
//! Replaces RAGFlow's file_api.py + file_commit_api.py.

use axum::{
    Json,
    extract::{Extension, Multipart, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use crate::server::{AppState, AuthContext};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub owner_id: String,
    pub parent_id: String,
    pub size: usize,
    #[serde(default)]
    pub content_hash: String,
    pub file_type: String,
    pub created_at: u64,
}

pub struct FileStore {
    files: RwLock<HashMap<String, FileRecord>>,
    pub(crate) data_dir: String,
    metadata_path: String,
    save_lock: Mutex<()>,
}

impl FileStore {
    pub fn new(data_dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let metadata_path = std::path::Path::new(data_dir).join("files.json");
        crate::persistence::restore_if_missing(&metadata_path)?;
        let files = if metadata_path.exists() {
            let data = std::fs::read_to_string(&metadata_path)?;
            serde_json::from_str::<Vec<FileRecord>>(&data).map_err(|error| {
                anyhow::anyhow!(
                    "Failed to parse file metadata '{}': {error}",
                    metadata_path.display()
                )
            })?
        } else {
            Vec::new()
        }
        .into_iter()
        .map(|file| (file.id.clone(), file))
        .collect();
        Ok(Self {
            files: RwLock::new(files),
            data_dir: data_dir.into(),
            metadata_path: metadata_path.to_string_lossy().into_owned(),
            save_lock: Mutex::new(()),
        })
    }

    pub fn list_for(&self, owner_id: &str, is_admin: bool, parent_id: &str) -> Vec<FileRecord> {
        self.files
            .read()
            .unwrap()
            .values()
            .filter(|file| {
                file.parent_id == parent_id
                    && (file.owner_id == owner_id || (is_admin && file.owner_id.is_empty()))
            })
            .cloned()
            .collect()
    }

    pub fn add(&self, f: FileRecord) -> anyhow::Result<()> {
        self.mutate(|files| {
            files.insert(f.id.clone(), f);
            Ok(())
        })
    }

    /// Insert a file while reserving a unique name for its owner and parent folder.
    pub fn add_unique(&self, mut file: FileRecord) -> anyhow::Result<FileRecord> {
        self.mutate(|files| {
            if files.contains_key(&file.id) {
                anyhow::bail!("File ID already exists");
            }
            file.name = crate::naming::duplicate_name(&file.name, |candidate| {
                files.values().any(|existing| {
                    existing.owner_id == file.owner_id
                        && existing.parent_id == file.parent_id
                        && existing.name == candidate
                })
            })?;
            files.insert(file.id.clone(), file.clone());
            Ok(file)
        })
    }

    pub fn remove(&self, id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|files| {
            let removed = files.remove(id).is_some();
            Ok((removed, removed))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, FileRecord>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_if_changed(|files| mutation(files).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, FileRecord>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut files = self.files.write().unwrap();
        let previous = files.clone();
        let (value, changed) = mutation(&mut files)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<FileRecord> = files.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *files = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist(&self, files: &[FileRecord]) -> anyhow::Result<()> {
        let data = serde_json::to_vec_pretty(&files)?;
        crate::persistence::atomic_write(std::path::Path::new(&self.metadata_path), &data)
    }
}

/// GET /api/v1/files — list files
pub async fn list_files(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": state.files.list_for(&auth.user_id, auth.is_admin, "root")
    }))
}

/// DELETE /api/v1/files/{id} — 删除文件记录 + 物理文件。
pub async fn delete_file(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(file_id): axum::extract::Path<String>,
) -> Response {
    let record = state
        .files
        .list_for(&auth.user_id, auth.is_admin, "root")
        .into_iter()
        .find(|record| record.id == file_id);
    let Some(record) = record else {
        return Json(serde_json::json!({ "code": 404, "message": "File not found" }))
            .into_response();
    };
    let extension = std::path::Path::new(&record.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let physical = std::path::Path::new(&state.files.data_dir).join(if extension.is_empty() {
        file_id.clone()
    } else {
        format!("{file_id}.{extension}")
    });
    if physical.exists() {
        let _ = std::fs::remove_file(&physical);
    }
    match state.files.remove(&file_id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        _ => Json(serde_json::json!({ "code": 404, "message": "File not found" })).into_response(),
    }
}

/// GET /api/v1/files/{id}/download — 下载文件内容。
pub async fn download_file(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(file_id): axum::extract::Path<String>,
) -> Response {
    let record = state
        .files
        .list_for(&auth.user_id, auth.is_admin, "root")
        .into_iter()
        .find(|record| record.id == file_id);
    let Some(record) = record else {
        return Json(serde_json::json!({ "code": 404, "message": "File not found" }))
            .into_response();
    };
    let extension = std::path::Path::new(&record.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let physical = std::path::Path::new(&state.files.data_dir).join(if extension.is_empty() {
        file_id.clone()
    } else {
        format!("{file_id}.{extension}")
    });
    match tokio::fs::read(&physical).await {
        Ok(bytes) => {
            let mime = crate::parser::mime_from_extension(&extension)
                .unwrap_or("application/octet-stream");
            ([(axum::http::header::CONTENT_TYPE, mime)], bytes).into_response()
        }
        Err(_) => Json(serde_json::json!({ "code": 404, "message": "File content not found" }))
            .into_response(),
    }
}

/// POST /api/v1/files/upload — upload one validated multipart file.
pub async fn upload_file(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    multipart: Multipart,
) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    let upload = match crate::server::persist_single_upload(
        multipart,
        std::path::Path::new(&state.files.data_dir),
        &id,
        state.max_upload_bytes,
    )
    .await
    {
        Ok(upload) => upload,
        Err(error) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
            )
                .into_response();
        }
    };
    let extension = std::path::Path::new(&upload.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let record = FileRecord {
        id: id.clone(),
        name: upload.name.clone(),
        owner_id: auth.user_id,
        parent_id: "root".into(),
        size: upload.size,
        content_hash: upload.content_hash.clone(),
        file_type: extension,
        created_at: now_ms(),
    };
    let record = match state.files.add_unique(record) {
        Ok(record) => record,
        Err(error) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
            )
                .into_response();
        }
    };
    upload.commit();
    (
        axum::http::StatusCode::CREATED,
        Json(serde_json::json!({ "code": 0, "data": record })),
    )
        .into_response()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str) -> FileRecord {
        FileRecord {
            id: id.into(),
            name: format!("{id}.txt"),
            owner_id: "owner-1".into(),
            parent_id: "root".into(),
            size: 1,
            content_hash: String::new(),
            file_type: "txt".into(),
            created_at: 1,
        }
    }

    #[test]
    fn failed_persistence_rolls_back_file_mutations() {
        let root =
            std::env::temp_dir().join(format!("rayrag-file-rollback-{}", uuid::Uuid::new_v4()));
        let store = FileStore::new(root.to_str().unwrap()).unwrap();
        store.add(record("file-1")).unwrap();
        let metadata_path = root.join("files.json");
        std::fs::remove_file(&metadata_path).unwrap();
        std::fs::create_dir(&metadata_path).unwrap();

        assert!(store.add(record("file-2")).is_err());
        assert_eq!(store.list_for("owner-1", false, "root").len(), 1);
        assert!(store.remove("file-1").is_err());
        assert_eq!(store.list_for("owner-1", false, "root").len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn corrupt_file_json_fails_startup_instead_of_clearing_store() {
        let root =
            std::env::temp_dir().join(format!("rayrag-file-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("files.json"), b"{not-json").unwrap();
        let error = match FileStore::new(root.to_str().unwrap()) {
            Err(error) => error,
            Ok(_) => panic!("corrupt file JSON must fail startup"),
        };
        assert!(error.to_string().contains("Failed to parse file metadata"));
        std::fs::remove_dir_all(root).ok();
    }
}
