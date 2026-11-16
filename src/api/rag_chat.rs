//! `POST /api/v1/chat/completions` — the API guide's conversation endpoint.
//!
//! The guide defines one endpoint with three modes and a stream of
//! `{"code":0,"message":"","data":{"answer":…,"reference":…,"final":…}}` frames. This project had the
//! path bound to its **OpenAI-shaped** handler (which stays available for OpenAI clients at
//! `/v1/chat/completions`), so a client written against the guide received `chat.completion.chunk`
//! objects and a `[DONE]` sentinel instead.
//!
//! Modes, as the guide describes them:
//!
//! | body | behaviour |
//! |---|---|
//! | no `chat_id`, no `session_id` | answer with the tenant's default chat model; `session_id` is `""` |
//! | `chat_id` only | use that assistant's datasets and model, and create a session for it |
//! | `chat_id` + `session_id` (or `session_id` alone) | continue that session |
//!
//! `legacy: true` keeps the v0.23.0 stream: the `answer` field carries everything generated so far
//! and thinking tags stay literal. Otherwise the tags are stripped and signalled with
//! `start_to_think` / `end_to_think`.

use axum::{
    Extension, Json,
    extract::State,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use serde::Deserialize;
use std::sync::Arc;

use crate::server::{AppState, AuthContext};

/// One message in the guide's `messages` list.
#[derive(Debug, Deserialize)]
pub struct RagChatMessage {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: serde_json::Value,
}

impl RagChatMessage {
    /// The text of this message, whether `content` is a string or the guide's list-of-parts form.
    fn text(&self) -> String {
        match &self.content {
            serde_json::Value::String(text) => text.clone(),
            serde_json::Value::Array(parts) => parts
                .iter()
                .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
                .collect::<Vec<_>>()
                .join(""),
            _ => String::new(),
        }
    }
}

/// The guide's request body, with this project's own names accepted alongside.
#[derive(Debug, Default, Deserialize)]
pub struct RagChatRequest {
    #[serde(default)]
    pub messages: Option<Vec<RagChatMessage>>,
    #[serde(default)]
    pub question: Option<String>,
    /// The guide defaults this endpoint to streaming.
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub llm_id: Option<String>,
    #[serde(default)]
    pub pass_all_history_messages: Option<bool>,
    #[serde(default)]
    pub legacy: Option<bool>,
    // This project's own names, kept so existing callers do not have to change.
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub kb_ids: Option<Vec<String>>,
    #[serde(default)]
    pub chat_model: Option<String>,
    #[serde(default)]
    pub embedding_model: Option<String>,
    #[serde(flatten)]
    pub generation: crate::generation_params::GenerationParamsPatch,
}

/// The `data` object of one non-final stream frame.
fn frame(
    answer: &str,
    message_id: &str,
    session_id: &str,
    chat_id: Option<&str>,
    legacy_cumulative: Option<&str>,
) -> serde_json::Value {
    let mut data = serde_json::json!({
        "answer": legacy_cumulative.unwrap_or(answer),
        "reference": { "chunks": [] },
        "audio_binary": null,
        "prompt": "",
        "created_at": now_seconds(),
        "final": false,
        "id": message_id,
        "session_id": session_id,
    });
    if let Some(chat_id) = chat_id
        && let Some(object) = data.as_object_mut()
    {
        object.insert("chat_id".to_string(), serde_json::json!(chat_id));
    }
    data
}

fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// `POST /api/v1/chat/completions`.
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<RagChatRequest>,
) -> Response {
    // "Either messages or question is required."
    let question = body
        .question
        .as_deref()
        .map(str::trim)
        .filter(|question| !question.is_empty())
        .map(str::to_string)
        .or_else(|| {
            body.messages
                .as_ref()?
                .iter()
                .rev()
                .find(|message| message.role == "user")
                .map(RagChatMessage::text)
                .filter(|text| !text.trim().is_empty())
        });
    let Some(question) = question else {
        return crate::server::api_error_code(
            axum::http::StatusCode::BAD_REQUEST,
            crate::server::code::INVALID_OR_MISSING_DATA,
            "Please input your question.",
        );
    };
    let stream = body.stream.unwrap_or(true);
    let legacy = body.legacy.unwrap_or(false);

    // Resolve the assistant and the session the guide's three modes describe.
    let requested_session = body
        .session_id
        .clone()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            body.conversation_id
                .clone()
                .filter(|value| !value.is_empty())
        });
    let assistant = match body.chat_id.as_deref().filter(|value| !value.is_empty()) {
        Some(chat_id) => match crate::api::features::chat_assistant_for(&state, &auth, chat_id) {
            Some(app) => Some(app),
            None => {
                return crate::server::api_error_code(
                    axum::http::StatusCode::NOT_FOUND,
                    crate::server::code::INVALID_OR_MISSING_DATA,
                    "no authorization",
                );
            }
        },
        None => None,
    };
    let (session_id, kb_ids, chat_model) = match requested_session {
        Some(session_id) => {
            let Some(session) = state.conversations.get_for(&session_id, &auth.user_id) else {
                return crate::server::api_error_code(
                    axum::http::StatusCode::NOT_FOUND,
                    crate::server::code::INVALID_OR_MISSING_DATA,
                    "Session not found!",
                );
            };
            (
                session_id,
                session.kb_ids.clone(),
                body.llm_id
                    .clone()
                    .or_else(|| body.chat_model.clone())
                    .or(session.chat_model.clone()),
            )
        }
        None => match &assistant {
            // "With chat_id but no session_id: use that chat's configuration and automatically
            // create a new session."
            Some(app) => {
                let name: String = question.chars().take(64).collect();
                let tenant_id = if app.tenant_id.is_empty() {
                    auth.user_id.clone()
                } else {
                    app.tenant_id.clone()
                };
                let created = state.conversations.create_for_tenant_settings(
                    &auth.user_id,
                    &tenant_id,
                    &name,
                    app.kb_ids.clone(),
                    body.llm_id.clone().or_else(|| app.llm_id.clone()),
                    body.embedding_model.clone(),
                );
                match created.and_then(|session| {
                    state
                        .conversations
                        .set_app_id(&session.id, &app.id)
                        .map(|_| session)
                }) {
                    Ok(session) => (
                        session.id,
                        session.kb_ids.clone(),
                        body.llm_id.clone().or_else(|| app.llm_id.clone()),
                    ),
                    Err(error) => {
                        let message = format!("Could not start a session: {error}");
                        return crate::server::api_error_code(
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            crate::server::code::OPERATION_ERROR,
                            &message,
                        );
                    }
                }
            }
            // "No chat_id: talk directly with the tenant's default chat model."
            None => (
                String::new(),
                body.kb_ids.clone().unwrap_or_default(),
                body.llm_id.clone().or_else(|| body.chat_model.clone()),
            ),
        },
    };
    let chat_id = assistant.as_ref().map(|app| app.id.clone());
    let message_id = format!("chatcmpl-{}", uuid::Uuid::new_v4());

    if stream {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(16);
        let state = state.clone();
        let auth = auth.clone();
        let question = question.clone();
        let session_id = session_id.clone();
        let message_id = message_id.clone();
        let history = if session_id.is_empty() {
            Vec::new()
        } else {
            state
                .conversations
                .get_for(&session_id, &auth.user_id)
                .map(|session| session.messages)
                .unwrap_or_default()
        };
        let generation = body.generation;
        tokio::spawn(async move {
            let sender = tx.clone();
            // `legacy` reports the whole answer so far; otherwise only the new text, with the
            // guide's thinking signals emitted when the model's own tags appear.
            let state_cell = std::sync::Mutex::new((String::new(), false, false));
            let on_chunk: Arc<dyn Fn(&str) + Send + Sync> = {
                let sender = sender.clone();
                let message_id = message_id.clone();
                let session_id = session_id.clone();
                let chat_id = chat_id.clone();
                Arc::new(move |chunk: &str| {
                    let mut guard = state_cell.lock().expect("chat stream state");
                    let (cumulative, started, ended) = &mut *guard;
                    cumulative.push_str(chunk);
                    let payload = if legacy {
                        frame(
                            chunk,
                            &message_id,
                            &session_id,
                            chat_id.as_deref(),
                            Some(cumulative),
                        )
                    } else {
                        let mut text = chunk.to_string();
                        let mut start = false;
                        let mut end = false;
                        if !*started && text.contains("<think>") {
                            text = text.replace("<think>", "");
                            *started = true;
                            start = true;
                        }
                        if text.contains("</think>") {
                            text = text.replace("</think>", "");
                            *ended = true;
                            end = true;
                        }
                        let mut payload =
                            frame(&text, &message_id, &session_id, chat_id.as_deref(), None);
                        if let Some(object) = payload.as_object_mut() {
                            if start {
                                object.insert("start_to_think".into(), serde_json::json!(true));
                            }
                            if end {
                                object.insert("end_to_think".into(), serde_json::json!(true));
                            }
                        }
                        payload
                    };
                    let event = serde_json::json!({ "code": 0, "message": "", "data": payload });
                    let _ = sender.blocking_send(Ok(Event::default().data(event.to_string())));
                })
            };
            let generated = if session_id.is_empty() {
                // No session means no dataset: answer with the tenant's default chat model.
                let mut messages = vec![crate::llm::ChatMessage::new("user", &question)];
                if let Some(previous) = history.last() {
                    messages.insert(0, previous.clone());
                }
                match state.llm.as_ref() {
                    Some(client) => client
                        .chat_completion_with_generation(&messages, generation)
                        .await
                        .map(|answer| {
                            on_chunk(&answer.content);
                            crate::api::features::GeneratedChatAnswer {
                                answer: answer.content,
                                citations: Vec::new(),
                                references: Vec::new(),
                                usage: answer.usage,
                            }
                        }),
                    None => Err(anyhow::anyhow!(
                        "No chat model is configured on this deployment"
                    )),
                }
            } else {
                crate::api::features::generate_chat_answer(
                    &state,
                    &auth,
                    crate::api::features::ChatGenerationRequest {
                        question: &question,
                        kb_ids: &kb_ids,
                        chat_model: chat_model.as_deref(),
                        embedding_model: body.embedding_model.as_deref(),
                        history: &history,
                        generation,
                        metadata_condition: None,
                    },
                    Some(on_chunk),
                )
                .await
            };
            match generated {
                Ok(generated) => {
                    let mut payload = frame("", &message_id, &session_id, chat_id.as_deref(), None);
                    if let Some(object) = payload.as_object_mut() {
                        object.insert(
                            "reference".to_string(),
                            crate::api::features::reference_payload(&generated.references),
                        );
                        if legacy {
                            object
                                .insert("answer".to_string(), serde_json::json!(generated.answer));
                        }
                    }
                    let event = serde_json::json!({ "code": 0, "message": "", "data": payload });
                    let _ = sender
                        .send(Ok(Event::default().data(event.to_string())))
                        .await;
                }
                Err(error) => {
                    // A failed generation that still ended with `data: true` would look like a
                    // successful empty answer; report it instead.
                    let message = error.to_string();
                    tracing::error!(%message, "chat completion failed");
                    let event =
                        serde_json::json!({ "code": 102, "message": message, "data": null });
                    let _ = sender
                        .send(Ok(Event::default().data(event.to_string())))
                        .await;
                }
            }
            let done = serde_json::json!({ "code": 0, "message": "", "data": true });
            let _ = sender
                .send(Ok(Event::default().data(done.to_string())))
                .await;
        });
        return Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx))
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    // Non-stream: the guide documents one complete object.
    let history = if session_id.is_empty() {
        Vec::new()
    } else {
        state
            .conversations
            .get_for(&session_id, &auth.user_id)
            .map(|session| session.messages)
            .unwrap_or_default()
    };
    let generated = if session_id.is_empty() {
        let messages = vec![crate::llm::ChatMessage::new("user", &question)];
        match match state.llm.as_ref() {
            Some(client) => {
                client
                    .chat_completion_with_generation(&messages, body.generation)
                    .await
            }
            None => Err(anyhow::anyhow!(
                "No chat model is configured on this deployment"
            )),
        } {
            Ok(answer) => Ok(crate::api::features::GeneratedChatAnswer {
                answer: answer.content,
                citations: Vec::new(),
                references: Vec::new(),
                usage: answer.usage,
            }),
            Err(error) => Err(error),
        }
    } else {
        crate::api::features::generate_chat_answer(
            &state,
            &auth,
            crate::api::features::ChatGenerationRequest {
                question: &question,
                kb_ids: &kb_ids,
                chat_model: chat_model.as_deref(),
                embedding_model: body.embedding_model.as_deref(),
                history: &history,
                generation: body.generation,
                metadata_condition: None,
            },
            None,
        )
        .await
    };
    match generated {
        Ok(generated) => {
            let mut data = serde_json::json!({
                "answer": generated.answer,
                "reference": crate::api::features::reference_payload(&generated.references),
                "audio_binary": null,
                "id": message_id,
                "session_id": session_id,
            });
            if let Some(chat_id) = chat_id.as_deref()
                && let Some(object) = data.as_object_mut()
            {
                object.insert("chat_id".to_string(), serde_json::json!(chat_id));
            }
            Json(serde_json::json!({ "code": 0, "message": "", "data": data })).into_response()
        }
        Err(error) => {
            let message = error.to_string();
            tracing::error!(%message, "chat completion failed");
            crate::server::api_error_code(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                crate::server::code::CONNECTION_ERROR,
                &message,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_message_supplies_the_question_when_question_is_absent() {
        let message = RagChatMessage {
            role: "user".into(),
            content: serde_json::json!("What is RayRAG?"),
        };
        assert_eq!(message.text(), "What is RayRAG?");
        let parts = RagChatMessage {
            role: "user".into(),
            content: serde_json::json!([{ "type": "text", "text": "part one " }, { "type": "text", "text": "part two" }]),
        };
        assert_eq!(parts.text(), "part one part two");
    }

    #[test]
    fn a_frame_carries_every_field_the_guide_documents() {
        let payload = frame("hello", "msg-1", "sess-1", Some("chat-1"), None);
        for key in [
            "answer",
            "reference",
            "audio_binary",
            "prompt",
            "created_at",
            "final",
            "id",
            "session_id",
            "chat_id",
        ] {
            assert!(payload.get(key).is_some(), "missing {key}: {payload}");
        }
        assert_eq!(payload["final"], serde_json::json!(false));
        assert_eq!(payload["answer"], serde_json::json!("hello"));
    }

    /// The reference object the guide documents: `chunks` keyed by index, and `doc_aggs` aggregated
    /// from the document names the retrieval path now carries.
    #[test]
    fn the_reference_object_carries_documents_and_their_aggregates() {
        let reference = |id: &str, doc: &str, with_metadata: bool| crate::llm::ChunkReference {
            id: id.into(),
            kb_id: "kb-1".into(),
            content: "text".into(),
            similarity: Some(0.5),
            vector_similarity: Some(0.4),
            term_similarity: Some(0.6),
            document_metadata: with_metadata.then(|| {
                let mut metadata = serde_json::Map::new();
                metadata.insert("author".into(), serde_json::json!("bob"));
                metadata
            }),
            document_id: Some(format!("doc-{doc}")),
            document_name: Some(doc.into()),
        };
        let payload = crate::api::features::reference_payload(&[
            reference("chunk-1", "INSTALL.md", true),
            reference("chunk-2", "INSTALL.md", false),
            reference("chunk-3", "guide.md", false),
        ]);
        assert_eq!(payload["chunks"]["0"]["id"], "chunk-1");
        assert_eq!(payload["chunks"]["0"]["dataset_id"], "kb-1");
        assert_eq!(payload["chunks"]["0"]["document_name"], "INSTALL.md");
        assert_eq!(
            payload["chunks"]["0"]["document_metadata"]["author"], "bob",
            "requested metadata travels with its chunk: {payload}"
        );
        assert!(
            payload["chunks"]["1"].get("document_metadata").is_none(),
            "a chunk without metadata does not invent one"
        );
        assert_eq!(payload["chunks"]["2"]["document_name"], "guide.md");
        assert_eq!(payload["doc_aggs"]["INSTALL.md"]["count"], 2);
        assert_eq!(
            payload["doc_aggs"]["INSTALL.md"]["doc_id"],
            "doc-INSTALL.md"
        );
        assert_eq!(payload["doc_aggs"]["guide.md"]["count"], 1);
    }

    #[test]
    fn the_legacy_frame_reports_everything_generated_so_far() {
        let payload = frame("ate", "msg-1", "sess-1", None, Some("generated so far"));
        assert_eq!(payload["answer"], serde_json::json!("generated so far"));
        assert!(
            payload.get("chat_id").is_none(),
            "a mode without an assistant has no chat_id: {payload}"
        );
    }
}
