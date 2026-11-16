//! Manual chunk CRUD, line-aligned with RAGFlow v0.27.2
//! `api/apps/restful_apis/chunk_api.py`.
//!
//! The live v0.27.2 instance answers every business result with HTTP 200 and a
//! body `code` (0 success / 100 exception / 101 argument / 102 data). Error
//! bodies carry only `code` + `message` (no `data`) unless upstream passes
//! one deliberately, and `get_result(code=0, …)` never emits a `message` key.
//! Request bodies are validated here instead of by the axum extractor so that a
//! malformed field answers upstream's `{"code":102,"message":…}` envelope
//! rather than a bare 422 rejection with no business code.

use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::chunk::token_count;
use crate::search::{IndexedChunk, SearchEngine};
use crate::server::{AppState, AuthContext, kb_accessible, kb_embedder_for, kb_manageable};

#[derive(Debug, Deserialize, Default)]
pub struct ChunkListQuery {
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
    #[serde(default)]
    pub keywords: String,
    /// Upstream reads `available` as a string and treats anything but `"true"`
    /// as unavailable, so the raw value is kept instead of a typed bool.
    #[serde(default)]
    pub available: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    /// Repeated/comma-separated `chunk_ids` filter (upstream `_get_query_id_list`).
    #[serde(default)]
    pub chunk_ids: Option<String>,
}

/// Request bodies are parsed as raw JSON because upstream's validators decide
/// the error text: a list field of the wrong type is a business `code:102`
/// message, not an axum rejection.
#[derive(Debug, Default)]
pub struct ChunkCreateRequest {
    pub content: String,
    pub important_keywords: Vec<String>,
    pub questions: Vec<String>,
    pub tag_kwd: Vec<String>,
    pub tag_feas: HashMap<String, f32>,
    pub available: bool,
    pub positions: Vec<Vec<i32>>,
}

#[derive(Debug, Default)]
pub struct ChunkUpdateRequest {
    pub content: Option<String>,
    pub important_keywords: Option<Vec<String>>,
    pub questions: Option<Vec<String>>,
    pub tag_kwd: Option<Vec<String>>,
    pub tag_feas: Option<HashMap<String, f32>>,
    pub available: Option<bool>,
    pub positions: Option<Vec<Vec<i32>>>,
}

pub async fn list_chunks(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    Query(query): Query<ChunkListQuery>,
) -> axum::response::Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return data_error(format!("You don't own the dataset {dataset_id}."));
    }
    let Some(doc) = document_for(&state, &dataset_id, &document_id) else {
        return data_error(format!("you don't own the document {document_id}"));
    };
    let mut id_filter: Vec<String> = Vec::new();
    if let Some(id) = query.id.as_ref().filter(|id| !id.trim().is_empty()) {
        id_filter.push(id.trim().to_string());
    }
    if let Some(ids) = query.chunk_ids.as_deref() {
        id_filter.extend(
            ids.split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string),
        );
    }
    let available_filter = query.available.as_deref().map(|value| value == "true");
    let mut chunks: Vec<_> = state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .into_iter()
        .filter(|chunk| chunk_belongs_to(chunk, &dataset_id, &document_id))
        .filter(|chunk| id_filter.is_empty() || id_filter.iter().any(|id| id == &chunk.id))
        .filter(|chunk| {
            available_filter.is_none_or(|available| chunk_is_available(chunk) == available)
        })
        .filter(|chunk| {
            query.keywords.trim().is_empty()
                || chunk
                    .content
                    .to_lowercase()
                    .contains(&query.keywords.trim().to_lowercase())
        })
        .collect();
    chunks.sort_by_key(|chunk| (chunk.position, chunk.id.clone()));
    let total = chunks.len();
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, 200);
    let offset = (page - 1).saturating_mul(page_size);
    let doc_token_count: usize = state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .iter()
        .filter(|chunk| chunk_belongs_to(chunk, &dataset_id, &document_id))
        .map(|chunk| chunk.token_count)
        .sum();
    let doc_entry = crate::api::document::doc_list_entry(&state, &doc, doc_token_count);
    let chunks: Vec<_> = chunks
        .into_iter()
        .skip(offset)
        .take(page_size)
        .map(|chunk| chunk_list_item(&chunk))
        .collect();
    Json(json!({
        "code": 0,
        "data": { "total": total, "chunks": chunks, "doc": doc_entry }
    }))
    .into_response()
}

