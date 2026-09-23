//! Persistent document-level metadata and RAGFlow-compatible filtering.
//!
//! The fixed upstream engine mappings expose exactly `id`, `kb_id` and a
//! dynamic JSON/object `meta_fields`. RayRAG keeps that wire schema in its
//! atomic JSON snapshot; PostgreSQL may mirror the complete snapshot, while
//! zvec-rust remains scoped to chunk vectors.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Mutex, RwLock};

use axum::{
    Json,
    extract::{Extension, Path, State},
    response::IntoResponse,
};
use std::sync::Arc;

use crate::server::{AppState, AuthContext, kb_accessible, kb_manageable};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentMetadataRecord {
    #[serde(rename = "id", alias = "doc_id")]
    pub doc_id: String,
    pub kb_id: String,
    #[serde(default)]
    pub meta_fields: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetadataFilter {
    #[serde(alias = "name")]
    pub key: String,
    #[serde(alias = "comparison_operator")]
    pub op: String,
    #[serde(default)]
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetadataSummaryField {
    #[serde(rename = "type")]
    pub value_type: String,
    pub values: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetadataUpdate {
    pub key: String,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub r#match: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MetadataDelete {
    pub key: String,
    #[serde(default)]
    pub value: Option<Value>,
}

#[derive(Debug, Deserialize)]
pub struct MetadataReplaceRequest {
    #[serde(default, alias = "metadata")]
    pub meta_fields: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct MetadataSummaryRequest {
    #[serde(default)]
    pub doc_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct MetadataBatchRequest {
    pub doc_ids: Vec<String>,
    #[serde(default)]
    pub updates: Vec<MetadataUpdate>,
    #[serde(default)]
    pub deletes: Vec<MetadataDelete>,
}

pub struct DocumentMetadataStore {
    records: RwLock<HashMap<String, DocumentMetadataRecord>>,
    file_path: Option<String>,
    save_lock: Mutex<()>,
}

impl DocumentMetadataStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        let records = if std::path::Path::new(path).exists() {
            let data = std::fs::read_to_string(path)?;
            let records: Vec<DocumentMetadataRecord> =
                serde_json::from_str(&data).map_err(|error| {
                    anyhow::anyhow!("Failed to parse document metadata store '{path}': {error}")
                })?;
            validate_records(&records)?;
            records
                .into_iter()
                .map(|record| (record.doc_id.clone(), record))
                .collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            records: RwLock::new(records),
            file_path: Some(path.into()),
            save_lock: Mutex::new(()),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            records: RwLock::new(HashMap::new()),
            file_path: None,
            save_lock: Mutex::new(()),
        }
    }

    /// Replace the complete metadata object for one document.
    pub fn replace(
        &self,
        doc_id: &str,
        kb_id: &str,
        meta_fields: Map<String, Value>,
    ) -> anyhow::Result<Map<String, Value>> {
        validate_identity(doc_id, kb_id)?;
        let meta_fields = normalize_metadata(meta_fields);
        self.mutate(|records| {
            records.insert(
                doc_id.into(),
                DocumentMetadataRecord {
                    doc_id: doc_id.into(),
                    kb_id: kb_id.into(),
                    meta_fields: meta_fields.clone(),
                },
            );
            Ok((meta_fields, true))
        })
    }

    pub fn get(&self, doc_id: &str, kb_id: &str) -> Option<Map<String, Value>> {
        self.records
            .read()
            .unwrap()
            .get(doc_id)
            .filter(|record| record.kb_id == kb_id)
            .map(|record| record.meta_fields.clone())
    }

    pub fn get_for_documents(
        &self,
        kb_id: &str,
        doc_ids: Option<&[String]>,
    ) -> HashMap<String, Map<String, Value>> {
        let requested = doc_ids.map(|ids| ids.iter().collect::<HashSet<_>>());
        self.records
            .read()
            .unwrap()
            .values()
            .filter(|record| record.kb_id == kb_id)
            .filter(|record| {
                requested
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&record.doc_id))
            })
            .filter(|record| !record.meta_fields.is_empty())
            .map(|record| (record.doc_id.clone(), record.meta_fields.clone()))
            .collect()
    }

    pub fn delete(&self, doc_id: &str, kb_id: &str) -> anyhow::Result<bool> {
        self.mutate(|records| {
            let deleted = records
                .get(doc_id)
                .is_some_and(|record| record.kb_id == kb_id);
            if deleted {
                records.remove(doc_id);
            }
            Ok((deleted, deleted))
        })
    }

    pub fn keys(&self, kb_ids: &[String]) -> Vec<String> {
        let kb_ids: HashSet<_> = kb_ids.iter().collect();
        self.records
            .read()
            .unwrap()
            .values()
            .filter(|record| kb_ids.contains(&record.kb_id))
            .flat_map(|record| record.meta_fields.keys().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn flattened(&self, kb_ids: &[String]) -> BTreeMap<String, BTreeMap<String, Vec<String>>> {
        let kb_ids: HashSet<_> = kb_ids.iter().collect();
        let mut flattened: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        for record in self
            .records
            .read()
            .unwrap()
            .values()
            .filter(|record| kb_ids.contains(&record.kb_id))
        {
            for (key, value) in &record.meta_fields {
                for scalar in scalar_values(value) {
                    flattened
                        .entry(key.clone())
                        .or_default()
                        .entry(value_string(scalar))
                        .or_default()
                        .push(record.doc_id.clone());
                }
            }
        }
        flattened
    }

    pub fn filter_doc_ids(
        &self,
        kb_ids: &[String],
        filters: &[MetadataFilter],
        logic: &str,
    ) -> anyhow::Result<Vec<String>> {
        if filters.is_empty() {
            return Ok(Vec::new());
        }
        if logic != "and" && logic != "or" {
            anyhow::bail!("Metadata filter logic must be 'and' or 'or'");
        }
        for filter in filters {
            validate_filter(filter)?;
        }
        let kb_ids: HashSet<_> = kb_ids.iter().collect();
        let mut matches: Vec<_> = self
            .records
            .read()
            .unwrap()
            .values()
            .filter(|record| kb_ids.contains(&record.kb_id))
            .filter(|record| {
                let predicate = |filter: &MetadataFilter| {
                    record
                        .meta_fields
                        .get(&filter.key)
                        .is_some_and(|value| metadata_matches(value, filter))
                };
                if logic == "and" {
                    filters.iter().all(predicate)
                } else {
                    filters.iter().any(predicate)
                }
            })
            .map(|record| record.doc_id.clone())
            .collect();
        matches.sort();
        matches.dedup();
        Ok(matches)
    }

    pub fn summary(
        &self,
        kb_id: &str,
        doc_ids: Option<&[String]>,
    ) -> BTreeMap<String, MetadataSummaryField> {
        let requested = doc_ids.map(|ids| ids.iter().collect::<HashSet<_>>());
        let records = self.records.read().unwrap();
        let mut counts: BTreeMap<String, HashMap<String, usize>> = BTreeMap::new();
        let mut types: BTreeMap<String, HashMap<String, usize>> = BTreeMap::new();
        for record in records.values().filter(|record| record.kb_id == kb_id) {
            if requested
                .as_ref()
                .is_some_and(|ids| !ids.contains(&record.doc_id))
            {
                continue;
            }
            for (key, value) in &record.meta_fields {
                *types
                    .entry(key.clone())
                    .or_default()
                    .entry(metadata_type(value).into())
                    .or_default() += 1;
                for scalar in scalar_values(value) {
                    *counts
                        .entry(key.clone())
                        .or_default()
                        .entry(value_string(scalar))
                        .or_default() += 1;
                }
            }
        }
        counts
            .into_iter()
            .map(|(key, values)| {
                let mut values: Vec<_> = values.into_iter().collect();
                values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                let value_type = types
                    .get(&key)
                    .and_then(|counts| {
                        counts
                            .iter()
                            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
                            .map(|(kind, _)| kind.clone())
                    })
                    .unwrap_or_else(|| "string".into());
                (key, MetadataSummaryField { value_type, values })
            })
            .collect()
    }

    pub fn batch_update(
        &self,
        kb_id: &str,
        doc_ids: &[String],
        updates: &[MetadataUpdate],
        deletes: &[MetadataDelete],
    ) -> anyhow::Result<usize> {
        if doc_ids.is_empty() {
            return Ok(0);
        }
        let requested: HashSet<_> = doc_ids.iter().collect();
        self.mutate(|records| {
            let mut updated = 0;
            let mut found = HashSet::new();
            let mut empty_records = Vec::new();
            for record in records
                .values_mut()
                .filter(|record| record.kb_id == kb_id && requested.contains(&record.doc_id))
            {
                found.insert(record.doc_id.clone());
                let original = record.meta_fields.clone();
                apply_updates(&mut record.meta_fields, updates);
                apply_deletes(&mut record.meta_fields, deletes);
                record.meta_fields = normalize_metadata(std::mem::take(&mut record.meta_fields));
                if record.meta_fields != original {
                    updated += 1;
                    if record.meta_fields.is_empty() {
                        empty_records.push(record.doc_id.clone());
                    }
                }
            }
            for doc_id in empty_records {
                records.remove(&doc_id);
            }
            for doc_id in requested
                .into_iter()
                .filter(|doc_id| !found.contains(*doc_id))
            {
                let mut meta_fields = Map::new();
                apply_updates(&mut meta_fields, updates);
                apply_deletes(&mut meta_fields, deletes);
                let meta_fields = normalize_metadata(meta_fields);
                if meta_fields.is_empty() {
                    continue;
                }
                records.insert(
                    doc_id.clone(),
                    DocumentMetadataRecord {
                        doc_id: doc_id.clone(),
                        kb_id: kb_id.into(),
                        meta_fields,
                    },
                );
                updated += 1;
            }
            Ok((updated, updated > 0))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, DocumentMetadataRecord>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut records = self.records.write().unwrap();
        let previous = records.clone();
        let (value, changed) = mutation(&mut records)?;
        if !changed {
            return Ok(value);
        }
        if let Err(error) = self.persist(&records) {
            *records = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist(&self, records: &HashMap<String, DocumentMetadataRecord>) -> anyhow::Result<()> {
        let Some(path) = self.file_path.as_deref() else {
            return Ok(());
        };
        let mut records: Vec<_> = records.values().cloned().collect();
        records.sort_by(|a, b| a.doc_id.cmp(&b.doc_id));
        validate_records(&records)?;
        let data = serde_json::to_vec_pretty(&records)?;
        crate::persistence::atomic_write(std::path::Path::new(path), &data)
    }
}

pub async fn get_document_metadata(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> axum::response::Response {
    if !document_accessible(&state, &auth, &kb_id, &doc_id) {
        return not_found();
    }
    Json(serde_json::json!({
        "code": 0,
        "data": state.document_metadata.get(&doc_id, &kb_id).unwrap_or_default()
    }))
    .into_response()
}

pub async fn replace_document_metadata(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
    Json(request): Json<MetadataReplaceRequest>,
) -> axum::response::Response {
    if !document_exists(&state, &kb_id, &doc_id) {
        return not_found();
    }
    if !kb_manageable(&state, &kb_id, &auth) {
        return forbidden();
    }
    match state
        .document_metadata
        .replace(&doc_id, &kb_id, request.meta_fields)
    {
        Ok(meta_fields) => {
            Json(serde_json::json!({ "code": 0, "data": meta_fields })).into_response()
        }
        Err(error) => server_error(&error.to_string()),
    }
}

pub async fn delete_document_metadata(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> axum::response::Response {
    if !document_exists(&state, &kb_id, &doc_id) {
        return not_found();
    }
    if !kb_manageable(&state, &kb_id, &auth) {
        return forbidden();
    }
    match state.document_metadata.delete(&doc_id, &kb_id) {
        Ok(deleted) => Json(serde_json::json!({ "code": 0, "data": deleted })).into_response(),
        Err(error) => server_error(&error.to_string()),
    }
}

pub async fn metadata_keys(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
) -> axum::response::Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return not_found();
    }
    Json(serde_json::json!({
        "code": 0,
        "data": state.document_metadata.keys(std::slice::from_ref(&kb_id))
    }))
    .into_response()
}

pub async fn metadata_summary(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Json(request): Json<MetadataSummaryRequest>,
) -> axum::response::Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return not_found();
    }
    if request.doc_ids.as_ref().is_some_and(|doc_ids| {
        doc_ids.iter().any(|doc_id| {
            state
                .docs
                .get(doc_id)
                .is_none_or(|document| document.kb_id != kb_id)
        })
    }) {
        return bad_request("One or more documents do not belong to the knowledge base");
    }
    Json(serde_json::json!({
        "code": 0,
        "data": state.document_metadata.summary(&kb_id, request.doc_ids.as_deref())
    }))
    .into_response()
}

/// `RetCode.DATA_ERROR` — `get_error_data_result`'s default and the code
/// upstream returns for a rejected metadata request.
const DATA_ERROR: i32 = 102;

/// `get_error_data_result(message)` — upstream answers HTTP 200 with the code in
/// the body, so the metadata endpoints do too.
fn data_error(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::OK,
        Json(serde_json::json!({ "code": DATA_ERROR, "message": message })),
    )
        .into_response()
}

/// `PATCH /api/v1/datasets/{id}/documents/metadatas` — upstream
/// `document_api.update_metadata`, the endpoint the dataset metadata manager
/// calls with `{selector: {document_ids, metadata_condition}, updates, deletes}`.
///
/// The selector is resolved exactly like upstream: `document_ids` are validated
/// against the dataset, `metadata_condition` is evaluated through
/// [`crate::metadata_filter::convert_conditions`] +
/// [`crate::metadata_filter::meta_filter`] and intersected with them, and when a
/// condition list was given but matched nothing the handler short-circuits with
/// `{updated: 0, matched_docs: 0}`. The response carries `matched_docs` as well
/// as `updated`.
pub async fn update_document_metadatas(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Json(request): Json<Value>,
) -> axum::response::Response {
    if !kb_manageable(&state, &kb_id, &auth) {
        // Upstream `f"You don't own the dataset {dataset_id}."`
        return data_error(&format!("You don't own the dataset {kb_id}."));
    }
    let Some(body) = request.as_object() else {
        return data_error("Request body must be an object.");
    };
    let empty = serde_json::Map::new();
    let selector = match body.get("selector") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(selector)) => selector,
        Some(_) => return data_error("selector must be an object."),
    };
    let updates = match body.get("updates") {
        None | Some(Value::Null) => &Vec::new(),
        Some(Value::Array(updates)) => updates,
        Some(_) => return data_error("updates and deletes must be lists."),
    };
    let deletes = match body.get("deletes") {
        None | Some(Value::Null) => &Vec::new(),
        Some(Value::Array(deletes)) => deletes,
        Some(_) => return data_error("updates and deletes must be lists."),
    };
    let metadata_condition = match selector.get("metadata_condition") {
        None | Some(Value::Null) => None,
        Some(condition @ Value::Object(_)) => Some(condition),
        Some(_) => return data_error("metadata_condition must be an object."),
    };
    let document_ids: Vec<String> = match selector.get("document_ids") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(ids)) => {
            let mut resolved = Vec::with_capacity(ids.len());
            for id in ids {
                let Some(id) = id.as_str() else {
                    return data_error("document_ids must be a list.");
                };
                resolved.push(id.to_string());
            }
            resolved
        }
        Some(_) => return data_error("document_ids must be a list."),
    };
    let mut parsed_updates = Vec::with_capacity(updates.len());
    for update in updates {
        let Some(key) = update.get("key").and_then(Value::as_str) else {
            return data_error("Each update requires key and value.");
        };
        if update.get("value").is_none() {
            return data_error("Each update requires key and value.");
        }
        parsed_updates.push(MetadataUpdate {
            key: key.to_string(),
            value: update.get("value").cloned().unwrap_or(Value::Null),
            r#match: update.get("match").cloned(),
        });
    }
    let mut parsed_deletes = Vec::with_capacity(deletes.len());
    for delete in deletes {
        let Some(key) = delete.get("key").and_then(Value::as_str) else {
            return data_error("Each delete requires key.");
        };
        parsed_deletes.push(MetadataDelete {
            key: key.to_string(),
            value: delete.get("value").cloned(),
        });
    }

    let mut target_doc_ids: BTreeSet<String> = BTreeSet::new();
    if !document_ids.is_empty() {
        let invalid: Vec<&String> = document_ids
            .iter()
            .filter(|doc_id| {
                state
                    .docs
                    .get(doc_id)
                    .is_none_or(|document| document.kb_id != kb_id)
            })
            .collect();
        if !invalid.is_empty() {
            return data_error(&format!(
                "These documents do not belong to dataset {kb_id}: {}",
                invalid
                    .iter()
                    .map(|doc_id| doc_id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        target_doc_ids.extend(document_ids.iter().cloned());
    }

    if let Some(condition) = metadata_condition {
        let metas = state
            .document_metadata
            .flattened(std::slice::from_ref(&kb_id));
        let logic = condition
            .get("logic")
            .and_then(Value::as_str)
            .unwrap_or("and");
        let filtered: BTreeSet<String> = crate::metadata_filter::meta_filter(
            &metas,
            &crate::metadata_filter::convert_conditions(Some(condition)),
            logic,
        )
        .into_iter()
        .collect();
        target_doc_ids = target_doc_ids.intersection(&filtered).cloned().collect();
        let has_conditions = condition
            .get("conditions")
            .and_then(Value::as_array)
            .is_some_and(|conditions| !conditions.is_empty());
        if has_conditions && target_doc_ids.is_empty() {
            return Json(serde_json::json!({
                "code": 0,
                "data": { "updated": 0, "matched_docs": 0 }
            }))
            .into_response();
        }
    }

    let matched_docs = target_doc_ids.len();
    let target_doc_ids: Vec<String> = target_doc_ids.into_iter().collect();
    match state.document_metadata.batch_update(
        &kb_id,
        &target_doc_ids,
        &parsed_updates,
        &parsed_deletes,
    ) {
        Ok(updated) => Json(serde_json::json!({
            "code": 0,
            "data": { "updated": updated, "matched_docs": matched_docs }
        }))
        .into_response(),
        Err(error) => server_error(&error.to_string()),
    }
}

/// RayRAG's original flat `{doc_ids, updates, deletes}` body on
/// `/api/v1/datasets/{id}/metadata/batch`, kept as a compatible alias for the
/// contract this build shipped first; it forwards to the upstream shape above.
pub async fn batch_update_metadata(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Json(request): Json<MetadataBatchRequest>,
) -> axum::response::Response {
    if !kb_manageable(&state, &kb_id, &auth) {
        return forbidden();
    }
    if request.doc_ids.is_empty() {
        return bad_request("doc_ids is required");
    }
    if request.doc_ids.iter().any(|doc_id| {
        state
            .docs
            .get(doc_id)
            .is_none_or(|document| document.kb_id != kb_id)
    }) {
        return bad_request("One or more documents do not belong to the knowledge base");
    }
    match state.document_metadata.batch_update(
        &kb_id,
        &request.doc_ids,
        &request.updates,
        &request.deletes,
    ) {
        Ok(updated) => {
            Json(serde_json::json!({ "code": 0, "data": { "updated": updated } })).into_response()
        }
        Err(error) => server_error(&error.to_string()),
    }
}

/// Everything `apply_meta_data_filter` needs beyond the request body.
///
/// Upstream reads the question for the LLM prompt and resolves the chat model
/// from the search app's `chat_id` when the request carries one, falling back to
/// the tenant's default chat model.
pub struct MetadataFilterContext<'a> {
    pub tenant_id: &'a str,
    pub question: &'a str,
    /// Search-app `chat_id` (upstream `search_config["chat_id"]`).
    pub chat_selector: Option<&'a str>,
}

impl<'a> MetadataFilterContext<'a> {
    pub fn new(tenant_id: &'a str, question: &'a str) -> Self {
        Self {
            tenant_id,
            question,
            chat_selector: None,
        }
    }

    pub fn with_chat_selector(mut self, selector: Option<&'a str>) -> Self {
        self.chat_selector = selector;
        self
    }
}

/// Resolve RAGFlow's metadata filter into the document IDs the retriever may
/// use — upstream `common/metadata_utils.py::apply_meta_data_filter`.
///
/// All three modes are supported now: `manual` filters are evaluated locally,
/// while `auto` and `semi_auto` first ask the tenant's chat model to turn the
/// question plus the knowledge base's metadata keys into conditions
/// (`rag/prompts/generator.py::gen_meta_filter`).
///
/// `Ok(None)` means "no document restriction": that is the answer when no filter
/// was sent, and it is also upstream's answer when an `auto`/`semi_auto` filter
/// produced nothing. A `manual` filter that matches nothing answers upstream's
/// `["-999"]` sentinel instead, which the retriever reads as "match nothing".
///
/// One documented deviation: when the request also carries explicit `doc_ids`,
/// upstream *extends* them with the filter matches (`doc_ids.extend(...)`), so a
/// request that restricts to `doc_ids=[A]` and adds a metadata filter can return
/// documents the caller excluded. RayRAG keeps the intersection instead.
pub async fn resolve_metadata_doc_ids(
    state: &AppState,
    context: &MetadataFilterContext<'_>,
    kb_ids: &[String],
    explicit_doc_ids: Option<&[String]>,
    meta_data_filter: Option<&Value>,
) -> anyhow::Result<Option<Vec<String>>> {
    let Some(filter) = meta_data_filter.filter(|filter| !filter.is_null()) else {
        return Ok(explicit_doc_ids.map(<[String]>::to_vec));
    };
    // Upstream's `if not meta_data_filter` treats an empty mapping as absent.
    if filter.as_object().is_some_and(|object| object.is_empty()) {
        return Ok(explicit_doc_ids.map(<[String]>::to_vec));
    }
    let chat = match crate::metadata_filter::filter_method(filter) {
        crate::metadata_filter::FilterMethod::Auto
        | crate::metadata_filter::FilterMethod::SemiAuto => {
            Some(metadata_filter_chat_model(state, context)?)
        }
        _ => None,
    };
    let base: Vec<String> = explicit_doc_ids.map(<[String]>::to_vec).unwrap_or_default();
    let applied = crate::metadata_filter::apply_meta_data_filter(
        filter,
        context.question,
        chat.as_deref(),
        &base,
        || state.document_metadata.flattened(kb_ids),
    )
    .await?;
    let Some(matched) = applied else {
        return Ok(None);
    };
    let Some(explicit) = explicit_doc_ids else {
        return Ok(Some(matched));
    };
    let explicit: HashSet<&String> = explicit.iter().collect();
    Ok(Some(
        matched
            .into_iter()
            .filter(|doc_id| {
                explicit.contains(doc_id) || doc_id == crate::metadata_filter::NO_MATCH_SENTINEL
            })
            .collect(),
    ))
}

/// Upstream `get_model_config_from_provider_instance(search_config["chat_id"])`
/// with `get_tenant_default_model_by_type(tenant_id, CHAT)` as the fallback.
fn metadata_filter_chat_model(
    state: &AppState,
    context: &MetadataFilterContext<'_>,
) -> anyhow::Result<Arc<dyn crate::llm::ChatModel>> {
    let selector = context
        .chat_selector
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| state.tenant_models.default_chat_model(context.tenant_id));
    state
        .tenant_models
        .resolve(
            &state.providers,
            context.tenant_id,
            crate::api::tenant_models::ModelCapability::Chat,
            selector.as_deref(),
        )?
        .map(|model| Arc::new(model.llm_client()) as Arc<dyn crate::llm::ChatModel>)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Automatic metadata filters need a chat model; set a default chat model for the tenant"
            )
        })
}

