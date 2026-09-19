//! OpenAI-compatible API proxy — exposes `/v1/chat/completions`.
//! Enables RayRAG to be used as a drop-in replacement for any OpenAI client.

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::llm::ChatMessage;
use crate::server::{AppState, AuthContext};

#[derive(Deserialize)]
pub struct OpenAiChatRequest {
    pub model: String,
    pub messages: Vec<OpenAiMessage>,
    #[serde(default)]
    pub stream: bool,
    #[serde(flatten)]
    pub generation: crate::generation_params::GenerationParamsPatch,
}

#[derive(Deserialize, Serialize)]
pub struct OpenAiMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize)]
pub struct OpenAiChatChoice {
    pub index: u32,
    pub message: OpenAiMessage,
    pub finish_reason: String,
}

#[derive(Serialize)]
pub struct OpenAiUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Serialize)]
pub struct OpenAiChatResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<OpenAiChatChoice>,
    pub usage: Option<OpenAiUsage>,
}

#[derive(Serialize)]
pub struct OpenAiModel {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub owned_by: String,
}

#[derive(Serialize)]
pub struct OpenAiModelList {
    pub object: String,
    pub data: Vec<OpenAiModel>,
}

/// POST /v1/chat/completions — OpenAI-compatible chat endpoint
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<OpenAiChatRequest>,
) -> impl IntoResponse {
    let default_selector = state.tenant_models.default_chat_model(&auth.user_id);
    let requested_selector =
        (body.model != "model" && body.model != "rayrag-v1").then_some(body.model.as_str());
    let selector = requested_selector.or(default_selector.as_deref());
    let tenant_llm = match state.tenant_models.resolve(
        &state.providers,
        &auth.user_id,
        crate::api::tenant_models::ModelCapability::Chat,
        selector,
    ) {
        Ok(model) => model.map(|model| model.llm_client()),
        // Legacy proxy requests accepted any model label while using the
        // environment client. Preserve that only when no tenant default is
        // configured; an explicit tenant default must never silently fall back.
        Err(_) if state.llm.is_some() && default_selector.is_none() => None,
        Err(error) => {
            return Json(serde_json::json!({
                "error": {"message": error.to_string(), "type": "invalid_request_error"}
            }))
            .into_response();
        }
    };
    let Some(llm) = tenant_llm.as_ref().or(state.llm.as_deref()) else {
        return Json(serde_json::json!({
            "error": {"message": "LLM not configured. Set LLM_API_KEY env var.", "type": "server_error"}
        })).into_response();
    };

    let messages: Vec<ChatMessage> = body
        .messages
        .iter()
        .map(|m| ChatMessage::new(m.role.clone(), m.content.clone()))
        .collect();

    if body.stream {
        use axum::response::sse::{Event, KeepAlive, Sse};
        
        use std::convert::Infallible;
        
        let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(8);
        let llm2 = llm.clone();
        let msgs2 = messages.clone();
        let gen2 = body.generation;
        let id2 = id.clone();
        tokio::spawn(async move {
            let full = match llm2.chat_stream(&msgs2, gen2, |_| {}).await {
                Ok(text) => text,
                Err(error) => {
                    let _ = tx
                        .send(Ok(Event::default().event("error").data(error.to_string())))
                        .await;
                    return;
                }
            };
            let chunk = serde_json::json!({
                "id": id2,
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {"role": "assistant", "content": full}, "finish_reason": null}],
            });
            let _ = tx.send(Ok(Event::default().data(chunk.to_string()))).await;
            let done = serde_json::json!({
                "id": id2,
                "object": "chat.completion.chunk",
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
            });
            let _ = tx.send(Ok(Event::default().data(done.to_string()))).await;
            let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
        });
        return Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    let completion = match llm
        .chat_completion_with_generation(&messages, body.generation)
        .await
    {
        Ok(completion) => completion,
        Err(e) => {
            return Json(serde_json::json!({
                "error": {"message": format!("{}", e), "type": "api_error"}
            }))
            .into_response();
        }
    };

    let response = OpenAiChatResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".into(),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        model: body.model,
        choices: vec![OpenAiChatChoice {
            index: 0,
            message: OpenAiMessage {
                role: "assistant".into(),
                content: completion.content,
            },
            finish_reason: "stop".into(),
        }],
        usage: completion.usage.map(|usage| OpenAiUsage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
        }),
    };

    Json(response).into_response()
}

