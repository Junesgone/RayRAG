//! Enhanced LLM module — chat model abstraction, provider-specific configs,
//! retry with exponential backoff, streaming stub.
//! Replaces RAGFlow's `rag/llm/chat_model.py` class hierarchy.

use crate::Result;
use serde::{Deserialize, Serialize};

/// Chat model provider type (matching RAGFlow's provider classes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ChatProvider {
    OpenAI,
    MiniMax,
    Qwen,
    LocalAI,
    HuggingFace,
    Custom(String),
}

impl ChatProvider {
    pub fn default_api_base(&self) -> &str {
        match self {
            Self::OpenAI => "https://api.openai.com/v1",
            Self::MiniMax => "https://api.minimaxi.com/v1",
            Self::Qwen => "http://127.0.0.1:8088/v1",
            Self::LocalAI => "http://127.0.0.1:8080/v1",
            Self::HuggingFace => "https://api-inference.huggingface.co/models",
            Self::Custom(_) => "",
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::OpenAI => "OpenAI",
            Self::MiniMax => "MiniMax",
            Self::Qwen => "Qwen",
            Self::LocalAI => "LocalAI",
            Self::HuggingFace => "HuggingFace",
            Self::Custom(s) => s.as_str(),
        }
    }

    pub fn from_name(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "openai" => Self::OpenAI,
            "minimax" => Self::MiniMax,
            "qwen" => Self::Qwen,
            "localai" | "local" => Self::LocalAI,
            "huggingface" | "hf" => Self::HuggingFace,
            other => Self::Custom(other.to_string()),
        }
    }
}

/// Chat model configuration (per-provider).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatModelConfig {
    pub provider: ChatProvider,
    pub model_name: String,
    pub api_base: String,
    pub api_key: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub enabled: bool,
    /// Retry config
    pub max_retries: u32,
    pub retry_delay_ms: u64,
}

impl Default for ChatModelConfig {
    fn default() -> Self {
        Self {
            provider: ChatProvider::OpenAI,
            model_name: "gpt-4".into(),
            api_base: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            max_tokens: 2048,
            temperature: 0.7,
            enabled: true,
            max_retries: 3,
            retry_delay_ms: 1000,
        }
    }
}

/// Model capability flags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub supports_chat: bool,
    pub supports_embedding: bool,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub supports_streaming: bool,
    pub max_context_tokens: u32,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            supports_chat: true,
            supports_embedding: false,
            supports_vision: false,
            supports_tools: false,
            supports_streaming: false,
            max_context_tokens: 8192,
        }
    }
}

/// Model metadata registry (matching RAGFlow's model_meta.py).
pub struct ModelRegistry;

impl ModelRegistry {
    /// Get capabilities for known models.
    pub fn capabilities(model: &str) -> ModelCapabilities {
        let lower = model.to_lowercase();
        if lower.contains("gpt-4") {
            ModelCapabilities {
                supports_embedding: true,
                supports_vision: true,
                supports_tools: true,
                supports_streaming: true,
                max_context_tokens: 128000,
                ..Default::default()
            }
        } else if lower.contains("gpt-3.5") {
            ModelCapabilities {
                max_context_tokens: 16385,
                ..Default::default()
            }
        } else if lower.contains("minimax") {
            ModelCapabilities {
                max_context_tokens: 65536,
                ..Default::default()
            }
        } else if lower.contains("qwen") {
            ModelCapabilities {
                max_context_tokens: if lower.contains("72b") || lower.contains("32b") {
                    32768
                } else {
                    8192
                },
                ..Default::default()
            }
        } else if lower.contains("claude") {
            ModelCapabilities {
                supports_vision: true,
                supports_tools: true,
                supports_streaming: true,
                max_context_tokens: 200000,
                ..Default::default()
            }
        } else {
            ModelCapabilities::default()
        }
    }

    /// List known models by provider.
    pub fn list_by_provider(provider: &ChatProvider) -> Vec<(&'static str, &'static str)> {
        match provider {
            ChatProvider::OpenAI => vec![
                ("gpt-4", "GPT-4"),
                ("gpt-4-turbo", "GPT-4 Turbo"),
                ("gpt-3.5-turbo", "GPT-3.5 Turbo"),
            ],
            ChatProvider::MiniMax => {
                vec![("MiniMax-M3", "MiniMax M3"), ("MiniMax-M2", "MiniMax M2")]
            }
            ChatProvider::Qwen => vec![
                ("Qwen3.5-9B", "Qwen 3.5 9B"),
                ("Qwen3-Embedding-4B", "Qwen3 Embedding"),
            ],
            _ => vec![("default", "Default")],
        }
    }
}

