//! Chunk manager, Chat system, Provider mgr, Memory, Stats, Task queue, Agent Canvas.
//! Combined module replacing RAGFlow's chunk_api + chat_api + provider_api + memory + stats + task + agent.

use anyhow::Context;
use axum::{
    Json,
    extract::{Extension, Path, Query, RawQuery, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use std::sync::{Mutex, RwLock};

use crate::server::{
    AppState, AuthContext, all_kbs_accessible, kb_accessible, kb_embedder_for, kb_reranker_for,
    validate_kb_embedding_bindings,
};

// ── Chunk Management ────────────────────────────────────────────

#[derive(Serialize)]
pub struct ChunkInfo {
    pub id: String,
    pub content: String,
    pub content_type: String,
    pub doc_name: String,
    pub token_count: usize,
    pub position: usize,
}

/// GET /api/v1/chunks/{doc_id} — list chunks for a document
pub async fn list_chunks(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    let Some(doc) = state.docs.get(&doc_id) else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Not found" })),
        )
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Not found" })),
        )
            .into_response();
    }
    let engine = state.engine.read().unwrap();
    let chunks: Vec<ChunkInfo> = engine
        .to_vec()
        .into_iter()
        .filter(|chunk| chunk.metadata.get("doc_id") == Some(&doc_id))
        .map(|chunk| ChunkInfo {
            id: chunk.id,
            content: chunk.content,
            content_type: chunk
                .metadata
                .get("content_type")
                .cloned()
                .unwrap_or_default(),
            doc_name: chunk.doc_name,
            token_count: chunk.token_count,
            position: chunk.position,
        })
        .collect();
    Json(serde_json::json!({ "code": 0, "data": chunks })).into_response()
}

// ── Full Chat System ────────────────────────────────────────────

#[derive(Default)]
pub struct AgentRunRegistry {
    active: Mutex<HashMap<String, ActiveAgentRun>>,
}

struct ActiveAgentRun {
    run_id: String,
    cancel: tokio::sync::watch::Sender<bool>,
}

struct AgentRunLease {
    registry: Arc<AgentRunRegistry>,
    canvas_id: String,
    run_id: String,
}

impl AgentRunRegistry {
    fn register(
        self: &Arc<Self>,
        canvas_id: &str,
        run_id: String,
        cancel: tokio::sync::watch::Sender<bool>,
    ) -> AgentRunLease {
        let previous = self.active.lock().unwrap().insert(
            canvas_id.to_owned(),
            ActiveAgentRun {
                run_id: run_id.clone(),
                cancel,
            },
        );
        if let Some(previous) = previous {
            let _ = previous.cancel.send(true);
        }
        AgentRunLease {
            registry: self.clone(),
            canvas_id: canvas_id.to_owned(),
            run_id,
        }
    }

    #[cfg(test)]
    fn contains(&self, canvas_id: &str) -> bool {
        self.active.lock().unwrap().contains_key(canvas_id)
    }
}

impl Drop for AgentRunLease {
    fn drop(&mut self) {
        let mut active = self.registry.active.lock().unwrap();
        if active
            .get(&self.canvas_id)
            .is_some_and(|run| run.run_id == self.run_id)
        {
            active.remove(&self.canvas_id);
        }
    }
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub conversation_id: Option<String>,
    pub question: String,
    pub kb_ids: Option<Vec<String>>,
    #[serde(default)]
    pub chat_model: Option<String>,
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(default)]
    pub tenant_id: Option<String>,
    /// Bind the new conversation to a chat app (RAGFlow 0.26.4 /chats model).
    #[serde(default)]
    pub app_id: Option<String>,
    #[serde(flatten)]
    pub generation: crate::generation_params::GenerationParamsPatch,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Deserialize)]
pub struct AgentChatRequest {
    pub conversation_id: Option<String>,
    #[serde(alias = "query")]
    pub question: String,
    pub kb_ids: Option<Vec<String>>,
    /// Runtime values consumed by the Canvas Begin component.
    #[serde(default)]
    pub inputs: serde_json::Map<String, serde_json::Value>,
    /// Opaque server-issued guard for resuming a waiting UserFillUp node.
    #[serde(default, alias = "checkpoint_id")]
    pub resume_token: Option<String>,
    /// Explicit scalar/object payload for the paused UserFillUp node.
    #[serde(default)]
    pub resume_data: Option<serde_json::Value>,
    /// Emit the RAGFlow agent-canvas SSE envelope. The legacy RayRAG JSON
    /// response remains the default until all live-stream/cancellation
    /// semantics are implemented.
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub tenant_id: Option<String>,
    #[serde(flatten)]
    pub generation: crate::generation_params::GenerationParamsPatch,
}

#[derive(Serialize)]
pub struct ChatResponse {
    pub answer: String,
    pub conversation_id: String,
    pub message_id: String,
    pub citations: Vec<String>,
    pub reference: Vec<crate::llm::ChunkReference>,
}

#[derive(Deserialize)]
pub struct ChatUpdateRequest {
    pub name: String,
}

#[derive(Deserialize, Default)]
pub struct RegenerateRequest {
    pub kb_ids: Option<Vec<String>>,
    #[serde(default)]
    pub chat_model: Option<String>,
    #[serde(default)]
    pub embedding_model: Option<String>,
}

pub(crate) struct GeneratedChatAnswer {
    pub(crate) answer: String,
    pub(crate) citations: Vec<String>,
    pub(crate) references: Vec<crate::llm::ChunkReference>,
    pub(crate) usage: Option<crate::llm::TokenUsage>,
}

pub(crate) struct ChatGenerationRequest<'a> {
    pub(crate) question: &'a str,
    pub(crate) kb_ids: &'a [String],
    pub(crate) chat_model: Option<&'a str>,
    pub(crate) embedding_model: Option<&'a str>,
    pub(crate) history: &'a [crate::llm::ChatMessage],
    pub(crate) generation: crate::generation_params::GenerationParamsPatch,
}

/// POST /api/v1/chats — create new chat session
pub async fn create_chat(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<ChatRequest>,
) -> axum::response::Response {
    let kb_ids = body.kb_ids.unwrap_or_default();
    if !kb_ids.is_empty() && !all_kbs_accessible(&state, &kb_ids, &auth) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "One or more knowledge bases are not accessible"
            })),
        )
            .into_response();
    }
    let tenant_id = body.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if !state.tenants.is_member(tenant_id, &auth.user_id) {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "code": 403,
                "message": "Tenant membership required"
            })),
        )
            .into_response();
    }
    match state.conversations.create_for_tenant_settings(
        &auth.user_id,
        tenant_id,
        &body.question.chars().take(30).collect::<String>(),
        kb_ids,
        body.chat_model,
        body.embedding_model,
    ) {
        Ok(session) => {
            if let Some(app_id) = body.app_id.as_deref().filter(|a| !a.is_empty()) {
                let _ = state.conversations.set_app_id(&session.id, app_id);
            }
            Json(
                serde_json::json!({ "code": 0, "data": { "id": session.id, "name": session.name } }),
            )
            .into_response()
        }
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

/// GET /api/v1/chats — list the authenticated user's sessions.
pub async fn list_chats(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> impl IntoResponse {
    Json(serde_json::json!({ "code": 0, "data": state.conversations.list_for(&auth.user_id) }))
}

/// GET /api/v1/chats/{id} — get one owned session.
pub async fn get_chat(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> axum::response::Response {
    match state.conversations.get_for(&session_id, &auth.user_id) {
        Some(session) => Json(serde_json::json!({ "code": 0, "data": session })).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Chat not found" })),
        )
            .into_response(),
    }
}

/// PATCH /api/v1/chats/{id} — rename one owned session.
pub async fn update_chat(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(body): Json<ChatUpdateRequest>,
) -> axum::response::Response {
    match state
        .conversations
        .rename_for(&session_id, &auth.user_id, &body.name)
    {
        Ok(Some(session)) => {
            Json(serde_json::json!({ "code": 0, "data": session })).into_response()
        }
        Ok(None) => chat_not_found(),
        Err(error) if error.to_string().contains("cannot be empty") => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
        )
            .into_response(),
        Err(error) => chat_server_error(error),
    }
}

/// DELETE /api/v1/chats/{id} — delete one owned session.
pub async fn delete_chat(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> axum::response::Response {
    if state
        .conversations
        .get_for(&session_id, &auth.user_id)
        .is_none()
    {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Chat not found" })),
        )
            .into_response();
    }
    match state.conversations.delete_for(&session_id, &auth.user_id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Chat not found" })),
        )
            .into_response(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /api/v1/chats/{session_id}/messages/{message_id} — delete one turn.
pub async fn delete_chat_message(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((session_id, message_id)): Path<(String, String)>,
) -> axum::response::Response {
    delete_owned_message_pair(&state, &auth, &session_id, &message_id)
}

/// RAGFlow-compatible message deletion route.
pub async fn delete_chat_session_message(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((_chat_id, session_id, message_id)): Path<(String, String, String)>,
) -> axum::response::Response {
    delete_owned_message_pair(&state, &auth, &session_id, &message_id)
}

fn delete_owned_message_pair(
    state: &AppState,
    auth: &AuthContext,
    session_id: &str,
    message_id: &str,
) -> axum::response::Response {
    match state
        .conversations
        .delete_message_pair_for(session_id, &auth.user_id, message_id)
    {
        Ok(Some(session)) => {
            Json(serde_json::json!({ "code": 0, "data": session })).into_response()
        }
        Ok(None) => chat_not_found(),
        Err(error) => chat_server_error(error),
    }
}

#[derive(Deserialize)]
pub struct SearchAskRequest {
    pub question: String,
    #[serde(default)]
    pub stream: bool,
}

/// POST /api/v1/searches/{search_id}/completions — upstream
/// `api.searchCompletion(searchId)` consumed by `pages/next-search/hooks.ts`
/// (`useSendMessageWithSse`): the AI-summary answer of a search app. The
/// retrieval scope and generation configuration come from the app
/// (`kb_ids` / `chat_id` / `llm_setting`, the upstream defaults), and the answer
/// streams as the same `chat.completion.chunk` events the chat page already
/// parses, followed by a `chat.completion.references` event carrying the
/// citations.
pub async fn search_complete(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(search_id): Path<String>,
    Json(body): Json<SearchAskRequest>,
) -> axum::response::Response {
    let Some(app) = state
        .search_apps
        .as_ref()
        .and_then(|store| store.get(&search_id))
    else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Search app not found" })),
        )
            .into_response();
    };
    let question = body.question.trim().to_string();
    if question.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": "question is required" })),
        )
            .into_response();
    }
    if !app.kb_ids.is_empty() && !all_kbs_accessible(&state, &app.kb_ids, &auth) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "One or more knowledge bases are not accessible"
            })),
        )
            .into_response();
    }
    // Upstream merges `search_config.llm_setting` (minus the nested `parameter`
    // payload) into the generation parameters, defaulting the temperature to
    // 0.9 exactly like the related-question path.
    let mut generation = crate::generation_params::GenerationParamsPatch {
        temperature: Some(0.9),
        ..Default::default()
    };
    if let Some(settings) = app.llm_setting.as_object() {
        let mut cleaned = settings.clone();
        cleaned.remove("parameter");
        if !cleaned.is_empty()
            && let Ok(mut patch) = crate::generation_params::GenerationParamsPatch::from_request(
                &serde_json::Value::Object(cleaned),
            )
        {
            if patch.temperature.is_none() {
                patch.temperature = Some(0.9);
            }
            generation = patch;
        }
    }
    // `chat_id` is the app's chat assistant; only a model selector can be fed
    // to the generator directly, otherwise the tenant default chat model applies.
    let chat_model = app.chat_id.contains('@').then(|| app.chat_id.clone());
    let kb_ids = app.kb_ids.clone();
    let state2 = state.clone();
    let auth2 = auth.clone();
    if !body.stream {
        let generated = generate_chat_answer(
            &state2,
            &auth2,
            ChatGenerationRequest {
                question: &question,
                kb_ids: &kb_ids,
                chat_model: chat_model.as_deref(),
                embedding_model: None,
                history: &[],
                generation,
            },
            None,
        )
        .await;
        return match generated {
            Ok(generated) => Json(serde_json::json!({
                "code": 0,
                "data": {
                    "answer": generated.answer,
                    "reference": references_payload(&generated.references),
                }
            }))
            .into_response(),
            Err(error) => (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
            )
                .into_response(),
        };
    }
    use axum::response::sse::{Event, KeepAlive, Sse};
    use std::convert::Infallible;
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(16);
    tokio::spawn(async move {
        let id = format!("searchcmpl-{}", uuid::Uuid::new_v4());
        let id2 = id.clone();
        let on_chunk: Arc<dyn Fn(&str) + Send + Sync> = {
            let tx = tx.clone();
            Arc::new(move |chunk: &str| {
                let event = serde_json::json!({
                    "id": id2.clone(),
                    "object": "chat.completion.chunk",
                    "choices": [{"index": 0, "delta": {"content": chunk}, "finish_reason": null}],
                });
                let _ = tx.blocking_send(Ok(Event::default().data(event.to_string())));
            })
        };
        let generated = generate_chat_answer(
            &state2,
            &auth2,
            ChatGenerationRequest {
                question: &question,
                kb_ids: &kb_ids,
                chat_model: chat_model.as_deref(),
                embedding_model: None,
                history: &[],
                generation,
            },
            Some(on_chunk),
        )
        .await;
        match generated {
            Ok(generated) => {
                let references = references_payload(&generated.references);
                let _ = tx
                    .send(Ok(Event::default().data(
                        serde_json::json!({
                            "object": "chat.completion.references",
                            "references": references,
                        })
                        .to_string(),
                    )))
                    .await;
                let _ = tx
                    .send(Ok(Event::default().data(
                        serde_json::json!({
                            "code": 0,
                            "data": {
                                "answer": generated.answer,
                                "reference": references_payload(&generated.references),
                            }
                        })
                        .to_string(),
                    )))
                    .await;
            }
            Err(error) => {
                let _ = tx
                    .send(Ok(Event::default().data(
                        serde_json::json!({ "code": 500, "message": error.to_string() })
                            .to_string(),
                    )))
                    .await;
            }
        }
        let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
    });
    Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// Upstream `chat.completion.references` payload shared by the chat and search
/// streams.
pub(crate) fn references_payload(references: &[crate::llm::ChunkReference]) -> serde_json::Value {
    serde_json::json!({
        "chunks": references
            .iter()
            .map(|reference| serde_json::json!({
                "id": reference.id,
                "kb_id": reference.kb_id,
                "content": reference.content,
                "similarity": reference.similarity,
                "vector_similarity": reference.vector_similarity,
                "term_similarity": reference.term_similarity,
            }))
            .collect::<Vec<_>>(),
        "total": references.len(),
    })
}

#[derive(Deserialize)]
pub struct MindMapRequest {
    pub question: String,
}

/// POST /api/v1/chat/mindmap — upstream `api.chatsMindmap` consumed by
/// `pages/next-search/mindmap-sheet.tsx` and the chat header: the query mind map
/// generated by the chat model as a `{name, children}` tree.
pub async fn chat_mindmap(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<MindMapRequest>,
) -> axum::response::Response {
    let question = body.question.trim().to_string();
    if question.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": "question is required" })),
        )
            .into_response();
    }
    // Upstream uses the tenant chat model for the mind map; the same resolution
    // chain as the related-question endpoint applies (default chat model,
    // falling back to the process-wide client).
    let selector = state.tenant_models.default_chat_model(&auth.user_id);
    let tenant_llm = match state.tenant_models.resolve(
        &state.providers,
        &auth.user_id,
        crate::api::tenant_models::ModelCapability::Chat,
        selector.as_deref(),
    ) {
        Ok(model) => model.map(|model| model.llm_client()),
        Err(error) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
            )
                .into_response();
        }
    };
    let Some(llm) = tenant_llm.as_ref().or(state.llm.as_deref()) else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "code": 503, "message": "Chat model is not configured" })),
        )
            .into_response();
    };
    let prompt = format!(
        "You are a knowledge-mapping assistant. Break the user's question into a concise mind map.\n\
         Return ONLY minified JSON of the shape {{\"name\":\"<short root label>\",\"children\":[{{\"name\":\"<branch>\",\"children\":[{{\"name\":\"<leaf>\"}}]}}]}}.\n\
         Use at most 4 branches and 3 leaves per branch, no prose, no code fences.\n\
         Question: {question}"
    );
    let messages = [crate::llm::ChatMessage::new("user", prompt)];
    match llm
        .chat_completion_with_generation(
            &messages,
            crate::generation_params::GenerationParamsPatch {
                temperature: Some(0.2),
                ..Default::default()
            },
        )
        .await
    {
        Ok(completion) => {
            let text = completion.content;
            let json = extract_json_object(&text).unwrap_or_else(|| {
                serde_json::json!({
                    "name": question,
                    "children": [{"name": text.trim()}],
                })
            });
            Json(serde_json::json!({ "code": 0, "data": json })).into_response()
        }
        Err(error) => (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
        )
            .into_response(),
    }
}

/// First `{...}` block inside a model answer, parsed as JSON.
fn extract_json_object(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

/// POST /api/v1/chats/{id}/completions — RAG chat completion
pub async fn chat_complete(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(body): Json<ChatRequest>,
) -> axum::response::Response {
    let Some(history) = state.conversations.get_for(&session_id, &auth.user_id) else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Chat not found" })),
        )
            .into_response();
    };
    let kb_ids = body
        .kb_ids
        .clone()
        .filter(|ids| !ids.is_empty())
        .unwrap_or_else(|| history.kb_ids.clone());
    let chat_model = body.chat_model.clone().or(history.chat_model.clone());
    let embedding_model = body
        .embedding_model
        .clone()
        .or(history.embedding_model.clone());
    if !kb_ids.is_empty() && !all_kbs_accessible(&state, &kb_ids, &auth) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "One or more knowledge bases are not accessible"
            })),
        )
            .into_response();
    }

    let started_at = std::time::Instant::now();
    if body.stream {
        use axum::response::sse::{Event, KeepAlive, Sse};
        use std::convert::Infallible;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(16);
        let state2 = state.clone();
        let auth2 = auth.clone();
        let question2 = body.question.clone();
        let kb_ids2 = kb_ids.clone();
        let chat_model2 = chat_model.clone();
        let embedding_model2 = embedding_model.clone();
        let history2 = history.messages.clone();
        let gen2 = body.generation;
        let id2 = format!("chatcmpl-{}", uuid::Uuid::new_v4());
        tokio::spawn(async move {
            let id3 = id2.clone();
            let on_chunk: Arc<dyn Fn(&str) + Send + Sync> = {
                let tx = tx.clone();
                Arc::new(move |chunk: &str| {
                    let event = serde_json::json!({
                        "id": id3.clone(),
                        "object": "chat.completion.chunk",
                        "choices": [{"index": 0, "delta": {"content": chunk}, "finish_reason": null}],
                    });
                    let _ = tx.blocking_send(Ok(Event::default().data(event.to_string())));
                })
            };
            let generated = generate_chat_answer(
                &state2,
                &auth2,
                ChatGenerationRequest {
                    question: &question2,
                    kb_ids: &kb_ids2,
                    chat_model: chat_model2.as_deref(),
                    embedding_model: embedding_model2.as_deref(),
                    history: &history2,
                    generation: gen2,
                },
                Some(on_chunk),
            )
            .await;
            if let Ok(generated) = generated {
                let refs: Vec<serde_json::Value> = generated
                    .references
                    .iter()
                    .map(|reference| {
                        serde_json::json!({
                            "chunk_id": reference.id,
                            "kb_id": reference.kb_id,
                            "score": reference.similarity,
                            "content": reference.content.chars().take(120).collect::<String>(),
                        })
                    })
                    .collect();
                let _ = tx.blocking_send(Ok(Event::default().data(
                    serde_json::json!({"object": "chat.completion.references", "references": refs})
                        .to_string(),
                )));
            }
            let done = serde_json::json!({
                "id": id2,
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            });
            let _ = tx.blocking_send(Ok(Event::default().data(done.to_string())));
            let _ = tx.blocking_send(Ok(Event::default().data("[DONE]")));
        });
        return Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    let generated = match generate_chat_answer(
        &state,
        &auth,
        ChatGenerationRequest {
            question: &body.question,
            kb_ids: &kb_ids,
            chat_model: chat_model.as_deref(),
            embedding_model: embedding_model.as_deref(),
            history: &history.messages,
            generation: body.generation,
        },
        None,
    )
    .await
    {
        Ok(generated) => generated,
        Err(error) => return chat_bad_request(error),
    };
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let message_id = match state.conversations.append_exchange_with_settings(
        &session_id,
        &auth.user_id,
        crate::llm::ConversationExchange {
            question: body.question,
            answer: generated.answer.clone(),
            citations: generated.citations.clone(),
            references: generated.references.clone(),
            settings: Some((kb_ids, chat_model, embedding_model)),
            duration_ms,
            usage: generated.usage,
        },
    ) {
        Ok(Some(message_id)) => message_id,
        Ok(None) => {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "code": 404, "message": "Chat not found" })),
            )
                .into_response();
        }
        Err(error) => {
            return (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
            )
                .into_response();
        }
    };
    Json(serde_json::json!({
        "code": 0,
        "data": ChatResponse {
            answer: generated.answer,
            conversation_id: session_id,
            message_id,
            citations: generated.citations,
            reference: generated.references,
        }
    }))
    .into_response()
}

/// POST /api/v1/chats/{session_id}/messages/{message_id}/regenerate
pub async fn regenerate_chat_message(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((session_id, message_id)): Path<(String, String)>,
    Json(body): Json<RegenerateRequest>,
) -> axum::response::Response {
    regenerate_owned_message(&state, &auth, &session_id, &message_id, body).await
}

/// RAGFlow-compatible regenerate route under a chat/session hierarchy.
pub async fn regenerate_chat_session_message(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((_chat_id, session_id, message_id)): Path<(String, String, String)>,
    Json(body): Json<RegenerateRequest>,
) -> axum::response::Response {
    regenerate_owned_message(&state, &auth, &session_id, &message_id, body).await
}

async fn regenerate_owned_message(
    state: &AppState,
    auth: &AuthContext,
    session_id: &str,
    message_id: &str,
    body: RegenerateRequest,
) -> axum::response::Response {
    let Some(target) =
        state
            .conversations
            .regeneration_target(session_id, &auth.user_id, message_id)
    else {
        return chat_not_found();
    };
    let kb_ids = body
        .kb_ids
        .filter(|ids| !ids.is_empty())
        .unwrap_or(target.kb_ids);
    let chat_model = body.chat_model.or(target.chat_model);
    let embedding_model = body.embedding_model.or(target.embedding_model);
    if !kb_ids.is_empty() && !all_kbs_accessible(state, &kb_ids, auth) {
        return chat_bad_request(anyhow::anyhow!(
            "One or more knowledge bases are not accessible"
        ));
    }
    let generated = match generate_chat_answer(
        state,
        auth,
        ChatGenerationRequest {
            question: &target.question,
            kb_ids: &kb_ids,
            chat_model: chat_model.as_deref(),
            embedding_model: embedding_model.as_deref(),
            history: &target.history,
            generation: crate::generation_params::GenerationParamsPatch::default(),
        },
        None,
    )
    .await
    {
        Ok(generated) => generated,
        Err(error) => return chat_bad_request(error),
    };
    match state.conversations.replace_assistant_message(
        session_id,
        &auth.user_id,
        message_id,
        crate::llm::AssistantMessageReplacement {
            expected_prior_answer: target.prior_answer,
            answer: generated.answer.clone(),
            citations: generated.citations.clone(),
            references: generated.references.clone(),
            kb_ids,
            chat_model,
            embedding_model,
            usage: generated.usage,
        },
    ) {
        Ok(true) => Json(serde_json::json!({
            "code": 0,
            "data": ChatResponse {
                answer: generated.answer,
                conversation_id: session_id.into(),
                message_id: message_id.into(),
                citations: generated.citations,
                reference: generated.references,
            }
        }))
        .into_response(),
        Ok(false) => chat_not_found(),
        Err(error) if error.to_string().contains("concurrently") => (
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({ "code": 409, "message": error.to_string() })),
        )
            .into_response(),
        Err(error) => chat_server_error(error),
    }
}

pub(crate) async fn generate_chat_answer(
    state: &AppState,
    auth: &AuthContext,
    request: ChatGenerationRequest<'_>,
    on_chunk: Option<Arc<dyn Fn(&str) + Send + Sync>>,
) -> anyhow::Result<GeneratedChatAnswer> {
    let ChatGenerationRequest {
        question,
        kb_ids,
        chat_model,
        embedding_model,
        history,
        generation,
    } = request;
    let tenant_id = kb_ids
        .first()
        .and_then(|kb_id| state.kbs.get(kb_id))
        .map(|kb| kb.owner_id)
        .unwrap_or_else(|| auth.user_id.clone());
    let stored_embedding = kb_ids
        .first()
        .and_then(|kb_id| state.kbs.get(kb_id))
        .map(|kb| kb.embd_id)
        .unwrap_or_default();
    if !kb_ids.is_empty()
        && let Some(selector) = embedding_model
    {
        let requested =
            crate::server::validate_tenant_embedding_selector(state, &tenant_id, Some(selector))?;
        if requested != stored_embedding {
            anyhow::bail!("Chat embedding_model must match the knowledge base embedding model");
        }
    }
    let search_results = if kb_ids.is_empty() {
        Vec::new()
    } else {
        let embedder = kb_embedder_for(state, kb_ids)?;
        match embedder.embed(&[question]).await {
            Ok(embeddings) => embeddings
                .into_iter()
                .next()
                .map(|query_vector| {
                    state
                        .engine
                        .read()
                        .unwrap()
                        .search_kbs(&query_vector, 5, kb_ids)
                })
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    };
    let mut contexts: Vec<String> = search_results
        .iter()
        .map(|result| result.chunk.content.clone())
        .collect();
    // 知识图谱上下文注入（若该数据集已构建图谱）
    if !kb_ids.is_empty() {
        let graph_context = state.graphs.context_for_query(kb_ids, question);
        if !graph_context.is_empty() {
            contexts.extend(graph_context.into_iter().take(4));
        }
    }
    let references: Vec<crate::llm::ChunkReference> = search_results
        .iter()
        .map(|result| crate::llm::ChunkReference {
            id: result.chunk.id.clone(),
            kb_id: result
                .chunk
                .metadata
                .get("kb_id")
                .cloned()
                .unwrap_or_default(),
            content: result.chunk.content.clone(),
            similarity: Some(result.score),
            vector_similarity: Some(result.score),
            term_similarity: None,
        })
        .collect();
    let default_chat_model = state.tenant_models.default_chat_model(&tenant_id);
    let chat_selector = chat_model.or(default_chat_model.as_deref());
    let tenant_llm = state
        .tenant_models
        .resolve(
            &state.providers,
            &tenant_id,
            crate::api::tenant_models::ModelCapability::Chat,
            chat_selector,
        )?
        .map(|model| model.llm_client());
    let completion = if let Some(llm) = tenant_llm.as_ref().or(state.llm.as_deref()) {
        if let Some(on_chunk) = on_chunk {
            let text = llm
                .rag_chat_stream(question, &contexts, history, generation, move |chunk| {
                    on_chunk(chunk)
                })
                .await?;
            return Ok(GeneratedChatAnswer {
                answer: text,
                citations: references
                    .iter()
                    .map(|reference| reference.content.clone())
                    .collect(),
                references,
                usage: None,
            });
        }
        llm.rag_chat_completion_with_generation(question, &contexts, history, generation)
            .await
            .unwrap_or_else(|error| crate::llm::ChatCompletion {
                content: format!("Error: {error}"),
                usage: None,
            })
    } else {
        crate::llm::ChatCompletion {
            content: format!(
                "Found {} relevant chunks. Configure LLM_API_KEY for chat.",
                contexts.len()
            ),
            usage: None,
        }
    };
    Ok(GeneratedChatAnswer {
        answer: completion.content,
        citations: contexts.iter().take(3).cloned().collect(),
        references,
        usage: completion.usage,
    })
}

fn chat_not_found() -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "code": 404, "message": "Chat not found" })),
    )
        .into_response()
}

fn chat_bad_request(error: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
    )
        .into_response()
}

fn chat_server_error(error: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
    )
        .into_response()
}

// ── LLM Provider Management ────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub api_base: String,
    pub models: Vec<String>,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
pub struct PublicProvider {
    pub id: String,
    pub name: String,
    pub api_base: String,
    pub models: Vec<String>,
    pub enabled: bool,
    pub api_key_configured: bool,
}