pub async fn get_chunk(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id, chunk_id)): Path<(String, String, String)>,
) -> axum::response::Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return data_error(format!("You don't own the dataset {dataset_id}."));
    }
    let Some(_doc) = document_for(&state, &dataset_id, &document_id) else {
        return data_error(format!("you don't own the document {document_id}"));
    };
    match state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .into_iter()
        .find(|chunk| chunk.id == chunk_id && chunk_belongs_to(chunk, &dataset_id, &document_id))
    {
        // Upstream _strip_chunk_runtime_fields: the stored row as-is, minus the
        // runtime *_tks / *_ltks / *_vec fields.
        Some(chunk) => {
            Json(json!({ "code": 0, "data": chunk_stored_fields(&chunk) })).into_response()
        }
        None => Json(json!({ "code": 102, "message": "Chunk not found!" })).into_response(),
    }
}

/// One chunk row as upstream list_chunks serialises it
/// (chunk_api.list_chunks). RAGFlow 0.27.2 emits these keys for both the
/// id-filtered and the paged branch; tag_feas, token_count and position stay
/// alongside them for RayRAG's own chunk workbench.
fn chunk_list_item(chunk: &IndexedChunk) -> Value {
    json!({
        "id": chunk.id,
        "content": chunk.content,
        "document_id": chunk.metadata.get("doc_id").cloned().unwrap_or_default(),
        "docnm_kwd": chunk.doc_name,
        "important_keywords": metadata_keywords(chunk, "important_kwd"),
        "tag_kwd": metadata_list(chunk, "tag_kwd"),
        "questions": metadata_list(chunk, "question_kwd"),
        "dataset_id": chunk.metadata.get("kb_id").cloned().unwrap_or_default(),
        "image_id": chunk.metadata.get("img_id").cloned().unwrap_or_default(),
        "doc_type_kwd": chunk
            .metadata
            .get("doc_type_kwd")
            .cloned()
            .unwrap_or_else(|| "text".into()),
        "available": chunk_is_available(chunk),
        "positions": metadata_positions(chunk),
        "tag_feas": metadata_map(chunk, "tag_feas"),
        "token_count": chunk.token_count,
        "position": chunk.position,
    })
}

/// The stored-row face of one chunk (upstream get_chunk): source field names,
/// available_int as an integer, lists kept as lists.
fn chunk_stored_fields(chunk: &IndexedChunk) -> Value {
    let mut entry = json!({
        "id": chunk.id,
        "content_with_weight": chunk.content,
        "doc_id": chunk.metadata.get("doc_id").cloned().unwrap_or_default(),
        "kb_id": chunk.metadata.get("kb_id").cloned().unwrap_or_default(),
        "docnm_kwd": chunk.doc_name,
        "important_kwd": metadata_keywords(chunk, "important_kwd"),
        "question_kwd": metadata_list(chunk, "question_kwd"),
        "tag_kwd": metadata_list(chunk, "tag_kwd"),
        "tag_feas": metadata_map(chunk, "tag_feas"),
        "available_int": if chunk_is_available(chunk) { 1 } else { 0 },
        "position_int": metadata_positions(chunk),
        "doc_type_kwd": chunk
            .metadata
            .get("doc_type_kwd")
            .cloned()
            .unwrap_or_else(|| "text".into()),
        "img_id": chunk.metadata.get("img_id").cloned().unwrap_or_default(),
    });
    if let Some(create_time) = chunk.metadata.get("create_time") {
        entry["create_time"] = Value::String(create_time.clone());
    }
    if let Some(created_at) = chunk.metadata.get("create_timestamp_flt") {
        if let Ok(value) = created_at.parse::<f64>() {
            entry["create_timestamp_flt"] = json!(value);
        }
    }
    entry
}

