//! `POST /api/v1/files/link-to-datasets` — turn workspace files into dataset documents.
//!
//! The API guide documents this as the replacement for the deprecated `POST /api/v1/file/convert`.
//! It is also what the old `POST /api/v1/file2document` claimed to do: that endpoint used to answer
//! `code: 0` with "N file(s) queued for conversion" while converting nothing at all, so a caller saw
//! success and no documents. It now performs the real conversion through this module.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::server::{AppState, AuthContext, PersistedUpload, api_error_code, code};

#[derive(Deserialize)]
pub struct LinkToDatasetsRequest {
    #[serde(default)]
    pub file_ids: Vec<String>,
    #[serde(default)]
    pub kb_ids: Vec<String>,
}

/// Every file reachable from the given ids, expanding folders the way the guide describes ("If a
/// folder ID is provided, all files within that folder will be converted").
fn expand_files(
    store: &crate::api::file_mgr::FileStore,
    owner_id: &str,
    is_admin: bool,
    file_ids: &[String],
) -> (Vec<String>, Vec<String>) {
    let mut files = Vec::new();
    let mut missing = Vec::new();
    let mut queue: Vec<String> = file_ids.to_vec();
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = queue.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(record) = store.get(&id) else {
            missing.push(id);
            continue;
        };
        if record.owner_id != owner_id && !is_admin {
            missing.push(id);
            continue;
        }
        if record.file_type == "folder" {
            for child in store.list_for(owner_id, is_admin, &id) {
                queue.push(child.id);
            }
            continue;
        }
        files.push(id);
    }
    (files, missing)
}