pub fn enrich_metadata(
    state: &AppState,
    kb_id: &str,
    doc_id: &str,
    fields: Option<&HashSet<String>>,
) -> Option<Map<String, Value>> {
    let mut metadata = state.document_metadata.get(doc_id, kb_id)?;
    if let Some(fields) = fields {
        metadata.retain(|key, _| fields.contains(key));
    }
    (!metadata.is_empty()).then_some(metadata)
}

pub fn reference_metadata_selection(payload: &Value) -> (bool, Option<HashSet<String>>) {
    let reference = payload.get("reference_metadata").and_then(Value::as_object);
    let include = payload
        .get("include_metadata")
        .and_then(Value::as_bool)
        .or_else(|| {
            reference
                .and_then(|reference| reference.get("include"))
                .and_then(Value::as_bool)
        })
        .unwrap_or(false);
    let fields = payload
        .get("metadata_fields")
        .or_else(|| reference.and_then(|reference| reference.get("fields")))
        .map(|fields| {
            fields
                .as_array()
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        });
    (include, fields)
}

fn document_exists(state: &AppState, kb_id: &str, doc_id: &str) -> bool {
    state
        .docs
        .get(doc_id)
        .is_some_and(|document| document.kb_id == kb_id)
}

fn document_accessible(state: &AppState, auth: &AuthContext, kb_id: &str, doc_id: &str) -> bool {
    kb_accessible(state, kb_id, auth) && document_exists(state, kb_id, doc_id)
}