/// Upstream add_chunk's answer body: get_result(data={"chunk": renamed}) with
/// the renamed key set (kb_id->dataset_id, doc_id->document_id,
/// content_with_weight->content, ...).
fn chunk_add_entry(chunk: &IndexedChunk) -> Value {
    json!({
        "id": chunk.id,
        "content": chunk.content,
        "document_id": chunk.metadata.get("doc_id").cloned().unwrap_or_default(),
        "important_keywords": metadata_keywords(chunk, "important_kwd"),
        "tag_kwd": metadata_list(chunk, "tag_kwd"),
        "questions": metadata_list(chunk, "question_kwd"),
        "dataset_id": chunk.metadata.get("kb_id").cloned().unwrap_or_default(),
        "create_time": chunk.metadata.get("create_time").cloned().unwrap_or_default(),
        "create_timestamp": chunk
            .metadata
            .get("create_timestamp_flt")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or_default(),
        "doc_type_kwd": chunk
            .metadata
            .get("doc_type_kwd")
            .cloned()
            .unwrap_or_else(|| "text".into()),
        "image_id": chunk.metadata.get("img_id").cloned().unwrap_or_default(),
        "available": chunk_is_available(chunk),
        "positions": metadata_positions(chunk),
    })
}

pub async fn add_chunk(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let request = match parse_json_object(&body) {
        Ok(request) => request,
        Err(message) => return exception_error(&message),
    };
    if !kb_accessible(&state, &dataset_id, &auth) {
        return data_error(format!("You don't own the dataset {dataset_id}."));
    }
    let Some(doc) = document_for(&state, &dataset_id, &document_id) else {
        return data_error(format!("you don't own the document {document_id}"));
    };
    let request = match parse_chunk_create(&request) {
        Ok(request) => request,
        Err(message) => return data_error(&message),
    };
    if request
        .tag_feas
        .iter()
        .any(|(tag, value)| tag.trim().is_empty() || !value.is_finite())
    {
        return data_error("tag_feas must contain non-empty tags and finite values");
    }
    let embedding = match embed_manual_chunk(
        &state,
        &dataset_id,
        &doc.name,
        &request.content,
        &request.questions,
    )
    .await
    {
        // Upstream surfaces a provider failure as {"code":100,"message":...} on
        // HTTP 200 (measured: LookupError('Provider  not found for model .')).
        Ok(embedding) => embedding,
        Err(error) => return exception_error(&error.to_string()),
    };
    let chunk_id = manual_chunk_id(&document_id, &request.content);
    let mut chunks = state.engine.read().unwrap().to_vec();
    let existing = chunks.iter().position(|chunk| {
        chunk.id == chunk_id && chunk_belongs_to(chunk, &dataset_id, &document_id)
    });
    let position = existing
        .map(|index| chunks[index].position)
        .unwrap_or_else(|| {
            chunks
                .iter()
                .filter(|chunk| chunk_belongs_to(chunk, &dataset_id, &document_id))
                .map(|chunk| chunk.position)
                .max()
                .map_or(0, |position| position + 1)
        });
    let chunk = IndexedChunk {
        id: chunk_id,
        doc_name: doc.name.clone(),
        content: request.content.trim().to_string(),
        embedding,
        token_count: token_count(&request.content),
        position,
        metadata: chunk_metadata(ChunkMetadataInput {
            dataset_id: &dataset_id,
            document_id: &document_id,
            doc_name: &doc.name,
            important_keywords: &request.important_keywords,
            questions: &request.questions,
            tag_kwd: &request.tag_kwd,
            tag_feas: &request.tag_feas,
            available: request.available,
            positions: &request.positions,
        }),
    };
    let entry = chunk_add_entry(&chunk);
    // A repeated POST of the same content resolves to the same xxh64 id; upstream
    // overwrites that row in the doc store. RayRAG replaces it without touching
    // the chunk counters in that case.
    let delta = if let Some(index) = existing {
        chunks[index] = chunk;
        0
    } else {
        chunks.push(chunk);
        1
    };
    match commit_chunk_count_change(&state, &doc, chunks, delta) {
        Ok(()) => Json(json!({ "code": 0, "data": { "chunk": entry } })).into_response(),
        Err(error) => exception_error(&error.to_string()),
    }
}

