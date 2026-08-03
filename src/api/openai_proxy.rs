//! OpenAI-compatible API proxy — exposes `/v1/chat/completions`.
//! Enables RayRAG to be used as a drop-in replacement for any OpenAI client.

use axum::{Extension, Json, extract::State, response::IntoResponse};
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
