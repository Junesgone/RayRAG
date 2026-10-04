//! `POST /api/v1/searchbots/ask` — the embedded search bot's streaming answer.
//!
//! Upstream (`bot_api.ask_about_embedded`) validates `question` and `kb_ids`, optionally merges the
//! `search_config` of a saved search app, refuses datasets the caller cannot reach, and answers with a
//! Server-Sent Events stream in its own envelope:
//!
//! ```text
//! data:{"code": 0, "message": "", "data": {"answer": "…", "reference": {…}}}
//! data:{"code": 500, "message": "…", "data": {"answer": "**ERROR**: …"}}
//! data:{"code": 0, "message": "", "data": true}
//! ```
//!
//! RayRAG had none of it. The retrieval half reuses the existing search endpoint rather than growing a
//! second retrieval path: the two must not drift, and the alternative is a copy of its scoring rules.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::llm::{ChatMessage, LlmClient};
use crate::server::{AppState, AuthContext, api_error_code, code, kb_accessible};

/// How much retrieved context to put in front of the model, in characters. Upstream lets the retrieval
/// settings decide how many chunks; this only stops one enormous chunk from filling the window.
const MAX_CONTEXT_CHARS: usize = 12_000;

#[derive(Debug, serde::Deserialize, Default)]
pub struct AskRequest {
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub kb_ids: Option<Vec<String>>,
    /// The embedded client sends the singular spelling; upstream's `ask` wants `kb_ids`.
    #[serde(default)]
    pub kb_id: Option<Value>,
    #[serde(default)]
    pub search_id: Option<String>,
    #[serde(default)]
    pub top_k: Option<u64>,
    #[serde(default)]
    pub similarity_threshold: Option<f64>,
    #[serde(default)]
    pub vector_similarity_weight: Option<f64>,
    #[serde(default)]
    pub rerank_id: Option<String>,
}

/// One SSE frame in upstream's envelope.
fn frame(value: Value) -> String {
    format!("data:{value}\n\n")
}

/// The `data` payload of an answer frame.
fn answer_frame(answer: &str, reference: Value) -> String {
    frame(json!({
        "code": 0,
        "message": "",
        "data": { "answer": answer, "reference": reference },
    }))
}

/// The failure inside the stream. Upstream keeps the HTTP status at 200 and reports the failure in the
/// frame, with the error text repeated inside `answer` so a client that renders only the answer still
/// shows what happened.
fn error_frame(message: &str) -> String {
    frame(json!({
        "code": 500,
        "message": message,
        "data": { "answer": format!("**ERROR**: {message}") },
    }))
}

/// The final frame, which tells a client the stream ended normally.
fn done_frame() -> String {
    frame(json!({ "code": 0, "message": "", "data": true }))
}