pub async fn update_chunk(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id, chunk_id)): Path<(String, String, String)>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let request = match parse_json_object(&body) {
        Ok(request) => request,
        Err(message) => return exception_error(&message),
    };
    if !kb_accessible(&state, &dataset_id, &auth) {
        return data_error(format!("You don't own the dataset {dataset_id}."));
    }
    let Some(doc) = document_for(&state, &dataset_id, &document_id) else {
        return data_error(format!("you don't own the document {document_id}"));
    };
    let request = match parse_chunk_update(&request) {
        Ok(request) => request,
        Err(message) => return data_error(&message),
    };
    let mut chunks = state.engine.read().unwrap().to_vec();
    let Some(index) = chunks.iter().position(|chunk| {
        chunk.id == chunk_id && chunk_belongs_to(chunk, &dataset_id, &document_id)
    }) else {
        return data_error(format!("Can't find this chunk {chunk_id}"));
    };
    let content = request
        .content
        .as_deref()
        .map(|content| content.trim().to_string())
        .unwrap_or_else(|| chunks[index].content.clone());
    let questions = request
        .questions
        .clone()
        .unwrap_or_else(|| metadata_list(&chunks[index], "question_kwd"));
    if request.content.is_some() || request.questions.is_some() {
        chunks[index].embedding =
            match embed_manual_chunk(&state, &dataset_id, &doc.name, &content, &questions).await {
                Ok(embedding) => embedding,
                Err(error) => return exception_error(&error.to_string()),
            };
    }
    chunks[index].content = content;
    chunks[index].token_count = token_count(&chunks[index].content);
    if let Some(values) = request.important_keywords {
        set_metadata_list(&mut chunks[index], "important_kwd", values);
    }
    if request.questions.is_some() {
        set_metadata_list(&mut chunks[index], "question_kwd", questions.clone());
        chunks[index]
            .metadata
            .insert("question_tks".into(), questions.join("\n"));
    }
    if let Some(values) = request.tag_kwd {
        set_metadata_list(&mut chunks[index], "tag_kwd", values);
    }
    if let Some(values) = request.tag_feas {
        chunks[index]
            .metadata
            .insert("tag_feas".into(), serde_json::to_string(&values).unwrap());
    }
    if let Some(available) = request.available {
        chunks[index].metadata.insert(
            "available_int".into(),
            if available { "1" } else { "0" }.into(),
        );
    }
    if let Some(positions) = request.positions {
        chunks[index].metadata.insert(
            "position_int".into(),
            serde_json::to_string(&positions).unwrap(),
        );
    }
    // Upstream update_chunk answers get_result(): {"code":0} with no data.
    match commit_index_only(&state, chunks) {
        Ok(()) => Json(json!({ "code": 0 })).into_response(),
        Err(error) => exception_error(&error.to_string()),
    }
}

pub async fn delete_chunks(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let request = match parse_optional_json_object(&body) {
        Ok(request) => request,
        Err(message) => return exception_error(&message),
    };
    if !kb_accessible(&state, &dataset_id, &auth) {
        return data_error(format!("You don't own the dataset {dataset_id}."));
    }
    let Some(doc) = document_for(&state, &dataset_id, &document_id) else {
        return data_error(format!("you don't own the document {document_id}"));
    };
    let delete_all = request
        .get("delete_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let requested: Vec<String> = request
        .get("chunk_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if requested.is_empty() && !delete_all {
        // Upstream: if not req: return get_result().
        return Json(json!({ "code": 0 })).into_response();
    }
    let unique: Vec<String> = {
        let mut seen = HashSet::new();
        requested
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .collect()
    };
    let mut chunks = state.engine.read().unwrap().to_vec();
    let before = chunks.len();
    if delete_all {
        chunks.retain(|chunk| !chunk_belongs_to(chunk, &dataset_id, &document_id));
    } else {
        let wanted: HashSet<&str> = unique.iter().map(String::as_str).collect();
        chunks.retain(|chunk| {
            !(chunk_belongs_to(chunk, &dataset_id, &document_id)
                && wanted.contains(chunk.id.as_str()))
        });
    }
    let removed = before - chunks.len();
    if !delete_all && removed != unique.len() {
        return data_error(format!(
            "rm_chunk deleted chunks {removed}, expect {}",
            unique.len()
        ));
    }
    match commit_chunk_count_change(&state, &doc, chunks, -(removed as isize)) {
        Ok(()) => Json(json!({ "code": 0, "message": format!("deleted {removed} chunks") }))
            .into_response(),
        Err(error) => exception_error(&error.to_string()),
    }
}

pub async fn switch_chunks(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let request = match parse_json_object(&body) {
        Ok(request) => request,
        Err(message) => return exception_error(&message),
    };
    if !kb_accessible(&state, &dataset_id, &auth) {
        return data_error(format!("You don't own the dataset {dataset_id}."));
    }
    let Some(chunk_ids) = request
        .get("chunk_ids")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|ids| !ids.is_empty())
    else {
        return data_error("`chunk_ids` is required.");
    };
    let available = match request.get("available_int") {
        // Upstream calls int() on the value: a value that cannot be read as an
        // integer raises and is reported as a Python exception, not silently 0.
        Some(value) => match int_flag(value) {
            Some(flag) => flag != 0,
            None => {
                return exception_error(format!(
                    "ValueError: invalid literal for int() with base 10: '{value}'"
                ));
            }
        },
        None => match request.get("available") {
            Some(value) => value.as_bool().unwrap_or(false),
            None => return data_error("`available_int` or `available` is required."),
        },
    };
    if document_for(&state, &dataset_id, &document_id).is_none() {
        return data_error("document not found");
    }
    let wanted: HashSet<&str> = chunk_ids.iter().map(String::as_str).collect();
    let mut chunks = state.engine.read().unwrap().to_vec();
    let mut updated = 0usize;
    for chunk in &mut chunks {
        if chunk_belongs_to(chunk, &dataset_id, &document_id) && wanted.contains(chunk.id.as_str())
        {
            chunk.metadata.insert(
                "available_int".into(),
                if available { "1" } else { "0" }.into(),
            );
            updated += 1;
        }
    }
    if updated != chunk_ids.len() {
        return data_error("Index updating failure");
    }
    match commit_index_only(&state, chunks) {
        Ok(()) => Json(json!({ "code": 0, "data": true })).into_response(),
        Err(error) => exception_error(&error.to_string()),
    }
}

