//! POST /api/v1/files/link-to-datasets — convert workspace files into dataset documents.
//!
//! Replaces RAGFlow's api/apps/restful_apis/file2document_api.py. The contract was read off the
//! pinned v0.27.2 tree and re-checked against a live instance:
//!
//! * missing body fields answer code 101 with "required argument are missing: ...; ";
//! * an unknown mode answers 101 "mode must be 'add' or 'replace'";
//! * an unknown file answers 102 "File not found!"; an unknown dataset answers 102
//!   "Can't find this dataset!"; a foreign resource answers 102 "no authorization";
//! * folder ids expand to the files inside them (innermost files only);
//! * success answers {code:0, message:"success", data:true} and the conversion runs in the
//!   background, exactly like the upstream executor hand-off.
//!
//! This handler is also what the deprecated POST /api/v1/file2document and
//! POST /api/v1/file/convert aliases resolve to.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, Request, State};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::api::file_mgr::{FileRecord, LinkedDocument};
use crate::server::{AppState, AuthContext, PersistedUpload};

fn success(data: Value) -> Response {
    Json(json!({ "code": 0, "message": "success", "data": data })).into_response()
}

fn data_error(message: &str) -> Response {
    Json(json!({ "code": 102, "message": message })).into_response()
}

fn argument_error(message: &str) -> Response {
    Json(json!({ "code": 101, "message": message })).into_response()
}

struct LinkRequest {
    file_ids: Vec<String>,
    kb_ids: Vec<String>,
}

fn string_list(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().map(str::to_string))
            .collect(),
        // Upstream passes the raw JSON value to a peewee in_() query; a bare string is
        // iterated character by character and never matches a real id.
        Value::String(text) => Some(text.chars().map(|ch| ch.to_string()).collect()),
        _ => None,
    }
}

fn parse_link_body(body: &Value) -> Result<LinkRequest, Response> {
    // kb_ids is the upstream spelling; dataset_id/dataset_ids are the pre-0.24 spellings the
    // RayRAG web UI used, kept as aliases so an older client keeps working.
    let kb_value = body
        .get("kb_ids")
        .or_else(|| body.get("dataset_ids"))
        .or_else(|| body.get("dataset_id"));
    let mut missing: Vec<&str> = Vec::new();
    if body.get("file_ids").is_none() {
        missing.push("file_ids");
    }
    if kb_value.is_none() {
        missing.push("kb_ids");
    }
    if !missing.is_empty() {
        return Err(argument_error(&format!(
            "required argument are missing: {}; ",
            missing.join(",")
        )));
    }
    let file_ids = match string_list(&body["file_ids"]) {
        Some(ids) => ids,
        None => return Err(data_error("File not found!")),
    };
    let kb_ids = match kb_value.and_then(string_list) {
        Some(ids) => ids,
        None => return Err(data_error("Can't find this dataset!")),
    };
    Ok(LinkRequest { file_ids, kb_ids })
}

fn parse_body(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap_or_else(|_| json!({}))
}

fn query_mode(query: Option<&str>) -> String {
    query
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "mode")
        .map(|(_, value)| value.to_string())
        .unwrap_or_else(|| "replace".to_string())
        .to_ascii_lowercase()
}

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

/// POST /api/v1/files/link-to-datasets (aliases: /api/v1/file2document, /api/v1/file/convert).
pub async fn link_files_to_datasets(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    request: Request,
) -> Response {
    let mode = query_mode(request.uri().query());
    let bytes = match axum::body::to_bytes(request.into_body(), 16 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return argument_error("required argument are missing: file_ids,kb_ids; "),
    };
    let body = parse_body(&bytes);
    let request = match parse_link_body(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if mode != "add" && mode != "replace" {
        return argument_error("mode must be 'add' or 'replace'");
    }
    // Validate every file exists before any work starts.
    for file_id in &request.file_ids {
        if state.files.get(file_id).is_none() {
            return data_error("File not found!");
        }
    }
    // Validate every dataset exists before scheduling the background work.
    for kb_id in &request.kb_ids {
        if state.kbs.get(kb_id).is_none() {
            return data_error("Can't find this dataset!");
        }
    }
    let (mut file_ids, missing) = expand_files(
        &state.files,
        &auth.user_id,
        auth.is_admin,
        &request.file_ids,
    );
    if !missing.is_empty() {
        return data_error("File not found!");
    }
    for file_id in &file_ids {
        if let Some(record) = state.files.get(file_id)
            && record.owner_id != auth.user_id
            && !auth.is_admin
        {
            return data_error("no authorization");
        }
    }
    for kb_id in &request.kb_ids {
        if !crate::server::kb_accessible(&state, kb_id, &auth) {
            return data_error("no authorization");
        }
    }
    file_ids.retain(|id| {
        state
            .files
            .get(id)
            .is_some_and(|record| record.file_type != "folder")
    });
    let replace = mode == "replace";
    let owner_id = auth.user_id.clone();
    let kb_ids = request.kb_ids.clone();
    let state_for_work = state.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = convert_files(&state_for_work, &owner_id, &file_ids, &kb_ids, replace) {
            tracing::error!(%error, "file-to-dataset conversion failed");
        }
    });
    success(json!(true))
}