fn bad_request(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 400, "message": message })),
    )
        .into_response()
}

fn forbidden() -> axum::response::Response {
    (
        axum::http::StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "code": 403, "message": "Knowledge base management required" })),
    )
        .into_response()
}

fn not_found() -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "code": 404, "message": "Not found" })),
    )
        .into_response()
}

fn server_error(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "code": 500, "message": message })),
    )
        .into_response()
}

fn validate_identity(doc_id: &str, kb_id: &str) -> anyhow::Result<()> {
    if doc_id.trim().is_empty() || kb_id.trim().is_empty() {
        anyhow::bail!("Document and knowledge base IDs are required");
    }
    Ok(())
}

fn validate_records(records: &[DocumentMetadataRecord]) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    for record in records {
        validate_identity(&record.doc_id, &record.kb_id)?;
        if !ids.insert(&record.doc_id) {
            anyhow::bail!("Duplicate document metadata ID: {}", record.doc_id);
        }
    }
    Ok(())
}

fn normalize_metadata(meta_fields: Map<String, Value>) -> Map<String, Value> {
    meta_fields
        .into_iter()
        .filter_map(|(key, value)| {
            let key = key.trim().to_string();
            if key.is_empty() {
                return None;
            }
            Some((key, normalize_value(value)))
        })
        .collect()
}

