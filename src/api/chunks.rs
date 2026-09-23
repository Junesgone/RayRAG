//! Manual chunk CRUD compatible with RAGFlow's dataset/document chunk API.

use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
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
    #[serde(default)]
    pub available: Option<bool>,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkCreateRequest {
    pub content: String,
    #[serde(default)]
    pub important_keywords: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub tag_kwd: Vec<String>,
    #[serde(default)]
    pub tag_feas: HashMap<String, f32>,
    #[serde(default = "default_true")]
    pub available: bool,
    #[serde(default)]
    pub positions: Vec<Vec<i32>>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkUpdateRequest {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub important_keywords: Option<Vec<String>>,
    #[serde(default)]
    pub questions: Option<Vec<String>>,
    #[serde(default)]
    pub tag_kwd: Option<Vec<String>>,
    #[serde(default)]
    pub tag_feas: Option<HashMap<String, f32>>,
    #[serde(default)]
    pub available: Option<bool>,
    #[serde(default)]
    pub positions: Option<Vec<Vec<i32>>>,
}

#[derive(Debug, Deserialize)]
pub struct ChunkDeleteRequest {
    #[serde(default)]
    pub chunk_ids: Vec<String>,
    #[serde(default)]
    pub delete_all: bool,
}

#[derive(Debug, Deserialize)]
pub struct ChunkSwitchRequest {
    pub chunk_ids: Vec<String>,
    #[serde(alias = "available_int")]
    pub available: bool,
}

#[derive(Debug, Serialize)]
pub struct ChunkResponse {
    pub id: String,
    pub content: String,
    pub document_id: String,
    pub dataset_id: String,
    pub docnm_kwd: String,
    pub important_keywords: Vec<String>,
    pub questions: Vec<String>,
    pub tag_kwd: Vec<String>,
    pub tag_feas: HashMap<String, f32>,
    pub available: bool,
    pub positions: Vec<Vec<i32>>,
    pub token_count: usize,
    pub position: usize,
}

pub async fn list_chunks(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    Query(query): Query<ChunkListQuery>,
) -> axum::response::Response {
    let Some(doc) = document_for(&state, &dataset_id, &document_id) else {
        return not_found("Document not found");
    };
    if !kb_accessible(&state, &dataset_id, &auth) {
        return not_found("Document not found");
    }
    let mut chunks: Vec<_> = state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .into_iter()
        .filter(|chunk| chunk_belongs_to(chunk, &dataset_id, &document_id))
        .filter(|chunk| query.id.as_ref().is_none_or(|id| chunk.id == *id))
        .filter(|chunk| {
            query
                .available
                .is_none_or(|available| chunk_is_available(chunk) == available)
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
    let chunks: Vec<_> = chunks
        .into_iter()
        .skip(offset)
        .take(page_size)
        .map(chunk_response)
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": { "total": total, "chunks": chunks, "doc": doc }
    }))
    .into_response()
}

pub async fn get_chunk(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id, chunk_id)): Path<(String, String, String)>,
) -> axum::response::Response {
    if document_for(&state, &dataset_id, &document_id).is_none()
        || !kb_accessible(&state, &dataset_id, &auth)
    {
        return not_found("Chunk not found");
    }
    match state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .into_iter()
        .find(|chunk| chunk.id == chunk_id && chunk_belongs_to(chunk, &dataset_id, &document_id))
    {
        Some(chunk) => {
            Json(serde_json::json!({ "code": 0, "data": chunk_response(chunk) })).into_response()
        }
        None => not_found("Chunk not found"),
    }
}

pub async fn add_chunk(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    Json(request): Json<ChunkCreateRequest>,
) -> axum::response::Response {
    let Some(doc) = document_for(&state, &dataset_id, &document_id) else {
        return not_found("Document not found");
    };
    if !kb_manageable(&state, &dataset_id, &auth) {
        return forbidden();
    }
    if let Err(error) = validate_content(&request.content)
        .and_then(|_| validate_positions(&request.positions))
        .and_then(|_| validate_tag_features(&request.tag_feas))
    {
        return bad_request(&error.to_string());
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
        Ok(embedding) => embedding,
        Err(error) => return service_error(&error.to_string()),
    };
    let chunk_id = manual_chunk_id(&document_id, &request.content);
    let mut chunks = state.engine.read().unwrap().to_vec();
    if chunks
        .iter()
        .any(|chunk| chunk.id == chunk_id && chunk_belongs_to(chunk, &dataset_id, &document_id))
    {
        return conflict("Chunk already exists");
    }
    let position = chunks
        .iter()
        .filter(|chunk| chunk_belongs_to(chunk, &dataset_id, &document_id))
        .map(|chunk| chunk.position)
        .max()
        .map_or(0, |position| position + 1);
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
    chunks.push(chunk.clone());
    match commit_chunk_count_change(&state, &doc, chunks, 1) {
        Ok(()) => (
            axum::http::StatusCode::CREATED,
            Json(serde_json::json!({ "code": 0, "data": { "chunk": chunk_response(chunk) } })),
        )
            .into_response(),
        Err(error) => server_error(&error.to_string()),
    }
}