/// Build the prompt from the question and the retrieved chunks, the way the chat surface does.
fn build_prompt(question: &str, chunks: &[Value]) -> String {
    let mut context = String::new();
    for chunk in chunks {
        let content = chunk
            .get("content_with_weight")
            .or_else(|| chunk.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if content.is_empty() {
            continue;
        }
        let name = chunk
            .get("docnm_kwd")
            .or_else(|| chunk.get("document_keyword"))
            .and_then(Value::as_str)
            .unwrap_or("document");
        context.push_str(&format!("【{name}】{content}\n"));
        if context.len() >= MAX_CONTEXT_CHARS {
            break;
        }
    }
    if context.is_empty() {
        return question.to_string();
    }
    format!(
        "Answer the question using only the reference material below. If it does not contain the \
         answer, say so.\n\nQuestion: {question}\n\nReference material:\n{context}"
    )
}

/// `POST /api/v1/searchbots/ask`.
pub async fn ask(
    State(state): State<Arc<AppState>>,
    axum::Extension(auth): axum::Extension<AuthContext>,
    Json(body): Json<AskRequest>,
) -> Response {
    // Upstream's `validate_request("question", "kb_ids")` answers with an argument error before it
    // looks at anything else.
    let question = body
        .question
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from);
    let Some(question) = question else {
        return api_error_code(
            StatusCode::BAD_REQUEST,
            code::INVALID_ARGUMENT,
            "Missing required argument: question",
        );
    };
    // `kb_ids` is the documented shape; the embedded client also sends `kb_id`, singular, which may be
    // one id or a list.
    let mut requested_kb_ids = body
        .kb_ids
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect::<Vec<_>>();
    if requested_kb_ids.is_empty() {
        requested_kb_ids = match body.kb_id.as_ref() {
            Some(Value::String(id)) if !id.trim().is_empty() => vec![id.clone()],
            Some(Value::Array(ids)) => ids
                .iter()
                .filter_map(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(String::from)
                .collect(),
            _ => Vec::new(),
        };
    }

    // A saved search app may carry its own datasets; upstream prefers them when present.
    let search_app = body
        .search_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(|id| state.search_apps.as_ref().and_then(|store| store.get(id)));
    let mut effective_kb_ids = requested_kb_ids.clone();
    if let Some(app) = &search_app {
        let configured: Vec<String> = app
            .kb_ids
            .iter()
            .filter(|id| !id.trim().is_empty())
            .cloned()
            .collect();
        if !configured.is_empty() {
            effective_kb_ids = configured;
        }
    }
    if effective_kb_ids.is_empty() {
        return api_error_code(
            StatusCode::BAD_REQUEST,
            code::INVALID_ARGUMENT,
            "Missing required argument: kb_ids",
        );
    }
    if !effective_kb_ids
        .iter()
        .all(|kb_id| kb_accessible(&state, kb_id, &auth))
    {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            "You don't own the requested dataset",
        );
    }

    // Retrieve through the existing search endpoint, so the scoring rules live in one place. Its
    // response is read back as JSON because that is the shape it publishes.
    let search_body = json!({
        "question": question,
        "kb_ids": effective_kb_ids,
        "top_k": body.top_k.unwrap_or(10),
        "similarity_threshold": body.similarity_threshold.unwrap_or(0.2),
        "vector_similarity_weight": body.vector_similarity_weight.unwrap_or(0.3),
        "rerank_id": body.rerank_id.clone().unwrap_or_default(),
    });
    let search_request =
        match serde_json::from_value::<crate::api::search::SearchRequest>(search_body) {
            Ok(request) => request,
            Err(error) => {
                return api_error_code(
                    StatusCode::BAD_REQUEST,
                    code::INVALID_ARGUMENT,
                    &format!("the search request could not be built: {error}"),
                );
            }
        };
    let search_response = crate::api::search::weighted_search(
        State(state.clone()),
        axum::Extension(auth.clone()),
        Json(search_request),
    )
    .await;
    let search_json: Value = match axum::body::to_bytes(search_response.into_body(), 8 << 20).await
    {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        Err(error) => {
            return api_error_code(
                StatusCode::INTERNAL_SERVER_ERROR,
                code::OPERATION_ERROR,
                &format!("the retrieval result could not be read: {error}"),
            );
        }
    };
    let chunks: Vec<Value> = search_json
        .get("data")
        .and_then(|data| data.get("chunks"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let doc_aggs = search_json
        .get("data")
        .and_then(|data| data.get("doc_aggs"))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let reference = json!({ "chunks": chunks, "doc_aggs": doc_aggs, "total": chunks.len() });

    // The answer itself needs a chat model, resolved the same way the rest of RayRAG resolves one.
    // When none is reachable the stream says so in a frame rather than answering from nothing.
    let prompt = build_prompt(&question, &chunks);
    let client = match state.tenant_models.resolve(
        &state.providers,
        &auth.user_id,
        crate::api::tenant_models::ModelCapability::Chat,
        None,
    ) {
        Ok(Some(model)) => model.llm_client(),
        Ok(None) => {
            let mut failure = error_frame("no chat model is configured for this account");
            failure.push_str(&answer_frame("", reference.clone()));
            failure.push_str(&done_frame());
            return sse_response(failure);
        }
        Err(error) => {
            let mut failure =
                error_frame(&format!("the chat model could not be resolved: {error}"));
            failure.push_str(&answer_frame("", reference.clone()));
            failure.push_str(&done_frame());
            return sse_response(failure);
        }
    };
    let messages = vec![ChatMessage::new("user", prompt)];
    let answer = match client.chat_completion(&messages).await {
        Ok(completion) if !completion.content.trim().is_empty() => completion.content,
        Ok(_) => "The model returned an empty answer for this question.".to_string(),
        Err(error) => {
            // Retrieval still succeeded, so the failure frame carries the references too: a client can
            // show what was found even when the model could not be reached.
            let mut failure = error_frame(&format!("the chat model could not answer: {error}"));
            failure.push_str(&answer_frame("", reference.clone()));
            failure.push_str(&done_frame());
            return sse_response(failure);
        }
    };

    let mut stream = answer_frame(&answer, reference);
    stream.push_str(&done_frame());
    sse_response(stream)
}

/// Upstream's streaming headers, verbatim.
fn sse_response(body: String) -> Response {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/event-stream; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (header::CONNECTION, "keep-alive"),
            (header::HeaderName::from_static("x-accel-buffering"), "no"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frames_use_upstreams_envelope() {
        let answer = answer_frame("hello", json!({"chunks": [], "doc_aggs": [], "total": 0}));
        assert!(answer.starts_with("data:{"), "{answer}");
        assert!(
            answer.ends_with("\n\n"),
            "each frame ends with a blank line"
        );
        let payload: Value =
            serde_json::from_str(answer.trim_start_matches("data:").trim()).unwrap();
        assert_eq!(payload["code"], 0);
        assert_eq!(payload["message"], "");
        assert_eq!(payload["data"]["answer"], "hello");
        assert!(payload["data"]["reference"]["chunks"].is_array());

        // The failure frame keeps the error in both places, as upstream does.
        let failure = error_frame("boom");
        let payload: Value =
            serde_json::from_str(failure.trim_start_matches("data:").trim()).unwrap();
        assert_eq!(payload["code"], 500);
        assert_eq!(payload["message"], "boom");
        assert_eq!(payload["data"]["answer"], "**ERROR**: boom");

        // And the stream ends with the boolean the client waits for.
        let done: Value =
            serde_json::from_str(done_frame().trim_start_matches("data:").trim()).unwrap();
        assert_eq!(done["data"], true);
    }

    #[test]
    fn the_prompt_carries_the_question_and_the_retrieved_material() {
        let chunks = vec![
            json!({"content_with_weight": "Rust ships cargo.", "docnm_kwd": "rust.md"}),
            json!({"content": "Cargo builds projects."}),
            json!({"content_with_weight": ""}),
        ];
        let prompt = build_prompt("What ships rust?", &chunks);
        assert!(prompt.contains("What ships rust?"), "{prompt}");
        assert!(prompt.contains("Rust ships cargo."), "{prompt}");
        assert!(prompt.contains("【rust.md】"), "{prompt}");
        // A chunk without a name is still used, under a neutral label.
        assert!(prompt.contains("Cargo builds projects."), "{prompt}");

        // Nothing retrieved means the question goes to the model on its own rather than an empty prompt.
        assert_eq!(build_prompt("only a question", &[]), "only a question");
    }
}