fn normalize_value(value: Value) -> Value {
    match value {
        Value::Array(values) => {
            let mut normalized = Vec::new();
            for value in values {
                if let Value::String(value) = value {
                    let split = value
                        .split(['、', ',', '，', ';', '；', '|'])
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(|value| Value::String(value.into()))
                        .collect::<Vec<_>>();
                    if split.is_empty() {
                        normalized.push(Value::String(value));
                    } else {
                        normalized.extend(split);
                    }
                } else {
                    normalized.push(normalize_value(value));
                }
            }
            Value::Array(dedupe_values(normalized))
        }
        Value::Object(object) => Value::Object(normalize_metadata(object)),
        value => value,
    }
}

fn dedupe_values(values: Vec<Value>) -> Vec<Value> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(serde_json::to_string(value).unwrap_or_default()))
        .collect()
}

fn scalar_values(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(values) => values.iter().collect(),
        Value::Null => Vec::new(),
        value => vec![value],
    }
}

fn value_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn metadata_type(value: &Value) -> &'static str {
    match value {
        Value::Array(_) => "list",
        Value::Number(_) => "number",
        Value::String(value) if is_iso_datetime(value) => "time",
        _ => "string",
    }
}

fn is_iso_datetime(value: &str) -> bool {
    value.len() == 19
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value.as_bytes().get(10) == Some(&b'T')
        && value.as_bytes().get(13) == Some(&b':')
        && value.as_bytes().get(16) == Some(&b':')
        && value
            .chars()
            .enumerate()
            .all(|(index, value)| matches!(index, 4 | 7 | 10 | 13 | 16) || value.is_ascii_digit())
}