impl From<&Provider> for PublicProvider {
    fn from(provider: &Provider) -> Self {
        Self {
            id: provider.id.clone(),
            name: provider.name.clone(),
            api_base: provider.api_base.clone(),
            models: provider.models.clone(),
            enabled: provider.enabled,
            api_key_configured: provider.api_key.as_ref().is_some_and(|key| !key.is_empty()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ProviderUpdate {
    pub name: String,
    pub api_base: String,
    pub models: Vec<String>,
    pub enabled: bool,
    #[serde(
        default,
        deserialize_with = "crate::model_meta::deserialize_optional_api_key"
    )]
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

pub struct ProviderStore {
    providers: RwLock<Vec<Provider>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl ProviderStore {
    pub fn new(path: impl AsRef<FsPath>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let providers = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            default_providers()
        };
        validate_provider_set(&providers)?;
        let store = Self {
            providers: RwLock::new(providers),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            providers: RwLock::new(default_providers()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn list(&self) -> Vec<PublicProvider> {
        self.providers
            .read()
            .unwrap()
            .iter()
            .map(PublicProvider::from)
            .collect()
    }

    pub fn list_configured(&self) -> Vec<Provider> {
        self.providers.read().unwrap().clone()
    }

    pub fn get_configured(&self, id: &str) -> Option<Provider> {
        self.providers
            .read()
            .unwrap()
            .iter()
            .find(|provider| provider.id == id)
            .cloned()
    }

    pub fn create(&self, id: &str, update: ProviderUpdate) -> anyhow::Result<PublicProvider> {
        let id = id.trim().to_ascii_lowercase();
        validate_provider_id(&id)?;
        self.mutate(move |providers| {
            if providers.iter().any(|provider| provider.id == id) {
                anyhow::bail!("Provider already exists");
            }
            let provider = provider_from_update(id, update, None)?;
            let public = PublicProvider::from(&provider);
            providers.push(provider);
            Ok(public)
        })
    }

    pub fn update(&self, id: &str, update: ProviderUpdate) -> anyhow::Result<PublicProvider> {
        self.mutate(|providers| {
            let Some(index) = providers.iter().position(|provider| provider.id == id) else {
                anyhow::bail!("Provider not found");
            };
            let provider =
                provider_from_update(id.to_string(), update, providers[index].api_key.clone())?;
            let public = PublicProvider::from(&provider);
            providers[index] = provider;
            Ok(public)
        })
    }

    /// Mark a catalog provider usable after a tenant has successfully
    /// verified and attached an upstream-style provider instance. The catalog
    /// flag contains no tenant secret; credentials remain exclusively on the
    /// tenant instance.
    pub fn ensure_enabled(&self, id: &str) -> anyhow::Result<PublicProvider> {
        self.mutate(|providers| {
            let provider = providers
                .iter_mut()
                .find(|provider| provider.id == id)
                .context("Provider not found")?;
            provider.enabled = true;
            Ok(PublicProvider::from(&*provider))
        })
    }

    /// Materialize one dynamic RAGFlow factory after its first successful
    /// tenant connection. Factories such as `New API` intentionally have no
    /// catalog endpoint or static model list, so they are absent from the
    /// startup provider store. Credentials remain tenant-scoped; this shared
    /// catalog record stores only the factory identity, base URL and selected
    /// model names needed by the existing runtime resolver.
    pub fn ensure_dynamic_factory(
        &self,
        id: &str,
        name: &str,
        api_base: &str,
        models: &[String],
    ) -> anyhow::Result<Provider> {
        let id = crate::providers::provider_id_slug(id);
        validate_provider_id(&id)?;
        self.mutate(|providers| {
            if let Some(provider) = providers.iter_mut().find(|provider| {
                provider.id == id
                    || crate::providers::canonical_provider_name(&provider.id, &provider.name)
                        .eq_ignore_ascii_case(name)
            }) {
                provider.enabled = true;
                for model in models {
                    if !provider.models.contains(model) {
                        provider.models.push(model.clone());
                    }
                }
                validate_provider(provider)?;
                return Ok(provider.clone());
            }
            let provider = Provider {
                id,
                name: name.trim().to_string(),
                api_base: api_base.trim().trim_end_matches('/').to_string(),
                models: models.to_vec(),
                enabled: true,
                api_key: None,
            };
            validate_provider(&provider)?;
            providers.push(provider.clone());
            Ok(provider)
        })
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<bool> {
        self.mutate(|providers| {
            let previous_len = providers.len();
            providers.retain(|provider| provider.id != id);
            Ok(previous_len != providers.len())
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut Vec<Provider>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut providers = self.providers.write().unwrap();
        let previous = providers.clone();
        let value = mutation(&mut providers)?;
        validate_provider_set(&providers)?;
        if let Err(error) = self.persist(&providers) {
            *providers = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(&self.providers.read().unwrap())
    }

    fn persist(&self, providers: &[Provider]) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(providers)?)?;
        restrict_provider_permissions(path)
    }
}

fn default_providers() -> Vec<Provider> {
    // Seed from the provider catalog (mirrors RAGFlow conf/llm_factories.json).
    // Only the deployment defaults are enabled; everything else is offered
    // disabled so the UI can switch them on without touching the file.
    let mut providers: Vec<Provider> = crate::providers::PROVIDER_PRESETS
        .iter()
        .filter(|preset| {
            !preset.base_url.is_empty()
                && !preset.base_url.starts_with("builtin://")
                && !preset.base_url.contains('<')
        })
        .map(|preset| Provider {
            id: crate::providers::provider_id_slug(preset.id),
            name: preset.name.to_string(),
            api_base: crate::model_meta::normalize_inference_base(
                preset.id,
                preset.name,
                preset.base_url,
            ),
            models: {
                // Prefer the embedded full factory catalog (all 943 models
                // from conf/llm_factories.json); fall back to the preset's
                // representative names when the factory is absent.
                let full = crate::providers::factory_full_model_names(preset.id, preset.name);
                if !full.is_empty() {
                    full
                } else {
                    preset
                        .models
                        .iter()
                        .map(|model| (*model).to_string())
                        .collect()
                }
            },
            enabled: false,
            api_key: None,
        })
        .collect();
    // Deployment defaults stay enabled: MiniMax (default chat) and the local
    // GPU llama.cpp endpoint.
    for (id, models, enabled) in [
        (
            "minimax",
            vec!["MiniMax-M3".to_string(), "MiniMax-M2".to_string()],
            true,
        ),
        (
            "openai-api-compatible",
            vec!["Qwen3.5-9B".to_string(), "Qwen3-Embedding-4B".to_string()],
            true,
        ),
    ] {
        if let Some(provider) = providers.iter_mut().find(|provider| provider.id == id) {
            provider.models = models;
            provider.enabled = enabled;
            provider.api_base = crate::model_meta::normalize_inference_base(
                provider.name.as_str(),
                provider.name.as_str(),
                &provider.api_base,
            );
        } else {
            providers.push(Provider {
                id: id.to_string(),
                name: if id == "minimax" {
                    "MiniMax".into()
                } else {
                    "OpenAI-API-Compatible".into()
                },
                api_base: if id == "minimax" {
                    "https://api.minimaxi.com/v1".into()
                } else {
                    "http://127.0.0.1:8088/v1".into()
                },
                models,
                enabled,
                api_key: None,
            });
        }
    }
    providers
}

fn provider_from_update(
    id: String,
    update: ProviderUpdate,
    current_api_key: Option<String>,
) -> anyhow::Result<Provider> {
    let api_key = if update.clear_api_key {
        None
    } else {
        update
            .api_key
            .filter(|key| !key.is_empty())
            .or(current_api_key)
    };
    let provider = Provider {
        id,
        name: update.name.trim().to_string(),
        api_base: update.api_base.trim().trim_end_matches('/').to_string(),
        models: update
            .models
            .into_iter()
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
            .collect(),
        enabled: update.enabled,
        api_key,
    };
    validate_provider(&provider)?;
    Ok(provider)
}

fn validate_provider_set(providers: &[Provider]) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    for provider in providers {
        validate_provider(provider)?;
        if !ids.insert(provider.id.as_str()) {
            anyhow::bail!("Duplicate provider id: {}", provider.id);
        }
    }
    Ok(())
}

fn validate_provider(provider: &Provider) -> anyhow::Result<()> {
    validate_provider_id(&provider.id)?;
    if provider.name.is_empty() {
        anyhow::bail!("Provider name is required");
    }
    let url = reqwest::Url::parse(&provider.api_base)?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("Provider API base must use http or https");
    }
    if provider.models.is_empty() {
        anyhow::bail!("At least one model is required");
    }
    let mut models = HashSet::new();
    if provider.models.iter().any(|model| !models.insert(model)) {
        anyhow::bail!("Provider models must be unique");
    }
    Ok(())
}

fn validate_provider_id(id: &str) -> anyhow::Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        anyhow::bail!("Provider id must contain lowercase letters, digits, or hyphens");
    }
    Ok(())
}

#[cfg(unix)]
fn restrict_provider_permissions(path: &FsPath) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_provider_permissions(_path: &FsPath) -> anyhow::Result<()> {
    Ok(())
}

/// GET /api/v1/providers — list LLM providers
pub async fn list_providers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({ "code": 0, "data": state.providers.list() }))
}

/// GET /api/v1/providers/presets — provider catalog mirroring RAGFlow
/// `conf/llm_factories.json`, annotated with mainland-China reachability.
pub async fn list_provider_presets() -> impl IntoResponse {
    let presets: Vec<serde_json::Value> = crate::providers::PROVIDER_PRESETS
        .iter()
        .map(|preset| {
            serde_json::json!({
                "id": preset.id,
                "name": preset.name,
                "base_url": preset.base_url,
                "api_key_env": preset.api_key_env,
                "domestic": preset.domestic,
                "kinds": preset.kinds.iter().map(|kind| kind.as_str()).collect::<Vec<_>>(),
                "models": preset.models,
                "note": preset.note,
            })
        })
        .collect();
    Json(serde_json::json!({ "code": 0, "data": presets }))
}

pub async fn get_provider(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    match state.providers.get_configured(&id) {
        Some(provider) => {
            Json(serde_json::json!({ "code": 0, "data": PublicProvider::from(&provider) }))
                .into_response()
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Provider not found" })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ProviderModelsQuery {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

pub async fn list_provider_models(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<ProviderModelsQuery>,
) -> axum::response::Response {
    // Upstream exposes this read-only discovery endpoint to every signed-in
    // user opening Model providers. Tenant/admin mutation rights are checked
    // only by Verify/Create, not by the picker itself.
    let _ = auth;
    let Some(provider) = crate::api::tenant_models::provider_connection_candidate(
        &state.providers,
        &id,
        query.base_url.as_deref(),
    ) else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Provider not found" })),
        )
            .into_response();
    };
    let base_url = query
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&provider.api_base);
    let api_key = query.api_key.as_deref().or(provider.api_key.as_deref());
    match crate::model_meta::discover_provider_models(
        &provider.id,
        &provider.name,
        base_url,
        api_key,
        &provider.models,
    )
    .await
    {
        Ok(models) => Json(serde_json::json!({ "code": 0, "data": models })).into_response(),
        Err(error) => (
            axum::http::StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "code": 502,
                "message": format!("Provider model discovery failed: {error}")
            })),
        )
            .into_response(),
    }
}

pub async fn create_provider(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(update): Json<ProviderUpdate>,
) -> axum::response::Response {
    if !auth.is_admin {
        return provider_admin_required();
    }
    match state.providers.create(&id, update) {
        Ok(provider) => Json(serde_json::json!({ "code": 0, "data": provider })).into_response(),
        Err(error) => provider_bad_request(error),
    }
}

pub async fn update_provider(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(update): Json<ProviderUpdate>,
) -> axum::response::Response {
    if !auth.is_admin {
        return provider_admin_required();
    }
    match state.providers.update(&id, update) {
        Ok(provider) => Json(serde_json::json!({ "code": 0, "data": provider })).into_response(),
        Err(error) if error.to_string() == "Provider not found" => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": error.to_string() })),
        )
            .into_response(),
        Err(error) => provider_bad_request(error),
    }
}

pub async fn delete_provider(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> axum::response::Response {
    if !auth.is_admin {
        return provider_admin_required();
    }
    if state.tenant_models.provider_in_use(&id) {
        return (
            axum::http::StatusCode::CONFLICT,
            Json(serde_json::json!({
                "code": 409,
                "message": "Provider is used by tenant model instances"
            })),
        )
            .into_response();
    }
    match state.providers.delete(&id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Provider not found" })),
        )
            .into_response(),
        Err(error) => provider_bad_request(error),
    }
}

fn provider_admin_required() -> axum::response::Response {
    (
        axum::http::StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "code": 403, "message": "Administrator access required" })),
    )
        .into_response()
}

fn provider_bad_request(error: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
    )
        .into_response()
}

// ── Memory System ───────────────────────────────────────────────

const MEMORY_NAME_LIMIT: usize = crate::common::constants::MEMORY_NAME_LIMIT;
const MEMORY_SIZE_LIMIT: u64 = crate::common::constants::MEMORY_SIZE_LIMIT as u64;
const MEMORY_DEFAULT_SIZE: u64 = 5 * 1024 * 1024;
const MEMORY_TYPES: [&str; 4] = ["raw", "semantic", "episodic", "procedural"];

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryEntry {
    pub id: String,
    pub name: String,
    pub tenant_id: String,
    pub memory_type: Vec<String>,
    pub storage_type: String,
    pub embd_id: String,
    pub llm_id: String,
    pub permissions: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub memory_size: u64,
    #[serde(default = "default_forgetting_policy")]
    pub forgetting_policy: String,
    #[serde(default = "default_memory_temperature")]
    pub temperature: f32,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub user_prompt: String,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct MemoryCreateRequest {
    pub name: String,
    pub memory_type: Vec<String>,
    pub embd_id: String,
    pub llm_id: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct MemoryUpdateRequest {
    pub name: Option<String>,
    pub memory_type: Option<Vec<String>>,
    pub embd_id: Option<String>,
    pub llm_id: Option<String>,
    pub permissions: Option<String>,
    pub avatar: Option<String>,
    pub description: Option<String>,
    pub memory_size: Option<u64>,
    pub forgetting_policy: Option<String>,
    pub temperature: Option<f32>,
    pub system_prompt: Option<String>,
    pub user_prompt: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct MemoryListQuery {
    pub keywords: Option<String>,
    pub memory_type: Option<String>,
    pub tenant_id: Option<String>,
    pub owner_ids: Option<String>,
    pub storage_type: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

pub struct MemoryStore {
    memories: RwLock<HashMap<String, MemoryEntry>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl MemoryStore {
    pub fn new(path: impl AsRef<FsPath>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let mut entries: Vec<MemoryEntry> = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            Vec::new()
        };
        // `private` was RayRAG's pre-v0.3.3 wire value. RAGFlow exposes the
        // same owner-only permission as `me`; migrate snapshots once so API
        // responses and persisted values use the fixed-version contract.
        for entry in &mut entries {
            if entry.permissions.eq_ignore_ascii_case("private") {
                entry.permissions = "me".into();
            }
        }
        validate_memory_entries(&entries)?;
        let store = Self {
            memories: RwLock::new(
                entries
                    .into_iter()
                    .map(|entry| (entry.id.clone(), entry))
                    .collect(),
            ),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            memories: RwLock::new(HashMap::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    /// Internal, owner-independent lookup used by durable background workers.
    /// HTTP routes must continue to use [`Self::get_accessible`] so restoring a
    /// persisted task never weakens the external Memory ACL.
    pub fn get(&self, id: &str) -> Option<MemoryEntry> {
        self.memories.read().unwrap().get(id).cloned()
    }

    pub fn get_accessible(
        &self,
        id: &str,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
    ) -> Option<MemoryEntry> {
        self.memories
            .read()
            .unwrap()
            .get(id)
            .filter(|entry| memory_accessible(entry, user_id, is_admin, &is_tenant_member))
            .cloned()
    }

    pub fn list_accessible(
        &self,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
        query: &MemoryListQuery,
    ) -> (Vec<MemoryEntry>, usize) {
        let requested_tenants = split_filter_values(
            query
                .tenant_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| {
                    query
                        .owner_ids
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                }),
        );
        let requested_types = split_filter_values(query.memory_type.as_deref());
        let keywords = query
            .keywords
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let storage_type = query.storage_type.as_deref().map(str::trim);
        let mut entries: Vec<_> = self
            .memories
            .read()
            .unwrap()
            .values()
            .filter(|entry| memory_accessible(entry, user_id, is_admin, &is_tenant_member))
            .filter(|entry| {
                requested_tenants.is_empty() || requested_tenants.contains(&entry.tenant_id)
            })
            .filter(|entry| {
                requested_types.is_empty()
                    || entry
                        .memory_type
                        .iter()
                        .any(|kind| requested_types.contains(kind))
            })
            .filter(|entry| storage_type.is_none_or(|value| entry.storage_type == value))
            .filter(|entry| {
                keywords.is_empty() || entry.name.to_ascii_lowercase().contains(&keywords)
            })
            .cloned()
            .collect();
        entries.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let total = entries.len();
        let page = query.page.unwrap_or(1).max(1);
        let page_size = query.page_size.unwrap_or(50).clamp(1, 200);
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        (
            entries.into_iter().skip(offset).take(page_size).collect(),
            total,
        )
    }

    pub fn create(
        &self,
        tenant_id: &str,
        request: MemoryCreateRequest,
    ) -> anyhow::Result<MemoryEntry> {
        let name = validate_memory_name(&request.name)?;
        let memory_type = normalize_memory_types(request.memory_type)?;
        let system_prompt = crate::memory::PromptAssembler::assemble_system_prompt(&memory_type);
        validate_memory_selector("embedding", &request.embd_id)?;
        validate_memory_selector("chat", &request.llm_id)?;
        self.mutate(|entries| {
            let name = duplicate_memory_name(entries.values(), tenant_id, name);
            if name.chars().count() > MEMORY_NAME_LIMIT {
                anyhow::bail!("Memory name exceeds limit of {MEMORY_NAME_LIMIT}");
            }
            let now = now_ms();
            let entry = MemoryEntry {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                tenant_id: tenant_id.into(),
                memory_type,
                storage_type: "table".into(),
                embd_id: request.embd_id.trim().into(),
                llm_id: request.llm_id.trim().into(),
                permissions: "me".into(),
                avatar: String::new(),
                description: request.description,
                memory_size: MEMORY_DEFAULT_SIZE,
                forgetting_policy: default_forgetting_policy(),
                temperature: default_memory_temperature(),
                system_prompt,
                user_prompt: String::new(),
                created_at: now,
                updated_at: now,
            };
            entries.insert(entry.id.clone(), entry.clone());
            Ok(entry)
        })
    }

    pub fn update_owned(
        &self,
        id: &str,
        owner_id: &str,
        has_content: bool,
        request: MemoryUpdateRequest,
    ) -> anyhow::Result<Option<MemoryEntry>> {
        self.mutate_if_changed(|entries| {
            let Some(current) = entries.get(id).cloned() else {
                return Ok((None, false));
            };
            if current.tenant_id != owner_id {
                return Ok((None, false));
            }
            let changes_memory_type = request.memory_type.as_ref().is_some_and(|types| {
                normalize_memory_types(types.clone())
                    .is_ok_and(|types| types != current.memory_type)
            });
            let changes_embedding = request
                .embd_id
                .as_deref()
                .is_some_and(|selector| selector.trim() != current.embd_id);
            if has_content && (changes_memory_type || changes_embedding) {
                anyhow::bail!(
                    "Memory embedding model and memory type cannot change after content is stored"
                );
            }
            let system_prompt_was_default = crate::memory::judge_system_prompt_is_default(
                &current.system_prompt,
                &current.memory_type,
            );
            let has_explicit_system_prompt = request.system_prompt.is_some();
            let mut updated = current.clone();
            if let Some(name) = request.name {
                let name = validate_memory_name(&name)?;
                updated.name = duplicate_memory_name(
                    entries.values().filter(|entry| entry.id != current.id),
                    owner_id,
                    name,
                );
            }
            if let Some(memory_type) = request.memory_type {
                let memory_type = normalize_memory_types(memory_type)?;
                if memory_type != current.memory_type
                    && !has_explicit_system_prompt
                    && system_prompt_was_default
                {
                    updated.system_prompt =
                        crate::memory::PromptAssembler::assemble_system_prompt(&memory_type);
                }
                updated.memory_type = memory_type;
            }
            if let Some(embd_id) = request.embd_id {
                validate_memory_selector("embedding", &embd_id)?;
                updated.embd_id = embd_id.trim().into();
            }
            if let Some(llm_id) = request.llm_id {
                validate_memory_selector("chat", &llm_id)?;
                updated.llm_id = llm_id.trim().into();
            }
            if let Some(permission) = request.permissions {
                updated.permissions = normalize_memory_permission(&permission)?.into();
            }
            if let Some(memory_size) = request.memory_size {
                if memory_size == 0 || memory_size > MEMORY_SIZE_LIMIT {
                    anyhow::bail!("Memory size must be between 1 and {MEMORY_SIZE_LIMIT} bytes");
                }
                updated.memory_size = memory_size;
            }
            if let Some(temperature) = request.temperature {
                if !temperature.is_finite() || !(0.0..=1.0).contains(&temperature) {
                    anyhow::bail!("Memory temperature must be in range [0, 1]");
                }
                updated.temperature = temperature;
            }
            if let Some(policy) = request.forgetting_policy {
                if policy != "FIFO" {
                    anyhow::bail!("Unsupported forgetting policy: {policy}");
                }
                updated.forgetting_policy = policy;
            }
            if let Some(value) = request.avatar {
                updated.avatar = value;
            }
            if let Some(value) = request.description {
                updated.description = value;
            }
            if let Some(value) = request.system_prompt {
                updated.system_prompt = value;
            }
            if let Some(value) = request.user_prompt {
                updated.user_prompt = value;
            }
            let changed = updated != current;
            if changed {
                updated.updated_at = now_ms();
                entries.insert(id.into(), updated.clone());
            }
            Ok((Some(updated), changed))
        })
    }

    pub fn delete_owned(&self, id: &str, owner_id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|entries| {
            let removable = entries
                .get(id)
                .is_some_and(|entry| entry.tenant_id == owner_id);
            if removable {
                entries.remove(id);
            }
            Ok((removable, removable))
        })
    }

    /// `memory_api_service._require_memory_access` + `update_memory` —
    /// joined-tenant mutation: the owner OR any member of the owning tenant
    /// (when `permissions == "team"`) may mutate, mirroring the fixed
    /// service's access gate. Returns `None` when absent or inaccessible.
    #[allow(clippy::too_many_arguments)]
    pub fn update_accessible(
        &self,
        id: &str,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
        has_content: bool,
        request: MemoryUpdateRequest,
    ) -> anyhow::Result<Option<MemoryEntry>> {
        self.mutate_if_changed(|entries| {
            let Some(current) = entries.get(id).cloned() else {
                return Ok((None, false));
            };
            if !memory_accessible(&current, user_id, is_admin, &is_tenant_member) {
                return Ok((None, false));
            }
            let changes_memory_type = request.memory_type.as_ref().is_some_and(|types| {
                normalize_memory_types(types.clone())
                    .is_ok_and(|types| types != current.memory_type)
            });
            let changes_embedding = request
                .embd_id
                .as_deref()
                .is_some_and(|selector| selector.trim() != current.embd_id);
            if has_content && (changes_memory_type || changes_embedding) {
                anyhow::bail!(
                    "Memory embedding model and memory type cannot change after content is stored"
                );
            }
            let system_prompt_was_default = crate::memory::judge_system_prompt_is_default(
                &current.system_prompt,
                &current.memory_type,
            );
            let has_explicit_system_prompt = request.system_prompt.is_some();
            let mut updated = current.clone();
            if let Some(name) = request.name {
                let name = validate_memory_name(&name)?;
                // Name dedup is scoped to the owning tenant, never the actor.
                updated.name = duplicate_memory_name(
                    entries.values().filter(|entry| entry.id != current.id),
                    &current.tenant_id,
                    name,
                );
            }
            if let Some(memory_type) = request.memory_type {
                let memory_type = normalize_memory_types(memory_type)?;
                if memory_type != current.memory_type
                    && !has_explicit_system_prompt
                    && system_prompt_was_default
                {
                    updated.system_prompt =
                        crate::memory::PromptAssembler::assemble_system_prompt(&memory_type);
                }
                updated.memory_type = memory_type;
            }
            if let Some(embd_id) = request.embd_id {
                validate_memory_selector("embedding", &embd_id)?;
                updated.embd_id = embd_id.trim().into();
            }
            if let Some(llm_id) = request.llm_id {
                validate_memory_selector("chat", &llm_id)?;
                updated.llm_id = llm_id.trim().into();
            }
            if let Some(permission) = request.permissions {
                updated.permissions = normalize_memory_permission(&permission)?.into();
            }
            if let Some(memory_size) = request.memory_size {
                if memory_size == 0 || memory_size > MEMORY_SIZE_LIMIT {
                    anyhow::bail!("Memory size must be between 1 and {MEMORY_SIZE_LIMIT} bytes");
                }
                updated.memory_size = memory_size;
            }
            if let Some(temperature) = request.temperature {
                if !temperature.is_finite() || !(0.0..=1.0).contains(&temperature) {
                    anyhow::bail!("Memory temperature must be in range [0, 1]");
                }
                updated.temperature = temperature;
            }
            if let Some(policy) = request.forgetting_policy {
                if policy != "FIFO" {
                    anyhow::bail!("Unsupported forgetting policy: {policy}");
                }
                updated.forgetting_policy = policy;
            }
            if let Some(value) = request.avatar {
                updated.avatar = value;
            }
            if let Some(value) = request.description {
                updated.description = value;
            }
            if let Some(value) = request.system_prompt {
                updated.system_prompt = value;
            }
            if let Some(value) = request.user_prompt {
                updated.user_prompt = value;
            }
            let changed = updated != current;
            if changed {
                updated.updated_at = now_ms();
                entries.insert(id.into(), updated.clone());
            }
            Ok((Some(updated), changed))
        })
    }

    /// `memory_api_service.delete_memory` access gate — the owner OR a member
    /// of the owning tenant (team permission) may delete.
    pub fn delete_accessible(
        &self,
        id: &str,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|entries| {
            let removable = entries.get(id).is_some_and(|entry| {
                memory_accessible(entry, user_id, is_admin, &is_tenant_member)
            });
            if removable {
                entries.remove(id);
            }
            Ok((removable, removable))
        })
    }

    /// `MemoryService.delete_memory` — raw row removal with no access check.
    /// Callers (the cross-store delete helper) must have verified access.
    pub fn delete_by_id(&self, id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|entries| {
            let removable = entries.contains_key(id);
            if removable {
                entries.remove(id);
            }
            Ok((removable, removable))
        })
    }

    pub fn owned_ids(&self, owner_id: &str) -> Vec<String> {
        self.memories
            .read()
            .unwrap()
            .values()
            .filter(|entry| entry.tenant_id == owner_id)
            .map(|entry| entry.id.clone())
            .collect()
    }

    /// Delete every memory owned by `owner_id`. Returns the number removed.
    pub fn clear_all_owned(&self, owner_id: &str) -> anyhow::Result<usize> {
        self.mutate_if_changed(|entries| {
            let removable: Vec<String> = entries
                .iter()
                .filter(|(_, entry)| entry.tenant_id == owner_id)
                .map(|(id, _)| id.clone())
                .collect();
            for id in &removable {
                entries.remove(id);
            }
            Ok((removable.len(), !removable.is_empty()))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, MemoryEntry>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_if_changed(|entries| mutation(entries).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, MemoryEntry>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut entries = self.memories.write().unwrap();
        let previous = entries.clone();
        let (value, changed) = mutation(&mut entries)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<_> = entries.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *entries = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let entries: Vec<_> = self.memories.read().unwrap().values().cloned().collect();
        self.persist(&entries)
    }

    fn persist(&self, entries: &[MemoryEntry]) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(entries)?)
    }
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

pub async fn list_memories(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<MemoryListQuery>,
) -> impl IntoResponse {
    let (memory_list, total_count) = state.memories.list_accessible(
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        &query,
    );
    let memory_list: Vec<_> = memory_list
        .into_iter()
        .map(|memory| memory_ret_data(&state, memory))
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": { "memory_list": memory_list, "total_count": total_count }
    }))
}

pub async fn create_memory(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<MemoryCreateRequest>,
) -> axum::response::Response {
    if let Err(error) =
        validate_memory_model_bindings(&state, &auth.user_id, &request.embd_id, &request.llm_id)
    {
        return memory_bad_request(error);
    }
    match state.memories.create(&auth.user_id, request) {
        Ok(entry) => Json(serde_json::json!({ "code": 0, "data": memory_ret_data(&state, entry) }))
            .into_response(),
        Err(error) => memory_bad_request(error),
    }
}

pub async fn get_memory_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(memory_id): Path<String>,
) -> axum::response::Response {
    match state.memories.get_accessible(
        &memory_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) {
        Some(entry) => {
            Json(serde_json::json!({ "code": 0, "data": memory_ret_data(&state, entry) }))
                .into_response()
        }
        None => memory_not_found(),
    }
}

pub async fn update_memory(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(memory_id): Path<String>,
    Json(request): Json<MemoryUpdateRequest>,
) -> axum::response::Response {
    if let Some(current) = state.memories.get_accessible(
        &memory_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) {
        let embd_id = request.embd_id.as_deref().unwrap_or(&current.embd_id);
        let llm_id = request.llm_id.as_deref().unwrap_or(&current.llm_id);
        if let Err(error) =
            validate_memory_model_bindings(&state, &current.tenant_id, embd_id, llm_id)
        {
            return memory_bad_request(error);
        }
    }
    let has_content = state.memory_messages.calculate_memory_size(&memory_id) > 0;
    // `memory_api_service.update_memory`: `_require_memory_access` — the
    // owner OR a joined member of the owning tenant may mutate team memories.
    match state.memories.update_accessible(
        &memory_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        has_content,
        request,
    ) {
        Ok(Some(entry)) => {
            Json(serde_json::json!({ "code": 0, "data": memory_ret_data(&state, entry) }))
                .into_response()
        }
        Ok(None) => memory_not_found(),
        Err(error) => memory_bad_request(error),
    }
}

/// `memory_api_service.delete_memory` — cross-store delete in the upstream
/// order: remove the memory row first, then — guarded by the native index
/// check — delete its messages. Neither store participates in the other's
/// transaction, exactly as upstream; the `has_index` guard keeps a memory
/// without a native index from touching the message engine.
fn delete_memory_with_messages(
    memories: &MemoryStore,
    messages: &crate::api::joint_services::MemoryMessageService,
    memory: &MemoryEntry,
) -> anyhow::Result<()> {
    memories.delete_by_id(&memory.id)?;
    if messages.has_index(&memory.tenant_id, &memory.id) {
        messages.delete_by_memory(&memory.id)?;
    }
    Ok(())
}

pub async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(memory_id): Path<String>,
) -> axum::response::Response {
    // `_require_memory_access`: any accessible user (owner or joined tenant
    // member of a team memory) may delete.
    let memory = match accessible_memory(&state, &auth, &memory_id) {
        Some(memory) => memory,
        None => return memory_not_found(),
    };
    let _commit_guard = state.memory_commit_lock.lock().unwrap();
    if let Err(error) = state.tasks.cancel_memory(&memory_id) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response();
    }
    if let Err(error) =
        delete_memory_with_messages(&state.memories, &state.memory_messages, &memory)
    {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response();
    }
    Json(serde_json::json!({ "code": 0, "data": true })).into_response()
}

