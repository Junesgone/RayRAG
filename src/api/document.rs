//! Document lifecycle manager.
//! Full document flow: upload → parse status → re-parse → delete → download.
//! Replaces RAGFlow's document_api.py (79K lines).

use axum::{
    Json,
    extract::{Extension, Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use crate::server::{AppState, AuthContext, kb_accessible, kb_manageable};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocRecord {
    pub id: String,
    pub name: String,
    pub kb_id: String,
    pub size: usize,
    /// Stored filename relative to the uploads directory.
    #[serde(default)]
    pub storage_name: String,
    /// xxHash128 content fingerprint, compatible with RAGFlow change detection semantics.
    #[serde(default)]
    pub content_hash: String,
    /// Content fingerprint that produced the currently committed search index.
    #[serde(default)]
    pub indexed_content_hash: String,
    /// Parse status: "UNSTARTED", "RUNNING", "DONE", "FAILED"
    pub run: String,
    pub progress: f32,
    pub progress_msg: String,
    pub chunk_count: usize,
    pub created_at: u64,
    pub updated_at: u64,
    /// Upstream `Document.status`: `"1"` when the document takes part in retrieval,
    /// `"0"` when the operator disabled it. Files on disk written before this field
    /// existed are enabled, which is what they were.
    #[serde(default = "enabled_status")]
    pub status: String,
}

/// Default document status: enabled (upstream stores `"1"`).
pub fn enabled_status() -> String {
    "1".to_string()
}

#[derive(Deserialize)]
pub struct UploadRequest {
    pub kb_id: String,
    pub name: String,
}

/// Global document store.
pub struct DocStore {
    docs: RwLock<HashMap<String, DocRecord>>,
    file_path: String,
    save_lock: Mutex<()>,
}

impl DocStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        let docs = if std::path::Path::new(path).exists() {
            let data = std::fs::read_to_string(path)?;
            let list: Vec<DocRecord> = serde_json::from_str(&data).map_err(|error| {
                anyhow::anyhow!("Failed to parse document metadata '{}': {error}", path)
            })?;
            list.into_iter().map(|d| (d.id.clone(), d)).collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            docs: RwLock::new(docs),
            file_path: path.into(),
            save_lock: Mutex::new(()),
        })
    }

    pub fn list(&self, kb_id: &str) -> Vec<DocRecord> {
        self.docs
            .read()
            .unwrap()
            .values()
            .filter(|d| d.kb_id == kb_id)
            .cloned()
            .collect()
    }

    pub fn list_all(&self) -> Vec<DocRecord> {
        self.docs.read().unwrap().values().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<DocRecord> {
        self.docs.read().unwrap().get(id).cloned()
    }

    pub fn insert(&self, doc: DocRecord) -> anyhow::Result<()> {
        self.mutate(|docs| {
            docs.insert(doc.id.clone(), doc);
            Ok(())
        })
    }

    /// Insert a document while reserving a RAGFlow-compatible unique name in its KB.
    pub fn insert_unique(&self, mut doc: DocRecord) -> anyhow::Result<DocRecord> {
        self.mutate(|docs| {
            if docs.contains_key(&doc.id) {
                anyhow::bail!("Document ID already exists");
            }
            doc.name = crate::naming::duplicate_name(&doc.name, |candidate| {
                docs.values()
                    .any(|existing| existing.kb_id == doc.kb_id && existing.name == candidate)
            })?;
            docs.insert(doc.id.clone(), doc.clone());
            Ok(doc)
        })
    }

    pub fn update_status(
        &self,
        id: &str,
        run: &str,
        progress: f32,
        msg: &str,
    ) -> anyhow::Result<bool> {
        self.update_parse_result(id, run, progress, msg, None)
    }

    /// Set the retrieval status (`"1"` enabled, `"0"` disabled) of many documents.
    ///
    /// Returns the ids that changed, so the caller can report a truthful count instead
    /// of assuming every requested document was updated.
    pub fn set_status(&self, ids: &[String], status: &str) -> anyhow::Result<Vec<String>> {
        if !matches!(status, "0" | "1") {
            anyhow::bail!("Document status must be '0' or '1'");
        }
        self.mutate_if_changed(|docs| {
            let mut changed = Vec::new();
            for id in ids {
                if let Some(doc) = docs.get_mut(id)
                    && doc.status != status
                {
                    doc.status = status.to_string();
                    doc.updated_at = crate::api::document::now_ms();
                    changed.push(id.clone());
                }
            }
            let dirty = !changed.is_empty();
            Ok((changed, dirty))
        })
    }

    /// Rename a document (display name only; storage name unchanged).
    pub fn rename(&self, id: &str, new_name: &str) -> anyhow::Result<bool> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            anyhow::bail!("Document name cannot be empty");
        }
        self.mutate_if_changed(|docs| {
            let Some(d) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            d.name = new_name.to_string();
            Ok((true, true))
        })
    }

    pub fn update_parse_result(
        &self,
        id: &str,
        run: &str,
        progress: f32,
        msg: &str,
        chunk_count: Option<usize>,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let Some(d) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            d.run = run.into();
            d.progress = progress;
            d.progress_msg = msg.into();
            if let Some(chunk_count) = chunk_count {
                d.chunk_count = chunk_count;
            }
            d.updated_at = now_ms();
            Ok((true, true))
        })
    }

    pub fn mark_indexed(
        &self,
        id: &str,
        content_hash: &str,
        chunk_count: usize,
        msg: &str,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let Some(doc) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            doc.run = "DONE".into();
            doc.progress = 1.0;
            doc.progress_msg = msg.into();
            doc.chunk_count = chunk_count;
            doc.indexed_content_hash = content_hash.into();
            doc.updated_at = now_ms();
            Ok((true, true))
        })
    }

    pub fn set_chunk_count(&self, id: &str, chunk_count: usize) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let Some(doc) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            if doc.chunk_count == chunk_count {
                return Ok((true, false));
            }
            doc.chunk_count = chunk_count;
            doc.updated_at = now_ms();
            Ok((true, true))
        })
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let deleted = docs.remove(id).is_some();
            Ok((deleted, deleted))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, DocRecord>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_if_changed(|docs| mutation(docs).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, DocRecord>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut docs = self.docs.write().unwrap();
        let previous = docs.clone();
        let (value, changed) = mutation(&mut docs)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<DocRecord> = docs.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *docs = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist(&self, docs: &[DocRecord]) -> anyhow::Result<()> {
        let data = serde_json::to_vec_pretty(&docs)?;
        crate::persistence::atomic_write(std::path::Path::new(&self.file_path), &data)
    }
}

