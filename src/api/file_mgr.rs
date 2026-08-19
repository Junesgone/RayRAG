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

#[derive(Deserialize)]
pub struct CreateFolderRequest {
    pub name: String,
    #[serde(default)]
    pub parent_id: String,
}

/// POST /api/v1/files/folder — create an empty folder in the file manager.
/// Parity with RAGFlow v0.26.4 "Add file → New folder" dropdown item.
pub async fn create_folder(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateFolderRequest>,
) -> Response {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": "Folder name is required" })),
        )
            .into_response();
    }
    let parent = if body.parent_id.is_empty() { "root".to_string() } else { body.parent_id };
    let id = uuid::Uuid::new_v4().to_string();
    let record = FileRecord {
        id: id.clone(),
        name: name.clone(),
        owner_id: auth.user_id,
        parent_id: parent,
        size: 0,
        content_hash: String::new(),
        file_type: "folder".into(),
        created_at: now_ms(),
    };
    match state.files.add(record) {
        Ok(()) => (
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({ "code": 0, "data": { "id": id, "name": name } })),
        )
            .into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ============================================================================
// DocFileCache — RAGFlow `rag/svr/cache_file_svr.py` port.
//
// RAGFlow runs a dedicated worker that polls the *ongoing* document tasks
// (`TaskService.get_ongoing_doc_name()`), fetches each `(kb_id, location)`
// file binary from object storage and caches it in Redis with a short TTL
// (`12 * 60` seconds). The cache is populated *before* the parser needs the
// file, so chunking workers read from the warm cache instead of hitting
// object storage directly.
//
// This port keeps the same contract against RayRAG's local uploads dir:
//   - key      = `"{kb_id}/{location}"` (same key layout as RAGFlow Redis);
//   - `cache_ongoing`   = `cache_file_svr.main()` poll step;
//   - `get`             = Redis `exist` + read (expired entries miss);
//   - `sweep_expired`   = Redis TTL eviction (expired entries + files).
//
// Like Redis, the cache is volatile: `DocFileCache::new` starts empty and
// drops stale cache files from a previous run.
// ============================================================================

/// One cached file entry (`cache_file_svr` Redis value + TTL bookkeeping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// `"{kb_id}/{location}"` — identical key layout to the RAGFlow worker.
    pub key: String,
    /// Physical file name inside the cache directory.
    pub file_name: String,
    pub size: usize,
    /// Epoch millis when the entry was cached.
    pub cached_at: u64,
    /// Epoch millis after which the entry is considered expired (Redis TTL).
    pub expires_at: u64,
}

/// Volatile on-disk cache for ongoing-task files (`cache_file_svr` port).
pub struct DocFileCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
    cache_dir: String,
}