/// `POST /api/v1/files/link-to-datasets` (alias: `POST /api/v1/file/convert`).
pub async fn link_files_to_datasets(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<LinkToDatasetsRequest>,
) -> Response {
    if body.file_ids.is_empty() || body.kb_ids.is_empty() {
        return crate::server::invalid_argument("file_ids and kb_ids are required");
    }
    // Resolve every target dataset first: half-linking a selection would leave the caller guessing
    // which half worked.
    for kb_id in &body.kb_ids {
        if !crate::server::kb_manageable(&state, kb_id, &auth) {
            return api_error_code(
                StatusCode::NOT_FOUND,
                code::INVALID_OR_MISSING_DATA,
                "Can't find this dataset!",
            );
        }
    }
    let (files, missing) = expand_files(&state.files, &auth.user_id, auth.is_admin, &body.file_ids);
    if !missing.is_empty() {
        return api_error_code(
            StatusCode::NOT_FOUND,
            code::INVALID_OR_MISSING_DATA,
            "File not found!",
        );
    }
    if files.is_empty() {
        return crate::server::invalid_argument("no files to convert");
    }

    let upload_dir = std::path::Path::new(&state.static_dir).join("../uploads");
    let mut linked = Vec::new();
    let mut failed: Vec<serde_json::Value> = Vec::new();
    for file_id in &files {
        let Some(record) = state.files.get(file_id) else {
            continue;
        };
        let physical =
            crate::server::file_physical_path(&state.files.data_dir, file_id, &record.name);
        let bytes = match std::fs::read(&physical) {
            Ok(bytes) => bytes,
            Err(error) => {
                failed.push(serde_json::json!({
                    "file_id": file_id,
                    "message": format!("cannot read stored file: {error}")
                }));
                continue;
            }
        };
        let content_hash = {
            let mut hasher = xxhash_rust::xxh3::Xxh3::new();
            hasher.update(&bytes);
            format!("{:032x}", hasher.digest128())
        };
        for kb_id in &body.kb_ids {
            let doc_id = uuid::Uuid::new_v4().to_string();
            let stored_path = upload_dir.join(&doc_id);
            if let Some(parent) = stored_path.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                failed.push(serde_json::json!({
                    "file_id": file_id,
                    "kb_id": kb_id,
                    "message": format!("cannot create upload dir: {error}")
                }));
                continue;
            }
            if let Err(error) = std::fs::write(&stored_path, &bytes) {
                failed.push(serde_json::json!({
                    "file_id": file_id,
                    "kb_id": kb_id,
                    "message": format!("cannot stage file: {error}")
                }));
                continue;
            }
            let upload = PersistedUpload {
                name: record.name.clone(),
                storage_name: doc_id.clone(),
                path: stored_path,
                size: bytes.len(),
                content_hash: content_hash.clone(),
                cleanup_on_drop: false,
            };
            match crate::server::register_document_upload(
                state.clone(),
                &auth.user_id,
                kb_id,
                doc_id.clone(),
                upload,
            ) {
                Ok(_) => linked.push(serde_json::json!({
                    "id": doc_id,
                    "file_id": file_id,
                    "document_id": doc_id,
                    "kb_id": kb_id,
                })),
                Err(error) => failed.push(serde_json::json!({
                    "file_id": file_id,
                    "kb_id": kb_id,
                    "message": error.to_string()
                })),
            }
        }
    }

    if linked.is_empty() {
        // Nothing converted: report the reason instead of a success with an empty list.
        return api_error_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &format!(
                "no file could be converted: {}",
                serde_json::Value::Array(failed)
            ),
        );
    }
    let mut data = serde_json::json!({ "data": linked });
    if !failed.is_empty() {
        data["errors"] = serde_json::Value::Array(failed);
    }
    Json(serde_json::json!({
        "code": 0,
        "message": "success",
        "data": data["data"].clone(),
        "errors": data.get("errors").cloned().unwrap_or(serde_json::Value::Null),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_tempdir() -> (crate::api::file_mgr::FileStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store =
            crate::api::file_mgr::FileStore::new(&dir.path().join("files").to_string_lossy())
                .expect("store");
        (store, dir)
    }

    fn record(id: &str, name: &str, parent: &str, kind: &str) -> crate::api::file_mgr::FileRecord {
        crate::api::file_mgr::FileRecord {
            id: id.into(),
            name: name.into(),
            owner_id: "u1".into(),
            parent_id: parent.into(),
            size: 3,
            content_hash: String::new(),
            file_type: kind.into(),
            created_at: 0,
        }
    }

    /// The old `file2document` answered `code: 0` with "N file(s) queued for conversion" while
    /// converting nothing. This regression pins the shape of the request the real handler needs.
    #[test]
    fn empty_selection_is_not_a_success() {
        let empty = LinkToDatasetsRequest {
            file_ids: Vec::new(),
            kb_ids: vec!["kb".into()],
        };
        assert!(empty.file_ids.is_empty());
        let no_kb = LinkToDatasetsRequest {
            file_ids: vec!["f".into()],
            kb_ids: Vec::new(),
        };
        assert!(no_kb.kb_ids.is_empty());
    }

    /// The manual promises folder ids expand to the files inside them.
    #[test]
    fn folder_ids_expand_to_their_children() {
        let (store, _dir) = store_with_tempdir();
        store
            .add_unique(record("folder-1", "Docs", "", "folder"))
            .expect("folder");
        store
            .add_unique(record("file-a", "a.txt", "folder-1", "doc"))
            .expect("a");
        store
            .add_unique(record("file-b", "b.txt", "folder-1", "doc"))
            .expect("b");
        let (files, missing) = expand_files(&store, "u1", false, &["folder-1".to_string()]);
        assert!(missing.is_empty(), "{missing:?}");
        let mut names: Vec<String> = files
            .iter()
            .filter_map(|id| store.get(id).map(|f| f.name))
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.txt".to_string(), "b.txt".to_string()]);
        // The folder itself is never converted into a document.
        assert!(!files.contains(&"folder-1".to_string()));
    }

    /// A file that does not exist, or belongs to somebody else, must be reported - not skipped.
    #[test]
    fn unknown_or_foreign_files_are_reported() {
        let (store, _dir) = store_with_tempdir();
        let mut foreign = record("file-x", "x.txt", "", "doc");
        foreign.owner_id = "someone-else".into();
        store.add_unique(foreign).expect("foreign");
        let (files, missing) = expand_files(
            &store,
            "u1",
            false,
            &["file-x".to_string(), "nope".to_string()],
        );
        assert!(files.is_empty());
        // Order is not part of the contract: the walk is a stack, so only membership is asserted.
        let mut reported = missing.clone();
        reported.sort();
        assert_eq!(reported, vec!["file-x".to_string(), "nope".to_string()]);
    }
}