/// GET /v1/models — list available models
/// POST /api/v1/embeddings — OpenAI-compatible embedding endpoint.
pub async fn embeddings(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let input = body
        .get("input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let texts: Vec<String> = match &input {
        serde_json::Value::String(text) => vec![text.clone()],
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => return Json(serde_json::json!({"error":{"message":"input must be a string or array","type":"invalid_request_error","code":400}})).into_response(),
    };
    if texts.is_empty() {
        return Json(serde_json::json!({"error":{"message":"input is empty","type":"invalid_request_error","code":400}})).into_response();
    }
    let resolved = state
        .tenant_models
        .resolve(
            &state.providers,
            &auth.user_id,
            crate::api::tenant_models::ModelCapability::Embedding,
            body.get("model").and_then(|value| value.as_str()),
        )
        .ok()
        .flatten();
    let embedder = resolved
        .map(|model| model.embedder())
        .or_else(|| state.embedder.clone());
    let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let vectors = match embedder {
        Some(embedder) => match embedder.embed(&text_refs).await {
            Ok(vectors) => vectors,
            Err(error) => {
                return Json(serde_json::json!({"error":{"message":error.to_string(),"type":"api_error","code":500}})).into_response();
            }
        },
        None => {
            return Json(serde_json::json!({"error":{"message":"No embedding model configured","type":"api_error","code":503}})).into_response();
        }
    };
    let model = body
        .get("model")
        .and_then(|value| value.as_str())
        .unwrap_or("default");
    let data: Vec<serde_json::Value> = vectors
        .iter()
        .enumerate()
        .map(|(index, vector)| {
            serde_json::json!({
                "object": "embedding",
                "index": index,
                "embedding": vector,
            })
        })
        .collect();
    Json(serde_json::json!({
        "object": "list",
        "data": data,
        "model": model,
        "usage": {"prompt_tokens": 0, "total_tokens": 0},
    }))
    .into_response()
}

/// POST /api/v1/rerank — OpenAI/Jina-compatible rerank endpoint.
pub async fn rerank(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let query = body
        .get("query")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let documents: Vec<String> = body
        .get("documents")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if query.trim().is_empty() || documents.is_empty() {
        return Json(serde_json::json!({
            "error": {"message": "query and documents are required", "type": "invalid_request_error", "code": 400}
        }))
        .into_response();
    }
    let reranker = match state.reranker.current() {
        Some(reranker) => reranker,
        None => {
            return Json(serde_json::json!({
                "error": {"message": "Reranker is not configured", "type": "server_error", "code": 503}
            }))
            .into_response();
        }
    };
    match reranker.rerank(query, &documents, documents.len()).await {
        Ok(scores) => {
            let results: Vec<serde_json::Value> = scores
                .iter()
                .map(|(index, score)| serde_json::json!({"index": index, "relevance_score": score}))
                .collect();
            let model = body
                .get("model")
                .and_then(|value| value.as_str())
                .unwrap_or("rerank");
            Json(serde_json::json!({
                "object": "rerank",
                "model": model,
                "results": results,
                "usage": {"total_tokens": 0},
            }))
            .into_response()
        }
        Err(error) => Json(serde_json::json!({
            "error": {"message": error.to_string(), "type": "api_error", "code": 502}
        }))
        .into_response(),
    }
}

pub async fn list_models() -> impl IntoResponse {
    Json(OpenAiModelList {
        object: "list".into(),
        data: vec![OpenAiModel {
            id: "rayrag-v1".into(),
            object: "model".into(),
            created: 1752000000,
            owned_by: "rayrag".into(),
        }],
    })
}

/// POST /v1/openai/{chat_id}/chat/completions — RAGFlow openai_api.py
/// parity: OpenAI-format completion bound to a conversation's knowledge
/// bases (retrieval-augmented), unlike the plain LLM proxy above.
pub async fn openai_rag_chat_completions(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(chat_id): Path<String>,
    Json(body): Json<OpenAiChatRequest>,
) -> Response {
    let Some(history) = state.conversations.get_for(&chat_id, &auth.user_id) else {
        return Json(serde_json::json!({
            "error": {"message": "Chat not found", "type": "invalid_request_error"}
        }))
        .into_response();
    };

    let question = body
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone())
        .unwrap_or_default();
    if question.trim().is_empty() {
        return Json(serde_json::json!({
            "error": {"message": "No user message provided", "type": "invalid_request_error"}
        }))
        .into_response();
    }

    let kb_ids = history.kb_ids.clone();
    let chat_model = history.chat_model.clone();
    let embedding_model = history.embedding_model.clone();
    let history_msgs = history.messages.clone();

    let generated = crate::api::features::generate_chat_answer(
        &state,
        &auth,
        crate::api::features::ChatGenerationRequest {
            question: &question,
            kb_ids: &kb_ids,
            chat_model: chat_model.as_deref(),
            embedding_model: embedding_model.as_deref(),
            history: &history_msgs,
            generation: body.generation,
        },
        None,
    )
    .await;

    let (answer, references) = match generated {
        Ok(g) => (g.answer, g.references),
        Err(e) => {
            return Json(serde_json::json!({
                "error": {"message": e.to_string(), "type": "server_error"}
            }))
            .into_response();
        }
    };

    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());
    let model = if body.model.is_empty() || body.model == "model" {
        "rayrag".to_string()
    } else {
        body.model.clone()
    };
    let created = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let reference_json: Vec<serde_json::Value> = references
        .iter()
        .map(|r| {
            serde_json::json!({
                "chunk_id": r.id,
                "kb_id": r.kb_id,
                "score": r.similarity,
                "content": r.content.chars().take(200).collect::<String>(),
            })
        })
        .collect();
    Json(serde_json::json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": answer},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
        "references": reference_json
    }))
    .into_response()
}