fn validate_filter(filter: &MetadataFilter) -> anyhow::Result<()> {
    if filter.key.trim().is_empty() {
        anyhow::bail!("Metadata filter key is required");
    }
    let operation = canonical_operator(&filter.op);
    if !matches!(
        operation.as_str(),
        "contains"
            | "not contains"
            | "in"
            | "not in"
            | "start with"
            | "end with"
            | "empty"
            | "not empty"
            | "="
            | "≠"
            | ">"
            | "<"
            | "≥"
            | "≤"
    ) {
        anyhow::bail!("Unsupported metadata operator: {}", filter.op);
    }
    Ok(())
}

fn canonical_operator(operator: &str) -> String {
    match operator.trim().to_lowercase().as_str() {
        "is" => "=".into(),
        "not is" | "!=" => "≠".into(),
        ">=" => "≥".into(),
        "<=" => "≤".into(),
        operator => operator.into(),
    }
}

fn metadata_matches(input: &Value, filter: &MetadataFilter) -> bool {
    let operator = canonical_operator(&filter.op);
    let inputs = scalar_values(input);
    match operator.as_str() {
        "empty" => value_is_empty(input),
        "not empty" => !value_is_empty(input),
        "not contains" => inputs
            .iter()
            .all(|input| !contains_case_insensitive(input, &filter.value)),
        "not in" => inputs.iter().all(|input| !value_in(input, &filter.value)),
        "contains" => inputs
            .iter()
            .any(|input| contains_case_insensitive(input, &filter.value)),
        "in" => inputs.iter().all(|input| value_in(input, &filter.value)),
        "start with" => joined_lowercase(input).starts_with(&lowercase_value(&filter.value)),
        "end with" => joined_lowercase(input).ends_with(&lowercase_value(&filter.value)),
        "=" | "≠" | ">" | "<" | "≥" | "≤" => inputs.first().is_some_and(|input| {
            compare_values(input, &filter.value)
                .is_some_and(|ordering| compare_ordering(ordering, &operator))
        }),
        _ => false,
    }
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        Value::Bool(value) => !value,
        Value::Number(value) => value.as_f64() == Some(0.0),
    }
}