const MEMORY_API_MAX_PAGE_SIZE: usize = 100;

/// Quart's `request.args` is a multi-value mapping. Axum's ordinary typed
/// query extractor rejects repeated scalar fields, so the memory routes parse
/// their query string as an ordered multi-map and preserve RAGFlow's
/// `getlist(...)` behavior for `agent_id` and `memory_id`.
#[derive(Debug, Default)]
struct MemoryApiQuery {
    values: HashMap<String, Vec<String>>,
}

impl MemoryApiQuery {
    fn parse(raw: Option<String>) -> Self {
        let mut values: HashMap<String, Vec<String>> = HashMap::new();
        for (key, value) in
            url::form_urlencoded::parse(raw.as_deref().unwrap_or_default().as_bytes())
        {
            values
                .entry(key.into_owned())
                .or_default()
                .push(value.into_owned());
        }
        Self { values }
    }

    fn first(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .and_then(|values| values.first())
            .map(String::as_str)
    }

    /// Match `args.getlist(key)`, including the fixed compatibility branch
    /// that splits a single comma-delimited value but leaves repeated values
    /// as distinct entries.
    fn list(&self, key: &str) -> Vec<String> {
        let Some(values) = self.values.get(key) else {
            return Vec::new();
        };
        let values = if values.len() == 1 && values[0].contains(',') {
            values[0].split(',').collect::<Vec<_>>()
        } else {
            values.iter().map(String::as_str).collect()
        };
        values
            .into_iter()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn usize_or(&self, key: &str, default: usize) -> anyhow::Result<usize> {
        self.first(key).map_or(Ok(default), |value| {
            value
                .parse()
                .with_context(|| format!("{key} must be a non-negative integer"))
        })
    }

    fn f64_or(&self, key: &str, default: f64) -> anyhow::Result<f64> {
        self.first(key).map_or(Ok(default), |value| {
            value
                .parse()
                .with_context(|| format!("{key} must be a number"))
        })
    }
}

fn memory_api_page_size(value: usize) -> anyhow::Result<usize> {
    if value > MEMORY_API_MAX_PAGE_SIZE {
        anyhow::bail!("page_size must be less than or equal to {MEMORY_API_MAX_PAGE_SIZE}");
    }
    Ok(value)
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum MemoryIds {
    One(String),
    Many(Vec<String>),
}

impl MemoryIds {
    fn into_vec(self) -> Vec<String> {
        let values = match self {
            Self::One(value) => value.split(',').map(str::to_owned).collect(),
            Self::Many(values) => values,
        };
        values
            .into_iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect()
    }
}

#[derive(Debug, Deserialize)]
pub struct AddMemoryMessageRequest {
    pub memory_id: MemoryIds,
    pub agent_id: String,
    pub session_id: String,
    pub user_input: String,
    pub agent_response: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemoryMessageRequest {
    pub status: bool,
}

/// GET /api/v1/memories/{memory_id} — raw messages plus grouped extracts.
pub async fn get_memory_messages(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(memory_id): Path<String>,
    RawQuery(raw_query): RawQuery,
) -> axum::response::Response {
    let Some(memory) = accessible_memory(&state, &auth, &memory_id) else {
        return memory_not_found();
    };
    let query = MemoryApiQuery::parse(raw_query);
    let agent_ids = query.list("agent_id");
    let keyword = query
        .first("keywords")
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let page = match query.usize_or("page", 1) {
        Ok(value) => value.max(1),
        Err(error) => return memory_bad_request(error),
    };
    let page_size = match query
        .usize_or("page_size", 50)
        .and_then(memory_api_page_size)
    {
        Ok(value) => value,
        Err(error) => return memory_bad_request(error),
    };
    let (raw_messages, total_count) = state
        .memory_messages
        .list_message(&memory_id, &agent_ids, keyword, page, page_size);
    let all_messages =
        state
            .memory_messages
            .query_with_options(crate::api::joint_services::MemoryMessageQuery {
                memory_ids: std::slice::from_ref(&memory_id),
                agent_id: None,
                session_id: None,
                user_id: None,
                status: None,
                top_n: None,
                hide_forgotten: false,
            });
    let extracts = crate::api::joint_services::group_extract_by_source(&all_messages);
    let task_by_source = state.tasks.memory_tasks_by_digest(&memory_id);
    let message_list: Vec<serde_json::Value> = raw_messages
        .into_iter()
        .map(|message| {
            let mut value = serde_json::to_value(&message).expect("memory message serializes");
            let object = value.as_object_mut().expect("memory message is an object");
            let agent_name = state
                .agents
                .get_accessible(
                    &message.agent_id,
                    &auth.user_id,
                    auth.is_admin,
                    |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
                )
                .map(|agent| agent.name)
                .unwrap_or_else(|| "Unknown".into());
            object.insert("agent_name".into(), serde_json::Value::String(agent_name));
            object.insert(
                "task".into(),
                task_by_source
                    .get(&message.message_id)
                    .and_then(|task| serde_json::to_value(task).ok())
                    .unwrap_or(serde_json::Value::Null),
            );
            let extract_values: Vec<serde_json::Value> = extracts
                .get(&message.message_id)
                .into_iter()
                .flatten()
                .map(|extract| {
                    let mut value =
                        serde_json::to_value(extract).expect("memory extract serializes");
                    let agent_name = state
                        .agents
                        .get_accessible(
                            &extract.agent_id,
                            &auth.user_id,
                            auth.is_admin,
                            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
                        )
                        .map(|agent| agent.name)
                        .unwrap_or_else(|| "Unknown".into());
                    value
                        .as_object_mut()
                        .expect("memory extract is an object")
                        .insert("agent_name".into(), serde_json::Value::String(agent_name));
                    value
                })
                .collect();
            object.insert("extract".into(), serde_json::Value::Array(extract_values));
            value
        })
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "messages": { "message_list": message_list, "total_count": total_count },
            "storage_type": memory.storage_type
        }
    }))
    .into_response()
}

/// POST /api/v1/messages — persist RAW first, then publish a durable Memory
/// extraction task. The worker performs non-RAW extraction asynchronously.
pub async fn add_memory_message(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<AddMemoryMessageRequest>,
) -> axum::response::Response {
    let memory_ids = request.memory_id.into_vec();
    let memories: Vec<_> = memory_ids
        .iter()
        .filter_map(|memory_id| accessible_memory(&state, &auth, memory_id))
        .collect();
    if memories.is_empty() {
        return memory_bad_request(anyhow::anyhow!("Memory not found."));
    }
    let content = crate::api::joint_services::build_raw_message_content(
        &request.user_input,
        &request.agent_response,
    );
    let valid_at = crate::common::time_utils::timestamp_to_date(
        crate::common::time_utils::current_timestamp(),
        crate::common::time_utils::DEFAULT_TIME_FORMAT,
    );
    for memory in memories {
        let embedder = match memory_embedder_for(&state, &memory) {
            Ok(embedder) => embedder,
            Err(error) => return memory_bad_request(error),
        };
        let embedding = match embedder.embed(&[&content]).await {
            Ok(mut embeddings) if embeddings.len() == 1 && !embeddings[0].is_empty() => {
                embeddings.remove(0)
            }
            Ok(_) => {
                return memory_bad_request(anyhow::anyhow!(
                    "Embedding model returned an invalid vector"
                ));
            }
            Err(error) => return memory_bad_request(anyhow::anyhow!(error.to_string())),
        };
        let source_message_id = state.memory_messages.next_message_id();
        let message = crate::api::joint_services::MemoryMessage {
            message_id: source_message_id,
            message_type: "raw".into(),
            source_id: 0,
            memory_id: memory.id.clone(),
            user_id: auth.user_id.clone(),
            agent_id: request.agent_id.clone(),
            session_id: request.session_id.clone(),
            content: content.clone(),
            valid_at: valid_at.clone(),
            invalid_at: None,
            forget_at: None,
            status: true,
            zone_id: 0,
            content_embed: embedding,
        };
        let budget = if memory.memory_size == 0 {
            usize::MAX
        } else {
            memory.memory_size as usize
        };
        let payload = MemoryTaskPayload::new(
            &auth.user_id,
            &request.agent_id,
            &request.session_id,
            &request.user_input,
            &request.agent_response,
        );
        let _commit_guard = state.memory_commit_lock.lock().unwrap();
        let task_id = match state.tasks.stage_memory_task(
            &memory.tenant_id,
            &memory.id,
            source_message_id,
            payload,
        ) {
            Ok(task_id) => task_id,
            Err(error) => return memory_bad_request(error),
        };
        if let Err(error) = state.memory_messages.save_messages_with_budget(
            &memory.id,
            &memory.forgetting_policy,
            budget,
            vec![message],
        ) {
            if let Err(discard_error) = state.tasks.discard_staged_memory_task(&task_id) {
                return memory_bad_request(anyhow::anyhow!(
                    "{}; failed to discard staged Memory task: {discard_error}",
                    error.message
                ));
            }
            return memory_bad_request(anyhow::anyhow!(error.message));
        }
        match state.tasks.publish_memory_task(&task_id) {
            Ok(true) => {}
            Ok(false) => {
                return memory_bad_request(anyhow::anyhow!("Failed to publish staged Memory task"));
            }
            // Keep RAW + staging durable: the dispatcher reconciles this
            // crash window after restart instead of deleting an already
            // committed dialogue (which could not restore FIFO evictions).
            Err(error) => return memory_bad_request(error),
        }
    }
    Json(serde_json::json!({ "code": 0, "message": "All add to task." })).into_response()
}

pub async fn forget_memory_message(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(message_ref): Path<String>,
) -> axum::response::Response {
    mutate_memory_message(&state, &auth, &message_ref, |condition| {
        state.memory_messages.update_messages(
            condition,
            &crate::api::joint_services::MessageUpdate {
                forget_at: Some(crate::common::time_utils::timestamp_to_date(
                    crate::common::time_utils::current_timestamp(),
                    crate::common::time_utils::DEFAULT_TIME_FORMAT,
                )),
                ..Default::default()
            },
        )
    })
}

pub async fn update_memory_message(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(message_ref): Path<String>,
    Json(request): Json<UpdateMemoryMessageRequest>,
) -> axum::response::Response {
    mutate_memory_message(&state, &auth, &message_ref, |condition| {
        state.memory_messages.update_messages(
            condition,
            &crate::api::joint_services::MessageUpdate {
                status: Some(request.status),
                ..Default::default()
            },
        )
    })
}

pub async fn get_memory_message_content(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(message_ref): Path<String>,
) -> axum::response::Response {
    let Ok((memory_id, message_id)) = parse_message_ref(&message_ref) else {
        return memory_not_found();
    };
    if accessible_memory(&state, &auth, &memory_id).is_none() {
        return memory_not_found();
    }
    match state
        .memory_messages
        .get_by_message_id(&memory_id, message_id)
    {
        Some(message) => {
            let content_embed = message.content_embed.clone();
            let mut data = serde_json::to_value(message).expect("memory message serializes");
            data.as_object_mut()
                .expect("memory message is an object")
                .insert("content_embed".into(), serde_json::json!(content_embed));
            Json(serde_json::json!({ "code": 0, "data": data })).into_response()
        }
        None => memory_not_found(),
    }
}

pub async fn get_recent_memory_messages(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> axum::response::Response {
    let query = MemoryApiQuery::parse(raw_query);
    let memory_ids = query.list("memory_id");
    if memory_ids.is_empty() {
        return memory_bad_request(anyhow::anyhow!("memory_ids is required."));
    }
    let accessible: Vec<String> = memory_ids
        .into_iter()
        .filter(|memory_id| accessible_memory(&state, &auth, memory_id).is_some())
        .collect();
    let limit = match query.usize_or("limit", 10).and_then(memory_api_page_size) {
        Ok(value) => value,
        Err(error) => return memory_bad_request(error),
    };
    let messages = state.memory_messages.recent_messages(
        &accessible,
        query.first("agent_id").unwrap_or_default(),
        query.first("session_id").unwrap_or_default(),
        limit,
    );
    Json(serde_json::json!({ "code": 0, "data": messages })).into_response()
}

pub async fn search_memory_messages(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    RawQuery(raw_query): RawQuery,
) -> axum::response::Response {
    let query = MemoryApiQuery::parse(raw_query);
    let question = query.first("query").unwrap_or_default().trim();
    if question.is_empty() {
        return memory_bad_request(anyhow::anyhow!("query is required."));
    }
    let memory_ids = query.list("memory_id");
    let memories: Vec<_> = memory_ids
        .iter()
        .filter_map(|memory_id| accessible_memory(&state, &auth, memory_id))
        .collect();
    let Some(first_memory) = memories.first() else {
        return Json(serde_json::json!({ "code": 0, "data": [] })).into_response();
    };
    let embedder = match memory_embedder_for(&state, first_memory) {
        Ok(embedder) => embedder,
        Err(error) => return memory_bad_request(error),
    };
    let query_vector = match embedder.embed(&[question]).await {
        Ok(mut embeddings) if embeddings.len() == 1 && !embeddings[0].is_empty() => {
            embeddings.remove(0)
        }
        Ok(_) => {
            return memory_bad_request(anyhow::anyhow!(
                "Embedding model returned an invalid vector"
            ));
        }
        Err(error) => return memory_bad_request(anyhow::anyhow!(error.to_string())),
    };
    let similarity_threshold = match query.f64_or("similarity_threshold", 0.2) {
        Ok(value) => value,
        Err(error) => return memory_bad_request(error),
    };
    let keywords_similarity_weight = match query.f64_or("keywords_similarity_weight", 0.7) {
        Ok(value) => value,
        Err(error) => return memory_bad_request(error),
    };
    let top_n = match query.usize_or("top_n", 5).and_then(memory_api_page_size) {
        Ok(value) => value,
        Err(error) => return memory_bad_request(error),
    };
    let accessible_ids: Vec<String> = memories.iter().map(|memory| memory.id.clone()).collect();
    let messages =
        state
            .memory_messages
            .search_hybrid(crate::api::joint_services::MemoryMessageSearch {
                memory_ids: &accessible_ids,
                agent_id: query.first("agent_id").filter(|value| !value.is_empty()),
                session_id: query.first("session_id").filter(|value| !value.is_empty()),
                user_id: query.first("user_id").filter(|value| !value.is_empty()),
                question,
                query_vector: &query_vector,
                similarity_threshold,
                keywords_similarity_weight,
                top_n,
            });
    Json(serde_json::json!({ "code": 0, "data": messages })).into_response()
}

fn accessible_memory(state: &AppState, auth: &AuthContext, memory_id: &str) -> Option<MemoryEntry> {
    state.memories.get_accessible(
        memory_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    )
}

pub(crate) fn memory_embedder_for(
    state: &AppState,
    memory: &MemoryEntry,
) -> anyhow::Result<crate::embed::SharedEmbedder> {
    if memory.embd_id.trim().is_empty() || memory.embd_id == "default" {
        return state
            .embedder
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Embedding is not configured"));
    }
    state
        .tenant_models
        .resolve(
            &state.providers,
            &memory.tenant_id,
            crate::api::tenant_models::ModelCapability::Embedding,
            Some(&memory.embd_id),
        )?
        .map(|model| model.embedder())
        .ok_or_else(|| anyhow::anyhow!("Embedding is not configured: {}", memory.embd_id))
}

fn mutate_memory_message(
    state: &AppState,
    auth: &AuthContext,
    message_ref: &str,
    mutation: impl FnOnce(&crate::api::joint_services::MessageCondition) -> anyhow::Result<usize>,
) -> axum::response::Response {
    let Ok((memory_id, message_id)) = parse_message_ref(message_ref) else {
        return memory_not_found();
    };
    if accessible_memory(state, auth, &memory_id).is_none()
        || state
            .memory_messages
            .get_by_message_id(&memory_id, message_id)
            .is_none()
    {
        return memory_not_found();
    }
    let condition = crate::api::joint_services::MessageCondition {
        memory_id: Some(memory_id),
        message_id: Some(message_id),
        ..Default::default()
    };
    match mutation(&condition) {
        Ok(1..) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(0) => memory_not_found(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

fn parse_message_ref(message_ref: &str) -> anyhow::Result<(String, i64)> {
    let (memory_id, message_id) = message_ref
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid memory message id"))?;
    if memory_id.is_empty() {
        anyhow::bail!("invalid memory message id");
    }
    Ok((memory_id.into(), message_id.parse()?))
}

fn validate_memory_model_bindings(
    state: &AppState,
    tenant_id: &str,
    embd_id: &str,
    llm_id: &str,
) -> anyhow::Result<()> {
    crate::server::validate_tenant_embedding_selector(state, tenant_id, Some(embd_id))?;
    if llm_id == "default" {
        if state.llm.is_none() {
            anyhow::bail!("Global chat model is not configured");
        }
        return Ok(());
    }
    let chat = state.tenant_models.resolve(
        &state.providers,
        tenant_id,
        crate::api::tenant_models::ModelCapability::Chat,
        Some(llm_id),
    );
    if !matches!(chat, Ok(Some(_))) {
        state
            .tenant_models
            .resolve(
                &state.providers,
                tenant_id,
                crate::api::tenant_models::ModelCapability::ImageToText,
                Some(llm_id),
            )?
            .ok_or_else(|| {
                anyhow::anyhow!("Chat or image2text model is not configured: {llm_id}")
            })?;
    }
    Ok(())
}

/// `memory_utils.format_ret_data_from_memory` — the complete upstream
/// response projection. `owner_name` is `null` when the owner is unknown
/// (Python `hasattr`-guard), `memory_type` stays the human-readable list,
/// and `create_time`/`update_time` (epoch ms) are paired with their
/// `create_date`/`update_date` string forms.
fn memory_ret_data(state: &AppState, memory: MemoryEntry) -> serde_json::Value {
    let owner_name = state
        .users
        .get_user_by_id(&memory.tenant_id)
        .map(|user| user.nickname);
    memory_ret_data_with_owner(memory, owner_name)
}

/// Pure projection for tests — every `format_ret_data_from_memory` key in the
/// upstream order, with the owner name injected by the caller.
fn memory_ret_data_with_owner(
    memory: MemoryEntry,
    owner_name: Option<String>,
) -> serde_json::Value {
    let create_time = memory.created_at;
    let update_time = memory.updated_at;
    let create_date = crate::common::time_utils::timestamp_to_date(
        create_time as i64,
        crate::common::time_utils::DEFAULT_TIME_FORMAT,
    );
    let update_date = crate::common::time_utils::timestamp_to_date(
        update_time as i64,
        crate::common::time_utils::DEFAULT_TIME_FORMAT,
    );
    serde_json::json!({
        "id": memory.id,
        "name": memory.name,
        "avatar": memory.avatar,
        "tenant_id": memory.tenant_id,
        "owner_name": owner_name,
        "memory_type": memory.memory_type,
        "storage_type": memory.storage_type,
        "embd_id": memory.embd_id,
        "llm_id": memory.llm_id,
        "permissions": memory.permissions,
        "description": memory.description,
        "memory_size": memory.memory_size,
        "forgetting_policy": memory.forgetting_policy,
        "temperature": memory.temperature,
        "system_prompt": memory.system_prompt,
        "user_prompt": memory.user_prompt,
        "create_time": create_time,
        "create_date": create_date,
        "update_time": update_time,
        "update_date": update_date,
    })
}

fn validate_memory_entries(entries: &[MemoryEntry]) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    for entry in entries {
        if !ids.insert(entry.id.as_str()) {
            anyhow::bail!("Duplicate memory id: {}", entry.id);
        }
        validate_memory_name(&entry.name)?;
        normalize_memory_types(entry.memory_type.clone())?;
        normalize_memory_permission(&entry.permissions)?;
        validate_memory_selector("embedding", &entry.embd_id)?;
        validate_memory_selector("chat", &entry.llm_id)?;
        if entry.tenant_id.trim().is_empty() {
            anyhow::bail!("Memory tenant_id must not be empty");
        }
        if entry.storage_type != "table" && entry.storage_type != "graph" {
            anyhow::bail!("Unsupported memory storage type: {}", entry.storage_type);
        }
    }
    Ok(())
}

fn memory_accessible(
    entry: &MemoryEntry,
    user_id: &str,
    is_admin: bool,
    is_tenant_member: &impl Fn(&str, &str) -> bool,
) -> bool {
    entry.tenant_id == user_id
        || (entry.permissions == "team" && is_tenant_member(&entry.tenant_id, user_id))
        || (is_admin && entry.tenant_id.is_empty())
}

fn validate_memory_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("Memory name cannot be empty or whitespace");
    }
    if name.chars().count() > MEMORY_NAME_LIMIT {
        anyhow::bail!("Memory name exceeds limit of {MEMORY_NAME_LIMIT}");
    }
    Ok(name.into())
}

fn normalize_memory_types(types: Vec<String>) -> anyhow::Result<Vec<String>> {
    let mut normalized: Vec<_> = types
        .into_iter()
        .map(|kind| kind.trim().to_ascii_lowercase())
        .filter(|kind| !kind.is_empty())
        .collect();
    normalized.sort();
    normalized.dedup();
    if normalized.is_empty() {
        anyhow::bail!("Memory type must contain at least one value");
    }
    if let Some(kind) = normalized
        .iter()
        .find(|kind| !MEMORY_TYPES.contains(&kind.as_str()))
    {
        anyhow::bail!("Memory type '{kind}' is not supported");
    }
    Ok(normalized)
}

fn normalize_memory_permission(permission: &str) -> anyhow::Result<&'static str> {
    match permission.trim().to_ascii_lowercase().as_str() {
        "private" | "me" => Ok("me"),
        "team" => Ok("team"),
        _ => anyhow::bail!("Memory permission must be 'me' or 'team'"),
    }
}

fn validate_memory_selector(kind: &str, selector: &str) -> anyhow::Result<()> {
    if selector.trim().is_empty() {
        anyhow::bail!("Memory {kind} model is required");
    }
    Ok(())
}

fn duplicate_memory_name<'a>(
    entries: impl Iterator<Item = &'a MemoryEntry>,
    tenant_id: &str,
    requested: String,
) -> String {
    let existing: HashSet<_> = entries
        .filter(|entry| entry.tenant_id == tenant_id)
        .map(|entry| entry.name.as_str())
        .collect();
    if !existing.contains(requested.as_str()) {
        return requested;
    }
    (1..)
        .map(|suffix| format!("{requested}({suffix})"))
        .find(|candidate| !existing.contains(candidate.as_str()))
        .unwrap()
}

fn split_filter_values(value: Option<&str>) -> HashSet<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(|item| item.trim().to_ascii_lowercase())
        .filter(|item| !item.is_empty())
        .collect()
}

fn default_forgetting_policy() -> String {
    "FIFO".into()
}

fn default_memory_temperature() -> f32 {
    0.5
}

fn memory_not_found() -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "code": 404, "message": "Memory not found" })),
    )
        .into_response()
}

fn memory_bad_request(error: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
    )
        .into_response()
}

// ── Statistics ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
pub struct StatsQuery {
    pub from_date: Option<String>,
    pub to_date: Option<String>,
    pub canvas_id: Option<String>,
    pub tenant_id: Option<String>,
}

#[derive(Default)]
struct DailyStats {
    users: HashSet<String>,
    tokens: u64,
    duration_ms: u64,
    rounds: u64,
    thumb_up: u64,
    conversations: u64,
}

/// GET /api/v1/stats — RAGFlow-compatible tenant conversation statistics.
pub async fn get_stats(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<StatsQuery>,
) -> axum::response::Response {
    let now = now_ms();
    let from = match stats_date_bound(query.from_date.as_deref(), false, now) {
        Ok(value) => value,
        Err(error) => return stats_bad_request(error),
    };
    let to = match stats_date_bound(query.to_date.as_deref(), true, now) {
        Ok(value) => value,
        Err(error) => return stats_bad_request(error),
    };
    if from > to {
        return stats_bad_request(anyhow::anyhow!("from_date must not be after to_date"));
    }

    let tenant_id = query.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if !state.tenants.is_member(tenant_id, &auth.user_id) {
        return stats_forbidden();
    }
    let (source, canvas_id) = match query.canvas_id.as_deref() {
        Some(canvas_id) => ("agent", Some(canvas_id)),
        None => ("chat", None),
    };
    let conversations = state
        .conversations
        .list_for_tenant_source(tenant_id, source, canvas_id);
    Json(serde_json::json!({ "code": 0, "data": build_stats(conversations, from, to) }))
        .into_response()
}

fn build_stats(
    conversations: Vec<crate::llm::Conversation>,
    from: u64,
    to: u64,
) -> serde_json::Value {
    let mut daily = std::collections::BTreeMap::<String, DailyStats>::new();
    for conversation in conversations
        .into_iter()
        .filter(|conversation| conversation.created_at >= from && conversation.created_at <= to)
    {
        let bucket = daily
            .entry(format_utc_day(conversation.created_at))
            .or_default();
        bucket.conversations += 1;
        bucket.users.insert(conversation.owner_id);
        bucket.duration_ms = bucket.duration_ms.saturating_add(conversation.duration_ms);
        bucket.tokens = bucket
            .tokens
            .saturating_add(conversation_token_total(&conversation.messages));
        for message in &conversation.messages {
            if message.role == "assistant" {
                bucket.rounds += 1;
                bucket.thumb_up += u64::from(message.thumbup == Some(true));
            }
        }
    }

    let mut pv = Vec::new();
    let mut uv = Vec::new();
    let mut speed = Vec::new();
    let mut tokens = Vec::new();
    let mut rounds = Vec::new();
    let mut thumb_up = Vec::new();
    for (day, stats) in daily {
        let total_tokens = stats.tokens as f64;
        let duration_seconds = stats.duration_ms as f64 / 1000.0;
        pv.push(serde_json::json!([day, stats.conversations]));
        uv.push(serde_json::json!([day, stats.users.len()]));
        speed.push(serde_json::json!([
            day,
            total_tokens / (duration_seconds + 0.1)
        ]));
        tokens.push(serde_json::json!([day, total_tokens / 1000.0]));
        rounds.push(serde_json::json!([
            day,
            stats.rounds as f64 / stats.conversations as f64
        ]));
        thumb_up.push(serde_json::json!([day, stats.thumb_up]));
    }
    serde_json::json!({
        "pv": pv,
        "uv": uv,
        "speed": speed,
        "tokens": tokens,
        "round": rounds,
        "thumb_up": thumb_up,
    })
}

fn conversation_token_total(messages: &[crate::llm::ChatMessage]) -> u64 {
    let exact_turn_ids: HashSet<&str> = messages
        .iter()
        .filter(|message| {
            message.role == "assistant"
                && message
                    .usage
                    .and_then(crate::llm::TokenUsage::normalized)
                    .is_some()
        })
        .map(|message| message.id.as_str())
        .collect();
    messages.iter().fold(0_u64, |total, message| {
        if exact_turn_ids.contains(message.id.as_str()) {
            if message.role == "assistant" {
                total.saturating_add(
                    message
                        .usage
                        .and_then(crate::llm::TokenUsage::normalized)
                        .map(|usage| usage.total_tokens)
                        .unwrap_or_default(),
                )
            } else {
                total
            }
        } else {
            total.saturating_add(estimate_tokens(&message.content))
        }
    })
}

/// Compatibility fallback for legacy JSON and providers without usage.
fn estimate_tokens(content: &str) -> u64 {
    u64::try_from(crate::chunk::token_count(content)).unwrap_or(u64::MAX)
}

fn stats_date_bound(value: Option<&str>, is_end: bool, now: u64) -> anyhow::Result<u64> {
    let Some(value) = value else {
        if is_end {
            return Ok(now);
        }
        return Ok(start_of_utc_day(now.saturating_sub(7 * 86_400_000)));
    };
    parse_utc_datetime(value, is_end)
}