/// GET /api/v1/datasets/{kb_id}/documents — list docs in KB
pub async fn list_docs(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
) -> impl IntoResponse {
    if !kb_accessible(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    let docs: Vec<_> = state
        .docs
        .list(&kb_id)
        .into_iter()
        .map(|doc| {
            let meta_fields = state
                .document_metadata
                .get(&doc.id, &kb_id)
                .unwrap_or_default();
            serde_json::json!({
                "id": doc.id,
                "name": doc.name,
                "kb_id": doc.kb_id,
                "size": doc.size,
                "storage_name": doc.storage_name,
                "content_hash": doc.content_hash,
                "indexed_content_hash": doc.indexed_content_hash,
                "run": doc.run,
                "progress": doc.progress,
                "progress_msg": doc.progress_msg,
                "chunk_count": doc.chunk_count,
                "created_at": doc.created_at,
                "updated_at": doc.updated_at,
                // Upstream `map_doc_keys_with_run_status` reports `status` on every row;
                // the files page renders the enabled/disabled state from it.
                "status": doc.status,
                "meta_fields": meta_fields,
            })
        })
        .collect();
    Json(serde_json::json!({ "code": 0, "data": docs }))
}

/// GET /api/v1/datasets/{kb_id}/documents/{doc_id} — get doc status
pub async fn get_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !kb_accessible(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    match state.docs.get(&doc_id).filter(|doc| doc.kb_id == kb_id) {
        Some(d) => {
            let meta_fields = state
                .document_metadata
                .get(&d.id, &kb_id)
                .unwrap_or_default();
            Json(serde_json::json!({ "code": 0, "data": {
                "id": d.id,
                "name": d.name,
                "kb_id": d.kb_id,
                "size": d.size,
                "storage_name": d.storage_name,
                "content_hash": d.content_hash,
                "indexed_content_hash": d.indexed_content_hash,
                "run": d.run,
                "progress": d.progress,
                "progress_msg": d.progress_msg,
                "chunk_count": d.chunk_count,
                "created_at": d.created_at,
                "updated_at": d.updated_at,
                "meta_fields": meta_fields,
            } }))
        }
        None => Json(serde_json::json!({ "code": 404, "message": "Not found" })),
    }
}

/// Wire a document into the async task executor (RAGFlow `task_executor.py`
/// port). Enqueues the doc into the persisted TaskQueue and hands execution to
/// the TaskExecutor worker pool; the upload and reparse flows share this path.
///
/// The returned `task_id` matches the RAGFlow `task_id` returned by
/// `/datasets/{kb}/documents` upload responses.
pub(crate) fn enqueue_document_processing(
    state: std::sync::Arc<crate::server::AppState>,
    owner_id: &str,
    doc: DocRecord,
    priority: i32,
) -> anyhow::Result<String> {
    crate::server::queue_document_processing(state, owner_id, doc, priority)
}

/// POST /api/v1/datasets/{kb_id}/documents/{doc_id}/reparse — re-parse
pub async fn reparse_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    };
    if doc.kb_id != kb_id {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    match enqueue_document_processing(
        state,
        &auth.user_id,
        doc,
        crate::api::features::TASK_PRIORITY_HIGH,
    ) {
        Ok(task_id) => Json(serde_json::json!({
            "code": 0,
            "message": "Re-parse queued",
            "data": { "task_id": task_id }
        })),
        Err(error) => Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
    }
}

