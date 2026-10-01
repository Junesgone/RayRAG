//! Harness: Agentic RAG orchestration layer — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/`.
//!
//! Mirrors the upstream package layout module-by-module: the shared chunk
//! accessors (`chunk_utils`), the thinking-mode authority (`config`), the
//! keyword extractor (`keywords`), the usage instrumentation (`stats`), the
//! prompt surfaces (`report_prompt`, `structure_qa`), the think-log bridge
//! (`think_log`) and the orchestrator/tool namespaces.

use async_trait::async_trait;
use serde_json::{Value, json};

pub mod action_session;
pub mod arithmetic;
pub mod chunk_utils;
pub mod config;
pub mod grep_sed_narrow;
pub mod keywords;
pub mod memory;
pub mod orchestrator;
pub mod report_prompt;
pub mod stats;
pub mod structure_qa;
pub mod think_log;
pub mod tools;

/// Minimal chat-model surface the harness drives — the subset of RAGFlow's
/// `LLMBundle` the ported modules call (`async_chat` + `max_length` +
/// provider usage reporting).
#[async_trait]
pub trait HarnessChat: Send + Sync {
    /// `chat_mdl.async_chat(system, history, gen_conf)`.
    async fn chat(
        &self,
        system: &str,
        history: &[Value],
        gen_conf: &Value,
    ) -> Result<String, String>;

    /// `chat_mdl.async_chat_streamly_delta`: streaming answer deltas. The
    /// default bridges to `chat` and yields the whole answer as ONE delta.
    async fn chat_streamly_delta(
        &self,
        system: &str,
        history: &[Value],
        gen_conf: &Value,
    ) -> Result<Vec<String>, String> {
        self.chat(system, history, gen_conf)
            .await
            .map(|text| vec![text])
    }

    /// Provider context budget (`chat_mdl.max_length`).
    fn max_length(&self) -> usize;

    /// Provider-reported usage for the last call (`mdl.last_usage`); `None`
    /// when the provider does not report one.
    async fn last_usage(&self) -> Option<Value> {
        None
    }
}

/// `rag.prompts.generator.form_message`: build the `[system, user]` message
/// pair for a one-shot call.
pub fn form_message(system: &str, query: &str) -> Vec<Value> {
    vec![
        json!({"role": "system", "content": system}),
        json!({"role": "user", "content": query}),
    ]
}

/// `rag.prompts.generator.message_fit_in`: keep the messages within the model
/// context. Returns `(total_tokens, messages)`; the user content is truncated
/// from the tail while the pair exceeds `max_length` (upstream searches for a
/// fitting prefix; the observable contract — messages fit the budget — is the
/// same).
pub fn message_fit_in(mut messages: Vec<Value>, max_length: usize) -> (usize, Vec<Value>) {
    let mut total: usize = messages
        .iter()
        .map(|message| {
            message
                .get("content")
                .and_then(Value::as_str)
                .map(crate::chunk::tokenizer::token_count)
                .unwrap_or(0)
        })
        .sum();
    while total > max_length && messages.len() > 1 {
        let last = messages.len() - 1;
        let Some(content) = messages[last]
            .get("content")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            break;
        };
        let mut chars: Vec<char> = content.chars().collect();
        if chars.is_empty() {
            break;
        }
        let keep = (chars.len() * 3) / 4;
        chars.truncate(keep.max(1));
        let truncated: String = chars.into_iter().collect();
        messages[last]["content"] = Value::String(truncated);
        total = messages
            .iter()
            .map(|message| {
                message
                    .get("content")
                    .and_then(Value::as_str)
                    .map(crate::chunk::tokenizer::token_count)
                    .unwrap_or(0)
            })
            .sum();
    }
    (total, messages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_and_fit_messages() {
        let messages = form_message("sys", "hello world");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], json!("system"));
        assert_eq!(messages[1]["content"], json!("hello world"));

        let long = "word ".repeat(400);
        let (fitted_tokens, fitted) = message_fit_in(form_message("sys", &long), 50);
        assert!(fitted_tokens <= 50);
        assert!(fitted[1]["content"].as_str().unwrap().len() < long.len());
    }
}