/// Retry wrapper with exponential backoff.
pub async fn retry_with_backoff<F, Fut, T>(max_retries: u32, base_delay_ms: u64, f: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut delay = base_delay_ms;
    let mut last_err = String::new();

    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                last_err = format!("{}", e);
                if attempt < max_retries {
                    tracing::warn!(
                        "Attempt {}/{} failed: {}. Retrying in {}ms...",
                        attempt + 1,
                        max_retries + 1,
                        last_err,
                        delay
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    delay *= 2;
                }
            }
        }
    }

    anyhow::bail!(
        "All {} attempts failed. Last error: {}",
        max_retries + 1,
        last_err
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_names() {
        assert_eq!(ChatProvider::MiniMax.name(), "MiniMax");
        assert_eq!(ChatProvider::from_name("qwen"), ChatProvider::Qwen);
        assert_eq!(
            ChatProvider::from_name("unknown"),
            ChatProvider::Custom("unknown".into())
        );
    }

    #[test]
    fn test_model_capabilities() {
        let cap = ModelRegistry::capabilities("gpt-4-turbo");
        assert!(cap.supports_vision);
        assert!(cap.supports_tools);
        assert_eq!(cap.max_context_tokens, 128000);
    }

    #[test]
    fn test_model_list() {
        let models = ModelRegistry::list_by_provider(&ChatProvider::OpenAI);
        assert!(models.len() >= 2);
    }

    #[tokio::test]
    async fn test_retry_success() {
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = counter.clone();
        let result = retry_with_backoff(2, 10, move || {
            let c = c.clone();
            async move {
                let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                if n < 3 {
                    anyhow::bail!("fail")
                } else {
                    Ok("ok")
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);
    }
}

// ── Streaming Support ───────────────────────────────────────────

/// SSE event for streaming chat.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StreamEvent {
    /// Event type: "delta", "done", "error"
    pub event: String,
    /// Content delta (for "delta" events)
    pub content: String,
    /// Full answer (for "done" events)
    pub answer: Option<String>,
    /// Error message (for "error" events)
    pub error: Option<String>,
}

/// Streaming chat client — SSE (Server-Sent Events) over HTTP.
pub struct StreamingLlmClient {
    pub api_base: String,
    pub api_key: String,
    pub model: String,
    client: reqwest::Client,
}

impl StreamingLlmClient {
    pub fn new(api_base: &str, api_key: &str, model: &str) -> Self {
        Self {
            api_base: api_base.trim_end_matches('/').into(),
            api_key: api_key.into(),
            model: model.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Stream chat completion as SSE events via a callback channel.
    pub async fn stream_chat<F>(
        &self,
        messages: &[crate::llm::ChatMessage],
        mut on_event: F,
    ) -> crate::Result<String>
    where
        F: FnMut(StreamEvent),
    {
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
            "max_tokens": 2048,
            "temperature": 0.7,
        });

        let resp = self
            .client
            .post(format!("{}/chat/completions", self.api_base))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Stream API error: {}", text);
        }

        let mut full_answer = String::new();
        let mut stream = resp.bytes_stream();

        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(e) => {
                    on_event(StreamEvent {
                        event: "error".into(),
                        content: String::new(),
                        answer: None,
                        error: Some(format!("{}", e)),
                    });
                    break;
                }
            };

            let text = String::from_utf8_lossy(&chunk);
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line == "data: [DONE]" {
                    continue;
                }
                if let Some(data) = line.strip_prefix("data: ")
                    && let Ok(json) = serde_json::from_str::<serde_json::Value>(data)
                    && let Some(delta) = json["choices"][0]["delta"]["content"].as_str()
                {
                    full_answer.push_str(delta);
                    on_event(StreamEvent {
                        event: "delta".into(),
                        content: delta.to_string(),
                        answer: None,
                        error: None,
                    });
                }
            }
        }

        on_event(StreamEvent {
            event: "done".into(),
            content: String::new(),
            answer: Some(full_answer.clone()),
            error: None,
        });

        Ok(full_answer)
    }
}