fn contains_case_insensitive(input: &Value, expected: &Value) -> bool {
    lowercase_value(input).contains(&lowercase_value(expected))
}

fn value_in(input: &Value, expected: &Value) -> bool {
    match expected {
        Value::Array(values) => values
            .iter()
            .any(|value| lowercase_value(value) == lowercase_value(input)),
        expected => lowercase_value(expected).contains(&lowercase_value(input)),
    }
}

fn joined_lowercase(value: &Value) -> String {
    scalar_values(value)
        .into_iter()
        .map(lowercase_value)
        .collect::<String>()
}

fn lowercase_value(value: &Value) -> String {
    value_string(value).to_lowercase()
}

fn compare_values(left: &Value, right: &Value) -> Option<Ordering> {
    if let (Some(left), Some(right)) = (number_value(left), number_value(right)) {
        return left.partial_cmp(&right);
    }
    let left = lowercase_value(left);
    let right = lowercase_value(right);
    Some(left.cmp(&right))
}

fn number_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        _ => None,
    }
}

fn compare_ordering(ordering: Ordering, operator: &str) -> bool {
    match operator {
        "=" => ordering == Ordering::Equal,
        "≠" => ordering != Ordering::Equal,
        ">" => ordering == Ordering::Greater,
        "<" => ordering == Ordering::Less,
        "≥" => ordering != Ordering::Less,
        "≤" => ordering != Ordering::Greater,
        _ => false,
    }
}