fn parse_utc_datetime(value: &str, is_end: bool) -> anyhow::Result<u64> {
    let value = value.trim();
    // Accept both `YYYY-MM-DD HH:MM:SS` and ISO-8601 `YYYY-MM-DDTHH:MM:SS`
    // (RAGFlow API4ConversationService._normalize_query_date accepts the
    // latter with an optional trailing `Z`; we normalize `Z`/`+00:00` to UTC
    // here instead of shifting to local time, matching RayRAG's UTC anchor).
    let value = value.strip_suffix('Z').unwrap_or(value);
    let (date, time) = match value.split_once(' ') {
        Some(parts) => parts,
        None => match value.split_once('T') {
            Some(parts) => parts,
            None if value.len() == 10 => (value, if is_end { "23:59:59" } else { "00:00:00" }),
            None => anyhow::bail!(
                "Dates must use YYYY-MM-DD or YYYY-MM-DD HH:MM:SS (or ISO-8601 with T)"
            ),
        },
    };
    let mut date_parts = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (
        date_parts.next().transpose()?,
        date_parts.next().transpose()?,
        date_parts.next().transpose()?,
    );
    if date_parts.next().is_some() {
        anyhow::bail!("Invalid date");
    }
    let mut time_parts = time.split(':').map(str::parse::<i64>);
    let (hour, minute, second) = (
        time_parts.next().transpose()?,
        time_parts.next().transpose()?,
        time_parts.next().transpose()?,
    );
    if time_parts.next().is_some() {
        anyhow::bail!("Invalid time");
    }
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) =
        (year, month, day, hour, minute, second)
    else {
        anyhow::bail!("Invalid date or time");
    };
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        anyhow::bail!("Invalid date or time");
    }
    let seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(hour * 3600 + minute * 60 + second))
        .ok_or_else(|| anyhow::anyhow!("Date is out of range"))?;
    u64::try_from(seconds)
        .map(|seconds| seconds * 1000)
        .map_err(|_| anyhow::anyhow!("Dates before 1970 are not supported"))
}

fn start_of_utc_day(timestamp_ms: u64) -> u64 {
    timestamp_ms / 86_400_000 * 86_400_000
}

fn format_utc_day(timestamp_ms: u64) -> String {
    let (year, month, day) = civil_from_days((timestamp_ms / 86_400_000) as i64);
    format!("{year:04}-{month:02}-{day:02}")
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let adjusted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * adjusted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn stats_bad_request(error: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
    )
        .into_response()
}

fn stats_forbidden() -> axum::response::Response {
    (
        axum::http::StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "code": 403, "message": "Tenant membership required" })),
    )
        .into_response()
}

#[cfg(test)]
mod stats_tests {
    use super::*;
    use crate::llm::{ChatMessage, Conversation};

    fn conversation(
        id: &str,
        owner_id: &str,
        created_at: u64,
        duration_ms: u64,
        answers: &[(bool, &str)],
    ) -> Conversation {
        let mut messages = Vec::new();
        for (round, (thumbup, answer)) in answers.iter().enumerate() {
            let message_id = format!("{id}-{round}");
            messages.push(ChatMessage {
                id: message_id.clone(),
                role: "user".into(),
                content: "one two".into(),
                citations: None,
                references: Vec::new(),
                thumbup: None,
                feedback: None,
                usage: None,
                created_at,
            });
            messages.push(ChatMessage {
                id: message_id,
                role: "assistant".into(),
                content: (*answer).into(),
                citations: None,
                references: Vec::new(),
                thumbup: Some(*thumbup),
                feedback: None,
                usage: None,
                created_at,
            });
        }
        Conversation {
            id: id.into(),
            name: id.into(),
            owner_id: owner_id.into(),
            tenant_id: owner_id.into(),
            source: "chat".into(),
            canvas_id: None,
            app_id: None,
            kb_ids: Vec::new(),
            chat_model: None,
            embedding_model: None,
            messages,
            duration_ms,
            dsl: None,
            errors: None,
            version_title: None,
            created_at,
            updated_at: created_at,
        }
    }

    #[test]
    fn stats_group_by_day_and_match_ragflow_series_formulas() {
        let day = parse_utc_datetime("2026-07-18", false).unwrap();
        let stats = build_stats(
            vec![
                conversation("a", "alice", day + 1000, 900, &[(true, "three four")]),
                conversation(
                    "b",
                    "bob",
                    day + 2000,
                    1000,
                    &[(false, "five"), (true, "six seven")],
                ),
            ],
            day,
            day + 86_399_999,
        );
        assert_eq!(stats["pv"], serde_json::json!([["2026-07-18", 2]]));
        assert_eq!(stats["uv"], serde_json::json!([["2026-07-18", 2]]));
        let expected_tokens = [
            "one two",
            "three four",
            "one two",
            "five",
            "one two",
            "six seven",
        ]
        .into_iter()
        .map(estimate_tokens)
        .sum::<u64>() as f64
            / 1000.0;
        assert_eq!(
            stats["tokens"],
            serde_json::json!([["2026-07-18", expected_tokens]])
        );
        assert_eq!(stats["round"], serde_json::json!([["2026-07-18", 1.5]]));
        assert_eq!(stats["thumb_up"], serde_json::json!([["2026-07-18", 2]]));
        let speed = stats["speed"][0][1].as_f64().unwrap();
        assert!((speed - expected_tokens * 1000.0 / 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn stats_prefers_exact_usage_per_turn_and_falls_back_for_legacy_turns() {
        let day = parse_utc_datetime("2026-07-18", false).unwrap();
        let mut mixed = conversation(
            "mixed",
            "alice",
            day,
            900,
            &[(false, "legacy answer"), (true, "exact answer")],
        );
        let exact_turn_id = mixed.messages[3].id.clone();
        mixed.messages[3].usage = Some(crate::llm::TokenUsage {
            prompt_tokens: 70,
            completion_tokens: 30,
            total_tokens: 100,
        });
        assert_eq!(mixed.messages[2].id, exact_turn_id);

        let expected_fallback = estimate_tokens(&mixed.messages[0].content)
            + estimate_tokens(&mixed.messages[1].content);
        assert_eq!(
            conversation_token_total(&mixed.messages),
            expected_fallback + 100
        );
        let stats = build_stats(vec![mixed], day, day + 86_399_999);
        assert_eq!(
            stats["tokens"],
            serde_json::json!([["2026-07-18", (expected_fallback + 100) as f64 / 1000.0]])
        );
    }

    #[test]
    fn zero_usage_from_legacy_or_nonconforming_provider_uses_fallback() {
        let day = parse_utc_datetime("2026-07-18", false).unwrap();
        let mut legacy = conversation("zero", "alice", day, 0, &[(false, "fallback answer")]);
        legacy.messages[1].usage = Some(crate::llm::TokenUsage::default());
        let expected = legacy
            .messages
            .iter()
            .map(|message| estimate_tokens(&message.content))
            .sum::<u64>();
        assert_eq!(conversation_token_total(&legacy.messages), expected);
    }

    #[test]
    fn stats_usage_total_does_not_double_count_paired_message_content() {
        let day = parse_utc_datetime("2026-07-18", false).unwrap();
        let mut exact = conversation("exact", "alice", day, 0, &[(false, "many answer words")]);
        exact.messages[1].usage = Some(crate::llm::TokenUsage {
            prompt_tokens: 8,
            completion_tokens: 4,
            total_tokens: 12,
        });
        assert_eq!(conversation_token_total(&exact.messages), 12);
    }

    #[test]
    fn stats_date_only_end_is_inclusive_and_invalid_dates_fail() {
        assert_eq!(
            parse_utc_datetime("2024-02-29", true).unwrap(),
            parse_utc_datetime("2024-02-29 23:59:59", false).unwrap()
        );
        assert!(parse_utc_datetime("2025-02-29", false).is_err());
        // ISO-8601 T separator and trailing Z are accepted (RAGFlow
        // _normalize_query_date parity); bare `Z` alone is not a date.
        assert_eq!(
            parse_utc_datetime("2026-07-19T00:00:00Z", false).unwrap(),
            parse_utc_datetime("2026-07-19 00:00:00", false).unwrap()
        );
        assert_eq!(
            parse_utc_datetime("2026-07-19T23:59:59", true).unwrap(),
            parse_utc_datetime("2026-07-19 23:59:59", true).unwrap()
        );
        assert!(parse_utc_datetime("Z", false).is_err());
        assert!(parse_utc_datetime("2026-07-19T25:00:00", false).is_err());
    }

    #[test]
    fn stats_filters_outside_the_requested_range() {
        let day = parse_utc_datetime("2026-07-18", false).unwrap();
        let stats = build_stats(
            vec![
                conversation("before", "alice", day - 1, 0, &[]),
                conversation("inside", "alice", day, 0, &[]),
                conversation("after", "alice", day + 86_400_000, 0, &[]),
            ],
            day,
            day + 86_399_999,
        );
        assert_eq!(stats["pv"], serde_json::json!([["2026-07-18", 1]]));
    }

    #[test]
    fn stats_input_can_be_scoped_to_shared_tenant_without_cross_tenant_records() {
        let day = parse_utc_datetime("2026-07-18", false).unwrap();
        let mut shared = conversation("shared", "alice", day, 0, &[]);
        shared.tenant_id = "shared-tenant".into();
        let private = conversation("private", "bob", day, 0, &[]);
        let selected: Vec<_> = [shared, private]
            .into_iter()
            .filter(|conversation| conversation.tenant_id == "shared-tenant")
            .collect();
        let stats = build_stats(selected, day, day + 86_399_999);
        assert_eq!(stats["pv"], serde_json::json!([["2026-07-18", 1]]));
        assert_eq!(stats["uv"], serde_json::json!([["2026-07-18", 1]]));
    }

    #[test]
    fn stats_source_canvas_selection_excludes_normal_chat_and_other_canvases() {
        let day = parse_utc_datetime("2026-07-18", false).unwrap();
        let mut target = conversation("target", "alice", day, 0, &[(true, "answer")]);
        target.source = "agent".into();
        target.canvas_id = Some("canvas-a".into());
        let mut other = conversation("other", "alice", day, 0, &[]);
        other.source = "agent".into();
        other.canvas_id = Some("canvas-b".into());
        let chat = conversation("chat", "alice", day, 0, &[]);
        let selected: Vec<_> = [target, other, chat]
            .into_iter()
            .filter(|entry| {
                entry.source == "agent" && entry.canvas_id.as_deref() == Some("canvas-a")
            })
            .collect();
        let stats = build_stats(selected, day, day + 86_399_999);
        assert_eq!(stats["pv"], serde_json::json!([["2026-07-18", 1]]));
        assert_eq!(stats["round"], serde_json::json!([["2026-07-18", 1.0]]));
        assert_eq!(stats["thumb_up"], serde_json::json!([["2026-07-18", 1]]));
    }
}

// ── Task Queue ──────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Task {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub doc_id: String,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub kb_id: String,
    #[serde(default = "default_task_type")]
    pub task_type: String,
    /// RAGFlow Task.digest. Memory tasks store their raw source message id here
    /// so the message-list API can attach progress to the corresponding row.
    #[serde(default)]
    pub digest: String,
    /// Durable replacement for the Redis-only memory task payload. This field
    /// is persisted in the private queue snapshot, but every HTTP response is
    /// built from an explicit public projection that omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) memory_payload: Option<MemoryTaskPayload>,
    pub status: String, // "staging", "pending", "running", "done", "failed", "cancelled"
    pub progress: f32,
    pub message: String,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default)]
    pub cancel_requested: bool,
    /// RAGFlow-compatible queue priority: 0=low, 1=high.
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub worker_id: String,
    #[serde(default)]
    pub lease_token: String,
    #[serde(default)]
    pub lease_expires_at: u64,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub finished_at: u64,
}

/// The conversation envelope required to resume a Memory extraction task
/// after process restart. It is intentionally crate-private and must never be
/// serialized into an API response.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub(crate) struct MemoryTaskPayload {
    pub(crate) user_id: String,
    pub(crate) agent_id: String,
    pub(crate) session_id: String,
    pub(crate) user_input: String,
    pub(crate) agent_response: String,
}

impl MemoryTaskPayload {
    pub(crate) fn new(
        user_id: impl Into<String>,
        agent_id: impl Into<String>,
        session_id: impl Into<String>,
        user_input: impl Into<String>,
        agent_response: impl Into<String>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            agent_id: agent_id.into(),
            session_id: session_id.into(),
            user_input: user_input.into(),
            agent_response: agent_response.into(),
        }
    }
}

/// Public task shape used by task-list and operation-log APIs. Keeping this
/// projection separate from [`Task`] prevents the private Memory conversation
/// payload from leaking when new internal fields are added.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct TaskView {
    pub id: String,
    pub name: String,
    pub doc_id: String,
    pub owner_id: String,
    pub kb_id: String,
    pub task_type: String,
    pub digest: String,
    pub status: String,
    pub progress: f32,
    pub message: String,
    pub retry_count: u32,
    pub max_retries: u32,
    pub cancel_requested: bool,
    pub priority: i32,
    pub created_at: u64,
    pub updated_at: u64,
    pub started_at: u64,
    pub finished_at: u64,
}

impl From<&Task> for TaskView {
    fn from(task: &Task) -> Self {
        Self {
            id: task.id.clone(),
            name: task.name.clone(),
            doc_id: task.doc_id.clone(),
            owner_id: task.owner_id.clone(),
            kb_id: task.kb_id.clone(),
            task_type: task.task_type.clone(),
            digest: task.digest.clone(),
            status: task.status.clone(),
            progress: task.progress,
            message: task.message.clone(),
            retry_count: task.retry_count,
            max_retries: task.max_retries,
            cancel_requested: task.cancel_requested,
            priority: task.priority,
            created_at: task.created_at,
            updated_at: task.updated_at,
            started_at: task.started_at,
            finished_at: task.finished_at,
        }
    }
}

/// RAGFlow Memory message task wire. Field names intentionally follow
/// `TaskService.get_tasks_progress_by_doc_ids`, not RayRAG's internal Task.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct MemoryTaskView {
    pub id: String,
    pub doc_id: String,
    pub from_page: usize,
    pub progress: f32,
    pub progress_msg: String,
    pub digest: String,
    pub chunk_ids: String,
    pub create_time: u64,
}

impl MemoryTaskView {
    fn from_task(task: &Task) -> Self {
        debug_assert_eq!(task.task_type, "memory");
        Self {
            id: task.id.clone(),
            doc_id: task.doc_id.clone(),
            from_page: 0,
            progress: if matches!(task.status.as_str(), "failed" | "cancelled") {
                -1.0
            } else {
                task.progress
            },
            progress_msg: task.message.clone(),
            digest: task.digest.clone(),
            chunk_ids: String::new(),
            create_time: task.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskLease {
    pub token: String,
    pub attempt: u32,
}

fn default_max_retries() -> u32 {
    3
}

fn default_task_type() -> String {
    "document_parse".into()
}

const TASK_MAX_LOG_LENGTH: usize = 3000;

/// File-backed task state. Workers atomically claim pending or lease-expired
/// tasks; a stale worker cannot renew or commit through a newer worker's lease.
pub struct TaskQueue {
    tasks: RwLock<Vec<Task>>,
    file_path: Option<String>,
    active_tasks: Mutex<HashSet<String>>,
    save_lock: Mutex<()>,
    notify: tokio::sync::Notify,
}

pub struct ActiveTaskGuard {
    queue: Arc<TaskQueue>,
    task_id: String,
}

impl Drop for ActiveTaskGuard {
    fn drop(&mut self) {
        self.queue
            .active_tasks
            .lock()
            .unwrap()
            .remove(&self.task_id);
        self.queue.notify.notify_one();
    }
}

impl TaskQueue {
    pub fn new(file_path: &str) -> anyhow::Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(file_path))?;
        let mut tasks: Vec<Task> = if std::path::Path::new(file_path).exists() {
            let data = std::fs::read_to_string(file_path)?;
            serde_json::from_str(&data).map_err(|error| {
                anyhow::anyhow!("Failed to parse task queue '{}': {error}", file_path)
            })?
        } else {
            Vec::new()
        };
        let now = now_ms();
        for task in &mut tasks {
            if task.task_type.is_empty() {
                task.task_type = default_task_type();
                task.updated_at = now;
            }
            if task.status == "running" && task.lease_expires_at <= now {
                clear_task_lease(task);
                if task.retry_count >= task.max_retries {
                    task.status = "failed".into();
                    task.progress = 1.0;
                    task.message = "Retry limit reached after worker lease expired".into();
                } else {
                    task.status = "pending".into();
                    task.message = "Recovered expired worker lease".into();
                }
                task.updated_at = now;
            } else if task.status == "pending" && task.retry_count >= task.max_retries {
                task.status = "failed".into();
                task.progress = 1.0;
                task.message = "Retry limit reached".into();
                task.updated_at = now;
            }
        }
        let queue = Self {
            tasks: RwLock::new(tasks),
            file_path: Some(file_path.into()),
            active_tasks: Mutex::new(HashSet::new()),
            save_lock: Mutex::new(()),
            notify: tokio::sync::Notify::new(),
        };
        queue.persist_current()?;
        Ok(queue)
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            tasks: RwLock::new(Vec::new()),
            file_path: None,
            active_tasks: Mutex::new(HashSet::new()),
            save_lock: Mutex::new(()),
            notify: tokio::sync::Notify::new(),
        }
    }

    /// Prevent duplicate local futures while the persisted lease handles
    /// ownership across dispatcher iterations and process restarts.
    pub fn activate(self: &Arc<Self>, id: &str) -> Option<ActiveTaskGuard> {
        let mut active = self.active_tasks.lock().unwrap();
        if !active.insert(id.into()) {
            return None;
        }
        Some(ActiveTaskGuard {
            queue: self.clone(),
            task_id: id.into(),
        })
    }

    pub fn list(&self) -> Vec<Task> {
        self.tasks.read().unwrap().clone()
    }

    pub fn get(&self, id: &str) -> Option<Task> {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .find(|task| task.id == id)
            .cloned()
    }

    pub fn notified(&self) -> tokio::sync::futures::Notified<'_> {
        self.notify.notified()
    }

    pub fn list_for(&self, owner_id: &str, is_admin: bool) -> Vec<Task> {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .filter(|task| task.owner_id == owner_id || (is_admin && task.owner_id.is_empty()))
            .cloned()
            .collect()
    }

    pub fn public_list_for(&self, owner_id: &str, is_admin: bool) -> Vec<TaskView> {
        self.list_for(owner_id, is_admin)
            .iter()
            .map(TaskView::from)
            .collect()
    }

    /// Memory task progress keyed by the raw message id stored in `digest`.
    /// Tasks are applied oldest-to-newest so a later retry/replacement wins,
    /// matching `memory_api_service.get_memory_messages` in the fixed source.
    pub fn memory_tasks_by_digest(
        &self,
        memory_id: &str,
    ) -> std::collections::BTreeMap<i64, MemoryTaskView> {
        let mut tasks: Vec<_> = self
            .tasks
            .read()
            .unwrap()
            .iter()
            .filter(|task| task.task_type == "memory" && task.doc_id == memory_id)
            .cloned()
            .collect();
        tasks.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut by_digest = std::collections::BTreeMap::new();
        for task in tasks {
            if let Ok(source_id) = task.digest.parse::<i64>() {
                by_digest.insert(source_id, MemoryTaskView::from_task(&task));
            }
        }
        by_digest
    }

    /// Durable Memory tasks that have not yet been published to workers. A
    /// startup reconciler checks whether the raw row exists and then calls
    /// `publish_memory_task` or `discard_staged_memory_task`.
    pub fn staged_memory_tasks(&self) -> Vec<Task> {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .filter(|task| task.task_type == "memory" && task.status == "staging")
            .cloned()
            .collect()
    }

    pub fn accessible(&self, id: &str, owner_id: &str, is_admin: bool) -> bool {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .find(|task| task.id == id)
            .is_some_and(|task| task.owner_id == owner_id || (is_admin && task.owner_id.is_empty()))
    }

    pub fn pending(&self) -> Vec<Task> {
        self.pending_at(now_ms())
    }

    fn pending_at(&self, now: u64) -> Vec<Task> {
        let mut pending: Vec<_> = self
            .tasks
            .read()
            .unwrap()
            .iter()
            .filter(|task| {
                (task.status == "pending"
                    || (task.status == "running" && task.lease_expires_at <= now))
                    && !task.cancel_requested
                    && task.retry_count < task.max_retries
            })
            .cloned()
            .collect();
        pending.sort_by(|left, right| {
            effective_priority(right, now)
                .cmp(&effective_priority(left, now))
                .then_with(|| left.created_at.cmp(&right.created_at))
                .then_with(|| left.id.cmp(&right.id))
        });
        pending
    }

    pub fn push(&self, owner_id: &str, name: &str, doc_id: &str) -> anyhow::Result<String> {
        self.push_with_priority(owner_id, name, doc_id, TASK_PRIORITY_LOW)
    }

    /// Queue at most one live processing task for a document. A repeated
    /// reparse request reuses the existing task and may promote a pending task.
    pub fn push_document_unique(
        &self,
        owner_id: &str,
        name: &str,
        doc_id: &str,
        kb_id: &str,
        priority: i32,
    ) -> anyhow::Result<(String, bool)> {
        if !matches!(priority, TASK_PRIORITY_LOW | TASK_PRIORITY_HIGH) {
            anyhow::bail!("Task priority must be 0 (low) or 1 (high)");
        }
        let now = now_ms();
        let (id, created, changed) = self.mutate_if_changed(|tasks| {
            let result = if let Some(task) = tasks.iter_mut().find(|task| {
                task.task_type == "document_parse"
                    && task.doc_id == doc_id
                    && matches!(task.status.as_str(), "pending" | "running")
                    && !task.cancel_requested
            }) {
                let mut changed = task.status == "pending" && priority > task.priority;
                if priority > task.priority {
                    task.priority = priority;
                }
                if task.kb_id.is_empty() && !kb_id.is_empty() {
                    task.kb_id = kb_id.into();
                    changed = true;
                }
                if changed {
                    task.updated_at = now;
                }
                (task.id.clone(), false, changed)
            } else {
                let task = new_task(
                    owner_id,
                    name,
                    doc_id,
                    kb_id,
                    "document_parse",
                    priority,
                    now,
                );
                let id = task.id.clone();
                tasks.push(task);
                (id, true, true)
            };
            let changed = result.2;
            Ok((result, changed))
        })?;
        if changed {
            self.notify.notify_one();
        }
        Ok((id, created))
    }

    /// Persist a non-claimable Memory extraction task before committing its raw
    /// message. Staging is a tiny write-ahead record: publish only after the raw
    /// message commit succeeds, or discard it when that commit is rolled back.
    /// Calls never deduplicate by memory/doc id; every raw dialogue gets a task.
    pub(crate) fn stage_memory_task(
        &self,
        owner_id: &str,
        memory_id: &str,
        source_message_id: i64,
        payload: MemoryTaskPayload,
    ) -> anyhow::Result<String> {
        if memory_id.is_empty() || source_message_id <= 0 {
            anyhow::bail!("Memory id and positive source message id are required");
        }
        let mut task = new_task(
            owner_id,
            "Extract memory",
            memory_id,
            "",
            "memory",
            TASK_PRIORITY_LOW,
            now_ms(),
        );
        task.digest = source_message_id.to_string();
        task.memory_payload = Some(payload);
        task.status = "staging".into();
        task.message = "Staging memory extraction".into();
        let id = task.id.clone();
        self.mutate(|tasks| {
            tasks.push(task);
            Ok(())
        })?;
        Ok(id)
    }

    /// Make a staged Memory task claimable after its raw message is durable.
    pub fn publish_memory_task(&self, id: &str) -> anyhow::Result<bool> {
        let changed = self.mutate_if_changed(|tasks| {
            let Some(task) = tasks
                .iter_mut()
                .find(|task| task.id == id && task.task_type == "memory")
            else {
                return Ok((false, false));
            };
            if task.status != "staging" {
                return Ok((false, false));
            }
            task.status = "pending".into();
            task.progress = 0.0;
            task.message = "Queued".into();
            task.updated_at = now_ms();
            Ok((true, true))
        })?;
        if changed {
            self.notify.notify_one();
        }
        Ok(changed)
    }

    /// Remove only an unpublished Memory task. Published/running history is
    /// never erased by rollback compensation.
    pub fn discard_staged_memory_task(&self, id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|tasks| {
            let before = tasks.len();
            tasks.retain(|task| {
                !(task.id == id && task.task_type == "memory" && task.status == "staging")
            });
            let changed = tasks.len() != before;
            Ok((changed, changed))
        })
    }

    pub fn push_with_priority(
        &self,
        owner_id: &str,
        name: &str,
        doc_id: &str,
        priority: i32,
    ) -> anyhow::Result<String> {
        if !matches!(priority, TASK_PRIORITY_LOW | TASK_PRIORITY_HIGH) {
            anyhow::bail!("Task priority must be 0 (low) or 1 (high)");
        }
        let task = new_task(
            owner_id,
            name,
            doc_id,
            "",
            "document_parse",
            priority,
            now_ms(),
        );
        let id = task.id.clone();
        self.mutate(|tasks| {
            tasks.push(task);
            Ok(())
        })?;
        self.notify.notify_one();
        Ok(id)
    }

    /// Atomically claim a pending or expired task for one worker.
    pub fn claim(
        &self,
        id: &str,
        worker_id: &str,
        lease_duration_ms: u64,
        message: &str,
    ) -> anyhow::Result<Option<TaskLease>> {
        if worker_id.is_empty() || lease_duration_ms == 0 {
            anyhow::bail!("Worker id and positive lease duration are required");
        }
        let lease = self.mutate_if_changed(|tasks| {
            let Some(task) = tasks.iter_mut().find(|task| task.id == id) else {
                return Ok((None, false));
            };
            let now = now_ms();
            let claimable = task.status == "pending"
                || (task.status == "running" && task.lease_expires_at <= now);
            if !claimable || task.cancel_requested || task.retry_count >= task.max_retries {
                return Ok((None, false));
            }
            task.retry_count += 1;
            task.status = "running".into();
            task.progress = 0.05;
            task.message = message.into();
            task.worker_id = worker_id.into();
            task.lease_token = uuid::Uuid::new_v4().to_string();
            task.lease_expires_at = now.saturating_add(lease_duration_ms);
            if task.started_at == 0 {
                task.started_at = now;
            }
            task.finished_at = 0;
            task.updated_at = now;
            let lease = TaskLease {
                token: task.lease_token.clone(),
                attempt: task.retry_count,
            };
            Ok((Some(lease), true))
        })?;
        Ok(lease)
    }

    pub fn renew_lease(
        &self,
        id: &str,
        lease_token: &str,
        lease_duration_ms: u64,
    ) -> anyhow::Result<bool> {
        if lease_duration_ms == 0 {
            return Ok(false);
        }
        self.mutate_if_changed(|tasks| {
            let Some(task) = tasks.iter_mut().find(|task| task.id == id) else {
                return Ok((false, false));
            };
            let now = now_ms();
            if task.status != "running"
                || task.cancel_requested
                || task.lease_token != lease_token
                || task.lease_expires_at <= now
            {
                return Ok((false, false));
            }
            task.lease_expires_at = now.saturating_add(lease_duration_ms);
            task.updated_at = now;
            Ok((true, true))
        })
    }

    pub fn owns_lease(&self, id: &str, lease_token: &str) -> bool {
        let now = now_ms();
        self.tasks
            .read()
            .unwrap()
            .iter()
            .find(|task| task.id == id)
            .is_some_and(|task| {
                task.status == "running"
                    && !task.cancel_requested
                    && task.lease_token == lease_token
                    && task.lease_expires_at > now
            })
    }

    pub fn update_claimed(
        &self,
        id: &str,
        lease_token: &str,
        status: &str,
        progress: f32,
        message: &str,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|tasks| {
            let Some(task) = tasks.iter_mut().find(|task| task.id == id) else {
                return Ok((false, false));
            };
            if task.status != "running"
                || task.lease_token != lease_token
                || task.lease_expires_at <= now_ms()
            {
                return Ok((false, false));
            }
            if !progress.is_finite() || !(0.0..=1.0).contains(&progress) {
                anyhow::bail!("Task progress must be finite and between 0 and 1");
            }
            let progress = if status == "running" {
                progress.max(task.progress)
            } else {
                progress
            };
            task.status = status.into();
            task.progress = progress;
            append_task_message(&mut task.message, message);
            task.updated_at = now_ms();
            if matches!(status, "done" | "failed" | "cancelled") {
                task.finished_at = task.updated_at;
                clear_task_lease(task);
            } else if status != "running" {
                task.finished_at = 0;
                clear_task_lease(task);
            }
            Ok((true, true))
        })
    }

    pub fn update(
        &self,
        id: &str,
        status: &str,
        progress: f32,
        message: &str,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|tasks| {
            let Some(task) = tasks.iter_mut().find(|task| task.id == id) else {
                return Ok((false, false));
            };
            task.status = status.into();
            task.progress = progress;
            task.message = message.into();
            task.updated_at = now_ms();
            if matches!(status, "done" | "failed" | "cancelled") {
                task.finished_at = task.updated_at;
                clear_task_lease(task);
            } else if status != "running" {
                task.finished_at = 0;
                clear_task_lease(task);
            }
            Ok((true, true))
        })
    }

    pub fn request_cancel(&self, id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|tasks| {
            let Some(task) = tasks.iter_mut().find(|task| task.id == id) else {
                return Ok((false, false));
            };
            if matches!(task.status.as_str(), "done" | "failed" | "cancelled") {
                Ok((false, false))
            } else {
                task.cancel_requested = true;
                task.status = "cancelled".into();
                append_task_message(&mut task.message, "Task stopped by user.");
                task.updated_at = now_ms();
                task.finished_at = task.updated_at;
                clear_task_lease(task);
                Ok((true, true))
            }
        })
    }

    pub fn is_cancelled(&self, id: &str) -> bool {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .find(|task| task.id == id)
            .is_some_and(|task| task.cancel_requested)
    }

    pub fn can_retry(&self, id: &str) -> bool {
        self.tasks
            .read()
            .unwrap()
            .iter()
            .find(|task| task.id == id)
            .is_some_and(|task| !task.cancel_requested && task.retry_count < task.max_retries)
    }

    pub fn cancel_document(&self, doc_id: &str) -> anyhow::Result<usize> {
        self.mutate_if_changed(|tasks| {
            let mut cancelled = 0;
            for task in tasks
                .iter_mut()
                .filter(|task| task.task_type == "document_parse" && task.doc_id == doc_id)
            {
                if !matches!(task.status.as_str(), "done" | "failed" | "cancelled") {
                    task.cancel_requested = true;
                    task.status = "cancelled".into();
                    task.message = "Document deleted".into();
                    task.updated_at = now_ms();
                    task.finished_at = task.updated_at;
                    clear_task_lease(task);
                    cancelled += 1;
                }
            }
            Ok((cancelled, cancelled > 0))
        })
    }

    pub fn cancel_memory(&self, memory_id: &str) -> anyhow::Result<usize> {
        self.mutate_if_changed(|tasks| {
            let mut cancelled = 0;
            for task in tasks
                .iter_mut()
                .filter(|task| task.task_type == "memory" && task.doc_id == memory_id)
            {
                if !matches!(task.status.as_str(), "done" | "failed" | "cancelled") {
                    task.cancel_requested = true;
                    task.status = "cancelled".into();
                    task.message = "Memory deleted".into();
                    task.updated_at = now_ms();
                    task.finished_at = task.updated_at;
                    clear_task_lease(task);
                    cancelled += 1;
                }
            }
            Ok((cancelled, cancelled > 0))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut Vec<Task>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_if_changed(|tasks| mutation(tasks).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut Vec<Task>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut tasks = self.tasks.write().unwrap();
        let previous = tasks.clone();
        let (value, changed) = mutation(&mut tasks)?;
        if !changed {
            return Ok(value);
        }
        if let Err(error) = self.persist(&tasks) {
            *tasks = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        let tasks = self.tasks.read().unwrap();
        self.persist(&tasks)
    }

    fn persist(&self, tasks: &[Task]) -> anyhow::Result<()> {
        let Some(path) = self.file_path.as_deref() else {
            return Ok(());
        };
        let data = serde_json::to_vec_pretty(tasks)?;
        crate::persistence::atomic_write(std::path::Path::new(path), &data)
    }
}

fn new_task(
    owner_id: &str,
    name: &str,
    doc_id: &str,
    kb_id: &str,
    task_type: &str,
    priority: i32,
    now: u64,
) -> Task {
    Task {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        doc_id: doc_id.into(),
        owner_id: owner_id.into(),
        kb_id: kb_id.into(),
        task_type: task_type.into(),
        digest: String::new(),
        memory_payload: None,
        status: "pending".into(),
        progress: 0.0,
        message: "Queued".into(),
        retry_count: 0,
        max_retries: default_max_retries(),
        cancel_requested: false,
        priority,
        worker_id: String::new(),
        lease_token: String::new(),
        lease_expires_at: 0,
        created_at: now,
        updated_at: now,
        started_at: 0,
        finished_at: 0,
    }
}

fn clear_task_lease(task: &mut Task) {
    task.worker_id.clear();
    task.lease_token.clear();
    task.lease_expires_at = 0;
}

fn append_task_message(current: &mut String, message: &str) {
    if message.is_empty() || current == message {
        return;
    }
    if current.is_empty() {
        current.push_str(message);
    } else {
        current.push('\n');
        current.push_str(message);
    }
    if current.len() <= TASK_MAX_LOG_LENGTH {
        return;
    }
    let cutoff = current.len() - TASK_MAX_LOG_LENGTH;
    let boundary = current[cutoff..]
        .find('\n')
        .map(|offset| cutoff + offset + 1)
        .unwrap_or(cutoff);
    current.drain(..boundary);
}

pub const TASK_PRIORITY_LOW: i32 = 0;
pub const TASK_PRIORITY_HIGH: i32 = 1;
const TASK_PRIORITY_AGING_MS: u64 = 60_000;

fn effective_priority(task: &Task, now: u64) -> i32 {
    if task.priority == TASK_PRIORITY_HIGH
        || now.saturating_sub(task.created_at) >= TASK_PRIORITY_AGING_MS
    {
        TASK_PRIORITY_HIGH
    } else {
        TASK_PRIORITY_LOW
    }
}

/// GET /api/v1/tasks — list tasks
pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": state.tasks.public_list_for(&auth.user_id, auth.is_admin)
    }))
}

