//! Chunk manager, Chat system, Provider mgr, Memory, Stats, Task queue, Agent Canvas.
//! Combined module replacing RAGFlow's chunk_api + chat_api + provider_api + memory + stats + task + agent.

use anyhow::Context;
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
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

struct GeneratedChatAnswer {
    answer: String,
    citations: Vec<String>,
    references: Vec<crate::llm::ChunkReference>,
    usage: Option<crate::llm::TokenUsage>,
}

struct ChatGenerationRequest<'a> {
    question: &'a str,
    kb_ids: &'a [String],
    chat_model: Option<&'a str>,
    embedding_model: Option<&'a str>,
    history: &'a [crate::llm::ChatMessage],
    generation: crate::generation_params::GenerationParamsPatch,
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
        Ok(session) => Json(
            serde_json::json!({ "code": 0, "data": { "id": session.id, "name": session.name } }),
        )
        .into_response(),
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

async fn generate_chat_answer(
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
    use crate::providers::ProviderKind;

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
    if !auth.is_admin {
        return provider_admin_required();
    }
    let Some(provider) = state.providers.get_configured(&id) else {
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

const MEMORY_NAME_LIMIT: usize = 128;
const MEMORY_SIZE_LIMIT: u64 = 128 * 1024 * 1024;
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
        let entries: Vec<MemoryEntry> = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            Vec::new()
        };
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
        let requested_tenants =
            split_filter_values(query.tenant_id.as_deref().or(query.owner_ids.as_deref()));
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
                permissions: "private".into(),
                avatar: String::new(),
                description: request.description,
                memory_size: 0,
                forgetting_policy: default_forgetting_policy(),
                temperature: default_memory_temperature(),
                system_prompt: String::new(),
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
        request: MemoryUpdateRequest,
    ) -> anyhow::Result<Option<MemoryEntry>> {
        self.mutate_if_changed(|entries| {
            let Some(current) = entries.get(id).cloned() else {
                return Ok((None, false));
            };
            if current.tenant_id != owner_id {
                return Ok((None, false));
            }
            if current.memory_size > 0
                && (request.memory_type.is_some() || request.embd_id.is_some())
            {
                anyhow::bail!(
                    "Memory embedding model and memory type cannot change after content is stored"
                );
            }
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
                updated.memory_type = normalize_memory_types(memory_type)?;
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
                if memory_size > MEMORY_SIZE_LIMIT {
                    anyhow::bail!("Memory size must not exceed {MEMORY_SIZE_LIMIT} bytes");
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
                if !matches!(policy.as_str(), "FIFO" | "LIFO" | "NONE") {
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
        Ok(entry) => Json(serde_json::json!({ "code": 0, "data": entry })).into_response(),
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
        Some(entry) => Json(serde_json::json!({ "code": 0, "data": entry })).into_response(),
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
    match state
        .memories
        .update_owned(&memory_id, &auth.user_id, request)
    {
        Ok(Some(entry)) => Json(serde_json::json!({ "code": 0, "data": entry })).into_response(),
        Ok(None) => memory_not_found(),
        Err(error) => memory_bad_request(error),
    }
}

pub async fn delete_memory(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(memory_id): Path<String>,
) -> axum::response::Response {
    match state.memories.delete_owned(&memory_id, &auth.user_id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => memory_not_found(),
        Err(error) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
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
    state
        .tenant_models
        .resolve(
            &state.providers,
            tenant_id,
            crate::api::tenant_models::ModelCapability::Chat,
            Some(llm_id),
        )?
        .ok_or_else(|| anyhow::anyhow!("Chat model is not configured: {llm_id}"))?;
    Ok(())
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
        "private" | "me" => Ok("private"),
        "team" => Ok("team"),
        _ => anyhow::bail!("Memory permission must be 'private' or 'team'"),
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
    0.1
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
            kb_ids: Vec::new(),
            chat_model: None,
            embedding_model: None,
            messages,
            duration_ms,
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
    pub status: String, // "pending", "running", "done", "failed", "cancelled"
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
                task.doc_id == doc_id
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
                if task.task_type.is_empty() {
                    task.task_type = "document_parse".into();
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
            for task in tasks.iter_mut().filter(|task| task.doc_id == doc_id) {
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
        "data": state.tasks.list_for(&auth.user_id, auth.is_admin)
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
        drop(store);

        let restored = MemoryStore::new(&path).unwrap();
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
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn team_members_can_read_but_only_owner_can_mutate_memory() {
        let store = MemoryStore::in_memory();
        let entry = store.create("owner", create_request("Shared")).unwrap();
        let updated = store
            .update_owned(
                &entry.id,
                "owner",
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
                .get_accessible(&entry.id, "member", false, |tenant_id, user_id| {
                    tenant_id == "owner" && user_id == "member"
                })
                .is_some()
        );
        assert!(
            store
                .update_owned(
                    &entry.id,
                    "member",
                    MemoryUpdateRequest {
                        description: Some("forbidden".into()),
                        ..Default::default()
                    },
                )
                .unwrap()
                .is_none()
        );
        assert!(!store.delete_owned(&entry.id, "member").unwrap());
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
    fn non_empty_memory_rejects_embedding_and_type_changes() {
        let store = MemoryStore::in_memory();
        let entry = store.create("owner", create_request("Non-empty")).unwrap();
        let entry = store
            .update_owned(
                &entry.id,
                "owner",
                MemoryUpdateRequest {
                    memory_size: Some(1),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
        let error = store
            .update_owned(
                &entry.id,
                "owner",
                MemoryUpdateRequest {
                    memory_type: Some(vec!["episodic".into()]),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("cannot change"));
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
    fn legacy_task_json_without_priority_defaults_to_low() {
        let root = std::env::temp_dir().join(format!("rayrag-tasks-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tasks.json");
        let queue = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let id = queue.push("owner-1", "Legacy", "doc-1").unwrap();
        drop(queue);

        let mut tasks: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        tasks[0].as_object_mut().unwrap().remove("priority");
        std::fs::write(&path, serde_json::to_vec_pretty(&tasks).unwrap()).unwrap();

        let recovered = TaskQueue::new(path.to_str().unwrap()).unwrap();
        let task = recovered
            .list()
            .into_iter()
            .find(|task| task.id == id)
            .unwrap();
        assert_eq!(task.priority, TASK_PRIORITY_LOW);
        std::fs::remove_dir_all(root).ok();
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
                query
                    .canvas_category
                    .as_deref()
                    .is_none_or(|category| agent.canvas_category == category)
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
        let category =
            normalize_agent_category(request.canvas_category.as_deref().unwrap_or("agent"))?;
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
) -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": state.agents.tag_counts(
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
            query.canvas_category.as_deref(),
        )
    }))
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
        let filtered_doc_ids = crate::api::document_metadata::resolve_metadata_doc_ids(
            self.state,
            &request.kb_ids,
            None,
            request.meta_data_filter.as_ref(),
        )?;
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

    let (completion, workflow_path, workflow_trace, workflow_references) =
        if let Some(crate::agent::WorkflowRunOutcome::Completed(result)) = workflow_outcome {
            (
                crate::llm::ChatCompletion {
                    content: result.answer,
                    usage: result.usage,
                },
                Some(result.path),
                Some(result.trace),
                result.references,
            )
        } else if let Some(llm) = llm {
            let mut messages = vec![crate::llm::ChatMessage::new("system", prompt)];
            messages.extend(conversation.messages.clone());
            messages.push(crate::llm::ChatMessage::new("user", body.question.clone()));
            match llm
                .chat_completion_with_generation(&messages, body.generation)
                .await
            {
                Ok(completion) => (completion, None, None, Vec::new()),
                Err(error) => (
                    crate::llm::ChatCompletion {
                        content: format!("Error: {error}"),
                        usage: None,
                    },
                    None,
                    None,
                    Vec::new(),
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
    "agent".into()
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

fn normalize_agent_category(category: &str) -> anyhow::Result<&str> {
    let category = category.trim();
    if category.is_empty() {
        anyhow::bail!("Agent canvas_category cannot be empty");
    }
    Ok(category)
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
            Some("agent"),
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