/// Parse a request body as a JSON object; upstream raises a Python exception on
/// a malformed body, which the global handler reports as code 100.
fn parse_json_object(body: &[u8]) -> Result<Value, String> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    let value: Value =
        serde_json::from_slice(body).map_err(|error| format!("BadRequest: {error}"))?;
    if !value.is_object() {
        return Err("TypeError('JSON payload must be an object.')".into());
    }
    Ok(value)
}

/// Empty bodies are legal on the DELETE handlers (upstream get_request_json
/// normalises them to {}).
fn parse_optional_json_object(body: &[u8]) -> Result<Value, String> {
    parse_json_object(body)
}

fn int_flag(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|value| value.is_finite())
                .map(|value| value as i64)
        }),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn string_list(value: Option<&Value>, field: &str) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let items = value
        .as_array()
        .ok_or_else(|| format!("`{field}` is required to be a list"))?;
    let mut output = Vec::with_capacity(items.len());
    for item in items {
        let text = item
            .as_str()
            .ok_or_else(|| format!("`{field}` must be a list of strings"))?;
        output.push(text.to_string());
    }
    Ok(output)
}

fn float_map(value: Option<&Value>, field: &str) -> Result<HashMap<String, f32>, String> {
    let Some(value) = value else {
        return Ok(HashMap::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| format!("`{field}` must be an object"))?;
    let mut output = HashMap::with_capacity(object.len());
    for (key, value) in object {
        let number = value
            .as_f64()
            .ok_or_else(|| format!("`{field}` must contain numbers"))?;
        output.insert(key.clone(), number as f32);
    }
    Ok(output)
}

fn positions(value: Option<&Value>) -> Result<Vec<Vec<i32>>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let rows = value
        .as_array()
        .ok_or_else(|| "`positions` should be a list".to_string())?;
    let mut output = Vec::with_capacity(rows.len());
    for row in rows {
        let entries = row
            .as_array()
            .ok_or_else(|| "`positions` should be a list".to_string())?;
        let mut parsed = Vec::with_capacity(entries.len());
        for entry in entries {
            let number = entry
                .as_i64()
                .ok_or_else(|| "Each position must contain integers".to_string())?;
            parsed.push(i32::try_from(number).map_err(|_| "Position value out of range")?);
        }
        if parsed.len() != 5 {
            return Err("Each position must contain five integers".into());
        }
        output.push(parsed);
    }
    Ok(output)
}