#[derive(Debug, Deserialize)]
pub struct OperationLogQuery {
    pub kb_id: String,
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
    pub keywords: Option<String>,
    pub operation_status: Option<String>,
    pub task_type: Option<String>,
    pub created_from: Option<u64>,
    pub created_to: Option<u64>,
}

fn default_page() -> usize {
    1
}

fn default_page_size() -> usize {
    20
}

/// GET /api/v1/pipeline/operation-logs — immutable terminal task history.
pub async fn list_operation_logs(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<OperationLogQuery>,
) -> axum::response::Response {
    if !kb_accessible(&state, &query.kb_id, &auth) {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Knowledge base not found" })),
        )
            .into_response();
    }
    if query.page == 0 || query.page_size == 0 || query.page_size > 200 {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "page must be positive and page_size must be between 1 and 200"
            })),
        )
            .into_response();
    }
    let keywords = query
        .keywords
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let statuses = split_filter(query.operation_status.as_deref());
    let task_types = split_filter(query.task_type.as_deref());
    let mut logs: Vec<_> = state
        .tasks
        .list()
        .into_iter()
        .filter(|task| task.kb_id == query.kb_id)
        .filter(|task| matches!(task.status.as_str(), "done" | "failed" | "cancelled"))
        .filter(|task| {
            keywords.as_ref().is_none_or(|keywords| {
                task.name.to_lowercase().contains(keywords)
                    || task.message.to_lowercase().contains(keywords)
            })
        })
        .filter(|task| statuses.is_empty() || statuses.contains(&task.status.to_lowercase()))
        .filter(|task| task_types.is_empty() || task_types.contains(&task.task_type.to_lowercase()))
        .filter(|task| {
            query
                .created_from
                .is_none_or(|from| task.created_at >= from)
        })
        .filter(|task| query.created_to.is_none_or(|to| task.created_at <= to))
        .collect();
    logs.sort_by(|left, right| {
        right
            .finished_at
            .cmp(&left.finished_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
            .then_with(|| right.id.cmp(&left.id))
    });
    let total = logs.len();
    let offset = query.page.saturating_sub(1).saturating_mul(query.page_size);
    let items: Vec<_> = logs
        .into_iter()
        .skip(offset)
        .take(query.page_size)
        .map(|task| TaskView::from(&task))
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "items": items,
            "total": total,
            "page": query.page,
            "page_size": query.page_size,
        }
    }))
    .into_response()
}

fn split_filter(value: Option<&str>) -> HashSet<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// POST /api/v1/tasks/{task_id}/cancel — cooperative cancellation.
pub async fn cancel_task(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(task_id): Path<String>,
) -> axum::response::Response {
    let task = state
        .tasks
        .list_for(&auth.user_id, auth.is_admin)
        .into_iter()
        .find(|task| task.id == task_id);
    let Some(task) = task else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Task not found" })),
        )
            .into_response();
    };
    match state.tasks.request_cancel(&task_id) {
        Ok(changed) => {
            if changed
                && !task.doc_id.is_empty()
                && let Some(document) = state.docs.get(&task.doc_id)
                && matches!(
                    document.run.as_str(),
                    "UNSTARTED" | "SCHEDULED" | "RUNNING" | "PARSING"
                )
                && let Err(error) = state.docs.update_status(
                    &task.doc_id,
                    "CANCELLED",
                    0.0,
                    "Task stopped by user.",
                )
            {
                tracing::warn!(%error, task_id = %task_id, doc_id = %task.doc_id, "Failed to persist document cancellation");
            }
            (
                axum::http::StatusCode::OK,
                Json(serde_json::json!({
                    "code": 0,
                    "message": "Task stopped",
                    "data": true
                })),
            )
                .into_response()
        }
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct TaskActionRequest {
    pub action: String,
}

/// PATCH /api/v1/tasks/{task_id} — RAGFlow-compatible task action endpoint.
pub async fn patch_task(
    state: State<Arc<AppState>>,
    auth: Extension<AuthContext>,
    path: Path<String>,
    Json(request): Json<TaskActionRequest>,
) -> axum::response::Response {
    if request.action != "stop" {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": format!(
                    "Invalid action '{}'. Only 'stop' is supported.",
                    request.action
                )
            })),
        )
            .into_response();
    }
    cancel_task(state, auth, path).await
}

#[cfg(test)]
mod memory_store_tests {
    use super::*;

    fn create_request(name: &str) -> MemoryCreateRequest {
        MemoryCreateRequest {
            name: name.into(),
            memory_type: vec!["semantic".into(), "raw".into()],
            embd_id: "embedding-model".into(),
            llm_id: "chat-model".into(),
            description: String::new(),
        }
    }

    #[test]
    fn memory_api_query_preserves_repeated_and_comma_delimited_values() {
        let repeated = MemoryApiQuery::parse(Some(
            "memory_id=memory-a&memory_id=memory-b&agent_id=agent-a&agent_id=agent-b".into(),
        ));
        assert_eq!(
            repeated.list("memory_id"),
            vec!["memory-a".to_string(), "memory-b".to_string()]
        );
        assert_eq!(
            repeated.list("agent_id"),
            vec!["agent-a".to_string(), "agent-b".to_string()]
        );

        let comma = MemoryApiQuery::parse(Some(
            "memory_id=memory-a%2Cmemory-b&agent_id=agent-a%2Cagent-b".into(),
        ));
        assert_eq!(comma.list("memory_id"), repeated.list("memory_id"));
        assert_eq!(comma.list("agent_id"), repeated.list("agent_id"));

        assert_eq!(memory_api_page_size(100).unwrap(), 100);
        assert_eq!(
            memory_api_page_size(101).unwrap_err().to_string(),
            "page_size must be less than or equal to 100"
        );
    }