/// `POST /api/v1/datasets/{id}/documents/batch-update-status` — enable/disable many
/// documents at once (upstream `document_api.py::batch_update_document_status`).
///
/// The status is stored with the document, not in the browser: a disabled document
/// must stop taking part in retrieval for every client, which is what upstream means
/// by the switch. The answer mirrors upstream's per-document result map, and a partial
/// failure is reported as such instead of a blanket success.
pub async fn batch_update_document_status(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    let doc_ids: Vec<String> = body
        .get("doc_ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if doc_ids.is_empty() {
        return Json(serde_json::json!({
            "code": 400,
            "message": "'doc_ids' must be a non-empty list."
        }));
    }
    let status = match body.get("status") {
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::String(value)) => value.clone(),
        _ => "-1".to_string(),
    };
    if !matches!(status.as_str(), "0" | "1") {
        return Json(serde_json::json!({
            "code": 400,
            "message": format!("'status' must be either 0 or 1: {status}!")
        }));
    }
    let mut result = serde_json::Map::new();
    let mut has_error = false;
    let mut accepted = Vec::new();
    for doc_id in &doc_ids {
        match state.docs.get(doc_id) {
            Some(doc) if doc.kb_id == kb_id => accepted.push(doc_id.clone()),
            Some(_) => {
                result.insert(
                    doc_id.clone(),
                    serde_json::json!({ "error": "Document not found in this dataset." }),
                );
                has_error = true;
            }
            None => {
                result.insert(
                    doc_id.clone(),
                    serde_json::json!({ "error": "Document not found" }),
                );
                has_error = true;
            }
        }
    }
    match state.docs.set_status(&accepted, &status) {
        Ok(changed) => {
            for doc_id in accepted {
                result.insert(doc_id, serde_json::json!({ "status": status }));
            }
            tracing::info!(
                kb_id = %kb_id,
                status = %status,
                requested = doc_ids.len(),
                changed = changed.len(),
                "Document status updated"
            );
        }
        Err(error) => {
            has_error = true;
            for doc_id in accepted {
                result.insert(
                    doc_id,
                    serde_json::json!({ "error": format!("Database error (Document update)! {error}") }),
                );
            }
        }
    }
    if has_error {
        return Json(serde_json::json!({
            "code": 500,
            "message": "Partial failure",
            "data": result,
        }));
    }
    Json(serde_json::json!({ "code": 0, "data": result }))
}