pub async fn update_chunk(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id, chunk_id)): Path<(String, String, String)>,
    Json(request): Json<ChunkUpdateRequest>,
) -> axum::response::Response {
    let Some(doc) = document_for(&state, &dataset_id, &document_id) else {
        return not_found("Document not found");
    };
    if !kb_manageable(&state, &dataset_id, &auth) {
        return forbidden();
    }
    if let Some(content) = request.content.as_deref()
        && let Err(error) = validate_content(content)
    {
        return bad_request(&error.to_string());
    }
    if let Some(positions) = request.positions.as_deref()
        && let Err(error) = validate_positions(positions)
    {
        return bad_request(&error.to_string());
    }
    if let Some(tag_feas) = request.tag_feas.as_ref()
        && let Err(error) = validate_tag_features(tag_feas)
    {
        return bad_request(&error.to_string());
    }
    let mut chunks = state.engine.read().unwrap().to_vec();
    let Some(index) = chunks.iter().position(|chunk| {
        chunk.id == chunk_id && chunk_belongs_to(chunk, &dataset_id, &document_id)
    }) else {
        return not_found("Chunk not found");
    };
    let content = request
        .content
        .as_deref()
        .unwrap_or(&chunks[index].content)
        .trim()
        .to_string();
    let questions = request
        .questions
        .clone()
        .unwrap_or_else(|| metadata_list(&chunks[index], "question_kwd"));
    if request.content.is_some() || request.questions.is_some() {
        chunks[index].embedding =
            match embed_manual_chunk(&state, &dataset_id, &doc.name, &content, &questions).await {
                Ok(embedding) => embedding,
                Err(error) => return service_error(&error.to_string()),
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
    let response = chunk_response(chunks[index].clone());
    match commit_index_only(&state, chunks) {
        Ok(()) => Json(serde_json::json!({ "code": 0, "data": response })).into_response(),
        Err(error) => server_error(&error.to_string()),
    }
}

pub async fn delete_chunks(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    Json(request): Json<ChunkDeleteRequest>,
) -> axum::response::Response {
    let Some(doc) = document_for(&state, &dataset_id, &document_id) else {
        return not_found("Document not found");
    };
    if !kb_manageable(&state, &dataset_id, &auth) {
        return forbidden();
    }
    let requested: HashSet<_> = request
        .chunk_ids
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect();
    if !request.delete_all && requested.is_empty() {
        return bad_request("chunk_ids or delete_all=true is required");
    }
    let mut chunks = state.engine.read().unwrap().to_vec();
    let before = chunks.len();
    chunks.retain(|chunk| {
        !chunk_belongs_to(chunk, &dataset_id, &document_id)
            || (!request.delete_all && !requested.contains(&chunk.id))
    });
    let removed = before - chunks.len();
    if !request.delete_all && removed != requested.len() {
        return not_found("One or more chunks were not found");
    }
    match commit_chunk_count_change(&state, &doc, chunks, -(removed as isize)) {
        Ok(()) => Json(serde_json::json!({
            "code": 0,
            "data": { "deleted": removed }
        }))
        .into_response(),
        Err(error) => server_error(&error.to_string()),
    }
}

pub async fn switch_chunks(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, document_id)): Path<(String, String)>,
    Json(request): Json<ChunkSwitchRequest>,
) -> axum::response::Response {
    if document_for(&state, &dataset_id, &document_id).is_none() {
        return not_found("Document not found");
    }
    if !kb_manageable(&state, &dataset_id, &auth) {
        return forbidden();
    }
    let requested: HashSet<_> = request
        .chunk_ids
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect();
    if requested.is_empty() {
        return bad_request("chunk_ids is required");
    }
    let mut chunks = state.engine.read().unwrap().to_vec();
    let mut updated = 0;
    for chunk in &mut chunks {
        if chunk_belongs_to(chunk, &dataset_id, &document_id) && requested.contains(&chunk.id) {
            chunk.metadata.insert(
                "available_int".into(),
                if request.available { "1" } else { "0" }.into(),
            );
            updated += 1;
        }
    }
    if updated != requested.len() {
        return not_found("One or more chunks were not found");
    }
    match commit_index_only(&state, chunks) {
        Ok(()) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Err(error) => server_error(&error.to_string()),
    }
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

fn chunk_response(chunk: IndexedChunk) -> ChunkResponse {
    let important_keywords = metadata_keywords(&chunk, "important_kwd");
    let questions = metadata_list(&chunk, "question_kwd");
    let tag_kwd = metadata_list(&chunk, "tag_kwd");
    let tag_feas = metadata_map(&chunk, "tag_feas");
    let available = chunk_is_available(&chunk);
    let positions = metadata_positions(&chunk);
    ChunkResponse {
        id: chunk.id,
        content: chunk.content,
        document_id: chunk.metadata.get("doc_id").cloned().unwrap_or_default(),
        dataset_id: chunk.metadata.get("kb_id").cloned().unwrap_or_default(),
        docnm_kwd: chunk.doc_name,
        important_keywords,
        questions,
        tag_kwd,
        tag_feas,
        available,
        positions,
        token_count: chunk.token_count,
        position: chunk.position,
    }
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

fn validate_content(content: &str) -> anyhow::Result<()> {
    if content.trim().is_empty() {
        anyhow::bail!("content is required");
    }
    Ok(())
}

fn validate_positions(positions: &[Vec<i32>]) -> anyhow::Result<()> {
    if positions.iter().any(|position| position.len() != 5) {
        anyhow::bail!("Each position must contain five integers");
    }
    Ok(())
}

fn validate_tag_features(features: &HashMap<String, f32>) -> anyhow::Result<()> {
    if features
        .iter()
        .any(|(tag, value)| tag.trim().is_empty() || !value.is_finite())
    {
        anyhow::bail!("tag_feas must contain non-empty tags and finite values");
    }
    Ok(())
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

fn default_true() -> bool {
    true
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

fn not_found(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "code": 404, "message": message })),
    )
        .into_response()
}

fn conflict(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::CONFLICT,
        Json(serde_json::json!({ "code": 409, "message": message })),
    )
        .into_response()
}

fn service_error(message: &str) -> axum::response::Response {
    (
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "code": 503, "message": message })),
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