fn parse_chunk_create(request: &Value) -> Result<ChunkCreateRequest, String> {
    if request.get("image_base64").is_some() {
        return Err("image_base64 chunks are not supported by RayRAG yet".into());
    }
    let content = request.get("content").and_then(Value::as_str).unwrap_or("");
    if content.trim().is_empty() {
        return Err("`content` is required".into());
    }
    Ok(ChunkCreateRequest {
        content: content.to_string(),
        important_keywords: string_list(request.get("important_keywords"), "important_keywords")?,
        questions: string_list(request.get("questions"), "questions")?,
        tag_kwd: string_list(request.get("tag_kwd"), "tag_kwd")?,
        tag_feas: float_map(request.get("tag_feas"), "tag_feas")?,
        available: true,
        positions: positions(request.get("positions"))?,
    })
}

fn parse_chunk_update(request: &Value) -> Result<ChunkUpdateRequest, String> {
    if request.get("image_base64").is_some() {
        return Err("image_base64 chunks are not supported by RayRAG yet".into());
    }
    if let Some(content) = request.get("content") {
        let text = content
            .as_str()
            .ok_or_else(|| "`content` is required".to_string())?;
        if text.trim().is_empty() {
            return Err("`content` is required".into());
        }
    }
    Ok(ChunkUpdateRequest {
        content: request
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string),
        important_keywords: request
            .get("important_keywords")
            .map(|value| string_list(Some(value), "important_keywords"))
            .transpose()?,
        questions: request
            .get("questions")
            .map(|value| string_list(Some(value), "questions"))
            .transpose()?,
        tag_kwd: request
            .get("tag_kwd")
            .map(|value| string_list(Some(value), "tag_kwd"))
            .transpose()?,
        tag_feas: request
            .get("tag_feas")
            .map(|value| float_map(Some(value), "tag_feas"))
            .transpose()?,
        available: request.get("available").map(|value| match value {
            Value::Bool(flag) => *flag,
            Value::Number(number) => number.as_i64().unwrap_or(0) != 0,
            Value::String(text) => text == "true" || text == "1",
            _ => false,
        }),
        positions: request
            .get("positions")
            .map(|value| positions(Some(value)))
            .transpose()?,
    })
}

async fn embed_manual_chunk(
    state: &AppState,
    dataset_id: &str,
    doc_name: &str,
    content: &str,
    questions: &[String],
) -> anyhow::Result<Vec<f32>> {
    let embedder = kb_embedder_for(state, &[dataset_id.to_string()])?;
    let body = if questions.is_empty() {
        content
    } else {
        &questions.join("\n")
    };
    let embeddings = embedder.embed(&[doc_name, body]).await?;
    if embeddings.len() != 2 || embeddings[0].len() != embeddings[1].len() {
        anyhow::bail!("Embedding provider returned incompatible vectors");
    }
    Ok(embeddings[0]
        .iter()
        .zip(&embeddings[1])
        .map(|(title, body)| 0.1 * title + 0.9 * body)
        .collect())
}

fn commit_index_only(state: &AppState, chunks: Vec<IndexedChunk>) -> anyhow::Result<()> {
    let _commit_guard = state.document_commit_lock.lock().unwrap();
    let mut engine = state.engine.write().unwrap();
    let previous = engine.to_vec();
    *engine = SearchEngine::from_chunks(chunks);
    crate::server::persist_index_change(state, &mut engine, &previous)
}

fn commit_chunk_count_change(
    state: &AppState,
    doc: &crate::api::document::DocRecord,
    chunks: Vec<IndexedChunk>,
    delta: isize,
) -> anyhow::Result<()> {
    let _commit_guard = state.document_commit_lock.lock().unwrap();
    let mut engine = state.engine.write().unwrap();
    let previous_index = engine.to_vec();
    let previous_doc_count = doc.chunk_count;
    let new_doc_count = (previous_doc_count as isize + delta).max(0) as usize;
    *engine = SearchEngine::from_chunks(chunks);
    crate::server::persist_index_change(state, &mut engine, &previous_index)?;
    if let Err(error) = state.docs.set_chunk_count(&doc.id, new_doc_count) {
        rollback_index(state, &mut engine, previous_index);
        return Err(error);
    }
    if let Err(error) = state.kbs.update_counts(&doc.kb_id, delta, 0) {
        state.docs.set_chunk_count(&doc.id, previous_doc_count).ok();
        rollback_index(state, &mut engine, previous_index);
        return Err(error);
    }
    Ok(())
}