/// `DELETE /api/v1/datasets/{id}/documents` — remove many documents (upstream
/// `document_api.py::delete_documents`).
///
/// Upstream accepts either `ids` or `delete_all` and refuses both at once; ids that do
/// not belong to the dataset are refused rather than silently skipped.
pub async fn delete_docs_bulk(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    let delete_all = body
        .get("delete_all")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let mut doc_ids: Vec<String> = body
        .get("ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if doc_ids.is_empty() && !delete_all {
        return Json(serde_json::json!({
            "code": 400,
            "message": format!("should either provide doc ids or set delete_all(true), dataset: {kb_id}."),
        }));
    }
    if !doc_ids.is_empty() && delete_all {
        return Json(serde_json::json!({
            "code": 400,
            "message": format!("should not provide both doc ids and delete_all(true), dataset: {kb_id}."),
        }));
    }
    let owned: std::collections::HashMap<String, DocRecord> = state
        .docs
        .list(&kb_id)
        .into_iter()
        .map(|doc| (doc.id.clone(), doc))
        .collect();
    if delete_all {
        doc_ids = owned.keys().cloned().collect();
    }
    let invalid: Vec<&String> = doc_ids
        .iter()
        .filter(|doc_id| !owned.contains_key(*doc_id))
        .collect();
    if !invalid.is_empty() {
        let list = invalid
            .iter()
            .map(|value| value.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Json(serde_json::json!({
            "code": 400,
            "message": format!(
                "These documents do not belong to dataset {kb_id} or Document not found: {list}"
            ),
        }));
    }
    let mut successes = 0usize;
    let mut errors: Vec<serde_json::Value> = Vec::new();
    for doc_id in &doc_ids {
        let Some(doc) = owned.get(doc_id) else {
            continue;
        };
        match crate::server::delete_document_data(&state, doc) {
            Ok(()) => successes += 1,
            Err(error) => errors
                .push(serde_json::json!({ "document_id": doc_id, "error": error.to_string() })),
        }
    }
    tracing::info!(kb_id = %kb_id, successes, failures = errors.len(), "Documents deleted");
    if errors.is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "data": { "success_count": successes },
        }));
    }
    Json(serde_json::json!({
        "code": 500,
        "message": "Partial failure",
        "data": { "success_count": successes, "errors": errors },
    }))
}

/// `POST /api/v1/documents/ingest` — queue (or cancel) the parsing of many documents
/// (upstream `document_api.py::ingest`).
///
/// `run: "1"` starts parsing, `run: "2"` cancels it. `delete` clears the chunks the
/// document already has before re-parsing (upstream's `redo` checkbox) and `apply_kb`
/// re-applies the dataset's automatic metadata settings.
pub async fn ingest_documents(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    let doc_ids: Vec<String> = body
        .get("doc_ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if doc_ids.is_empty() {
        return Json(serde_json::json!({ "code": 400, "message": "'doc_ids' is required" }));
    }
    let run = match body.get("run") {
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::String(value)) => value.clone(),
        _ => String::new(),
    };
    if !matches!(run.as_str(), "1" | "2") {
        return Json(serde_json::json!({
            "code": 400,
            "message": format!("'run' must be either 0 or 1: {run}")
        }));
    }
    let clear_chunks = body
        .get("delete")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let mut accepted = Vec::new();
    for doc_id in &doc_ids {
        let Some(doc) = state.docs.get(doc_id) else {
            return Json(serde_json::json!({
                "code": 404,
                "message": format!("Document not found: {doc_id}")
            }));
        };
        if !kb_manageable(&state, &doc.kb_id, &auth) {
            return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
        }
        accepted.push(doc);
    }
    if run == "2" {
        let mut cancelled = 0usize;
        for doc in &accepted {
            match state.tasks.cancel_document(&doc.id) {
                Ok(count) => cancelled += count,
                Err(error) => {
                    return Json(serde_json::json!({ "code": 500, "message": error.to_string() }));
                }
            }
            let _ = state
                .docs
                .update_status(&doc.id, "CANCELLED", 1.0, "Processing cancelled");
        }
        tracing::info!(
            documents = accepted.len(),
            cancelled,
            "Document parsing cancelled"
        );
        return Json(serde_json::json!({
            "code": 0,
            "message": "Cancelled",
            "data": { "doc_ids": accepted.iter().map(|doc| doc.id.clone()).collect::<Vec<_>>(), "cancelled": cancelled }
        }));
    }
    let mut queued = Vec::new();
    let mut errors: Vec<serde_json::Value> = Vec::new();
    for doc in accepted {
        if clear_chunks {
            // Upstream's `redo` checkbox: drop the chunks this document already
            // contributed before parsing it again, so the new run cannot leave a
            // mixture of two parses behind.
            match crate::server::delete_document_data(&state, &doc) {
                Ok(()) => {
                    // The document row survives the chunk removal (its file is
                    // re-parsed from disk), so it is re-inserted with an empty index.
                    let _ = state.docs.set_chunk_count(&doc.id, 0);
                }
                Err(error) => {
                    errors.push(
                        serde_json::json!({ "document_id": doc.id, "error": error.to_string() }),
                    );
                    continue;
                }
            }
        }
        match state
            .docs
            .get(&doc.id)
            .ok_or_else(|| anyhow::anyhow!("Document not found"))
            .and_then(|doc| {
                enqueue_document_processing(
                    state.clone(),
                    &auth.user_id,
                    doc,
                    crate::api::features::TASK_PRIORITY_HIGH,
                )
            }) {
            Ok(task_id) => {
                queued.push(serde_json::json!({ "document_id": doc.id, "task_id": task_id }))
            }
            Err(error) => errors
                .push(serde_json::json!({ "document_id": doc.id, "error": error.to_string() })),
        }
    }
    tracing::info!(
        queued = queued.len(),
        failures = errors.len(),
        "Documents queued for parsing"
    );
    if errors.is_empty() {
        return Json(serde_json::json!({ "code": 0, "data": queued }));
    }
    Json(serde_json::json!({
        "code": 500,
        "message": "Partial failure",
        "data": { "queued": queued, "errors": errors },
    }))
}

