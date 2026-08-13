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

/// Resolve RAGFlow's manual metadata filter and intersect it with explicit document IDs.
pub fn resolve_metadata_doc_ids(
    state: &AppState,
    kb_ids: &[String],
    explicit_doc_ids: Option<&[String]>,
    meta_data_filter: Option<&Value>,
) -> anyhow::Result<Option<Vec<String>>> {
    let Some(filter) = meta_data_filter.filter(|filter| !filter.is_null()) else {
        return Ok(explicit_doc_ids.map(<[String]>::to_vec));
    };
    let method = filter
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("manual");
    if method != "manual" {
        anyhow::bail!("Only manual metadata filters are supported without an LLM filter generator");
    }
    let filters: Vec<MetadataFilter> = serde_json::from_value(
        filter
            .get("manual")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())),
    )?;
    if filters.is_empty() {
        return Ok(explicit_doc_ids.map(<[String]>::to_vec));
    }
    let logic = filter.get("logic").and_then(Value::as_str).unwrap_or("and");
    let matched = state
        .document_metadata
        .filter_doc_ids(kb_ids, &filters, logic)?;
    if let Some(explicit) = explicit_doc_ids {
        let explicit: HashSet<_> = explicit.iter().collect();
        return Ok(Some(
            matched
                .into_iter()
                .filter(|doc_id| explicit.contains(doc_id))
                .collect(),
        ));
    }
    Ok(Some(matched))
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