fn rollback_index(state: &AppState, engine: &mut SearchEngine, previous: Vec<IndexedChunk>) {
    crate::store::rollback_online_index(
        engine,
        &state.index_path,
        &state.vector_mirror,
        &previous,
        "Chunk mutation rollback",
    );
}

struct ChunkMetadataInput<'a> {
    dataset_id: &'a str,
    document_id: &'a str,
    doc_name: &'a str,
    important_keywords: &'a [String],
    questions: &'a [String],
    tag_kwd: &'a [String],
    tag_feas: &'a HashMap<String, f32>,
    available: bool,
    positions: &'a [Vec<i32>],
}

fn chunk_metadata(input: ChunkMetadataInput<'_>) -> HashMap<String, String> {
    HashMap::from([
        (
            "create_time".into(),
            crate::api::compat::ms_to_datetime_string(now_ms()),
        ),
        (
            "create_timestamp_flt".into(),
            format!("{:.3}", now_ms() as f64 / 1000.0),
        ),
        ("doc_id".into(), input.document_id.into()),
        ("kb_id".into(), input.dataset_id.into()),
        ("file_name".into(), input.doc_name.into()),
        ("content_type".into(), "text".into()),
        ("important_kwd".into(), input.important_keywords.join(" ")),
        (
            "question_kwd".into(),
            serde_json::to_string(input.questions).unwrap(),
        ),
        ("question_tks".into(), input.questions.join("\n")),
        (
            "tag_kwd".into(),
            serde_json::to_string(input.tag_kwd).unwrap(),
        ),
        (
            "tag_feas".into(),
            serde_json::to_string(input.tag_feas).unwrap(),
        ),
        (
            "available_int".into(),
            if input.available { "1" } else { "0" }.into(),
        ),
        (
            "position_int".into(),
            serde_json::to_string(input.positions).unwrap(),
        ),
        ("manual_chunk".into(), "1".into()),
    ])
}

fn document_for(
    state: &AppState,
    dataset_id: &str,
    document_id: &str,
) -> Option<crate::api::document::DocRecord> {
    state
        .docs
        .get(document_id)
        .filter(|doc| doc.kb_id == dataset_id)
}

fn chunk_belongs_to(chunk: &IndexedChunk, dataset_id: &str, document_id: &str) -> bool {
    chunk.metadata.get("kb_id").map(String::as_str) == Some(dataset_id)
        && chunk.metadata.get("doc_id").map(String::as_str) == Some(document_id)
}

fn chunk_is_available(chunk: &IndexedChunk) -> bool {
    chunk
        .metadata
        .get("available_int")
        .map(String::as_str)
        .is_none_or(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
}

fn metadata_list(chunk: &IndexedChunk, key: &str) -> Vec<String> {
    chunk
        .metadata
        .get(key)
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default()
}

fn metadata_keywords(chunk: &IndexedChunk, key: &str) -> Vec<String> {
    chunk
        .metadata
        .get(key)
        .map(|value| value.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn metadata_map(chunk: &IndexedChunk, key: &str) -> HashMap<String, f32> {
    chunk
        .metadata
        .get(key)
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default()
}

fn metadata_positions(chunk: &IndexedChunk) -> Vec<Vec<i32>> {
    chunk
        .metadata
        .get("position_int")
        .and_then(|value| serde_json::from_str(value).ok())
        .unwrap_or_default()
}

fn set_metadata_list(chunk: &mut IndexedChunk, key: &str, values: Vec<String>) {
    let value = if key == "important_kwd" {
        values.join(" ")
    } else {
        serde_json::to_string(&values).unwrap()
    };
    chunk.metadata.insert(key.into(), value);
}

fn manual_chunk_id(document_id: &str, content: &str) -> String {
    format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(format!("{}{}", content.trim(), document_id).as_bytes())
    )
}

fn default_page() -> usize {
    1
}

fn default_page_size() -> usize {
    30
}

fn data_error(message: impl AsRef<str>) -> axum::response::Response {
    Json(json!({ "code": 102, "message": message.as_ref() })).into_response()
}

#[allow(dead_code)]
fn argument_error(message: impl AsRef<str>) -> axum::response::Response {
    Json(json!({ "code": 101, "message": message.as_ref() })).into_response()
}

fn exception_error(message: impl AsRef<str>) -> axum::response::Response {
    Json(json!({ "code": 100, "message": message.as_ref() })).into_response()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}