/// DELETE /api/v1/datasets/{kb_id}/documents/{doc_id} — delete doc
/// PATCH /api/v1/datasets/{id}/documents/{did} — rename a document.
pub async fn rename_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    let Some(name) = body.get("name").and_then(|value| value.as_str()) else {
        return Json(serde_json::json!({ "code": 400, "message": "name is required" }));
    };
    if state
        .docs
        .get(&doc_id)
        .filter(|doc| doc.kb_id == kb_id)
        .is_none()
    {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    match state.docs.rename(&doc_id, name) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "message": "Renamed" })),
        Ok(false) => Json(serde_json::json!({ "code": 404, "message": "Not found" })),
        Err(error) => Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
    }
}

/// Download the original file of a document (RAGFlow-compatible:
/// GET /api/v1/datasets/{dataset_id}/documents/{document_id}/download).
pub async fn download_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> axum::response::Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" })).into_response();
    }
    let Some(doc) = state.docs.get(&doc_id).filter(|doc| doc.kb_id == kb_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found" }))
            .into_response();
    };
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let filename = doc.name.replace(['"', '\\', '/'], "_");
    let disposition = format!("attachment; filename=\"{filename}\"");
    crate::api::common::stream_stored_file(
        &physical,
        "application/octet-stream",
        &disposition,
        "File content missing",
    )
    .await
}

/// `GET /api/v1/documents/{doc_id}/preview` — upstream
/// `useGetDocumentUrl`'s document branch (`hooks/use-document-request.ts`):
/// the raw bytes of a document, addressed by document id alone and served
/// inline so the previewer can render it (a download keeps its `attachment`
/// disposition on the dataset route).
pub async fn preview_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let filename = doc.name.replace(['"', '\\', '/'], "_");
    let disposition = format!("inline; filename=\"{filename}\"");
    crate::api::common::stream_stored_file(
        &physical,
        mime_for_document(&doc.name),
        &disposition,
        "File content missing",
    )
    .await
}

/// `GET /api/v1/documents/{document_id}` — upstream
/// `api/apps/restful_apis/document_api.py::download_document`
/// (`@manager.route("/documents/<document_id>", methods=["GET"])`). It is the branch
/// `utils/file-util.ts::fetchPreviewBlob` takes for `resource === 'document'`, i.e. the
/// byte source behind `previewHtmlFile()` and `downloadDocument()`. Upstream answers
/// "Document not found!" for a document the caller cannot read, "This file is empty."
/// when the stored blob is missing, and serves the bytes with an `attachment`
/// disposition under the document's own MIME type.
pub async fn download_document(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found!" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found!" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let filename = doc.name.replace(['"', '\\', '/'], "_");
    let disposition = format!("attachment; filename=\"{filename}\"");
    crate::api::common::stream_stored_file(
        &physical,
        mime_for_document(&doc.name),
        &disposition,
        "This file is empty.",
    )
    .await
}