fn convert_files(
    state: &Arc<AppState>,
    owner_id: &str,
    file_ids: &[String],
    kb_ids: &[String],
    replace: bool,
) -> anyhow::Result<()> {
    let upload_dir = std::path::Path::new(&state.static_dir).join("../uploads");
    for file_id in file_ids {
        let Some(mut record) = state.files.get(file_id) else {
            continue;
        };
        if replace {
            let mut kept = Vec::new();
            for link in record.linked_documents.drain(..) {
                if kb_ids.contains(&link.kb_id) {
                    kept.push(link);
                    continue;
                }
                if let Some(doc) = state.docs.get(&link.document_id)
                    && let Err(error) = crate::server::delete_document_data(state, &doc)
                {
                    tracing::warn!(
                        %error,
                        doc_id = %link.document_id,
                        "failed to unlink the replaced document"
                    );
                }
            }
            record.linked_documents = kept;
            if let Err(error) = state.files.update(record.clone()) {
                tracing::warn!(%error, file_id = %file_id, "failed to persist unlink");
            }
        }
        let physical =
            crate::server::file_physical_path(&state.files.data_dir, &record.id, &record.name);
        let bytes = match std::fs::read(&physical) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::warn!(%error, file_id = %file_id, "linked file bytes are missing");
                continue;
            }
        };
        for kb_id in kb_ids {
            if record
                .linked_documents
                .iter()
                .any(|link| &link.kb_id == kb_id)
            {
                continue;
            }
            if state.kbs.get(kb_id).is_none() {
                continue;
            }
            let doc_id = uuid::Uuid::new_v4().to_string();
            let stored_path = upload_dir.join(&doc_id);
            if let Some(parent) = stored_path.parent()
                && let Err(error) = std::fs::create_dir_all(parent)
            {
                tracing::warn!(%error, "cannot create the upload directory");
                continue;
            }
            if let Err(error) = std::fs::write(&stored_path, &bytes) {
                tracing::warn!(%error, "cannot stage the linked file");
                continue;
            }
            let mut hasher = xxhash_rust::xxh3::Xxh3::new();
            hasher.update(&bytes);
            let upload = PersistedUpload {
                name: record.name.clone(),
                storage_name: doc_id.clone(),
                path: stored_path,
                size: bytes.len(),
                content_hash: format!("{:032x}", hasher.digest128()),
                cleanup_on_drop: false,
            };
            match crate::server::register_document_upload(
                state.clone(),
                owner_id,
                kb_id,
                doc_id.clone(),
                upload,
            ) {
                Ok(_) => {
                    record.linked_documents.push(LinkedDocument {
                        kb_id: kb_id.clone(),
                        document_id: doc_id,
                    });
                }
                Err(error) => {
                    tracing::warn!(%error, %kb_id, "failed to register the linked document");
                }
            }
        }
        if let Err(error) = state.files.update(record) {
            tracing::warn!(%error, file_id = %file_id, "failed to persist the link mapping");
        }
    }
    Ok(())
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

    fn record(id: &str, name: &str, parent: &str, kind: &str) -> FileRecord {
        FileRecord {
            id: id.into(),
            name: name.into(),
            owner_id: "u1".into(),
            parent_id: parent.into(),
            size: 3,
            file_type: kind.into(),
            ..Default::default()
        }
    }

    fn response_body(response: Response) -> Value {
        let bytes = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(axum::body::to_bytes(response.into_body(), usize::MAX))
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn missing_body_fields_keep_upstream_wording() {
        let response = parse_link_body(&json!({})).err().expect("missing fields");
        let body = response_body(response);
        assert_eq!(body["code"], 101);
        assert_eq!(
            body["message"],
            "required argument are missing: file_ids,kb_ids; "
        );
    }

    #[test]
    fn unknown_file_and_dataset_keep_upstream_wording() {
        let files = parse_link_body(&json!({"file_ids": [1], "kb_ids": []}));
        let body = response_body(files.err().expect("file type"));
        assert_eq!(body["message"], "File not found!");

        let kbs = parse_link_body(&json!({"file_ids": [], "kb_ids": {}}));
        let body = response_body(kbs.err().expect("kb type"));
        assert_eq!(body["message"], "Can't find this dataset!");
    }

    #[test]
    fn folder_ids_expand_to_their_children() {
        let (store, _dir) = store_with_tempdir();
        store
            .add_unique(record("folder-1", "Docs", "root", "folder"))
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
        assert!(!files.contains(&"folder-1".to_string()));
    }

    #[test]
    fn unknown_or_foreign_files_are_reported() {
        let (store, _dir) = store_with_tempdir();
        let mut foreign = record("file-x", "x.txt", "root", "doc");
        foreign.owner_id = "someone-else".into();
        store.add_unique(foreign).expect("foreign");
        let (files, missing) = expand_files(
            &store,
            "u1",
            false,
            &["file-x".to_string(), "nope".to_string()],
        );
        assert!(files.is_empty());
        let mut reported = missing.clone();
        reported.sort();
        assert_eq!(reported, vec!["file-x".to_string(), "nope".to_string()]);
    }

    #[test]
    fn mode_defaults_to_replace_and_is_lowercased() {
        assert_eq!(query_mode(None), "replace");
        assert_eq!(query_mode(Some("mode=ADD")), "add");
        assert_eq!(query_mode(Some("x=1&mode=replace")), "replace");
    }
}
