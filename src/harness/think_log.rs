//! Surface selected internal INFO logs to the client as `<think>` content —
//! RAGFlow v0.27.2 `rag/advanced_rag/think_log.py`.
//!
//! During an agentic `rag_agent` turn the pipeline's bracket-tagged progress
//! logs (`[Agentic RAG]`, `[Formalization]`, `[Hybrid search]`, `[Tool loop]`, …)
//! are streamed to the front end as reasoning without instrumenting every call
//! site. Upstream keeps the per-request sink in a `ContextVar`; RayRAG carries
//! the same sink on its request context, and this module owns the *decision*
//! layer (which records surface, and how they are framed).

use std::sync::{Arc, Mutex};

/// Only bracket-tagged INFO lines from these logger namespaces are surfaced.
pub const SCOPED_PREFIXES: [&str; 3] = [
    "rag.advanced_rag",
    "rag.llm.chat_model",
    "rag.llm.tool_decorator",
];

/// `ThinkLogHandler` filter: in-scope logger + bracket-tagged non-empty line.
pub fn is_think_log(logger_name: &str, message: &str) -> bool {
    if !SCOPED_PREFIXES
        .iter()
        .any(|prefix| logger_name.starts_with(prefix))
    {
        return false;
    }
    if message.is_empty() {
        return false;
    }
    message.trim_start().starts_with('[')
}

/// The exact framing upstream forwards: `<br>` + the trimmed line.
pub fn format_think_line(message: &str) -> String {
    format!("<br>{}", message.trim())
}

/// Per-request forwarding sink (`_think_log_sink`).
#[derive(Clone, Default)]
pub struct ThinkLogSink {
    inner: Arc<Mutex<Option<Arc<dyn Fn(String) + Send + Sync>>>>,
}

impl ThinkLogSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// `set_think_log_sink`: activate the sink; returns the previous one.
    pub fn set(
        &self,
        sink: Option<Arc<dyn Fn(String) + Send + Sync>>,
    ) -> Option<Arc<dyn Fn(String) + Send + Sync>> {
        std::mem::replace(&mut *self.inner.lock().unwrap(), sink)
    }

    /// `reset_think_log_sink`: clear the sink.
    pub fn reset(&self) -> Option<Arc<dyn Fn(String) + Send + Sync>> {
        self.inner.lock().unwrap().take()
    }

    /// `ThinkLogHandler.emit`: forward one record when it qualifies. Returns
    /// whether the line was forwarded.
    pub fn forward(&self, logger_name: &str, message: &str) -> bool {
        if !is_think_log(logger_name, message) {
            return false;
        }
        let sink = self.inner.lock().unwrap().clone();
        match sink {
            Some(sink) => {
                sink(format_think_line(message));
                true
            }
            None => false,
        }
    }
}