/// `GET /api/v1/documents/{doc_id}/preview/sheets` — the workbook grid the
/// spreadsheet previewer renders. Upstream builds this in the browser through
/// `@js-preview/excel` (`components/document-preview/hooks.ts::useFetchExcel`,
/// which hands the raw arraybuffer to the library); RayRAG answers the same grid
/// from its own Rust reader (`parser::excel::read_xlsx_sheets`), so the preview
/// needs no npm dependency and stays inside the Rust implementation.
///
/// The payload is `{sheets:[{name, rows:[[cell,…],…]}], truncated:bool}`. Rows and
/// columns are capped so one huge workbook cannot bloat the response; the flag
/// tells the page to say so instead of silently cutting the sheet short.
pub async fn preview_sheets(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    const MAX_ROWS: usize = 500;
    const MAX_COLUMNS: usize = 64;
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let Ok(bytes) = tokio::fs::read(&physical).await else {
        return Json(serde_json::json!({ "code": 404, "message": "File content missing" }))
            .into_response();
    };
    let sheets = match crate::parser::excel::read_xlsx_sheets(&bytes, MAX_ROWS) {
        Ok(sheets) => sheets,
        Err(error) => {
            return Json(
                serde_json::json!({ "code": 500, "message": format!("Failed to read workbook: {error}") }),
            )
            .into_response();
        }
    };
    let mut truncated = false;
    let payload: Vec<serde_json::Value> = sheets
        .into_iter()
        .map(|sheet| {
            truncated |= sheet.truncated;
            let rows: Vec<Vec<String>> = sheet
                .rows
                .into_iter()
                .map(|row| {
                    let mut cells = row;
                    if cells.len() > MAX_COLUMNS {
                        truncated = true;
                        cells.truncate(MAX_COLUMNS);
                    }
                    cells
                })
                .collect();
            serde_json::json!({ "name": sheet.name, "rows": rows })
        })
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": { "sheets": payload, "truncated": truncated },
    }))
    .into_response()
}

/// `GET /api/v1/documents/{doc_id}/preview/slides` — the deck text the slides
/// previewer renders. Upstream draws real slides in the browser with
/// `pptx-preview` (`components/document-preview/ppt-preview.tsx`); RayRAG returns
/// the per-slide text its own reader extracts (`parser::ppt::slide_texts`), which
/// is the part of a deck the chunker indexes. Shapes, theme and images are not
/// reproduced — the ledger keeps this branch `partial` for exactly that reason.
pub async fn preview_slides(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Document not found" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let Ok(bytes) = tokio::fs::read(&physical).await else {
        return Json(serde_json::json!({ "code": 404, "message": "File content missing" }))
            .into_response();
    };
    let slides = match crate::parser::ppt::slide_texts(&bytes) {
        Ok(slides) => slides,
        Err(error) => {
            return Json(
                serde_json::json!({ "code": 500, "message": format!("Failed to read presentation: {error}") }),
            )
            .into_response();
        }
    };
    Json(serde_json::json!({ "code": 0, "data": { "slides": slides } })).into_response()
}

/// Content type for a stored document, by extension. The previewer renders PDFs
/// through pdf.js, images and text natively, and everything else as a download.
fn mime_for_document(name: &str) -> &'static str {
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "txt" | "md" | "mdx" | "markdown" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

pub async fn delete_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    }
    let Some(doc) = state.docs.get(&doc_id).filter(|doc| doc.kb_id == kb_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Not found" }));
    };
    match crate::server::delete_document_data(&state, &doc) {
        Ok(()) => Json(serde_json::json!({ "code": 0, "message": "Deleted" })),
        Err(error) => Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod store_tests {
    use super::*;

    fn sample_doc(id: &str) -> DocRecord {
        DocRecord {
            id: id.into(),
            name: "sample.txt".into(),
            kb_id: "kb-1".into(),
            size: 6,
            storage_name: "sample.txt".into(),
            content_hash: "hash".into(),
            indexed_content_hash: String::new(),
            run: "UNSTARTED".into(),
            progress: 0.0,
            progress_msg: String::new(),
            chunk_count: 0,
            created_at: 1,
            updated_at: 1,
            status: enabled_status(),
        }
    }

    #[test]
    fn failed_persistence_rolls_back_document_mutations() {
        let root =
            std::env::temp_dir().join(format!("rayrag-doc-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("docs.json");
        let store = DocStore::new(path.to_str().unwrap()).unwrap();
        store.insert(sample_doc("doc-1")).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .update_status("doc-1", "RUNNING", 0.2, "Parsing")
                .is_err()
        );
        let doc = store.get("doc-1").unwrap();
        assert_eq!(doc.run, "UNSTARTED");
        assert_eq!(doc.progress, 0.0);

        assert!(store.delete("doc-1").is_err());
        assert!(store.get("doc-1").is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn corrupt_document_json_fails_startup_instead_of_clearing_store() {
        let root =
            std::env::temp_dir().join(format!("rayrag-doc-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("docs.json");
        std::fs::write(&path, b"{not-json").unwrap();
        let error = match DocStore::new(path.to_str().unwrap()) {
            Err(error) => error,
            Ok(_) => panic!("corrupt document JSON must fail startup"),
        };
        assert!(
            error
                .to_string()
                .contains("Failed to parse document metadata")
        );
        std::fs::remove_dir_all(root).ok();
    }
}