impl DocFileCache {
    /// Create an empty cache rooted at `cache_dir`. Stale files from a
    /// previous run are removed (the cache is volatile, like Redis).
    pub fn new(cache_dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(cache_dir)?;
        for entry in std::fs::read_dir(cache_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.file_name().to_string_lossy().starts_with("cache-")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(Self {
            entries: RwLock::new(HashMap::new()),
            cache_dir: cache_dir.into(),
        })
    }

    /// `"{kb_id}/{location}"` cache key (RAGFlow `"{}/{}".format(kb_id, loc)`).
    pub fn key(kb_id: &str, location: &str) -> String {
        format!("{kb_id}/{location}")
    }

    /// Path of the cached file if a non-expired entry exists.
    pub fn get(&self, kb_id: &str, location: &str, now: u64) -> Option<std::path::PathBuf> {
        let key = Self::key(kb_id, location);
        let entries = self.entries.read().unwrap();
        let entry = entries.get(&key)?;
        if entry.expires_at <= now {
            return None; // Redis `exist` after TTL eviction == false
        }
        let path = std::path::Path::new(&self.cache_dir).join(&entry.file_name);
        path.is_file().then_some(path)
    }

    /// `cache_file_svr.main()` poll step: for each `(kb_id, location)` of an
    /// ongoing task, cache the source file (read from `source_dir`) unless a
    /// valid cache entry already exists. Returns the number of newly cached
    /// files (the RAGFlow `CACHE: <loc>` log lines). Missing source files are
    /// skipped, mirroring the `STORAGE_IMPL.get` error path.
    pub fn cache_ongoing(
        &self,
        locations: &[(String, String)],
        source_dir: &std::path::Path,
        ttl: std::time::Duration,
    ) -> anyhow::Result<usize> {
        let now = now_ms();
        let mut cached = 0usize;
        for (kb_id, location) in locations {
            let key = Self::key(kb_id, location);
            {
                let entries = self.entries.read().unwrap();
                if entries
                    .get(&key)
                    .is_some_and(|entry| entry.expires_at > now)
                {
                    continue;
                }
            }
            let Ok(data) = std::fs::read(source_dir.join(location)) else {
                continue; // RAGFlow logs the storage error and keeps polling
            };
            let file_name = format!("cache-{}.bin", uuid::Uuid::new_v4());
            let path = std::path::Path::new(&self.cache_dir).join(&file_name);
            std::fs::write(&path, &data)?;
            let entry = CacheEntry {
                key: key.clone(),
                file_name,
                size: data.len(),
                cached_at: now,
                expires_at: now.saturating_add(ttl.as_millis() as u64),
            };
            self.entries.write().unwrap().insert(key, entry);
            cached += 1;
        }
        Ok(cached)
    }

    /// Evict expired entries and delete their physical cache files
    /// (Redis TTL eviction). Returns the number of evicted entries.
    pub fn sweep_expired(&self, now: u64) -> usize {
        let expired: Vec<CacheEntry> = self
            .entries
            .read()
            .unwrap()
            .values()
            .filter(|entry| entry.expires_at <= now)
            .cloned()
            .collect();
        if expired.is_empty() {
            return 0;
        }
        let mut entries = self.entries.write().unwrap();
        for entry in &expired {
            entries.remove(&entry.key);
            let _ =
                std::fs::remove_file(std::path::Path::new(&self.cache_dir).join(&entry.file_name));
        }
        expired.len()
    }

    /// Number of entries currently held (expired entries are counted until
    /// swept, matching Redis semantics where keys disappear on access).
    pub fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
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

    #[test]
    fn doc_file_cache_caches_ongoing_files_with_ttl() {
        let root = std::env::temp_dir().join(format!("rayrag-filecache-{}", uuid::Uuid::new_v4()));
        let cache = DocFileCache::new(root.join("cache").to_str().unwrap()).unwrap();
        let source_dir = root.join("uploads");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("a.pdf"), b"pdf-bytes-a").unwrap();
        std::fs::write(source_dir.join("b.txt"), b"text-b").unwrap();

        let now = now_ms();
        let ttl = std::time::Duration::from_secs(60);
        let locations = vec![
            ("kb-1".to_string(), "a.pdf".to_string()),
            ("kb-1".to_string(), "b.txt".to_string()),
            // Missing source file is skipped, not fatal (RAGFlow storage-error path).
            ("kb-1".to_string(), "missing.pdf".to_string()),
        ];
        let cached = cache.cache_ongoing(&locations, &source_dir, ttl).unwrap();
        assert_eq!(cached, 2);
        assert_eq!(cache.len(), 2);

        // Warm read returns the cached file path with the right bytes.
        let path = cache.get("kb-1", "a.pdf", now).expect("cached file");
        assert_eq!(std::fs::read(path).unwrap(), b"pdf-bytes-a");
        // Unknown key misses.
        assert!(cache.get("kb-2", "a.pdf", now).is_none());

        // Re-polling the same locations does not re-cache (Redis `exist` skip).
        let cached = cache.cache_ongoing(&locations, &source_dir, ttl).unwrap();
        assert_eq!(cached, 0);
        assert_eq!(cache.len(), 2);

        // After TTL the entry expires: get() misses (Redis TTL eviction)...
        let later = now + 61_000;
        assert!(cache.get("kb-1", "a.pdf", later).is_none());
        assert_eq!(cache.len(), 2, "expired entries linger until swept");
        // ...and sweep_expired removes the entry and its physical file.
        assert_eq!(cache.sweep_expired(later), 2);
        assert_eq!(cache.len(), 0);
        let cache_dir = root.join("cache");
        let leftover: Vec<_> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(leftover.is_empty(), "cache files must be deleted on sweep");

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn doc_file_cache_is_volatile_across_restarts() {
        let root = std::env::temp_dir().join(format!("rayrag-filecache2-{}", uuid::Uuid::new_v4()));
        let cache_dir = root.join("cache");
        let source_dir = root.join("uploads");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("c.md"), b"hello").unwrap();

        let ttl = std::time::Duration::from_secs(60);
        let cache = DocFileCache::new(cache_dir.to_str().unwrap()).unwrap();
        cache
            .cache_ongoing(
                &[("kb-1".to_string(), "c.md".to_string())],
                &source_dir,
                ttl,
            )
            .unwrap();
        assert_eq!(cache.len(), 1);

        // A new DocFileCache (worker restart) drops stale files: Redis-like
        // volatility — the cache is a warm-up, not durable storage.
        let restarted = DocFileCache::new(cache_dir.to_str().unwrap()).unwrap();
        assert_eq!(restarted.len(), 0);
        assert!(restarted.get("kb-1", "c.md", 1_000_000u64).is_none());

        std::fs::remove_dir_all(root).ok();
    }
}