    #[test]
    fn memory_store_persists_deduplicates_and_filters() {
        let root = std::env::temp_dir().join(format!("rayrag-memories-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("memories.json");
        let store = MemoryStore::new(&path).unwrap();
        let first = store.create("owner", create_request("Research")).unwrap();
        let second = store.create("owner", create_request("Research")).unwrap();
        assert_eq!(first.name, "Research");
        assert_eq!(second.name, "Research(1)");
        assert_eq!(first.memory_type, vec!["raw", "semantic"]);
        assert_eq!(first.permissions, "me");
        assert_eq!(first.memory_size, MEMORY_DEFAULT_SIZE);
        assert_eq!(first.temperature, 0.5);
        assert_eq!(first.forgetting_policy, "FIFO");
        assert_eq!(
            first.system_prompt,
            crate::memory::PromptAssembler::assemble_system_prompt(&first.memory_type)
        );
        drop(store);

        let restored = MemoryStore::new(&path).unwrap();
        assert_eq!(restored.get(&first.id), Some(first.clone()));
        let query = MemoryListQuery {
            keywords: Some("research".into()),
            memory_type: Some("semantic".into()),
            tenant_id: Some("owner".into()),
            page: Some(1),
            page_size: Some(1),
            ..Default::default()
        };
        let (page, total) = restored.list_accessible(
            "owner",
            false,
            |tenant_id, user_id| tenant_id == user_id,
            &query,
        );
        assert_eq!(total, 2);
        assert_eq!(page.len(), 1);

        let owner_query = MemoryListQuery {
            tenant_id: Some("  ".into()),
            owner_ids: Some("owner".into()),
            ..Default::default()
        };
        assert_eq!(
            restored
                .list_accessible("owner", false, |_, _| false, &owner_query)
                .1,
            2
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_private_permission_migrates_to_me_wire_value() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-memory-permission-migration-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("memories.json");
        let store = MemoryStore::in_memory();
        let mut entry = store.create("owner", create_request("Legacy")).unwrap();
        entry.permissions = "private".into();
        entry.system_prompt.clear();
        std::fs::write(&path, serde_json::to_vec(&vec![entry]).unwrap()).unwrap();

        let restored = MemoryStore::new(&path).unwrap();
        assert_eq!(
            restored.get_accessible("not-used", "owner", false, |_, _| false),
            None
        );
        let (entries, _) =
            restored.list_accessible("owner", false, |_, _| false, &MemoryListQuery::default());
        assert_eq!(entries[0].permissions, "me");
        // An empty prompt may have been explicitly saved through the fixed
        // update API, so restart must not guess that it is legacy data.
        assert!(entries[0].system_prompt.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn joined_team_members_can_mutate_team_memory_and_outsiders_cannot() {
        // `memory_api_service.update_memory` / `delete_memory`:
        // `_require_memory_access` allows the owner OR a member of the owning
        // tenant (when permissions == "team") to mutate; outsiders are
        // rejected exactly like a missing memory.
        let member = |tenant_id: &str, user_id: &str| tenant_id == "owner" && user_id == "member";
        let store = MemoryStore::in_memory();
        let entry = store.create("owner", create_request("Shared")).unwrap();
        let updated = store
            .update_owned(
                &entry.id,
                "owner",
                false,
                MemoryUpdateRequest {
                    permissions: Some("team".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.permissions, "team");
        assert!(
            store
                .get_accessible(&entry.id, "member", false, member)
                .is_some()
        );
        // Joined member mutation is the fixed-version semantics.
        let by_member = store
            .update_accessible(
                &entry.id,
                "member",
                false,
                member,
                false,
                MemoryUpdateRequest {
                    description: Some("team edit".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(by_member.description, "team edit");
        // An outsider is indistinguishable from a missing memory.
        assert!(
            store
                .update_accessible(
                    &entry.id,
                    "stranger",
                    false,
                    member,
                    false,
                    MemoryUpdateRequest {
                        description: Some("forbidden".into()),
                        ..Default::default()
                    },
                )
                .unwrap()
                .is_none()
        );
        assert!(
            !store
                .delete_accessible(&entry.id, "stranger", false, member)
                .unwrap()
        );
        // The joined member may delete the team memory.
        assert!(
            store
                .delete_accessible(&entry.id, "member", false, member)
                .unwrap()
        );
        assert!(store.get(&entry.id).is_none());
    }

    #[test]
    fn delete_memory_with_messages_follows_upstream_two_phase_order() {
        // `memory_api_service.delete_memory`: row first, then the
        // `has_index`-guarded message cleanup.
        let memories = MemoryStore::in_memory();
        let entry = memories.create("owner", create_request("Gone")).unwrap();

        // Empty memory: no native index → the message store is untouched.
        let messages = crate::api::joint_services::MemoryMessageService::in_memory();
        delete_memory_with_messages(&memories, &messages, &entry).unwrap();
        assert!(memories.get(&entry.id).is_none());

        // Indexed memory: rows are removed with the row.
        let memories = MemoryStore::in_memory();
        let entry = memories
            .create("owner", create_request("With rows"))
            .unwrap();
        let messages = crate::api::joint_services::MemoryMessageService::in_memory();
        messages
            .insert_messages(vec![crate::api::joint_services::MemoryMessage {
                message_id: 1,
                message_type: "raw".into(),
                source_id: 0,
                memory_id: entry.id.clone(),
                user_id: "owner".into(),
                agent_id: "a1".into(),
                session_id: "s1".into(),
                content: "row".into(),
                valid_at: "2026-08-01 00:00:00".into(),
                invalid_at: None,
                forget_at: None,
                status: true,
                zone_id: 0,
                content_embed: vec![0.1],
            }])
            .unwrap();
        assert!(messages.has_index("owner", &entry.id));
        delete_memory_with_messages(&memories, &messages, &entry).unwrap();
        assert!(memories.get(&entry.id).is_none());
        assert_eq!(messages.calculate_memory_size(&entry.id), 0);
    }

    #[test]
    fn memory_ret_data_projection_matches_upstream_key_set() {
        let store = MemoryStore::in_memory();
        let entry = store.create("owner", create_request("Projection")).unwrap();
        let owner_name = Some("Owner Nick".to_string());
        let projection = memory_ret_data_with_owner(entry.clone(), owner_name.clone());
        let object = projection.as_object().expect("projection is an object");
        let keys: std::collections::BTreeSet<&str> = object.keys().map(String::as_str).collect();
        let expected: std::collections::BTreeSet<&str> = [
            "id",
            "name",
            "avatar",
            "tenant_id",
            "owner_name",
            "memory_type",
            "storage_type",
            "embd_id",
            "llm_id",
            "permissions",
            "description",
            "memory_size",
            "forgetting_policy",
            "temperature",
            "system_prompt",
            "user_prompt",
            "create_time",
            "create_date",
            "update_time",
            "update_date",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, expected);
        assert_eq!(
            object.get("owner_name"),
            Some(&serde_json::json!(owner_name))
        );
        assert_eq!(
            object.get("create_time"),
            Some(&serde_json::json!(entry.created_at))
        );
        assert_eq!(
            object.get("update_time"),
            Some(&serde_json::json!(entry.updated_at))
        );
        assert_eq!(
            object.get("memory_type"),
            Some(&serde_json::json!(entry.memory_type))
        );
        assert!(
            object
                .get("create_date")
                .and_then(|value| value.as_str())
                .is_some_and(|value| !value.is_empty())
        );
        // Unknown owner → null, mirroring Python's hasattr guard.
        let anonymous = memory_ret_data_with_owner(entry, None);
        assert_eq!(anonymous.get("owner_name"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn failed_memory_persistence_rolls_back_mutation() {
        let root =
            std::env::temp_dir().join(format!("rayrag-memory-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("memories.json");
        let store = MemoryStore::new(&path).unwrap();
        let entry = store.create("owner", create_request("Stable")).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .update_owned(
                    &entry.id,
                    "owner",
                    false,
                    MemoryUpdateRequest {
                        description: Some("must rollback".into()),
                        ..Default::default()
                    },
                )
                .is_err()
        );
        let current = store
            .get_accessible(&entry.id, "owner", false, |_, _| false)
            .unwrap();
        assert!(current.description.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn content_presence_not_capacity_budget_locks_embedding_and_type_changes() {
        let store = MemoryStore::in_memory();
        let entry = store.create("owner", create_request("Non-empty")).unwrap();
        let entry = store
            .update_owned(
                &entry.id,
                "owner",
                false,
                MemoryUpdateRequest {
                    memory_size: Some(MEMORY_SIZE_LIMIT),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        let updated = store
            .update_owned(
                &entry.id,
                "owner",
                false,
                MemoryUpdateRequest {
                    memory_type: Some(vec!["episodic".into(), "raw".into()]),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(updated.memory_type, vec!["episodic", "raw"]);
        let error = store
            .update_owned(
                &entry.id,
                "owner",
                true,
                MemoryUpdateRequest {
                    memory_type: Some(vec!["raw".into(), "semantic".into()]),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("cannot change"));
        assert!(
            store
                .update_owned(
                    &entry.id,
                    "owner",
                    false,
                    MemoryUpdateRequest {
                        memory_size: Some(0),
                        ..Default::default()
                    },
                )
                .unwrap_err()
                .to_string()
                .contains("between 1")
        );
    }

    #[test]
    fn memory_type_change_refreshes_only_the_fixed_default_system_prompt() {
        let store = MemoryStore::in_memory();
        let entry = store.create("owner", create_request("Prompted")).unwrap();
        assert!(crate::memory::judge_system_prompt_is_default(
            &entry.system_prompt,
            &entry.memory_type
        ));

        let episodic = store
            .update_owned(
                &entry.id,
                "owner",
                false,
                MemoryUpdateRequest {
                    memory_type: Some(vec!["raw".into(), "episodic".into()]),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert!(episodic.system_prompt.contains("EXTRACT EPISODIC"));
        assert!(!episodic.system_prompt.contains("EXTRACT SEMANTIC"));
        assert!(crate::memory::judge_system_prompt_is_default(
            &episodic.system_prompt,
            &episodic.memory_type
        ));

        let customized = store
            .update_owned(
                &entry.id,
                "owner",
                false,
                MemoryUpdateRequest {
                    system_prompt: Some("keep my custom prompt".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(customized.system_prompt, "keep my custom prompt");

        let procedural = store
            .update_owned(
                &entry.id,
                "owner",
                false,
                MemoryUpdateRequest {
                    memory_type: Some(vec!["raw".into(), "procedural".into()]),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(procedural.system_prompt, "keep my custom prompt");

        let explicit = store
            .update_owned(
                &entry.id,
                "owner",
                false,
                MemoryUpdateRequest {
                    memory_type: Some(vec!["raw".into(), "semantic".into()]),
                    system_prompt: Some(String::new()),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert!(explicit.system_prompt.is_empty());
    }
}

#[cfg(test)]
mod provider_store_tests {
    use super::*;

    fn provider_update(api_key: Option<&str>) -> ProviderUpdate {
        ProviderUpdate {
            name: "Local OpenAI".into(),
            api_base: "http://127.0.0.1:8080/v1/".into(),
            models: vec!["model-a".into(), "model-b".into()],
            enabled: true,
            api_key: api_key.map(str::to_string),
            clear_api_key: false,
        }
    }

    #[test]
    fn provider_config_survives_restart_without_exposing_secret() {
        let root = std::env::temp_dir().join(format!("rayrag-providers-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("providers.json");
        let store = ProviderStore::new(&path).unwrap();
        let public = store
            .create("local-openai", provider_update(Some("secret")))
            .unwrap();
        assert!(public.api_key_configured);
        assert_eq!(public.api_base, "http://127.0.0.1:8080/v1");
        assert!(
            serde_json::to_value(&public)
                .unwrap()
                .get("api_key")
                .is_none()
        );
        drop(store);

        let restored = ProviderStore::new(&path).unwrap();
        let configured = restored
            .list_configured()
            .into_iter()
            .find(|provider| provider.id == "local-openai")
            .unwrap();
        assert_eq!(configured.api_key.as_deref(), Some("secret"));
        assert!(
            restored
                .list()
                .into_iter()
                .find(|provider| provider.id == "local-openai")
                .unwrap()
                .api_key_configured
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn provider_update_preserves_or_clears_secret_explicitly() {
        let store = ProviderStore::in_memory();
        store
            .create("local-openai", provider_update(Some("secret")))
            .unwrap();
        let mut update = provider_update(None);
        update.models = vec!["model-c".into()];
        assert!(
            store
                .update("local-openai", update)
                .unwrap()
                .api_key_configured
        );

        let mut clear = provider_update(None);
        clear.clear_api_key = true;
        assert!(
            !store
                .update("local-openai", clear)
                .unwrap()
                .api_key_configured
        );
        assert!(
            store
                .list_configured()
                .into_iter()
                .find(|provider| provider.id == "local-openai")
                .unwrap()
                .api_key
                .is_none()
        );
    }

    #[test]
    fn failed_provider_persistence_rolls_back_memory() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-providers-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("providers.json");
        let store = ProviderStore::new(&path).unwrap();
        let before = store.list();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .create("local-openai", provider_update(Some("secret")))
                .is_err()
        );
        assert_eq!(store.list(), before);
        std::fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod task_queue_tests {
    use super::*;

    fn memory_payload(secret: &str) -> MemoryTaskPayload {
        MemoryTaskPayload::new(
            "memory-user",
            "memory-agent",
            "memory-session",
            secret,
            "private agent response",
        )
    }

    #[test]
    fn task_queue_persists_and_recovers_interrupted_tasks() {
        let root = std::env::temp_dir().join(format!("rayrag-tasks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.json");
        let queue = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        let lease = queue.claim(&id, "worker-1", 1, "Parsing").unwrap().unwrap();
        assert_eq!(lease.attempt, 1);
        std::thread::sleep(std::time::Duration::from_millis(2));
        drop(queue);

        let recovered = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let task = recovered
            .list()
            .into_iter()
            .find(|task| task.id == id)
            .unwrap();
        assert_eq!(task.status, "pending");
        assert_eq!(task.doc_id, "doc-1");
        assert_eq!(task.retry_count, 1);
        assert!(task.message.contains("lease"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_task_json_defaults_new_memory_fields_and_task_type() {
        let root = std::env::temp_dir().join(format!("rayrag-tasks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.json");
        let queue = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let id = queue.push("owner-1", "Legacy", "doc-1").unwrap();
        drop(queue);

        let mut tasks: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let legacy = tasks[0].as_object_mut().unwrap();
        legacy.remove("priority");
        legacy.remove("task_type");
        legacy.remove("digest");
        legacy.remove("memory_payload");
        std::fs::write(&path, serde_json::to_vec_pretty(&tasks).unwrap()).unwrap();

        let recovered = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let task = recovered
            .list()
            .into_iter()
            .find(|task| task.id == id)
            .unwrap();
        assert_eq!(task.priority, TASK_PRIORITY_LOW);
        assert_eq!(task.task_type, "document_parse");
        assert!(task.digest.is_empty());
        assert!(task.memory_payload.is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn memory_staging_survives_restart_and_can_publish_or_discard() {
        let root =
            std::env::temp_dir().join(format!("rayrag-memory-tasks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.json");
        let queue = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let staged_id = queue
            .stage_memory_task("owner-1", "memory-1", 41, memory_payload("private input"))
            .unwrap();
        assert!(queue.pending().is_empty());
        drop(queue);

        let recovered = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let staged = recovered.get(&staged_id).unwrap();
        assert_eq!(staged.status, "staging");
        assert_eq!(staged.task_type, "memory");
        assert_eq!(staged.digest, "41");
        assert_eq!(
            staged.memory_payload.as_ref().unwrap().user_input,
            "private input"
        );
        assert_eq!(recovered.staged_memory_tasks().len(), 1);
        assert!(recovered.publish_memory_task(&staged_id).unwrap());
        assert!(!recovered.publish_memory_task(&staged_id).unwrap());

        let discarded_id = recovered
            .stage_memory_task("owner-1", "memory-1", 42, memory_payload("rollback"))
            .unwrap();
        assert!(recovered.discard_staged_memory_task(&discarded_id).unwrap());
        assert!(!recovered.discard_staged_memory_task(&discarded_id).unwrap());
        drop(recovered);

        let restarted = TaskQueue::new(path.to_str().unwrap()).unwrap();
        assert_eq!(restarted.get(&staged_id).unwrap().status, "pending");
        assert!(restarted.get(&discarded_id).is_none());
        assert!(
            restarted
                .pending()
                .into_iter()
                .any(|task| task.id == staged_id)
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn document_and_memory_dedupe_and_cancel_are_isolated_by_task_type() {
        let queue = TaskQueue::in_memory();
        let first_memory = queue
            .stage_memory_task("owner-1", "shared-id", 51, memory_payload("first"))
            .unwrap();
        let second_memory = queue
            .stage_memory_task("owner-1", "shared-id", 52, memory_payload("second"))
            .unwrap();
        assert_ne!(first_memory, second_memory);
        assert!(queue.publish_memory_task(&first_memory).unwrap());
        assert!(queue.publish_memory_task(&second_memory).unwrap());

        let (document, created) = queue
            .push_document_unique(
                "owner-1",
                "Parse shared",
                "shared-id",
                "kb-1",
                TASK_PRIORITY_LOW,
            )
            .unwrap();
        assert!(created);
        let (duplicate, created) = queue
            .push_document_unique(
                "owner-1",
                "Reparse shared",
                "shared-id",
                "kb-1",
                TASK_PRIORITY_HIGH,
            )
            .unwrap();
        assert!(!created);
        assert_eq!(duplicate, document);
        assert_eq!(queue.list().len(), 3);

        assert_eq!(queue.cancel_document("shared-id").unwrap(), 1);
        assert_eq!(queue.get(&document).unwrap().status, "cancelled");
        assert_eq!(queue.get(&first_memory).unwrap().status, "pending");
        assert_eq!(queue.get(&second_memory).unwrap().status, "pending");
        assert_eq!(queue.cancel_memory("shared-id").unwrap(), 2);
        assert_eq!(queue.get(&first_memory).unwrap().status, "cancelled");
        assert_eq!(queue.get(&second_memory).unwrap().status, "cancelled");
    }

    #[test]
    fn public_task_projections_hide_payload_and_worker_lease_credentials() {
        let queue = TaskQueue::in_memory();
        let id = queue
            .stage_memory_task("owner-1", "memory-1", 61, memory_payload("top secret"))
            .unwrap();
        assert!(queue.publish_memory_task(&id).unwrap());
        queue
            .claim(&id, "private-worker", 10_000, "Extracting")
            .unwrap()
            .unwrap();

        let internal = serde_json::to_value(queue.get(&id).unwrap()).unwrap();
        assert!(internal.get("memory_payload").is_some());
        assert_eq!(internal["worker_id"], "private-worker");
        assert!(
            internal["lease_token"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );

        let public = serde_json::to_value(&queue.public_list_for("owner-1", false)[0]).unwrap();
        assert_eq!(public["digest"], "61");
        assert!(public.get("memory_payload").is_none());
        assert!(public.get("worker_id").is_none());
        assert!(public.get("lease_token").is_none());
        assert!(public.get("lease_expires_at").is_none());

        let message_task =
            serde_json::to_value(queue.memory_tasks_by_digest("memory-1").get(&61).unwrap())
                .unwrap();
        assert!(message_task.get("memory_payload").is_none());
        assert!(message_task.get("worker_id").is_none());
        assert!(message_task.get("lease_token").is_none());
        assert!(message_task.get("lease_expires_at").is_none());
    }

    #[test]
    fn memory_task_digest_query_uses_fixed_wire_and_latest_task() {
        let queue = TaskQueue::in_memory();
        let old = queue
            .stage_memory_task("owner-1", "memory-1", 71, memory_payload("old"))
            .unwrap();
        let latest = queue
            .stage_memory_task("owner-1", "memory-1", 71, memory_payload("latest"))
            .unwrap();
        queue.publish_memory_task(&old).unwrap();
        queue.publish_memory_task(&latest).unwrap();
        queue
            .update(&latest, "failed", 1.0, "Extraction failed")
            .unwrap();
        {
            let mut tasks = queue.tasks.write().unwrap();
            tasks
                .iter_mut()
                .find(|task| task.id == old)
                .unwrap()
                .created_at = 10;
            tasks
                .iter_mut()
                .find(|task| task.id == latest)
                .unwrap()
                .created_at = 20;
        }

        let task = queue
            .memory_tasks_by_digest("memory-1")
            .remove(&71)
            .unwrap();
        assert_eq!(task.id, latest);
        assert_eq!(task.progress, -1.0);
        let value = serde_json::to_value(task).unwrap();
        let keys: std::collections::BTreeSet<_> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            std::collections::BTreeSet::from([
                "chunk_ids",
                "create_time",
                "digest",
                "doc_id",
                "from_page",
                "id",
                "progress",
                "progress_msg",
            ])
        );
    }

    #[test]
    fn cancellation_prevents_additional_attempts() {
        let queue = TaskQueue::in_memory();
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        assert!(queue.request_cancel(&id).unwrap());
        assert!(!queue.request_cancel(&id).unwrap());
        assert_eq!(queue.claim(&id, "worker-1", 1000, "Parsing").unwrap(), None);
        assert!(queue.is_cancelled(&id));
        let task = queue.list().into_iter().find(|task| task.id == id).unwrap();
        assert_eq!(task.message, "Queued\nTask stopped by user.");
    }

    #[test]
    fn retry_limit_prevents_a_fourth_attempt() {
        let queue = TaskQueue::in_memory();
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        for expected in 1..=3 {
            let lease = queue
                .claim(&id, "worker-1", 1000, "Parsing")
                .unwrap()
                .unwrap();
            assert_eq!(lease.attempt, expected);
            queue
                .update_claimed(&id, &lease.token, "pending", 0.0, "Retrying")
                .unwrap();
        }
        assert_eq!(queue.claim(&id, "worker-1", 1000, "Parsing").unwrap(), None);
        assert!(!queue.can_retry(&id));
    }

    #[test]
    fn only_one_worker_can_claim_an_active_lease() {
        let queue = TaskQueue::in_memory();
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        let lease = queue
            .claim(&id, "worker-1", 1000, "Parsing")
            .unwrap()
            .unwrap();
        assert_eq!(lease.attempt, 1);
        assert_eq!(queue.claim(&id, "worker-2", 1000, "Parsing").unwrap(), None);
        assert!(queue.owns_lease(&id, &lease.token));
    }

    #[test]
    fn expired_lease_is_reclaimed_and_stale_worker_cannot_commit() {
        let queue = TaskQueue::in_memory();
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        let stale = queue.claim(&id, "worker-1", 1, "Parsing").unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let current = queue
            .claim(&id, "worker-2", 1000, "Reclaimed")
            .unwrap()
            .unwrap();
        assert_eq!(current.attempt, 2);
        assert!(
            !queue
                .update_claimed(&id, &stale.token, "done", 1.0, "Stale result")
                .unwrap()
        );
        assert!(
            queue
                .update_claimed(&id, &current.token, "done", 1.0, "Current result")
                .unwrap()
        );
    }

    #[test]
    fn lease_renewal_requires_current_unexpired_token() {
        let queue = TaskQueue::in_memory();
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        let lease = queue
            .claim(&id, "worker-1", 1000, "Parsing")
            .unwrap()
            .unwrap();
        assert!(!queue.renew_lease(&id, "wrong-token", 1000).unwrap());
        assert!(queue.renew_lease(&id, &lease.token, 1000).unwrap());
    }

    #[test]
    fn expired_lease_cannot_commit_before_reclaim() {
        let queue = TaskQueue::in_memory();
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        let lease = queue.claim(&id, "worker-1", 1, "Parsing").unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(
            !queue
                .update_claimed(&id, &lease.token, "done", 1.0, "Late result")
                .unwrap()
        );
    }

    #[test]
    fn running_progress_is_monotonic_and_messages_are_bounded() {
        let queue = TaskQueue::in_memory();
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        let lease = queue
            .claim(&id, "worker-1", 1000, "Parsing")
            .unwrap()
            .unwrap();
        assert!(
            queue
                .update_claimed(&id, &lease.token, "running", 0.7, "Embedded")
                .unwrap()
        );
        assert!(
            queue
                .update_claimed(&id, &lease.token, "running", 0.4, &"x".repeat(4000))
                .unwrap()
        );
        let task = queue.list().into_iter().find(|task| task.id == id).unwrap();
        assert_eq!(task.progress, 0.7);
        assert!(task.message.len() <= TASK_MAX_LOG_LENGTH);
        assert!(task.message.ends_with(&"x".repeat(TASK_MAX_LOG_LENGTH)));
    }

    #[test]
    fn active_guard_blocks_duplicate_local_future_until_drop() {
        let queue = Arc::new(TaskQueue::in_memory());
        let id = queue.push("owner-1", "Parse test.txt", "doc-1").unwrap();
        let guard = queue.activate(&id).unwrap();
        assert!(queue.activate(&id).is_none());
        drop(guard);
        assert!(queue.activate(&id).is_some());
    }

    #[test]
    fn concurrent_pushes_persist_without_losing_newer_snapshots() {
        let root =
            std::env::temp_dir().join(format!("rayrag-concurrent-tasks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.json");
        let queue = Arc::new(TaskQueue::new(path.to_str().unwrap()).unwrap());
        let handles: Vec<_> = (0..16)
            .map(|index| {
                let queue = queue.clone();
                std::thread::spawn(move || {
                    queue
                        .push(
                            "owner-1",
                            &format!("Parse {index}.txt"),
                            &format!("doc-{index}"),
                        )
                        .unwrap()
                })
            })
            .collect();
        let mut expected_ids: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        expected_ids.sort();
        drop(queue);

        let recovered = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let mut actual_ids: Vec<_> = recovered.list().into_iter().map(|task| task.id).collect();
        actual_ids.sort();
        assert_eq!(actual_ids, expected_ids);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn pending_orders_high_priority_before_low_priority() {
        let queue = TaskQueue::in_memory();
        let low = queue
            .push_with_priority("owner-1", "Low", "doc-low", TASK_PRIORITY_LOW)
            .unwrap();
        let high = queue
            .push_with_priority("owner-1", "High", "doc-high", TASK_PRIORITY_HIGH)
            .unwrap();
        let ids: Vec<_> = queue.pending().into_iter().map(|task| task.id).collect();
        assert_eq!(ids, vec![high, low]);
    }

    #[test]
    fn pending_preserves_fifo_within_priority() {
        let queue = TaskQueue::in_memory();
        let first = queue
            .push_with_priority("owner-1", "First", "doc-1", TASK_PRIORITY_HIGH)
            .unwrap();
        let second = queue
            .push_with_priority("owner-1", "Second", "doc-2", TASK_PRIORITY_HIGH)
            .unwrap();
        {
            let mut tasks = queue.tasks.write().unwrap();
            tasks
                .iter_mut()
                .find(|task| task.id == first)
                .unwrap()
                .created_at = 10;
            tasks
                .iter_mut()
                .find(|task| task.id == second)
                .unwrap()
                .created_at = 20;
        }
        let ids: Vec<_> = queue
            .pending_at(20)
            .into_iter()
            .map(|task| task.id)
            .collect();
        assert_eq!(ids, vec![first, second]);
    }

    #[test]
    fn aging_promotes_waiting_low_priority_without_losing_fifo() {
        let queue = TaskQueue::in_memory();
        let first_low = queue
            .push_with_priority("owner-1", "First low", "doc-1", TASK_PRIORITY_LOW)
            .unwrap();
        let second_low = queue
            .push_with_priority("owner-1", "Second low", "doc-2", TASK_PRIORITY_LOW)
            .unwrap();
        let high = queue
            .push_with_priority("owner-1", "High", "doc-3", TASK_PRIORITY_HIGH)
            .unwrap();
        {
            let mut tasks = queue.tasks.write().unwrap();
            tasks
                .iter_mut()
                .find(|task| task.id == first_low)
                .unwrap()
                .created_at = 10;
            tasks
                .iter_mut()
                .find(|task| task.id == second_low)
                .unwrap()
                .created_at = 20;
            tasks
                .iter_mut()
                .find(|task| task.id == high)
                .unwrap()
                .created_at = TASK_PRIORITY_AGING_MS + 10;
        }
        let now = TASK_PRIORITY_AGING_MS + 20;
        let ids: Vec<_> = queue
            .pending_at(now)
            .into_iter()
            .map(|task| task.id)
            .collect();
        assert_eq!(ids, vec![first_low, second_low, high]);
    }

    #[test]
    fn task_priority_rejects_values_outside_ragflow_range() {
        let queue = TaskQueue::in_memory();
        assert!(
            queue
                .push_with_priority("owner-1", "Invalid", "doc-1", 2)
                .is_err()
        );
        assert!(queue.list().is_empty());
    }

    #[test]
    fn duplicate_document_queue_reuses_live_task_and_promotes_pending_priority() {
        let queue = TaskQueue::in_memory();
        let (first, created) = queue
            .push_document_unique("owner-1", "Parse doc", "doc-1", "kb-1", TASK_PRIORITY_LOW)
            .unwrap();
        assert!(created);
        let (second, created) = queue
            .push_document_unique(
                "owner-1",
                "Reparse doc",
                "doc-1",
                "kb-1",
                TASK_PRIORITY_HIGH,
            )
            .unwrap();
        assert!(!created);
        assert_eq!(second, first);
        assert_eq!(queue.list().len(), 1);
        assert_eq!(queue.list()[0].priority, TASK_PRIORITY_HIGH);
        assert_eq!(queue.list()[0].kb_id, "kb-1");
        assert_eq!(queue.list()[0].task_type, "document_parse");
    }

    #[test]
    fn task_records_started_and_finished_operation_times() {
        let queue = TaskQueue::in_memory();
        let (id, _) = queue
            .push_document_unique("owner-1", "Parse doc", "doc-1", "kb-1", TASK_PRIORITY_LOW)
            .unwrap();
        let lease = queue
            .claim(&id, "worker-1", 10_000, "Parsing")
            .unwrap()
            .unwrap();
        let running = queue.list().into_iter().find(|task| task.id == id).unwrap();
        assert!(running.started_at >= running.created_at);
        assert_eq!(running.finished_at, 0);
        assert!(
            queue
                .update_claimed(&id, &lease.token, "done", 1.0, "Done")
                .unwrap()
        );
        let done = queue.list().into_iter().find(|task| task.id == id).unwrap();
        assert!(done.finished_at >= done.started_at);
    }

    #[test]
    fn retry_pending_is_not_a_finished_operation() {
        let queue = TaskQueue::in_memory();
        let (id, _) = queue
            .push_document_unique("owner-1", "Parse doc", "doc-1", "kb-1", TASK_PRIORITY_LOW)
            .unwrap();
        let lease = queue
            .claim(&id, "worker-1", 10_000, "Parsing")
            .unwrap()
            .unwrap();
        assert!(
            queue
                .update_claimed(&id, &lease.token, "pending", 0.0, "Retrying")
                .unwrap()
        );
        let pending = queue.list().into_iter().find(|task| task.id == id).unwrap();
        assert_eq!(pending.status, "pending");
        assert_eq!(pending.finished_at, 0);
        assert!(pending.started_at > 0);
    }

    #[test]
    fn terminal_document_task_allows_a_new_attempt() {
        let queue = TaskQueue::in_memory();
        let (first, _) = queue
            .push_document_unique("owner-1", "Parse doc", "doc-1", "kb-1", TASK_PRIORITY_LOW)
            .unwrap();
        assert!(queue.update(&first, "failed", 1.0, "Failed").unwrap());
        let (second, created) = queue
            .push_document_unique(
                "owner-1",
                "Reparse doc",
                "doc-1",
                "kb-1",
                TASK_PRIORITY_HIGH,
            )
            .unwrap();
        assert!(created);
        assert_ne!(second, first);
        assert_eq!(queue.list().len(), 2);
    }

    #[test]
    fn failed_persistence_rolls_back_task_mutations() {
        let root =
            std::env::temp_dir().join(format!("rayrag-task-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.json");
        let queue = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let id = queue.push("owner-1", "Parse", "doc-1").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(queue.claim(&id, "worker-1", 1000, "Parsing").is_err());
        let task = queue.list().into_iter().find(|task| task.id == id).unwrap();
        assert_eq!(task.status, "pending");
        assert_eq!(task.retry_count, 0);
        assert!(task.lease_token.is_empty());

        assert!(queue.request_cancel(&id).is_err());
        let task = queue.list().into_iter().find(|task| task.id == id).unwrap();
        assert_eq!(task.status, "pending");
        assert!(!task.cancel_requested);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn corrupt_task_json_fails_startup_instead_of_clearing_queue() {
        let root =
            std::env::temp_dir().join(format!("rayrag-task-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.json");
        std::fs::write(&path, b"{not-json").unwrap();
        let error = match TaskQueue::new(path.to_str().unwrap()) {
            Err(error) => error,
            Ok(_) => panic!("corrupt task JSON must fail startup"),
        };
        assert!(error.to_string().contains("Failed to parse task queue"));
        std::fs::remove_dir_all(root).ok();
    }
}

// ── Agent Canvas ────────────────────────────────────────────────

const AGENT_TITLE_LIMIT: usize = 128;
const AGENT_TAG_LIMIT: usize = 64;
const AGENT_TAGS_FIELD_LIMIT: usize = 512;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Agent {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default = "default_agent_permission")]
    pub permission: String,
    #[serde(default)]
    pub kb_ids: Vec<String>,
    #[serde(default = "default_agent_prompt")]
    pub prompt_template: String,
    #[serde(default)]
    pub dsl: serde_json::Value,
    #[serde(default = "default_agent_category")]
    pub canvas_category: String,
    #[serde(default)]
    pub canvas_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub built_in: bool,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct AgentCreateRequest {
    #[serde(alias = "title")]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub kb_ids: Vec<String>,
    #[serde(default)]
    pub prompt_template: Option<String>,
    #[serde(default)]
    pub dsl: serde_json::Value,
    #[serde(default)]
    pub canvas_category: Option<String>,
    #[serde(default)]
    pub canvas_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub release: Option<bool>,
}

/// Upstream `GET /v1/agents/templates` (`CanvasTemplateService.get_all()`):
/// the seeded RAGFlow workflow templates with their localized title and
/// description. The DSL is omitted here and served per template so the list
/// payload stays small.
pub async fn list_agent_templates(Extension(_auth): Extension<AuthContext>) -> impl IntoResponse {
    let templates = crate::agent_templates::template_metadata();
    Json(serde_json::json!({
        "code": 0,
        "message": "ok",
        "data": templates,
    }))
}

/// `GET /v1/agents/templates/{id}` — one template including its DSL, used by
/// the `/agent-templates` "Use" action to seed a new agent.
pub async fn get_agent_template(Path(id): Path<String>) -> axum::response::Response {
    match crate::agent_templates::template_detail(&id) {
        Some(template) => Json(serde_json::json!({ "code": 0, "data": template })).into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 102, "message": "Template not found" })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct AgentUpdateRequest {
    #[serde(alias = "title")]
    pub name: Option<String>,
    pub description: Option<String>,
    pub permission: Option<String>,
    pub kb_ids: Option<Vec<String>>,
    pub prompt_template: Option<String>,
    pub dsl: Option<serde_json::Value>,
    pub canvas_category: Option<String>,
    pub canvas_type: Option<String>,
    pub tags: Option<Vec<String>>,
    pub avatar: Option<String>,
    pub release: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AgentListQuery {
    pub keywords: Option<String>,
    pub owner_ids: Option<String>,
    pub tags: Option<String>,
    pub canvas_category: Option<String>,
    pub canvas_type: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
    pub desc: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
pub struct AgentTagsUpdate {
    #[serde(default)]
    pub tags: Vec<String>,
}

pub struct AgentStore {
    agents: RwLock<HashMap<String, Agent>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

const CANVAS_VERSION_UNRELEASED_LIMIT: usize = 20;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct CanvasVersion {
    pub id: String,
    pub user_canvas_id: String,
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub release: bool,
    pub dsl: serde_json::Value,
    pub created_at: u64,
    pub updated_at: u64,
}

pub struct CanvasVersionStore {
    versions: RwLock<HashMap<String, CanvasVersion>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl CanvasVersionStore {
    pub fn new(path: impl AsRef<FsPath>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let versions: Vec<CanvasVersion> = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            Vec::new()
        };
        validate_canvas_versions(&versions)?;
        let store = Self {
            versions: RwLock::new(
                versions
                    .into_iter()
                    .map(|version| (version.id.clone(), version))
                    .collect(),
            ),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            versions: RwLock::new(HashMap::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn list(&self, canvas_id: &str) -> Vec<CanvasVersion> {
        let mut versions: Vec<_> = self
            .versions
            .read()
            .unwrap()
            .values()
            .filter(|version| version.user_canvas_id == canvas_id)
            .cloned()
            .collect();
        versions.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| right.id.cmp(&left.id))
        });
        versions
    }

    pub fn get(&self, canvas_id: &str, version_id: &str) -> Option<CanvasVersion> {
        self.versions
            .read()
            .unwrap()
            .get(version_id)
            .filter(|version| version.user_canvas_id == canvas_id)
            .cloned()
    }

    pub fn latest_title(&self, canvas_id: &str, released_only: bool) -> Option<String> {
        self.list(canvas_id)
            .into_iter()
            .find(|version| !released_only || version.release)
            .and_then(|version| version.title)
    }

    pub fn save_or_replace_latest(
        &self,
        canvas_id: &str,
        dsl: serde_json::Value,
        title: Option<String>,
        description: Option<String>,
        release: Option<bool>,
    ) -> anyhow::Result<CanvasVersion> {
        let dsl = crate::agent::normalize_agent_dsl_for_canvas(&dsl);
        crate::agent::validate_agent_dsl(&dsl)?;
        self.mutate(|versions| {
            let latest_id = versions
                .values()
                .filter(|version| version.user_canvas_id == canvas_id)
                .max_by(|left, right| {
                    left.created_at
                        .cmp(&right.created_at)
                        .then_with(|| left.id.cmp(&right.id))
                })
                .map(|version| version.id.clone());
            let now = versions
                .values()
                .filter(|version| version.user_canvas_id == canvas_id)
                .map(|version| version.created_at)
                .max()
                .map_or_else(now_ms, |previous| now_ms().max(previous.saturating_add(1)));
            let saved = if let Some(latest) = latest_id
                .as_ref()
                .and_then(|id| versions.get(id))
                .cloned()
                .filter(|latest| latest.dsl == dsl)
            {
                if latest.release && !release.unwrap_or(false) {
                    let version = CanvasVersion {
                        id: uuid::Uuid::new_v4().to_string(),
                        user_canvas_id: canvas_id.into(),
                        title,
                        description,
                        release: release.unwrap_or(false),
                        dsl,
                        created_at: now,
                        updated_at: now,
                    };
                    versions.insert(version.id.clone(), version.clone());
                    version
                } else {
                    let version = versions.get_mut(&latest.id).unwrap();
                    version.dsl = dsl;
                    if description.is_some() {
                        version.description = description;
                    }
                    if let Some(release) = release {
                        version.release = release;
                    }
                    version.updated_at = now;
                    version.clone()
                }
            } else {
                let version = CanvasVersion {
                    id: uuid::Uuid::new_v4().to_string(),
                    user_canvas_id: canvas_id.into(),
                    title,
                    description,
                    release: release.unwrap_or(false),
                    dsl,
                    created_at: now,
                    updated_at: now,
                };
                versions.insert(version.id.clone(), version.clone());
                version
            };
            prune_canvas_versions(versions, canvas_id);
            Ok(saved)
        })
    }

    pub fn delete_canvas(&self, canvas_id: &str) -> anyhow::Result<()> {
        self.mutate_if_changed(|versions| {
            let before = versions.len();
            versions.retain(|_, version| version.user_canvas_id != canvas_id);
            Ok(((), versions.len() != before))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, CanvasVersion>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_if_changed(|versions| mutation(versions).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, CanvasVersion>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut versions = self.versions.write().unwrap();
        let previous = versions.clone();
        let (value, changed) = mutation(&mut versions)?;
        if changed {
            let snapshot: Vec<_> = versions.values().cloned().collect();
            if let Err(error) = self.persist(&snapshot) {
                *versions = previous;
                return Err(error);
            }
        }
        Ok(value)
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let versions: Vec<_> = self.versions.read().unwrap().values().cloned().collect();
        self.persist(&versions)
    }

    fn persist(&self, versions: &[CanvasVersion]) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(versions)?)
    }
}

impl Default for CanvasVersionStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

impl AgentStore {
    pub fn new(path: impl AsRef<FsPath>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let mut agents: Vec<Agent> = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            Vec::new()
        };
        for agent in &mut agents {
            agent.canvas_category = normalize_agent_category(&agent.canvas_category)?.into();
        }
        if !agents.iter().any(|agent| agent.id == "default") {
            agents.push(default_agent());
        }
        validate_agents(&agents)?;
        let store = Self {
            agents: RwLock::new(
                agents
                    .into_iter()
                    .map(|agent| (agent.id.clone(), agent))
                    .collect(),
            ),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        let default = default_agent();
        Self {
            agents: RwLock::new(HashMap::from([(default.id.clone(), default)])),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn list(&self) -> Vec<Agent> {
        self.agents.read().unwrap().values().cloned().collect()
    }

    pub fn get_accessible(
        &self,
        id: &str,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
    ) -> Option<Agent> {
        self.agents
            .read()
            .unwrap()
            .get(id)
            .filter(|agent| agent_accessible(agent, user_id, is_admin, &is_tenant_member))
            .cloned()
    }

    pub fn list_accessible(
        &self,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
        query: &AgentListQuery,
    ) -> anyhow::Result<(Vec<Agent>, usize)> {
        let owner_ids = split_filter_values(query.owner_ids.as_deref());
        if owner_ids
            .iter()
            .any(|owner_id| owner_id != user_id && !is_tenant_member(owner_id, user_id))
        {
            anyhow::bail!("Only authorized owner_ids can be queried");
        }
        let requested_tags = split_filter_values(query.tags.as_deref());
        let canvas_category = query
            .canvas_category
            .as_deref()
            .map(normalize_agent_category)
            .transpose()?;
        let keywords = query
            .keywords
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let mut agents: Vec<_> = self
            .agents
            .read()
            .unwrap()
            .values()
            .filter(|agent| agent_accessible(agent, user_id, is_admin, &is_tenant_member))
            .filter(|agent| owner_ids.is_empty() || owner_ids.contains(&agent.owner_id))
            .filter(|agent| {
                keywords.is_empty() || agent.name.to_ascii_lowercase().contains(&keywords)
            })
            .filter(|agent| {
                canvas_category.is_none_or(|category| agent.canvas_category == category)
            })
            .filter(|agent| {
                query
                    .canvas_type
                    .as_deref()
                    .is_none_or(|canvas_type| agent.canvas_type == canvas_type)
            })
            .filter(|agent| {
                requested_tags.is_empty()
                    || agent
                        .tags
                        .iter()
                        .any(|tag| requested_tags.contains(&tag.to_ascii_lowercase()))
            })
            .cloned()
            .collect();
        agents.sort_by(|left, right| {
            let order = left
                .created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id));
            if query.desc.unwrap_or(true) {
                order.reverse()
            } else {
                order
            }
        });
        let total = agents.len();
        let page = query.page.unwrap_or(1).max(1);
        let page_size = query.page_size.unwrap_or(50).clamp(1, 200);
        let offset = page.saturating_sub(1).saturating_mul(page_size);
        Ok((
            agents.into_iter().skip(offset).take(page_size).collect(),
            total,
        ))
    }

    pub fn create(&self, owner_id: &str, request: AgentCreateRequest) -> anyhow::Result<Agent> {
        let name = validate_agent_title(&request.name)?;
        let dsl = crate::agent::normalize_agent_dsl_for_canvas(&request.dsl);
        crate::agent::validate_agent_dsl(&dsl)?;
        let permission =
            normalize_agent_permission(request.permission.as_deref().unwrap_or("private"))?;
        let category = normalize_agent_category(
            request
                .canvas_category
                .as_deref()
                .unwrap_or(crate::api::db::CanvasCategory::Agent.as_str()),
        )?;
        let tags = normalize_agent_tags(request.tags);
        self.mutate(|agents| {
            ensure_unique_agent_title(agents.values(), owner_id, category, &name, None)?;
            let now = now_ms();
            let agent = Agent {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                description: request.description,
                owner_id: owner_id.into(),
                permission: permission.into(),
                kb_ids: stable_unique(request.kb_ids),
                prompt_template: request.prompt_template.unwrap_or_else(default_agent_prompt),
                dsl,
                canvas_category: category.into(),
                canvas_type: request.canvas_type,
                tags,
                avatar: request.avatar,
                built_in: false,
                created_at: now,
                updated_at: now,
            };
            agents.insert(agent.id.clone(), agent.clone());
            Ok(agent)
        })
    }

    pub fn update_owned(
        &self,
        id: &str,
        owner_id: &str,
        request: AgentUpdateRequest,
    ) -> anyhow::Result<Option<Agent>> {
        self.mutate_if_changed(|agents| {
            let Some(current) = agents.get(id).cloned() else {
                return Ok((None, false));
            };
            if current.built_in || current.owner_id != owner_id {
                return Ok((None, false));
            }
            let mut updated = current.clone();
            if let Some(name) = request.name {
                updated.name = validate_agent_title(&name)?;
            }
            if let Some(category) = request.canvas_category {
                updated.canvas_category = normalize_agent_category(&category)?.into();
            }
            ensure_unique_agent_title(
                agents.values(),
                owner_id,
                &updated.canvas_category,
                &updated.name,
                Some(id),
            )?;
            if let Some(value) = request.description {
                updated.description = value;
            }
            if let Some(value) = request.permission {
                updated.permission = normalize_agent_permission(&value)?.into();
            }
            if let Some(value) = request.kb_ids {
                updated.kb_ids = stable_unique(value);
            }
            if let Some(value) = request.prompt_template {
                updated.prompt_template = value;
            }
            if let Some(value) = request.dsl {
                updated.dsl = crate::agent::normalize_agent_dsl_for_canvas(&value);
            }
            if let Some(value) = request.canvas_type {
                updated.canvas_type = value;
            }
            if let Some(value) = request.tags {
                updated.tags = normalize_agent_tags(value);
            }
            if let Some(value) = request.avatar {
                updated.avatar = value;
            }
            crate::agent::validate_agent_dsl(&updated.dsl)?;
            let changed = updated != current;
            if changed {
                updated.updated_at = now_ms();
                agents.insert(id.into(), updated.clone());
            }
            Ok((Some(updated), changed))
        })
    }

    pub fn update_tags_owned(
        &self,
        id: &str,
        owner_id: &str,
        tags: Vec<String>,
    ) -> anyhow::Result<Option<Agent>> {
        self.update_owned(
            id,
            owner_id,
            AgentUpdateRequest {
                tags: Some(normalize_agent_tags(tags)),
                ..Default::default()
            },
        )
    }

    pub fn reset_owned(&self, id: &str, owner_id: &str) -> anyhow::Result<Option<Agent>> {
        self.mutate_if_changed(|agents| {
            let Some(current) = agents.get(id).cloned() else {
                return Ok((None, false));
            };
            if current.built_in || current.owner_id != owner_id {
                return Ok((None, false));
            }
            let mut reset = current.clone();
            reset.dsl = crate::agent::normalize_agent_dsl_for_canvas(
                &crate::agent::reset_agent_dsl(&current.dsl),
            );
            crate::agent::validate_agent_dsl(&reset.dsl)?;
            let changed = reset.dsl != current.dsl;
            if changed {
                reset.updated_at = now_ms();
                agents.insert(id.into(), reset.clone());
            }
            Ok((Some(reset), changed))
        })
    }

    pub fn delete_owned(&self, id: &str, owner_id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|agents| {
            let removable = agents
                .get(id)
                .is_some_and(|agent| !agent.built_in && agent.owner_id == owner_id);
            if removable {
                agents.remove(id);
            }
            Ok((removable, removable))
        })
    }

    pub fn tag_counts(
        &self,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
        category: Option<&str>,
    ) -> Vec<serde_json::Value> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for agent in self.agents.read().unwrap().values().filter(|agent| {
            agent_accessible(agent, user_id, is_admin, &is_tenant_member)
                && category.is_none_or(|category| agent.canvas_category == category)
        }) {
            for tag in &agent.tags {
                *counts.entry(tag.clone()).or_default() += 1;
            }
        }
        let mut counts: Vec<_> = counts.into_iter().collect();
        counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        counts
            .into_iter()
            .map(|(tag, count)| serde_json::json!({ "tag": tag, "count": count }))
            .collect()
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, Agent>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_if_changed(|agents| mutation(agents).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, Agent>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut agents = self.agents.write().unwrap();
        let previous = agents.clone();
        let (value, changed) = mutation(&mut agents)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<_> = agents.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *agents = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let agents: Vec<_> = self.agents.read().unwrap().values().cloned().collect();
        self.persist(&agents)
    }

    fn persist(&self, agents: &[Agent]) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(agents)?)
    }
}

impl Default for AgentStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

fn agent_canvas_view(mut agent: Agent) -> Agent {
    agent.dsl = crate::agent::normalize_agent_dsl_for_canvas(&agent.dsl);
    agent
}

fn canvas_version_view(mut version: CanvasVersion) -> CanvasVersion {
    version.dsl = crate::agent::normalize_agent_dsl_for_canvas(&version.dsl);
    version
}

pub async fn list_agents(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<AgentListQuery>,
) -> axum::response::Response {
    match state.agents.list_accessible(
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        &query,
    ) {
        Ok((canvas, total)) => {
            let canvas: Vec<_> = canvas.into_iter().map(agent_canvas_view).collect();
            Json(serde_json::json!({ "code": 0, "data": { "canvas": canvas, "total": total } }))
                .into_response()
        }
        Err(error) => agent_bad_request(error),
    }
}

pub async fn create_agent(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<AgentCreateRequest>,
) -> axum::response::Response {
    if !request.kb_ids.is_empty() && !all_kbs_accessible(&state, &request.kb_ids, &auth) {
        return agent_bad_request(anyhow::anyhow!(
            "One or more knowledge bases are not accessible"
        ));
    }
    let snapshot_description = request.description.clone();
    let snapshot_release = request.release;
    match state.agents.create(&auth.user_id, request) {
        Ok(agent) => {
            if let Err(error) = state.canvas_versions.save_or_replace_latest(
                &agent.id,
                agent.dsl.clone(),
                Some(build_canvas_version_title(
                    state
                        .users
                        .get_user_by_id(&auth.user_id)
                        .map(|user| user.nickname)
                        .as_deref(),
                    &agent.name,
                )),
                Some(snapshot_description),
                snapshot_release,
            ) {
                let _ = state.agents.delete_owned(&agent.id, &auth.user_id);
                return agent_server_error(error);
            }
            Json(serde_json::json!({ "code": 0, "data": agent_canvas_view(agent) })).into_response()
        }
        Err(error) => agent_bad_request(error),
    }
}

pub async fn get_agent(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<String>,
) -> axum::response::Response {
    match state.agents.get_accessible(
        &agent_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) {
        Some(agent) => {
            let agent = agent_canvas_view(agent);
            Json(serde_json::json!({
                "code": 0,
                "data": agent,
                "last_publish_time": state.canvas_versions.list(&agent_id).into_iter()
                    .find(|version| version.release)
                    .map(|version| version.updated_at)
            }))
            .into_response()
        }
        None => agent_not_found(),
    }
}

pub async fn update_agent(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<String>,
    Json(request): Json<AgentUpdateRequest>,
) -> axum::response::Response {
    if request
        .kb_ids
        .as_ref()
        .is_some_and(|kb_ids| !kb_ids.is_empty() && !all_kbs_accessible(&state, kb_ids, &auth))
    {
        return agent_bad_request(anyhow::anyhow!(
            "One or more knowledge bases are not accessible"
        ));
    }
    let Some(current) = state.agents.get_accessible(
        &agent_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) else {
        return agent_not_found();
    };
    let version_update = request.dsl.as_ref().map(|dsl| {
        (
            crate::agent::normalize_agent_dsl_for_canvas(dsl),
            request.description.clone(),
            request.name.clone().unwrap_or_else(|| current.name.clone()),
            request.release,
        )
    });
    match state
        .agents
        .update_owned(&agent_id, &current.owner_id, request)
    {
        Ok(Some(agent)) => {
            if let Some((dsl, description, title, release)) = version_update
                && let Err(error) = state.canvas_versions.save_or_replace_latest(
                    &agent_id,
                    dsl,
                    Some(build_canvas_version_title(
                        state
                            .users
                            .get_user_by_id(&current.owner_id)
                            .map(|user| user.nickname)
                            .as_deref(),
                        &title,
                    )),
                    description,
                    release,
                )
            {
                return agent_server_error(error);
            }
            Json(serde_json::json!({ "code": 0, "data": agent_canvas_view(agent) })).into_response()
        }
        Ok(None) => agent_not_found(),
        Err(error) => agent_bad_request(error),
    }
}

pub async fn delete_agent(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<String>,
) -> axum::response::Response {
    match state.agents.delete_owned(&agent_id, &auth.user_id) {
        Ok(true) => match state.canvas_versions.delete_canvas(&agent_id) {
            Ok(()) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
            Err(error) => agent_server_error(error),
        },
        Ok(false) => agent_not_found(),
        Err(error) => agent_server_error(error),
    }
}

pub async fn list_agent_versions(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<String>,
) -> axum::response::Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return agent_not_found();
    }
    let versions: Vec<_> = state
        .canvas_versions
        .list(&agent_id)
        .into_iter()
        .map(canvas_version_view)
        .collect();
    Json(serde_json::json!({ "code": 0, "data": versions })).into_response()
}

pub async fn get_agent_version(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((agent_id, version_id)): Path<(String, String)>,
) -> axum::response::Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return agent_not_found();
    }
    match state.canvas_versions.get(&agent_id, &version_id) {
        Some(version) => {
            Json(serde_json::json!({ "code": 0, "data": canvas_version_view(version) }))
                .into_response()
        }
        None => agent_not_found(),
    }
}

pub async fn list_agent_tags(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<AgentListQuery>,
) -> axum::response::Response {
    let category = match query
        .canvas_category
        .as_deref()
        .map(normalize_agent_category)
        .transpose()
    {
        Ok(category) => category,
        Err(error) => return agent_bad_request(error),
    };
    Json(serde_json::json!({
        "code": 0,
        "data": state.agents.tag_counts(
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
            category,
        )
    }))
    .into_response()
}

pub async fn update_agent_tags(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<String>,
    Json(request): Json<AgentTagsUpdate>,
) -> axum::response::Response {
    let Some(current) = state.agents.get_accessible(
        &agent_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) else {
        return agent_not_found();
    };
    match state
        .agents
        .update_tags_owned(&agent_id, &current.owner_id, request.tags)
    {
        Ok(Some(agent)) => {
            Json(serde_json::json!({ "code": 0, "data": agent.tags })).into_response()
        }
        Ok(None) => agent_not_found(),
        Err(error) => agent_bad_request(error),
    }
}

/// POST /api/v1/agents/{id}/reset — clear persisted per-run Canvas state.
pub async fn reset_agent(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<String>,
) -> axum::response::Response {
    let Some(current) = state.agents.get_accessible(
        &agent_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) else {
        return agent_not_found();
    };
    match state.agents.reset_owned(&agent_id, &current.owner_id) {
        Ok(Some(agent)) => {
            Json(serde_json::json!({ "code": 0, "data": agent.dsl })).into_response()
        }
        Ok(None) => agent_not_found(),
        Err(error) => agent_server_error(error),
    }
}

/// GET /api/v1/agents/{id}/components/{component_id}/input-form
///
/// Static forms are borrowed from the persisted DSL; the fixed component set's
/// dynamic forms are synthesized without constructing networked tool clients.
pub async fn get_agent_component_input_form(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((agent_id, component_id)): Path<(String, String)>,
) -> axum::response::Response {
    let Some(agent) = state.agents.get_accessible(
        &agent_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) else {
        return Json(serde_json::json!({
            "code": 103,
            "data": null,
            "message": "Make sure you have permission to access the agent."
        }))
        .into_response();
    };

    match crate::agent::agent_component_input_form(&agent.dsl, &component_id) {
        Ok(form) => Json(serde_json::json!({
            "code": 0,
            "data": form,
            "message": "success"
        }))
        .into_response(),
        Err(error) => agent_dsl_error(&component_id, error),
    }
}

/// POST /api/v1/agents/{id}/components/{component_id}/debug
pub async fn debug_agent_component(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((agent_id, component_id)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let request: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(error) => {
            return agent_argument_error(format!("Invalid request: {error}"));
        }
    };
    let Some(params) = request.get("params").and_then(serde_json::Value::as_object) else {
        return agent_argument_error("`params` is required.");
    };
    let mut inputs = serde_json::Map::new();
    for (name, descriptor) in params {
        let Some(descriptor) = descriptor.as_object() else {
            return agent_argument_error(format!("`params.{name}.value` is required."));
        };
        let Some(value) = descriptor.get("value") else {
            return agent_argument_error(format!("`params.{name}.value` is required."));
        };
        inputs.insert(name.clone(), value.clone());
    }

    let Some(agent) = state.agents.get_accessible(
        &agent_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) else {
        return Json(serde_json::json!({
            "code": 103,
            "data": null,
            "message": "Make sure you have permission to access the agent."
        }))
        .into_response();
    };
    let tenant_id = if agent.built_in {
        auth.user_id.as_str()
    } else {
        agent.owner_id.as_str()
    };
    let retriever = AgentWorkflowRetriever {
        state: &state,
        auth: &auth,
        tenant_id,
    };
    let llm_resolver = AgentWorkflowLlmResolver {
        state: &state,
        tenant_id,
    };
    match crate::agent::debug_agent_component(
        &agent.dsl,
        &component_id,
        &inputs,
        crate::agent::AgentComponentDebugContext {
            authenticated_user_id: &auth.user_id,
            llm: state.llm.as_deref(),
            llm_resolver: Some(&llm_resolver),
            retriever: Some(&retriever),
            fallback_kb_ids: &agent.kb_ids,
        },
    )
    .await
    {
        Ok(outputs) => Json(serde_json::json!({
            "code": 0,
            "data": outputs,
            "message": "success"
        }))
        .into_response(),
        Err(crate::agent::AgentComponentDebugError::Dsl(error)) => {
            agent_dsl_error(&component_id, error)
        }
        Err(crate::agent::AgentComponentDebugError::UnsupportedComponent(name)) => {
            Json(serde_json::json!({
                "code": 102,
                "data": null,
                "message": format!("component factory: unsupported component {name}")
            }))
            .into_response()
        }
        Err(crate::agent::AgentComponentDebugError::Invoke(error)) => Json(serde_json::json!({
            "code": 500,
            "data": null,
            "message": format!("invoke: {error}")
        }))
        .into_response(),
    }
}

struct AgentWorkflowRetriever<'a> {
    state: &'a AppState,
    auth: &'a AuthContext,
    tenant_id: &'a str,
}

struct AgentWorkflowLlmResolver<'a> {
    state: &'a AppState,
    tenant_id: &'a str,
}

impl crate::agent::WorkflowLlmResolver for AgentWorkflowLlmResolver<'_> {
    fn resolve_chat_model(&self, selector: &str) -> anyhow::Result<crate::llm::LlmClient> {
        self.state
            .tenant_models
            .resolve(
                &self.state.providers,
                self.tenant_id,
                crate::api::tenant_models::ModelCapability::Chat,
                Some(selector),
            )?
            .map(|model| model.llm_client())
            .ok_or_else(|| anyhow::anyhow!("Chat model is not configured: {selector}"))
    }
}

#[async_trait::async_trait]
impl crate::agent::WorkflowRetriever for AgentWorkflowRetriever<'_> {
    async fn retrieve(
        &self,
        mut request: crate::agent::WorkflowRetrievalRequest,
    ) -> anyhow::Result<crate::agent::WorkflowRetrievalResult> {
        let accessible = self.state.kbs.list_accessible(
            &self.auth.user_id,
            self.auth.is_admin,
            |owner_id, user_id| self.state.tenants.is_member(owner_id, user_id),
        );
        let mut resolved_kb_ids = Vec::with_capacity(request.kb_ids.len());
        for selector in &request.kb_ids {
            if self.state.kbs.get(selector).is_some() {
                resolved_kb_ids.push(selector.clone());
                continue;
            }
            let mut matches = accessible
                .iter()
                .filter(|kb| kb.owner_id == self.tenant_id && kb.name == *selector);
            let kb = matches
                .next()
                .ok_or_else(|| anyhow::anyhow!("Dataset '{selector}' does not exist"))?;
            if matches.next().is_some() {
                anyhow::bail!("Dataset name '{selector}' is ambiguous");
            }
            resolved_kb_ids.push(kb.id.clone());
        }
        resolved_kb_ids.sort();
        resolved_kb_ids.dedup();
        request.kb_ids = resolved_kb_ids;
        if request.kb_ids.is_empty() || !all_kbs_accessible(self.state, &request.kb_ids, self.auth)
        {
            anyhow::bail!("At least one accessible kb_id is required");
        }
        validate_kb_embedding_bindings(self.state, &request.kb_ids)?;
        let metadata_context = crate::api::document_metadata::MetadataFilterContext::new(
            self.tenant_id,
            &request.query,
        );
        let filtered_doc_ids = crate::api::document_metadata::resolve_metadata_doc_ids(
            self.state,
            &metadata_context,
            &request.kb_ids,
            None,
            request.meta_data_filter.as_ref(),
        )
        .await?;
        let vector_weight = (1.0 - request.keywords_similarity_weight).clamp(0.0, 1.0);
        let embedding = if vector_weight > 0.0 {
            Some(
                kb_embedder_for(self.state, &request.kb_ids)?
                    .embed(&[request.query.as_str()])
                    .await
                    .context("Embedding the Retrieval query failed")?
                    .into_iter()
                    .next()
                    .ok_or_else(|| {
                        anyhow::anyhow!("Embedding provider returned no query vector")
                    })?,
            )
        } else {
            None
        };
        let mut candidates =
            self.state
                .engine
                .read()
                .unwrap()
                .hybrid_search_kbs(crate::search::HybridSearchQuery {
                    query: &request.query,
                    query_embedding: embedding.as_deref(),
                    top_k: request.top_k,
                    kb_ids: &request.kb_ids,
                    vector_weight,
                    doc_ids: filtered_doc_ids.as_deref(),
                    rank_feature: None,
                });

        if let Some(rerank_id) = request.rerank_id.as_deref()
            && !candidates.is_empty()
        {
            let reranker = kb_reranker_for(self.state, &request.kb_ids, Some(rerank_id))?
                .ok_or_else(|| anyhow::anyhow!("Reranker is not configured"))?;
            let documents: Vec<_> = candidates
                .iter()
                .map(|candidate| candidate.chunk.content.clone())
                .collect();
            let scores = reranker
                .rerank(&request.query, &documents, candidates.len())
                .await
                .context("Reranking Retrieval candidates failed")?;
            candidates = crate::rerank::apply_hybrid_rerank(candidates, &scores, vector_weight)?;
        }

        let aggregate_candidates: Vec<_> = candidates
            .iter()
            .filter(|candidate| candidate.score >= request.similarity_threshold)
            .cloned()
            .collect();
        let selected: Vec<_> = aggregate_candidates
            .iter()
            .take(request.top_n)
            .cloned()
            .collect();
        let chunks = selected
            .iter()
            .map(|result| {
                let doc_id = result
                    .chunk
                    .metadata
                    .get("doc_id")
                    .cloned()
                    .unwrap_or_default();
                let kb_id = result
                    .chunk
                    .metadata
                    .get("kb_id")
                    .cloned()
                    .unwrap_or_default();
                serde_json::json!({
                    "score": result.score,
                    "vector_similarity": result.vector_score,
                    "term_similarity": result.term_score,
                    "content": result.chunk.content,
                    "doc_name": result.chunk.doc_name,
                    "chunk_id": result.chunk.id,
                    "doc_id": doc_id,
                    "kb_id": kb_id,
                })
            })
            .collect();
        let references = selected
            .iter()
            .map(|result| crate::llm::ChunkReference {
                id: result.chunk.id.clone(),
                kb_id: result
                    .chunk
                    .metadata
                    .get("kb_id")
                    .cloned()
                    .unwrap_or_default(),
                content: result.chunk.content.clone(),
                similarity: Some(result.score),
                vector_similarity: Some(result.vector_score),
                term_similarity: Some(result.term_score),
            })
            .collect();
        let doc_aggs = crate::search::aggregate_documents(&aggregate_candidates)
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()?;
        let formalized_content = format_retrieval_content(&selected, 200_000);
        Ok(crate::agent::WorkflowRetrievalResult {
            formalized_content,
            chunks,
            doc_aggs,
            references,
        })
    }
}

fn format_retrieval_content(
    results: &[crate::search::HybridSearchResult],
    character_limit: usize,
) -> String {
    let mut output = String::new();
    for (index, result) in results.iter().enumerate() {
        let header = format!("Reference {} | {}\n", index + 1, result.chunk.doc_name);
        let separator = if output.is_empty() { "" } else { "\n\n" };
        let remaining = character_limit.saturating_sub(output.chars().count());
        if remaining == 0 {
            break;
        }
        let entry = format!("{separator}{header}{}", result.chunk.content);
        output.extend(entry.chars().take(remaining));
    }
    output
}

/// POST /api/v1/agents/{id}/completions — agent chat completion
pub async fn agent_complete(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(agent_id): Path<String>,
    Json(body): Json<AgentChatRequest>,
) -> axum::response::Response {
    if body.stream {
        if state
            .agents
            .get_accessible(
                &agent_id,
                &auth.user_id,
                auth.is_admin,
                |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
            )
            .is_none()
        {
            return agent_not_found();
        }
        return agent_live_sse_response(state, auth, agent_id, body);
    }
    agent_complete_inner(state, auth, agent_id, body, None).await
}

async fn agent_complete_inner(
    state: Arc<AppState>,
    auth: AuthContext,
    agent_id: String,
    body: AgentChatRequest,
    stream: Option<&AgentStreamContext>,
) -> axum::response::Response {
    let event_question = body.question.clone();
    let Some(agent) = state.agents.get_accessible(
        &agent_id,
        &auth.user_id,
        auth.is_admin,
        |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
    ) else {
        return agent_not_found();
    };
    let workflow = match crate::agent::AgentWorkflow::from_value(&agent.dsl) {
        Ok(workflow) => workflow,
        Err(error) => return agent_bad_request(error),
    };
    if let Some(workflow) = workflow.as_ref() {
        let unsupported = workflow.unsupported_components();
        if !unsupported.is_empty() {
            return agent_bad_request(anyhow::anyhow!(
                "Canvas contains unsupported components: {}",
                unsupported.join(", ")
            ));
        }
    }
    let default_tenant = if agent.built_in {
        auth.user_id.as_str()
    } else {
        agent.owner_id.as_str()
    };
    let tenant_id = body.tenant_id.as_deref().unwrap_or(default_tenant);
    if !state.tenants.is_member(tenant_id, &auth.user_id) {
        return stats_forbidden();
    }
    if !agent.built_in && tenant_id != agent.owner_id {
        return agent_bad_request(anyhow::anyhow!(
            "Agent sessions must use the agent owner's tenant"
        ));
    }
    let kb_ids = if agent.kb_ids.is_empty() {
        body.kb_ids.as_deref().unwrap_or_default()
    } else {
        agent.kb_ids.as_slice()
    };
    if !kb_ids.is_empty() && !all_kbs_accessible(&state, kb_ids, &auth) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "At least one accessible kb_id is required"
            })),
        )
            .into_response();
    }
    let conversation = if let Some(conversation_id) = body.conversation_id.as_deref() {
        let Some(conversation) =
            state
                .conversations
                .get_agent_for(conversation_id, &auth.user_id, tenant_id, &agent.id)
        else {
            return agent_not_found();
        };
        conversation
    } else {
        match state.conversations.create_agent_for(
            &auth.user_id,
            tenant_id,
            &agent.id,
            &body.question.chars().take(30).collect::<String>(),
            kb_ids.to_vec(),
        ) {
            Ok(conversation) => conversation,
            Err(error) => return agent_server_error(error),
        }
    };
    // RAGFlow stamps the canvas version title when the session is created
    // (`create_agent_session` → `UserCanvasVersionService.get_latest_version_title`)
    // so the agent log's Version column is stable for the life of the session.
    if conversation.version_title.is_none()
        && let Some(title) = state.canvas_versions.latest_title(&agent.id, false)
    {
        let _ = state
            .conversations
            .set_agent_version_title(&conversation.id, &title);
    }
    let prompt = agent.prompt_template;
    let dsl_fingerprint = match crate::agent_checkpoint::agent_dsl_fingerprint(&agent.dsl) {
        Ok(fingerprint) => fingerprint,
        Err(error) => return agent_bad_request(error),
    };
    let checkpoint_claim = match state.agent_checkpoints.claim(
        &conversation.id,
        &auth.user_id,
        tenant_id,
        &agent.id,
        body.resume_token.as_deref(),
    ) {
        Ok(crate::agent_checkpoint::AgentCheckpointClaimResult::Claimed(claim)) => Some(claim),
        Ok(crate::agent_checkpoint::AgentCheckpointClaimResult::Missing) => {
            if body.resume_token.is_some() {
                return agent_conflict("Agent resume token is stale or no longer exists");
            }
            None
        }
        Ok(crate::agent_checkpoint::AgentCheckpointClaimResult::Busy) => {
            return agent_conflict("Agent checkpoint is already being resumed");
        }
        Ok(crate::agent_checkpoint::AgentCheckpointClaimResult::ResumeTokenMismatch) => {
            return agent_conflict("Agent resume token does not match the pending checkpoint");
        }
        Err(error) => return agent_server_error(error),
    };
    let resumed = checkpoint_claim.is_some();
    if let Some(claim) = checkpoint_claim.as_ref() {
        if claim.record.dsl_fingerprint != dsl_fingerprint {
            if let Err(error) = state.agent_checkpoints.release(claim) {
                return agent_server_error(error);
            }
            return agent_conflict(
                "Agent canvas changed while waiting; restart the conversation before resuming",
            );
        }
        if workflow.is_none() {
            if let Err(error) = state.agent_checkpoints.release(claim) {
                return agent_server_error(error);
            }
            return agent_conflict(
                "Agent canvas no longer has an executable workflow; restart the conversation",
            );
        }
    }

    let started_at = std::time::Instant::now();
    let default_chat_model = state.tenant_models.default_chat_model(tenant_id);
    let tenant_llm = match state.tenant_models.resolve(
        &state.providers,
        tenant_id,
        crate::api::tenant_models::ModelCapability::Chat,
        default_chat_model.as_deref(),
    ) {
        Ok(model) => model.map(|model| model.llm_client()),
        Err(error) => return agent_bad_request(error),
    };
    let llm = tenant_llm.as_ref().or(state.llm.as_deref());
    let workflow_retriever = AgentWorkflowRetriever {
        state: &state,
        auth: &auth,
        tenant_id,
    };
    let workflow_llm_resolver = AgentWorkflowLlmResolver {
        state: &state,
        tenant_id,
    };
    if let Some(stream) = stream {
        stream.register(state.agent_runs.clone(), &agent.id);
    }
    let event_emitter =
        stream.map(|stream| AgentRunEventEmitter::new(stream, conversation.id.clone()));
    if !resumed && let Some(emitter) = event_emitter.as_ref() {
        emitter.workflow_started(&event_question);
    }
    let mut workflow_outcome = if let Some(workflow) = workflow.as_ref() {
        let input = crate::agent::WorkflowRunInput {
            question: &body.question,
            user_id: &auth.user_id,
            inputs: &body.inputs,
            history: &conversation.messages,
            generation: body.generation,
            llm_resolver: Some(&workflow_llm_resolver),
            retriever: Some(&workflow_retriever),
            fallback_kb_ids: kb_ids,
        };
        let outcome = if let Some(claim) = checkpoint_claim.as_ref() {
            let resume_data = body.resume_data.clone().unwrap_or_else(|| {
                if body.inputs.is_empty() {
                    serde_json::Value::String(body.question.clone())
                } else {
                    serde_json::Value::Object(body.inputs.clone())
                }
            });
            if let Some(observer) = event_emitter.as_ref() {
                workflow
                    .resume_interactive_observed(
                        llm,
                        input,
                        claim.record.checkpoint.clone(),
                        resume_data,
                        observer,
                    )
                    .await
            } else {
                workflow
                    .resume_interactive(llm, input, claim.record.checkpoint.clone(), resume_data)
                    .await
            }
        } else if let Some(observer) = event_emitter.as_ref() {
            workflow
                .run_interactive_observed(llm, input, observer)
                .await
        } else {
            workflow.run_interactive(llm, input).await
        };
        match outcome {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                if let Some(claim) = checkpoint_claim.as_ref()
                    && let Err(release_error) = state.agent_checkpoints.release(claim)
                {
                    return agent_server_error(anyhow::anyhow!(
                        "{error}; additionally failed to release Agent checkpoint: {release_error}"
                    ));
                }
                if let Some(emitter) = event_emitter.as_ref() {
                    emitter.error(&error.to_string());
                }
                // RAGFlow persists the failure on the session
                // (`conv.errors = canvas.error`) so the log table can render a
                // red status dot for the run.
                let _ = state.conversations.record_agent_run(
                    &conversation.id,
                    &agent.dsl,
                    Some(&error.to_string()),
                );
                return agent_bad_request(error);
            }
        }
    } else {
        None
    };

    if matches!(
        workflow_outcome.as_ref(),
        Some(crate::agent::WorkflowRunOutcome::WaitingForUser(_))
    ) {
        let Some(crate::agent::WorkflowRunOutcome::WaitingForUser(waiting)) =
            workflow_outcome.take()
        else {
            unreachable!("waiting outcome was checked");
        };
        let crate::agent::WorkflowWaitingForUser {
            component_id,
            interrupt_id,
            tips,
            inputs,
            path,
            trace,
            checkpoint,
        } = *waiting;
        let saved = if let Some(claim) = checkpoint_claim.as_ref() {
            state
                .agent_checkpoints
                .replace_claimed(claim, &dsl_fingerprint, checkpoint)
        } else {
            state.agent_checkpoints.save_waiting(
                &conversation.id,
                &auth.user_id,
                tenant_id,
                &agent.id,
                &dsl_fingerprint,
                checkpoint,
            )
        };
        let saved = match saved {
            Ok(saved) => saved,
            Err(error) => {
                if let Some(claim) = checkpoint_claim.as_ref() {
                    let _ = state.agent_checkpoints.release(claim);
                }
                return agent_server_error(error);
            }
        };
        let exchange = crate::llm::ConversationExchange {
            question: body.question.clone(),
            answer: String::new(),
            citations: Vec::new(),
            references: Vec::new(),
            settings: None,
            duration_ms: started_at.elapsed().as_millis() as u64,
            usage: None,
        };
        let appended = if let Some(stream) = stream {
            state.conversations.append_exchange_with_settings_id(
                &conversation.id,
                &auth.user_id,
                exchange,
                stream.message_id.clone(),
            )
        } else {
            state.conversations.append_exchange_with_settings(
                &conversation.id,
                &auth.user_id,
                exchange,
            )
        };
        let message_id = match appended {
            Ok(Some(message_id)) => message_id,
            Ok(None) => {
                if checkpoint_claim.is_none() {
                    let _ = state.agent_checkpoints.delete_unclaimed(&saved);
                }
                return agent_not_found();
            }
            Err(error) => {
                if checkpoint_claim.is_none() {
                    let _ = state.agent_checkpoints.delete_unclaimed(&saved);
                }
                return agent_server_error(error);
            }
        };
        // A paused run is a successful step: refresh the DSL snapshot and
        // clear any earlier failure so the log row shows a green dot.
        let _ = state
            .conversations
            .record_agent_run(&conversation.id, &agent.dsl, None);
        if let Some(emitter) = event_emitter.as_ref() {
            emitter.waiting_for_user(
                &component_id,
                tips.as_deref(),
                &inputs,
                &saved.checkpoint_id,
            );
        }
        return Json(serde_json::json!({
            "code": 0,
            "data": {
                "event": "waiting_for_user",
                "answer": "",
                "conversation_id": conversation.id,
                "message_id": message_id,
                "resume_token": saved.checkpoint_id,
                "waiting_for_user": {
                    "kind": "user_fill_up",
                    "cpn_id": component_id,
                    "interrupt_id": interrupt_id,
                    "tips": tips,
                    "inputs": inputs
                },
                "workflow_path": path,
                "workflow_trace": trace,
                "reference": []
            }
        }))
        .into_response();
    }

    let (completion, workflow_path, workflow_trace, workflow_references, run_errors) =
        if let Some(crate::agent::WorkflowRunOutcome::Completed(result)) = workflow_outcome {
            (
                crate::llm::ChatCompletion {
                    content: result.answer,
                    usage: result.usage,
                },
                Some(result.path),
                Some(result.trace),
                result.references,
                None,
            )
        } else if let Some(llm) = llm {
            let mut messages = vec![crate::llm::ChatMessage::new("system", prompt)];
            messages.extend(conversation.messages.clone());
            messages.push(crate::llm::ChatMessage::new("user", body.question.clone()));
            match llm
                .chat_completion_with_generation(&messages, body.generation)
                .await
            {
                Ok(completion) => (completion, None, None, Vec::new(), None),
                Err(error) => (
                    crate::llm::ChatCompletion {
                        content: format!("Error: {error}"),
                        usage: None,
                    },
                    None,
                    None,
                    Vec::new(),
                    Some(error.to_string()),
                ),
            }
        } else {
            (
                crate::llm::ChatCompletion {
                    content: "LLM not configured".into(),
                    usage: None,
                },
                None,
                None,
                Vec::new(),
                Some("LLM not configured".to_string()),
            )
        };
    let exchange = crate::llm::ConversationExchange {
        question: body.question,
        answer: completion.content.clone(),
        citations: Vec::new(),
        references: workflow_references.clone(),
        settings: None,
        duration_ms: started_at.elapsed().as_millis() as u64,
        usage: completion.usage,
    };
    let appended = if let Some(stream) = stream {
        state.conversations.append_exchange_with_settings_id(
            &conversation.id,
            &auth.user_id,
            exchange,
            stream.message_id.clone(),
        )
    } else {
        state
            .conversations
            .append_exchange_with_settings(&conversation.id, &auth.user_id, exchange)
    };
    let message_id = match appended {
        Ok(Some(message_id)) => message_id,
        Ok(None) => {
            if let Some(claim) = checkpoint_claim.as_ref()
                && let Err(error) = state.agent_checkpoints.release(claim)
            {
                return agent_server_error(error);
            }
            return agent_not_found();
        }
        Err(error) => {
            if let Some(claim) = checkpoint_claim.as_ref()
                && let Err(release_error) = state.agent_checkpoints.release(claim)
            {
                return agent_server_error(anyhow::anyhow!(
                    "{error}; additionally failed to release Agent checkpoint: {release_error}"
                ));
            }
            return agent_server_error(error);
        }
    };
    if let Some(claim) = checkpoint_claim.as_ref()
        && let Err(error) = state.agent_checkpoints.delete_claimed(claim)
    {
        return agent_server_error(error);
    }
    // `Completion.save` writes the canvas snapshot and the run's error state
    // onto the session row, which is what the agent log reads back.
    let _ =
        state
            .conversations
            .record_agent_run(&conversation.id, &agent.dsl, run_errors.as_deref());

    if let Some(emitter) = event_emitter.as_ref() {
        emitter.completed(
            &event_question,
            started_at.elapsed(),
            &completion.content,
            &workflow_references,
            completion.usage,
        );
    }
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "answer": completion.content,
            "conversation_id": conversation.id,
            "message_id": message_id,
            "workflow_path": workflow_path,
            "workflow_trace": workflow_trace,
            "reference": workflow_references
        }
    }))
    .into_response()
}

struct AgentStreamContext {
    sender: tokio::sync::mpsc::UnboundedSender<String>,
    run_id: String,
    message_id: String,
    task_id: String,
    cancel: tokio::sync::watch::Sender<bool>,
    lease: Mutex<Option<AgentRunLease>>,
    terminal: std::sync::atomic::AtomicBool,
}

impl AgentStreamContext {
    fn new(
        sender: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> (Self, tokio::sync::watch::Receiver<bool>) {
        let (cancel, cancelled) = tokio::sync::watch::channel(false);
        (
            Self {
                sender,
                run_id: uuid::Uuid::new_v4().to_string(),
                message_id: uuid::Uuid::new_v4().to_string(),
                task_id: uuid::Uuid::new_v4().to_string(),
                cancel,
                lease: Mutex::new(None),
                terminal: std::sync::atomic::AtomicBool::new(false),
            },
            cancelled,
        )
    }

    fn register(&self, registry: Arc<AgentRunRegistry>, canvas_id: &str) {
        let mut lease = self.lease.lock().unwrap();
        if lease.is_none() {
            *lease = Some(registry.register(canvas_id, self.run_id.clone(), self.cancel.clone()));
        }
    }

    fn send_event(&self, session_id: &str, event: &str, data: serde_json::Value) {
        self.send_json(agent_run_event(
            event,
            data,
            &self.message_id,
            &self.task_id,
            session_id,
        ));
    }

    fn send_json(&self, value: serde_json::Value) {
        let serialized = serde_json::to_string(&value).unwrap_or_else(|_| {
            r#"{"code":500,"message":"event serialization failed","data":false}"#.into()
        });
        let _ = self.sender.send(format!("data:{serialized}\n\n"));
    }

    fn error(&self, message: &str) {
        if self
            .terminal
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        self.send_json(serde_json::json!({
            "code": 500,
            "message": message,
            "data": false
        }));
    }

    fn is_terminal(&self) -> bool {
        self.terminal.load(std::sync::atomic::Ordering::Acquire)
    }

    fn done(&self) {
        let _ = self.sender.send("data: [DONE]\n\n".into());
    }
}

struct AgentRunEventEmitter<'a> {
    stream: &'a AgentStreamContext,
    session_id: String,
}

impl<'a> AgentRunEventEmitter<'a> {
    fn new(stream: &'a AgentStreamContext, session_id: String) -> Self {
        Self { stream, session_id }
    }

    fn workflow_started(&self, question: &str) {
        self.stream.send_event(
            &self.session_id,
            "workflow_started",
            serde_json::json!({"inputs": question}),
        );
    }

    fn waiting_for_user(
        &self,
        component_id: &str,
        tips: Option<&str>,
        inputs: &serde_json::Map<String, serde_json::Value>,
        resume_token: &str,
    ) {
        if self
            .stream
            .terminal
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let mut waiting = serde_json::json!({
            "cpn_id": component_id,
            "resume_token": resume_token
        });
        if let Some(tips) = tips {
            waiting["tips"] = serde_json::Value::String(tips.to_owned());
        }
        if !inputs.is_empty() {
            waiting["inputs"] = serde_json::Value::Object(inputs.clone());
        }
        self.stream
            .send_event(&self.session_id, "waiting_for_user", waiting);
    }

    fn completed(
        &self,
        question: &str,
        elapsed: std::time::Duration,
        answer: &str,
        references: &[crate::llm::ChunkReference],
        usage: Option<crate::llm::TokenUsage>,
    ) {
        if self
            .stream
            .terminal
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let mut message = serde_json::json!({"content": answer});
        if !references.is_empty() {
            message["reference"] = serde_json::json!(references);
        }
        self.stream.send_event(&self.session_id, "message", message);

        let mut message_end = serde_json::json!({});
        if !references.is_empty() {
            message_end["reference"] = serde_json::json!(references);
        }
        self.stream
            .send_event(&self.session_id, "message_end", message_end);

        let mut finished = serde_json::json!({
            "inputs": {"query": question},
            "outputs": answer,
            "elapsed_time": elapsed.as_secs_f64(),
            "created_at": agent_event_time_f64()
        });
        if let Some(usage) = usage {
            finished["usage"] = serde_json::json!({
                "prompt_tokens": usage.prompt_tokens,
                "completion_tokens": usage.completion_tokens,
                "total_tokens": usage.total_tokens,
                "calls": 1
            });
        }
        self.stream
            .send_event(&self.session_id, "workflow_finished", finished);
    }

    fn error(&self, message: &str) {
        self.stream.error(message);
    }
}

impl crate::agent::WorkflowEventObserver for AgentRunEventEmitter<'_> {
    fn emit(&self, event: crate::agent::WorkflowLifecycleEvent) {
        self.stream
            .send_event(&self.session_id, event.event, event.data);
    }
}

fn agent_run_event(
    event: &str,
    data: serde_json::Value,
    message_id: &str,
    task_id: &str,
    session_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "event": event,
        "message_id": message_id,
        "created_at": agent_event_time_f64() as u64,
        "task_id": task_id,
        "session_id": session_id,
        "data": data
    })
}

fn agent_live_sse_response(
    state: Arc<AppState>,
    auth: AuthContext,
    agent_id: String,
    mut body: AgentChatRequest,
) -> axum::response::Response {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let (stream, mut cancelled) = AgentStreamContext::new(sender);
    body.stream = false;
    tokio::spawn(async move {
        let run = agent_complete_inner(state, auth, agent_id, body, Some(&stream));
        let Some(response) = await_agent_run_or_cancel(run, &stream.sender, &mut cancelled).await
        else {
            return;
        };
        if !stream.is_terminal() {
            let message = agent_response_error_message(response).await;
            stream.error(&message);
        }
        stream.done();
    });

    let body_stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|frame| (Ok::<String, std::convert::Infallible>(frame), receiver))
    });
    let mut response = axum::body::Body::from_stream(body_stream).into_response();
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        axum::http::header::CONNECTION,
        axum::http::HeaderValue::from_static("keep-alive"),
    );
    response
}

async fn await_agent_run_or_cancel<F>(
    run: F,
    sender: &tokio::sync::mpsc::UnboundedSender<String>,
    cancelled: &mut tokio::sync::watch::Receiver<bool>,
) -> Option<axum::response::Response>
where
    F: std::future::Future<Output = axum::response::Response>,
{
    tokio::pin!(run);
    tokio::select! {
        _ = wait_for_agent_run_cancel(cancelled) => None,
        _ = sender.closed() => None,
        response = &mut run => Some(response),
    }
}

async fn wait_for_agent_run_cancel(cancelled: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *cancelled.borrow() {
            return;
        }
        if cancelled.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn agent_response_error_message(response: axum::response::Response) -> String {
    let status = response.status();
    let fallback = status
        .canonical_reason()
        .unwrap_or("Agent run failed")
        .to_owned();
    let Ok(body) = axum::body::to_bytes(response.into_body(), 1024 * 1024).await else {
        return fallback;
    };
    serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|body| {
            body.get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .filter(|message| !message.is_empty())
        .unwrap_or(fallback)
}

fn agent_event_time_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn default_agent() -> Agent {
    Agent {
        id: "default".into(),
        name: "Default Agent".into(),
        description: "Built-in RAG agent".into(),
        owner_id: String::new(),
        permission: "team".into(),
        kb_ids: Vec::new(),
        prompt_template: default_agent_prompt(),
        dsl: serde_json::json!({}),
        canvas_category: default_agent_category(),
        canvas_type: String::new(),
        tags: Vec::new(),
        avatar: String::new(),
        built_in: true,
        created_at: 0,
        updated_at: 0,
    }
}

fn default_agent_prompt() -> String {
    "You are a helpful assistant. Use the context to answer questions.".into()
}

fn default_agent_permission() -> String {
    "private".into()
}

fn default_agent_category() -> String {
    crate::api::db::CanvasCategory::Agent.as_str().into()
}

fn agent_accessible(
    agent: &Agent,
    user_id: &str,
    is_admin: bool,
    is_tenant_member: &impl Fn(&str, &str) -> bool,
) -> bool {
    agent.built_in
        || agent.owner_id == user_id
        || (agent.permission == "team" && is_tenant_member(&agent.owner_id, user_id))
        || (is_admin && agent.owner_id.is_empty())
}

fn validate_agents(agents: &[Agent]) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    for agent in agents {
        if !ids.insert(agent.id.as_str()) {
            anyhow::bail!("Duplicate agent id: {}", agent.id);
        }
        validate_agent_title(&agent.name)?;
        normalize_agent_permission(&agent.permission)?;
        normalize_agent_category(&agent.canvas_category)?;
        crate::agent::validate_agent_dsl(&agent.dsl)?;
        if !agent.built_in && agent.owner_id.trim().is_empty() {
            anyhow::bail!("Agent owner_id must not be empty");
        }
    }
    Ok(())
}

fn validate_canvas_versions(versions: &[CanvasVersion]) -> anyhow::Result<()> {
    let mut ids = HashSet::new();
    for version in versions {
        if !ids.insert(version.id.as_str()) {
            anyhow::bail!("Duplicate canvas version id: {}", version.id);
        }
        if version.user_canvas_id.trim().is_empty() {
            anyhow::bail!("Canvas version user_canvas_id must not be empty");
        }
        crate::agent::validate_agent_dsl(&version.dsl)?;
    }
    Ok(())
}

fn prune_canvas_versions(versions: &mut HashMap<String, CanvasVersion>, canvas_id: &str) {
    let mut unpublished: Vec<_> = versions
        .values()
        .filter(|version| version.user_canvas_id == canvas_id && !version.release)
        .map(|version| (version.created_at, version.id.clone()))
        .collect();
    unpublished.sort_by(|left, right| right.cmp(left));
    for (_, id) in unpublished
        .into_iter()
        .skip(CANVAS_VERSION_UNRELEASED_LIMIT)
    {
        versions.remove(&id);
    }
}

fn build_canvas_version_title(user_nickname: Option<&str>, agent_title: &str) -> String {
    let tenant = user_nickname
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("tenant");
    let title = match agent_title.trim() {
        "" => "agent",
        title => title,
    };
    format!("{tenant}_{title}_{}", now_ms())
}

fn validate_agent_title(title: &str) -> anyhow::Result<String> {
    let title = title.trim();
    if title.is_empty() {
        anyhow::bail!("Agent title cannot be empty");
    }
    if title.chars().count() > AGENT_TITLE_LIMIT {
        anyhow::bail!("Agent title exceeds limit of {AGENT_TITLE_LIMIT}");
    }
    Ok(title.into())
}

fn normalize_agent_permission(permission: &str) -> anyhow::Result<&'static str> {
    match permission.trim().to_ascii_lowercase().as_str() {
        "private" | "me" => Ok("private"),
        "team" => Ok("team"),
        _ => anyhow::bail!("Agent permission must be 'private' or 'team'"),
    }
}

fn normalize_agent_category(category: &str) -> anyhow::Result<&'static str> {
    match category.trim().to_ascii_lowercase().as_str() {
        "agent_canvas" | "agent" | "standard" => Ok(crate::api::db::CanvasCategory::Agent.as_str()),
        "dataflow_canvas" | "dataflow" | "ingestion" | "ingestion_pipeline" => {
            Ok(crate::api::db::CanvasCategory::DataFlow.as_str())
        }
        _ => anyhow::bail!("Agent canvas_category must be 'agent_canvas' or 'dataflow_canvas'"),
    }
}

fn ensure_unique_agent_title<'a>(
    mut agents: impl Iterator<Item = &'a Agent>,
    owner_id: &str,
    category: &str,
    title: &str,
    exclude_id: Option<&str>,
) -> anyhow::Result<()> {
    if agents.any(|agent| {
        !agent.built_in
            && agent.owner_id == owner_id
            && agent.canvas_category == category
            && exclude_id != Some(agent.id.as_str())
            && agent.name.eq_ignore_ascii_case(title)
    }) {
        anyhow::bail!("Agent '{title}' already exists");
    }
    Ok(())
}

fn normalize_agent_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    let mut used = 0;
    for tag in tags {
        let tag = tag.replace(',', " ");
        let tag: String = tag.trim().chars().take(AGENT_TAG_LIMIT).collect();
        if tag.is_empty() || !seen.insert(tag.to_ascii_lowercase()) {
            continue;
        }
        let extra = tag.len() + usize::from(!normalized.is_empty());
        if used + extra > AGENT_TAGS_FIELD_LIMIT {
            break;
        }
        used += extra;
        normalized.push(tag);
    }
    normalized
}

fn stable_unique(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && seen.insert(value.clone()))
        .collect()
}

fn agent_not_found() -> axum::response::Response {
    (
        axum::http::StatusCode::NOT_FOUND,
        Json(serde_json::json!({ "code": 404, "message": "Agent not found" })),
    )
        .into_response()
}

fn agent_bad_request(error: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
    )
        .into_response()
}

fn agent_conflict(message: impl Into<String>) -> axum::response::Response {
    (
        axum::http::StatusCode::CONFLICT,
        Json(serde_json::json!({ "code": 409, "message": message.into() })),
    )
        .into_response()
}

fn agent_server_error(error: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
    )
        .into_response()
}

fn agent_argument_error(message: impl Into<String>) -> axum::response::Response {
    Json(serde_json::json!({
        "code": 101,
        "data": null,
        "message": message.into()
    }))
    .into_response()
}

fn agent_dsl_error(
    component_id: &str,
    error: crate::agent::AgentDslError,
) -> axum::response::Response {
    let message = match &error {
        crate::agent::AgentDslError::ComponentNotFound(_) => {
            format!("component not found: {component_id}")
        }
        crate::agent::AgentDslError::MissingInputForm(_) => {
            format!("component has no input_form: {component_id}")
        }
        crate::agent::AgentDslError::Malformed(_) => format!("malformed dsl: {error}"),
    };
    Json(serde_json::json!({
        "code": 102,
        "data": null,
        "message": message
    }))
    .into_response()
}

#[cfg(test)]
mod tag_normalization_tests {
    use super::*;

    #[test]
    fn canvas_tags_are_cleaned_deduped_and_bounded() {
        // Mirrors canvas_service.UserCanvasService.update_tags: commas inside
        // a tag are stripped (single comma-separated column), tags deduped
        // case-insensitively preserving order, capped at 64 chars, and the
        // joined value truncated to fit the 512-char field.
        let normalized = normalize_agent_tags(vec![
            "  Marketing ".to_string(),
            "marketing".to_string(),
            "a,b".to_string(),
            "".to_string(),
            "  ".to_string(),
        ]);
        assert_eq!(normalized, vec!["Marketing", "a b"]);
    }

    #[test]
    fn canvas_tags_truncate_individual_tag_and_total_field() {
        let long_tag = "x".repeat(100);
        let normalized = normalize_agent_tags(vec![long_tag.clone(), "y".to_string()]);
        assert_eq!(normalized[0].chars().count(), AGENT_TAG_LIMIT);
        assert_eq!(normalized.len(), 2);

        // Total joined length must not exceed AGENT_TAGS_FIELD_LIMIT.
        let many = (0..20).map(|i| format!("tag-{i}")).collect::<Vec<_>>();
        let normalized = normalize_agent_tags(many);
        let joined = normalized.join(",");
        assert!(joined.len() <= AGENT_TAGS_FIELD_LIMIT);
    }

    #[test]
    fn update_tags_owned_uses_normalized_tags() {
        let store = AgentStore::in_memory();
        let created = store
            .create(
                "owner",
                AgentCreateRequest {
                    name: "Tagged".into(),
                    description: String::new(),
                    permission: None,
                    kb_ids: Vec::new(),
                    prompt_template: None,
                    dsl: serde_json::json!({"components": []}),
                    canvas_category: None,
                    canvas_type: String::new(),
                    tags: vec!["A".into(), "a".into(), "B".into()],
                    avatar: String::new(),
                    release: None,
                },
            )
            .unwrap();
        let agent_id = created.id.clone();
        let updated = store
            .update_tags_owned(&agent_id, "owner", vec!["c,d".into(), "C".into()])
            .unwrap()
            .unwrap();
        assert_eq!(updated.tags, vec!["c d", "C"]);
    }
}

#[cfg(test)]
mod agent_run_registry_tests {
    use super::*;

    struct DropMarker(Arc<std::sync::atomic::AtomicBool>);

    impl Drop for DropMarker {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn dropping_sse_receiver_drops_the_in_flight_run_future() {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (_cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let started = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let run_started = started.clone();
        let run_dropped = dropped.clone();
        let task = tokio::spawn(async move {
            let run = async move {
                let _marker = DropMarker(run_dropped);
                run_started.notify_one();
                std::future::pending::<()>().await;
                axum::body::Body::empty().into_response()
            };
            await_agent_run_or_cancel(run, &sender, &mut cancelled).await
        });
        started.notified().await;
        drop(receiver);
        assert!(task.await.unwrap().is_none());
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn newer_canvas_run_cancels_old_future_without_old_lease_removing_new_run() {
        let registry = Arc::new(AgentRunRegistry::default());
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        let (first_stream, mut first_cancelled) = AgentStreamContext::new(sender);
        first_stream.register(registry.clone(), "canvas-1");
        let started = Arc::new(tokio::sync::Notify::new());
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let run_started = started.clone();
        let run_dropped = dropped.clone();
        let first = tokio::spawn(async move {
            let run = async move {
                let _marker = DropMarker(run_dropped);
                run_started.notify_one();
                std::future::pending::<()>().await;
                axum::body::Body::empty().into_response()
            };
            await_agent_run_or_cancel(run, &first_stream.sender, &mut first_cancelled).await
        });
        started.notified().await;

        let (second_sender, second_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (second_stream, second_cancelled) = AgentStreamContext::new(second_sender);
        second_stream.register(registry.clone(), "canvas-1");
        assert!(first.await.unwrap().is_none());
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
        assert!(registry.contains("canvas-1"));
        assert!(!*second_cancelled.borrow());

        drop(second_stream);
        drop(second_receiver);
        drop(receiver);
        assert!(!registry.contains("canvas-1"));
    }
}

#[cfg(test)]
mod agent_store_tests {
    use super::*;

    fn create_request(name: &str) -> AgentCreateRequest {
        AgentCreateRequest {
            name: name.into(),
            description: "Research assistant".into(),
            permission: None,
            kb_ids: vec!["kb-a".into(), "kb-a".into()],
            prompt_template: None,
            dsl: serde_json::json!({ "components": [] }),
            canvas_category: None,
            canvas_type: String::new(),
            tags: vec!["Rust".into(), "rust".into(), "RAG,Flow".into()],
            avatar: String::new(),
            release: None,
        }
    }

    #[test]
    fn agents_persist_filter_and_enforce_unique_titles() {
        let root = std::env::temp_dir().join(format!("rayrag-agents-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("agents.json");
        let store = AgentStore::new(&path).unwrap();
        let agent = store.create("owner", create_request("Research")).unwrap();
        assert_eq!(agent.canvas_category, "agent_canvas");
        assert_eq!(agent.kb_ids, vec!["kb-a"]);
        assert_eq!(agent.tags, vec!["Rust", "RAG Flow"]);
        assert!(store.create("owner", create_request("research")).is_err());
        drop(store);

        let restored = AgentStore::new(&path).unwrap();
        let query = AgentListQuery {
            keywords: Some("search".into()),
            tags: Some("rust".into()),
            ..Default::default()
        };
        let (agents, total) = restored
            .list_accessible(
                "owner",
                false,
                |tenant_id, user_id| tenant_id == user_id,
                &query,
            )
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(agents[0].id, agent.id);
        assert!(restored.list().iter().any(|agent| agent.id == "default"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn canvas_categories_use_upstream_values_and_migrate_legacy_aliases() {
        let store = AgentStore::in_memory();
        let mut request = create_request("Pipeline");
        request.canvas_category = Some("ingestion".into());
        let pipeline = store.create("owner", request).unwrap();
        assert_eq!(pipeline.canvas_category, "dataflow_canvas");

        let query = AgentListQuery {
            canvas_category: Some("dataflow".into()),
            ..Default::default()
        };
        let (agents, total) = store
            .list_accessible("owner", false, |_, _| false, &query)
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(agents[0].id, pipeline.id);

        let mut invalid = create_request("Invalid category");
        invalid.canvas_category = Some("workflow".into());
        assert!(store.create("owner", invalid).is_err());

        let root =
            std::env::temp_dir().join(format!("rayrag-agent-category-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("agents.json");
        let mut legacy = default_agent();
        legacy.canvas_category = "agent".into();
        std::fs::write(&path, serde_json::to_vec_pretty(&vec![legacy]).unwrap()).unwrap();
        let restored = AgentStore::new(&path).unwrap();
        assert_eq!(
            restored
                .get_accessible("default", "owner", false, |_, _| false)
                .unwrap()
                .canvas_category,
            "agent_canvas"
        );
        let persisted: Vec<Agent> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted[0].canvas_category, "agent_canvas");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn team_agents_are_readable_but_only_owner_can_mutate() {
        let store = AgentStore::in_memory();
        let agent = store.create("owner", create_request("Shared")).unwrap();
        let agent = store
            .update_owned(
                &agent.id,
                "owner",
                AgentUpdateRequest {
                    permission: Some("team".into()),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        assert!(
            store
                .get_accessible(&agent.id, "member", false, |tenant_id, user_id| {
                    tenant_id == "owner" && user_id == "member"
                })
                .is_some()
        );
        assert!(
            store
                .update_owned(
                    &agent.id,
                    "member",
                    AgentUpdateRequest {
                        description: Some("forbidden".into()),
                        ..Default::default()
                    },
                )
                .unwrap()
                .is_none()
        );
        assert!(!store.delete_owned(&agent.id, "member").unwrap());
        assert!(!store.delete_owned("default", "owner").unwrap());
    }

    #[test]
    fn agent_reset_is_atomic_idempotent_and_preserves_canvas_structure() {
        let store = AgentStore::in_memory();
        let mut request = create_request("Resettable");
        request.dsl = serde_json::json!({
            "components": [],
            "graph": {"nodes": [{"id": "begin"}]},
            "history": ["old"],
            "retrieval": [{"id": "chunk"}],
            "memory": ["old"],
            "path": ["begin"],
            "variables": {"region": {"type": "string", "value": "cn"}},
            "globals": {
                "sys.query": "old",
                "env.region": "stale",
                "user.keep": 1
            }
        });
        let original = request.dsl.clone();
        let agent = store.create("owner", request).unwrap();

        let reset = store.reset_owned(&agent.id, "owner").unwrap().unwrap();
        assert_eq!(reset.dsl["history"], serde_json::json!([]));
        assert_eq!(reset.dsl["globals"]["sys.query"], "");
        assert_eq!(reset.dsl["globals"]["env.region"], "cn");
        assert_eq!(reset.dsl["globals"]["user.keep"], 1);
        assert_eq!(reset.dsl["graph"], original["graph"]);
        assert_eq!(original["history"], serde_json::json!(["old"]));

        let unchanged_at = reset.updated_at;
        let again = store.reset_owned(&agent.id, "owner").unwrap().unwrap();
        assert_eq!(again.updated_at, unchanged_at);
        assert!(store.reset_owned(&agent.id, "member").unwrap().is_none());
        assert!(store.reset_owned("default", "owner").unwrap().is_none());
    }

    #[test]
    fn failed_agent_persistence_rolls_back_mutation() {
        let root =
            std::env::temp_dir().join(format!("rayrag-agent-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("agents.json");
        let store = AgentStore::new(&path).unwrap();
        let agent = store.create("owner", create_request("Stable")).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .update_owned(
                    &agent.id,
                    "owner",
                    AgentUpdateRequest {
                        description: Some("must rollback".into()),
                        ..Default::default()
                    },
                )
                .is_err()
        );
        let current = store
            .get_accessible(&agent.id, "owner", false, |_, _| false)
            .unwrap();
        assert_eq!(current.description, "Research assistant");
        let before_reset = current.dsl.clone();
        assert!(store.reset_owned(&agent.id, "owner").is_err());
        assert_eq!(
            store
                .get_accessible(&agent.id, "owner", false, |_, _| false)
                .unwrap()
                .dsl,
            before_reset
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tag_counts_follow_visibility_and_stable_order() {
        let store = AgentStore::in_memory();
        let first = store.create("owner", create_request("One")).unwrap();
        let second = store.create("owner", create_request("Two")).unwrap();
        store
            .update_owned(
                &first.id,
                "owner",
                AgentUpdateRequest {
                    permission: Some("team".into()),
                    tags: Some(vec!["shared".into(), "rust".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        store
            .update_owned(
                &second.id,
                "owner",
                AgentUpdateRequest {
                    tags: Some(vec!["private".into()]),
                    ..Default::default()
                },
            )
            .unwrap();
        let counts = store.tag_counts(
            "member",
            false,
            |tenant_id, user_id| tenant_id == "owner" && user_id == "member",
            Some("agent_canvas"),
        );
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0]["count"], 1);
        assert!(counts.iter().all(|entry| entry["tag"] != "private"));
    }

    #[test]
    fn canvas_versions_replace_equal_dsl_and_protect_released_snapshot() {
        let store = CanvasVersionStore::in_memory();
        let first = store
            .save_or_replace_latest(
                "agent-1",
                serde_json::json!({"components": []}),
                Some("first".into()),
                Some("draft".into()),
                Some(true),
            )
            .unwrap();
        let draft = store
            .save_or_replace_latest(
                "agent-1",
                serde_json::json!({"components": []}),
                Some("second".into()),
                Some("new draft".into()),
                Some(false),
            )
            .unwrap();
        assert_ne!(first.id, draft.id);
        assert_eq!(store.list("agent-1").len(), 2);

        let replaced = store
            .save_or_replace_latest(
                "agent-1",
                serde_json::json!({"components": []}),
                Some("ignored".into()),
                Some("updated draft".into()),
                None,
            )
            .unwrap();
        assert_eq!(replaced.id, draft.id);
        assert_eq!(replaced.title.as_deref(), Some("second"));
        assert_eq!(replaced.description.as_deref(), Some("updated draft"));
        assert_eq!(
            store.latest_title("agent-1", true).as_deref(),
            Some("first")
        );
    }

    #[test]
    fn canvas_versions_keep_all_releases_and_only_twenty_drafts() {
        let store = CanvasVersionStore::in_memory();
        store
            .save_or_replace_latest(
                "agent-1",
                serde_json::json!({"version": "release"}),
                Some("release".into()),
                None,
                Some(true),
            )
            .unwrap();
        for number in 0..25 {
            store
                .save_or_replace_latest(
                    "agent-1",
                    serde_json::json!({"version": number}),
                    Some(format!("draft-{number}")),
                    None,
                    Some(false),
                )
                .unwrap();
        }
        let versions = store.list("agent-1");
        assert_eq!(versions.iter().filter(|version| version.release).count(), 1);
        assert_eq!(
            versions.iter().filter(|version| !version.release).count(),
            20
        );
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