fn apply_updates(meta: &mut Map<String, Value>, updates: &[MetadataUpdate]) {
    for update in updates
        .iter()
        .filter(|update| !update.key.trim().is_empty())
    {
        let match_provided = update
            .r#match
            .as_ref()
            .is_some_and(|value| !value.is_null() && value.as_str() != Some(""));
        match meta.get_mut(&update.key) {
            None if !match_provided => {
                meta.insert(update.key.clone(), update.value.clone());
            }
            Some(Value::Array(values)) if !match_provided => {
                match &update.value {
                    Value::Array(additions) => values.extend(additions.clone()),
                    value => values.push(value.clone()),
                }
                *values = dedupe_values(std::mem::take(values));
            }
            Some(Value::Array(values)) => {
                let Some(expected) = update.r#match.as_ref() else {
                    continue;
                };
                for value in values {
                    if value_string(value) == value_string(expected) {
                        *value = update.value.clone();
                    }
                }
            }
            Some(value) if !match_provided => *value = update.value.clone(),
            Some(value)
                if update
                    .r#match
                    .as_ref()
                    .is_some_and(|expected| value_string(value) == value_string(expected)) =>
            {
                *value = update.value.clone();
            }
            Some(_) => {}
            None => {}
        }
    }
}

fn apply_deletes(meta: &mut Map<String, Value>, deletes: &[MetadataDelete]) {
    for delete in deletes
        .iter()
        .filter(|delete| !delete.key.trim().is_empty())
    {
        let Some(expected) = delete.value.as_ref() else {
            meta.remove(&delete.key);
            continue;
        };
        let mut remove_field = false;
        if let Some(value) = meta.get_mut(&delete.key) {
            match value {
                Value::Array(values) => {
                    values.retain(|value| value_string(value) != value_string(expected));
                    remove_field = values.is_empty();
                }
                value => remove_field = value_string(value) == value_string(expected),
            }
        }
        if remove_field {
            meta.remove(&delete.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn record_wire_schema_uses_upstream_id_and_accepts_legacy_doc_id() {
        let record = DocumentMetadataRecord {
            doc_id: "doc-a".into(),
            kb_id: "kb-a".into(),
            meta_fields: object(serde_json::json!({ "author": "Ada" })),
        };
        let encoded = serde_json::to_value(&record).unwrap();
        assert_eq!(encoded["id"], "doc-a");
        assert_eq!(encoded["kb_id"], "kb-a");
        assert_eq!(encoded["meta_fields"]["author"], "Ada");
        assert!(encoded.get("doc_id").is_none());

        let legacy: DocumentMetadataRecord = serde_json::from_value(serde_json::json!({
            "doc_id": "legacy-doc",
            "kb_id": "legacy-kb",
            "meta_fields": {}
        }))
        .unwrap();
        assert_eq!(legacy.doc_id, "legacy-doc");
        assert_eq!(legacy.kb_id, "legacy-kb");

        let canonical: DocumentMetadataRecord = serde_json::from_value(serde_json::json!({
            "id": "canonical-doc",
            "kb_id": "canonical-kb"
        }))
        .unwrap();
        assert_eq!(canonical.doc_id, "canonical-doc");
        assert!(canonical.meta_fields.is_empty());
    }

    #[test]
    fn replacement_normalizes_lists_and_removes_old_keys() {
        let store = DocumentMetadataStore::in_memory();
        store
            .replace(
                "doc-a",
                "kb-a",
                object(serde_json::json!({
                    "authors": ["关羽、孙权", "孙权", "张辽"],
                    "old": true
                })),
            )
            .unwrap();
        let replaced = store
            .replace(
                "doc-a",
                "kb-a",
                object(serde_json::json!({ "authors": ["关羽, 张辽"] })),
            )
            .unwrap();
        assert_eq!(replaced["authors"], serde_json::json!(["关羽", "张辽"]));
        assert!(!replaced.contains_key("old"));
    }

    #[test]
    fn metadata_filter_supports_lists_comparisons_and_logic() {
        let store = DocumentMetadataStore::in_memory();
        store
            .replace(
                "doc-a",
                "kb-a",
                object(serde_json::json!({ "tags": ["Water", "Fish"], "score": 10 })),
            )
            .unwrap();
        store
            .replace(
                "doc-b",
                "kb-a",
                object(serde_json::json!({ "tags": ["Feed"], "score": 4 })),
            )
            .unwrap();
        let filters = vec![
            MetadataFilter {
                key: "tags".into(),
                op: "contains".into(),
                value: Value::String("water".into()),
            },
            MetadataFilter {
                key: "score".into(),
                op: ">=".into(),
                value: serde_json::json!(8),
            },
        ];
        assert_eq!(
            store
                .filter_doc_ids(&["kb-a".into()], &filters, "and")
                .unwrap(),
            vec!["doc-a"]
        );
        assert_eq!(
            store
                .filter_doc_ids(&["kb-a".into()], &filters, "or")
                .unwrap(),
            vec!["doc-a"]
        );
    }

    #[test]
    fn batch_update_creates_missing_rows_and_removes_empty_rows() {
        let store = DocumentMetadataStore::in_memory();
        assert_eq!(
            store
                .batch_update(
                    "kb-a",
                    &["doc-a".into()],
                    &[MetadataUpdate {
                        key: "authors".into(),
                        value: Value::String("关羽、孙权".into()),
                        r#match: None,
                    }],
                    &[],
                )
                .unwrap(),
            1
        );
        assert_eq!(
            store.get("doc-a", "kb-a").unwrap()["authors"],
            Value::String("关羽、孙权".into())
        );

        assert_eq!(
            store
                .batch_update(
                    "kb-a",
                    &["doc-a".into()],
                    &[],
                    &[MetadataDelete {
                        key: "authors".into(),
                        value: None,
                    }],
                )
                .unwrap(),
            1
        );
        assert!(store.get("doc-a", "kb-a").is_none());

        assert_eq!(
            store
                .batch_update(
                    "kb-a",
                    &["doc-b".into()],
                    &[MetadataUpdate {
                        key: "authors".into(),
                        value: Value::String("ignored".into()),
                        r#match: Some(Value::String("missing".into())),
                    }],
                    &[],
                )
                .unwrap(),
            0
        );
        assert!(store.get("doc-b", "kb-a").is_none());

        assert_eq!(
            store
                .batch_update(
                    "kb-a",
                    &["doc-c".into()],
                    &[MetadataUpdate {
                        key: "rank".into(),
                        value: Value::Number(1.into()),
                        r#match: Some(Value::Number(0.into())),
                    }],
                    &[],
                )
                .unwrap(),
            0
        );
        assert!(store.get("doc-c", "kb-a").is_none());
    }

    #[test]
    fn summary_batch_update_and_restart_are_consistent() {
        let root =
            std::env::temp_dir().join(format!("rayrag-document-metadata-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("document_metadata.json");
        let store = DocumentMetadataStore::new(path.to_str().unwrap()).unwrap();
        store
            .replace(
                "doc-a",
                "kb-a",
                object(serde_json::json!({ "tags": ["water"], "year": 2026 })),
            )
            .unwrap();
        assert_eq!(
            store
                .batch_update(
                    "kb-a",
                    &["doc-a".into()],
                    &[MetadataUpdate {
                        key: "tags".into(),
                        value: Value::String("fish".into()),
                        r#match: None,
                    }],
                    &[MetadataDelete {
                        key: "year".into(),
                        value: None,
                    }],
                )
                .unwrap(),
            1
        );
        let summary = store.summary("kb-a", None);
        assert_eq!(summary["tags"].value_type, "list");
        assert_eq!(summary["tags"].values.len(), 2);
        let persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted[0]["id"], "doc-a");
        assert!(persisted[0].get("doc_id").is_none());
        drop(store);
        let restored = DocumentMetadataStore::new(path.to_str().unwrap()).unwrap();
        assert_eq!(
            restored.get("doc-a", "kb-a").unwrap()["tags"],
            serde_json::json!(["water", "fish"])
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn persistence_failure_rolls_back_metadata_replacement() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-document-metadata-failure-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("document_metadata.json");
        let store = DocumentMetadataStore::new(path.to_str().unwrap()).unwrap();
        store
            .replace(
                "doc-a",
                "kb-a",
                object(serde_json::json!({ "author": "old" })),
            )
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(
            store
                .replace(
                    "doc-a",
                    "kb-a",
                    object(serde_json::json!({ "author": "new" })),
                )
                .is_err()
        );
        assert_eq!(store.get("doc-a", "kb-a").unwrap()["author"], "old");
        std::fs::remove_dir_all(root).ok();
    }
}
