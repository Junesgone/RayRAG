//! LLM integration — RAG-enhanced chat with OpenAI-compatible APIs.
//!
//! Replaces RAGFlow's `rag/llm/` and `api/db/services/conversation_service.py`.
//! Supports:
//! - RAG chat (retrieve → build prompt → LLM answer)
//! - Multi-turn conversations (JSON persistence)
//! - Multiple LLM backends (MiniMax, OpenAI, local Qwen, etc.)

use crate::Result;
use crate::generation_params::{GenerationParams, GenerationParamsPatch};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

/// A single message in a conversation (OpenAI format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    #[serde(default = "new_message_id")]
    pub id: String,
    pub role: String, // "system", "user", "assistant"
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub references: Vec<ChunkReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbup: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
    /// Provider-reported usage for the completion that produced this message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default = "now_ms")]
    pub created_at: u64,
}

/// Token accounting returned by an upstream chat provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

impl TokenUsage {
    pub(crate) fn normalized(self) -> Option<Self> {
        let inferred_total = self.prompt_tokens.saturating_add(self.completion_tokens);
        let total_tokens = if self.total_tokens == 0 {
            inferred_total
        } else {
            self.total_tokens
        };
        (total_tokens > 0).then_some(Self {
            total_tokens,
            ..self
        })
    }
}

/// Content plus optional exact provider token usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatCompletion {
    pub content: String,
    pub usage: Option<TokenUsage>,
}

/// OpenAI-compatible function payload embedded in an assistant tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolFunctionCall {
    pub name: String,
    pub arguments: String,
}

/// A single model-requested function call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunctionCall,
}

/// The richer message envelope required by the OpenAI tool-calling protocol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolChatMessage {
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ToolChatMessage {
    pub fn from_chat(message: &ChatMessage) -> Self {
        Self {
            role: message.role.clone(),
            content: Some(message.content.clone()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".into(),
            content: None,
            tool_calls,
            tool_call_id: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
}

/// Assistant text, requested function calls and exact per-request usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolChatCompletion {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkReference {
    /// The document this chunk came from, when the caller knows it. The guide's reference objects
    /// carry it (`document_id`, `document_name`), and `doc_aggs` is aggregated from the names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_name: Option<String>,

    #[serde(alias = "chunk_id")]
    pub id: String,
    #[serde(alias = "dataset_id")]
    pub kb_id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub similarity: Option<f32>,
    #[serde(default)]
    pub vector_similarity: Option<f32>,
    #[serde(default)]
    pub term_similarity: Option<f32>,
    /// The metadata of the document this chunk belongs to, when the caller asked for it.
    /// `extra_body.reference_metadata` in the OpenAI-compatible API is what populates it, so it is
    /// absent unless requested rather than always carried around.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_metadata: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone)]
pub struct MessageFeedbackTarget {
    pub prior_thumb: Option<bool>,
    pub prior_feedback: Option<String>,
    pub references: Vec<ChunkReference>,
}

#[derive(Debug, Clone)]
pub struct RegenerationTarget {
    pub question: String,
    pub prior_answer: String,
    pub history: Vec<ChatMessage>,
    pub kb_ids: Vec<String>,
    pub chat_model: Option<String>,
    pub embedding_model: Option<String>,
}

pub struct AssistantMessageReplacement {
    pub expected_prior_answer: String,
    pub answer: String,
    pub citations: Vec<String>,
    pub references: Vec<ChunkReference>,
    pub kb_ids: Vec<String>,
    pub chat_model: Option<String>,
    pub embedding_model: Option<String>,
    pub usage: Option<TokenUsage>,
}

pub struct ConversationExchange {
    pub question: String,
    pub answer: String,
    pub citations: Vec<String>,
    pub references: Vec<ChunkReference>,
    pub settings: Option<(Vec<String>, Option<String>, Option<String>)>,
    pub duration_ms: u64,
    pub usage: Option<TokenUsage>,
}

impl ChatMessage {
    pub fn new(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: new_message_id(),
            role: role.into(),
            content: content.into(),
            citations: None,
            references: Vec::new(),
            thumbup: None,
            feedback: None,
            usage: None,
            created_at: now_ms(),
        }
    }
}

/// A conversation (multi-turn chat session).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub owner_id: String,
    /// Tenant that owns this session. Legacy records are migrated to owner_id.
    #[serde(default)]
    pub tenant_id: String,
    /// Session origin. Legacy records predate this field and are normal chats.
    #[serde(default = "default_conversation_source")]
    pub source: String,
    /// Agent/canvas identifier for agent-originated sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas_id: Option<String>,
    /// Chat app this conversation belongs to (RAGFlow 0.26.4 chat-app model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default)]
    pub kb_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    pub messages: Vec<ChatMessage>,
    /// Cumulative generation time for completed assistant rounds.
    #[serde(default)]
    pub duration_ms: u64,
    /// Canvas DSL snapshot: captured when an agent session is created and
    /// refreshed after every agent run (RAGFlow `API4Conversation.dsl`,
    /// written by `conv.dsl = str(canvas)` in `Completion.save`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dsl: Option<serde_json::Value>,
    /// Error raised by the latest agent run (RAGFlow
    /// `API4Conversation.errors` / `conv.errors = canvas.error`). `None` means
    /// the last run finished cleanly, which is the log page's green status dot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<String>,
    /// Canvas version title captured at session creation (RAGFlow
    /// `API4Conversation.version_title`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_title: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

fn default_conversation_source() -> String {
    "chat".into()
}

/// Everything `create_agent_session` needs to persist a new agent session.
/// Borrowed so the handler can pass request strings straight through; `dsl` and
/// `version_title` are owned because both come from other stores.
pub struct NewAgentSession<'a> {
    pub owner_id: &'a str,
    pub tenant_id: &'a str,
    pub canvas_id: &'a str,
    pub name: &'a str,
    pub kb_ids: Vec<String>,
    pub prologue: &'a str,
    pub dsl: Option<serde_json::Value>,
    pub version_title: Option<String>,
}

/// LLM client configuration.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    /// API base URL (OpenAI-compatible)
    pub api_base: String,
    /// API key
    pub api_key: String,
    /// Model name
    pub model: String,
    /// Generation defaults overridden by supported request fields.
    pub generation: GenerationParams,
    /// System prompt prefix
    pub system_prompt: String,
    /// DeepSeek application-API switches (deepseek-flash only).
    pub deepseek: DeepSeekOptions,
}

/// DeepSeek's built-in application tools and safety switch.
///
/// deepseek-flash (V4.1-Flash) accepts server-side tools in the ordinary
/// Chat Completions request: deepseek_web_search runs web/image search,
/// deepseek_read_url reads links in the user message, and safety_check
/// enables input/output moderation. They are service-side, so the client
/// declares them but never executes a tool round itself.
///
/// All values are opt-in environment switches; None means "omit the field".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeepSeekOptions {
    pub web_search: bool,
    pub read_url: bool,
    pub safety_check: Option<String>,
    pub thinking: Option<String>,
}

impl DeepSeekOptions {
    /// Read the DeepSeek switches from the process environment.
    ///
    /// DEEPSEEK_WEB_SEARCH / DEEPSEEK_READ_URL accept the usual truthy
    /// spellings; DEEPSEEK_SAFETY_CHECK accepts low/high; and
    /// DEEPSEEK_THINKING accepts enabled/disabled. Unknown values are
    /// ignored rather than forwarded (DeepSeek answers 400 for an invalid
    /// safety_check.level).
    pub fn from_env() -> Self {
        fn flag(name: &str) -> bool {
            std::env::var(name)
                .map(|value| {
                    matches!(
                        value.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
                .unwrap_or(false)
        }
        fn choice(name: &str, allowed: &[&str]) -> Option<String> {
            std::env::var(name)
                .ok()
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| allowed.contains(&value.as_str()))
        }
        Self {
            web_search: flag("DEEPSEEK_WEB_SEARCH"),
            read_url: flag("DEEPSEEK_READ_URL"),
            safety_check: choice("DEEPSEEK_SAFETY_CHECK", &["low", "high"]),
            thinking: choice("DEEPSEEK_THINKING", &["enabled", "disabled"]),
        }
    }
}

/// True for DeepSeek's OpenAI-compatible endpoints.
fn is_deepseek_endpoint(api_base: &str) -> bool {
    api_base.to_ascii_lowercase().contains("api.deepseek.com")
}

/// The application API (server tools, safety, thinking) is documented for
/// deepseek-flash; the V4 aliases are the same family. The legacy
/// deepseek-chat / deepseek-reasoner names do not accept these fields.
fn is_deepseek_app_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    model == "deepseek-flash"
        || model.starts_with("deepseek-v4")
        || model.starts_with("deepseek-flash-")
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.minimaxi.com/v1".into(),
            api_key: String::new(),
            model: "MiniMax-M3".into(),
            generation: GenerationParams::default(),
            system_prompt:
                "You are a helpful AI assistant. Answer questions based on the provided context."
                    .into(),
            deepseek: DeepSeekOptions::default(),
        }
    }
}

/// Provider protocol used by the chat client. Most providers are
/// OpenAI-compatible; Anthropic's Messages API has a different endpoint,
/// authentication header, request body and SSE event shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatProtocol {
    OpenAi,
    Anthropic,
}

fn is_anthropic_endpoint(api_base: &str) -> bool {
    let base = api_base.trim_end_matches('/').to_ascii_lowercase();
    base.contains("api.anthropic.com")
        || base.ends_with("/v1/messages")
        || base.ends_with("/messages")
        // DeepSeek exposes an Anthropic-compatible base at
        // https://api.deepseek.com/anthropic (its SDK appends /v1/messages).
        || base.ends_with("/anthropic")
        || base.contains("api.deepseek.com/anthropic")
}

fn anthropic_endpoint(api_base: &str) -> String {
    let base = api_base.trim_end_matches('/');
    if base.ends_with("/v1/messages") || base.ends_with("/messages") {
        return base.to_string();
    }
    if base.ends_with("/v1") {
        return format!("{base}/messages");
    }
    format!("{base}/v1/messages")
}

/// Split system messages out and coalesce consecutive same-role messages;
/// Anthropic requires the system prompt in its own top-level field and rejects
/// some adjacent role patterns.
fn anthropic_messages(messages: &[ChatMessage]) -> (Option<String>, Vec<serde_json::Value>) {
    let mut system_parts: Vec<String> = Vec::new();
    let mut output: Vec<(String, String)> = Vec::new();
    for message in messages {
        let role = message.role.trim().to_ascii_lowercase();
        let content = message.content.trim();
        if content.is_empty() {
            continue;
        }
        if role == "system" || role == "developer" {
            system_parts.push(content.to_string());
            continue;
        }
        let role = if role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        if let Some((last_role, last_content)) = output.last_mut()
            && last_role == role
        {
            last_content.push_str("\n\n");
            last_content.push_str(content);
            continue;
        }
        output.push((role.to_string(), content.to_string()));
    }
    let messages = output
        .into_iter()
        .map(|(role, content)| serde_json::json!({"role": role, "content": content}))
        .collect();
    let system = (!system_parts.is_empty()).then(|| system_parts.join("\n\n"));
    (system, messages)
}

fn build_anthropic_request_body(
    config: &LlmConfig,
    messages: &[ChatMessage],
    generation: GenerationParams,
    stream: bool,
) -> serde_json::Value {
    let (system, messages) = anthropic_messages(messages);
    let mut body = serde_json::json!({
        "model": config.model,
        "max_tokens": generation.max_tokens,
        "messages": messages,
        "temperature": generation.temperature,
        "top_p": generation.top_p,
    });
    if let Some(system) = system {
        body["system"] = serde_json::Value::String(system);
    }
    if stream {
        body["stream"] = serde_json::Value::Bool(true);
    }
    body
}

fn parse_anthropic_chat_completion(json: &serde_json::Value) -> Result<ChatCompletion> {
    let content = json["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"].as_str() == Some("text"))
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    let content = if content.is_empty() {
        "(empty response)".to_string()
    } else {
        content
    };
    let usage = json.get("usage").and_then(|usage| {
        TokenUsage {
            prompt_tokens: usage
                .get("input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            completion_tokens: usage
                .get("output_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            total_tokens: 0,
        }
        .normalized()
    });
    Ok(ChatCompletion { content, usage })
}

/// LLM client for OpenAI-compatible APIs.
#[derive(Clone)]
pub struct LlmClient {
    config: LlmConfig,
    client: reqwest::Client,
}

impl LlmClient {
    /// Create a new LLM client.
    ///
    /// The HTTP budget follows the endpoint: a local OpenAI-API-Compatible model
    /// (loopback / LAN / single-label host names) gets no total timeout because slow
    /// local GPUs are normal, while remote providers keep RAYRAG_MODEL_TIMEOUT
    /// instead of the two-hour command budget.
    pub fn new(config: LlmConfig) -> Self {
        let mut config = config;
        // DeepSeek's application-API switches are process-wide environment
        // settings, like the rest of the OpenAI-compatible endpoint config.
        // Only fill them in when the caller did not set them explicitly.
        if is_deepseek_endpoint(&config.api_base) && config.deepseek == DeepSeekOptions::default() {
            config.deepseek = DeepSeekOptions::from_env();
        }
        let client = crate::common::cmd_timeout::model_client_for(&config.api_base);
        Self { config, client }
    }

    fn chat_protocol(&self) -> ChatProtocol {
        if is_anthropic_endpoint(&self.config.api_base) {
            ChatProtocol::Anthropic
        } else {
            ChatProtocol::OpenAi
        }
    }

    fn anthropic_request(&self, body: &serde_json::Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(anthropic_endpoint(&self.config.api_base))
            .header("anthropic-version", "2023-06-01")
            .json(body);
        if !self.config.api_key.is_empty() {
            request = request.header("x-api-key", &self.config.api_key);
        }
        request
    }

    async fn anthropic_chat_completion(
        &self,
        messages: &[ChatMessage],
        generation: GenerationParams,
    ) -> Result<ChatCompletion> {
        let body = build_anthropic_request_body(&self.config, messages, generation, false);
        let response = self
            .anthropic_request(&body)
            .send()
            .await
            .map_err(|error| {
                anyhow::anyhow!("Anthropic API request failed: {}", error.without_url())
            })?;
        let status = response.status();
        let json: serde_json::Value = crate::common::cmd_timeout::read_json_limited(
            response,
            crate::common::cmd_timeout::body_limit_bytes(),
            "Anthropic API",
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!("Anthropic API response failed (HTTP {status}): {error}")
        })?;
        if !status.is_success() {
            let message = json
                .pointer("/error/message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Unknown error");
            anyhow::bail!("Anthropic API error ({status}): {message}");
        }
        parse_anthropic_chat_completion(&json)
    }

    async fn anthropic_chat_stream<F: FnMut(&str) + ?Sized>(
        &self,
        messages: &[ChatMessage],
        generation: GenerationParams,
        on_chunk: &mut F,
    ) -> Result<String> {
        use futures_util::StreamExt;
        let body = build_anthropic_request_body(&self.config, messages, generation, true);
        let response = self
            .anthropic_request(&body)
            .send()
            .await
            .map_err(|error| {
                anyhow::anyhow!("Anthropic stream request failed: {}", error.without_url())
            })?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Anthropic stream error ({status}): {text}");
        }
        let mut full = String::new();
        let mut buffer = String::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(newline) = buffer.find('\n') {
                let line: String = buffer.drain(..=newline).collect();
                let line = line.trim();
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                    continue;
                };
                if value["type"].as_str() == Some("error") {
                    let message = value
                        .pointer("/error/message")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("stream error");
                    anyhow::bail!("Anthropic stream error: {message}");
                }
                if value["type"].as_str() == Some("content_block_delta")
                    && let Some(delta) = value
                        .pointer("/delta/text")
                        .and_then(serde_json::Value::as_str)
                {
                    full.push_str(delta);
                    on_chunk(delta);
                }
            }
        }
        if full.is_empty() {
            anyhow::bail!("Anthropic stream returned no text");
        }
        Ok(full)
    }

    /// Simple chat completion (single turn), preserving provider usage when present.
    pub async fn chat_completion(&self, messages: &[ChatMessage]) -> Result<ChatCompletion> {
        self.chat_completion_with_generation(messages, GenerationParamsPatch::default())
            .await
    }

    pub async fn chat_completion_with_generation(
        &self,
        messages: &[ChatMessage],
        patch: GenerationParamsPatch,
    ) -> Result<ChatCompletion> {
        let generation = self.config.generation.merged(patch);
        if self.chat_protocol() == ChatProtocol::Anthropic {
            return self.anthropic_chat_completion(messages, generation).await;
        }
        let public_messages: Vec<_> = messages
            .iter()
            .map(|message| {
                serde_json::json!({
                    "role": message.role,
                    "content": message.content,
                })
            })
            .collect();
        let body = build_chat_request_body(&self.config, public_messages, generation);

        let json = self.send_chat_request(&body).await?;
        parse_openai_chat_completion(&json)
    }

    /// Run one OpenAI-compatible tool-calling round.
    ///
    /// Passing an empty tool list deliberately omits `tools` and `tool_choice`;
    /// Agent uses that shape for the fixed max-round fallback request while
    /// preserving prior assistant/tool messages.
    /// 流式聊天（OpenAI 兼容 stream=true）。逐块回调 `on_chunk`，返回完整文本。
    pub async fn chat_stream<F>(
        &self,
        messages: &[ChatMessage],
        patch: crate::generation_params::GenerationParamsPatch,
        on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let mut on_chunk = on_chunk;
        self.chat_stream_impl(messages, patch, &mut on_chunk).await
    }

    /// Non-generic SSE loop shared by [`Self::chat_stream`] and the
    /// [`ChatModel`] trait implementation (which receives a boxed closure).
    async fn chat_stream_impl<F: FnMut(&str) + ?Sized>(
        &self,
        messages: &[ChatMessage],
        patch: crate::generation_params::GenerationParamsPatch,
        on_chunk: &mut F,
    ) -> Result<String> {
        let generation = self.config.generation.merged(patch);
        if self.chat_protocol() == ChatProtocol::Anthropic {
            return self
                .anthropic_chat_stream(messages, generation, on_chunk)
                .await;
        }
        let public_messages = messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect::<Vec<_>>();
        let mut body = build_chat_request_body(&self.config, public_messages, generation);
        body["stream"] = serde_json::json!(true);
        let mut request = self
            .client
            .post(format!("{}/chat/completions", self.config.api_base))
            .json(&body);
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(&self.config.api_key);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM stream error ({}): {}", status, text);
        }
        let mut full = String::new();
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt;
        let mut buffer = String::new();
        let mut filtered = false;
        let mut refusal = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));
            // 逐行解析 SSE
            while let Some(newline) = buffer.find('\n') {
                let line: String = buffer.drain(..=newline).collect();
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(data) = line.strip_prefix("data:") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        break;
                    }
                    match serde_json::from_str::<serde_json::Value>(data) {
                        Ok(value) => {
                            // An in-stream error frame is part of the OpenAI SSE contract;
                            // skipping it would turn a provider failure into a silently
                            // truncated answer.
                            if let Some(error) = value.get("error") {
                                tracing::warn!(%error, "Model stream returned an error frame");
                            }
                            if value
                                .pointer("/choices/0/finish_reason")
                                .and_then(serde_json::Value::as_str)
                                == Some("content_filter")
                            {
                                filtered = true;
                            }
                            if let Some(text) = value
                                .pointer("/choices/0/delta/refusal")
                                .and_then(serde_json::Value::as_str)
                            {
                                refusal.push_str(text);
                            }
                            if let Some(delta) = value
                                .pointer("/choices/0/delta/content")
                                .and_then(|content| content.as_str())
                            {
                                full.push_str(delta);
                                on_chunk(delta);
                            }
                        }
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                frame = %data.chars().take(200).collect::<String>(),
                                "Skipping an unparsable model stream frame"
                            );
                        }
                    }
                }
            }
        }
        if filtered || !refusal.is_empty() {
            let text = if refusal.is_empty() {
                CONTENT_REFUSAL_MESSAGE.to_string()
            } else {
                format!("{CONTENT_REFUSAL_MESSAGE} {refusal}")
            };
            if full.is_empty() {
                full.push_str(&text);
                on_chunk(&text);
            } else {
                full.push('\n');
                full.push_str(&text);
                on_chunk(&format!("\n{text}"));
            }
        }
        Ok(full)
    }

    pub async fn tool_chat_completion_with_generation(
        &self,
        messages: &[ToolChatMessage],
        tools: &[serde_json::Value],
        patch: GenerationParamsPatch,
    ) -> Result<ToolChatCompletion> {
        let generation = self.config.generation.merged(patch);
        anyhow::ensure!(
            self.chat_protocol() != ChatProtocol::Anthropic,
            "Native Anthropic tool calling is not supported by this build yet; configure an OpenAI-compatible gateway for tool-using agents, or use Claude for plain chat"
        );
        let public_messages = messages
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut body = build_chat_request_body(&self.config, public_messages, generation);
        if !tools.is_empty() {
            let object = body
                .as_object_mut()
                .expect("the static chat request body is an object");
            // Keep any DeepSeek server tools the builder already declared; the
            // function tools an agent supplies must not erase them.
            let mut merged = match object.remove("tools") {
                Some(serde_json::Value::Array(existing)) => existing,
                _ => Vec::new(),
            };
            merged.extend(tools.iter().cloned());
            object.insert("tools".into(), serde_json::Value::Array(merged));
            object.insert(
                "tool_choice".into(),
                serde_json::Value::String("auto".into()),
            );
        }
        let json = self.send_chat_request(&body).await?;
        parse_openai_tool_chat_completion(&json)
    }

    async fn send_chat_request(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        let resp = self
            .client
            .post(format!("{}/chat/completions", self.config.api_base))
            .bearer_auth(&self.config.api_key)
            .json(body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("LLM API error ({}): {}", status, text);
        }
        Ok(resp.json().await?)
    }

    /// Model name of this client (RAGFlow `Base.model_name`).
    pub fn model_name(&self) -> &str {
        &self.config.model
    }

    /// API base URL of this client.
    pub fn api_base(&self) -> &str {
        &self.config.api_base
    }

    /// Raw OpenAI-compatible chat request — used by vision (cv) models that
    /// need structured `content` parts (text + image_url).
    pub async fn chat_raw(&self, body: serde_json::Value) -> Result<serde_json::Value> {
        if self.chat_protocol() == ChatProtocol::Anthropic {
            anyhow::bail!(
                "Native Anthropic chat_raw requests are not supported; use the typed chat API or an OpenAI-compatible gateway"
            );
        }
        self.send_chat_request(&body).await
    }

    /// Raw multipart POST returning JSON — used by audio transcription
    /// (`/audio/transcriptions`).
    pub async fn raw_post_multipart(
        &self,
        url: &str,
        form: reqwest::multipart::Form,
    ) -> Result<serde_json::Value> {
        let mut request = self.client.post(url).multipart(form);
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(&self.config.api_key);
        }
        let resp = request.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("LLM API error ({}): {}", status, text);
        }
        Ok(resp.json().await?)
    }

    /// Raw JSON POST returning bytes — used by TTS (`/audio/speech`).
    pub async fn raw_post_bytes(&self, url: &str, body: &serde_json::Value) -> Result<Vec<u8>> {
        let mut request = self.client.post(url).json(body);
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(&self.config.api_key);
        }
        let resp = request.send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("LLM API error ({}): {}", status, text);
        }
        crate::common::cmd_timeout::read_body_limited(
            resp,
            crate::common::cmd_timeout::body_limit_bytes(),
            "LLM",
        )
        .await
    }

    /// Backward-compatible content-only adapter.
    /// `{model}@{provider}` style selector, for diagnostics.
    pub fn model_selector(&self) -> String {
        self.config.model.clone()
    }

    /// The context window the harness uses to trim history (`HarnessChat::max_length`).
    pub fn max_context_tokens(&self) -> usize {
        self.config.generation.max_tokens.max(1024) as usize
    }

    pub async fn chat(&self, messages: &[ChatMessage]) -> Result<String> {
        Ok(self.chat_completion(messages).await?.content)
    }

    /// RAG-enhanced chat, preserving provider usage when present.
    pub async fn rag_chat_completion(
        &self,
        question: &str,
        contexts: &[String],
        history: &[ChatMessage],
    ) -> Result<ChatCompletion> {
        // Build context block
        let context_text = if contexts.is_empty() {
            String::new()
        } else {
            let mut ctx = String::from("Relevant document context:\n\n");
            for (i, c) in contexts.iter().enumerate() {
                ctx.push_str(&format!("[{}] {}\n\n", i + 1, c));
            }
            ctx
        };

        // Build messages: system + history + context + question
        let mut messages: Vec<ChatMessage> = vec![ChatMessage::new(
            "system",
            format!(
                "{}\n\n{}If the context does not contain the answer, say 'I don't have enough information to answer this question.'.",
                self.config.system_prompt, context_text
            ),
        )];

        // Add conversation history (last 10 messages to stay within limits)
        let recent = if history.len() > 10 {
            &history[history.len() - 10..]
        } else {
            history
        };
        messages.extend(recent.iter().cloned());

        // Add current question
        messages.push(ChatMessage::new("user", question));

        self.chat_completion(&messages).await
    }

    pub async fn rag_chat_completion_with_generation(
        &self,
        question: &str,
        contexts: &[String],
        history: &[ChatMessage],
        patch: GenerationParamsPatch,
    ) -> Result<ChatCompletion> {
        let context_text = if contexts.is_empty() {
            String::new()
        } else {
            let mut context = String::from("Relevant document context:\n\n");
            for (index, value) in contexts.iter().enumerate() {
                context.push_str(&format!("[{}] {}\n\n", index + 1, value));
            }
            context
        };
        let mut messages = vec![ChatMessage::new(
            "system",
            format!(
                "{}\n\n{}If the context does not contain the answer, say 'I don't have enough information to answer this question.'.",
                self.config.system_prompt, context_text
            ),
        )];
        let recent = if history.len() > 10 {
            &history[history.len() - 10..]
        } else {
            history
        };
        messages.extend(recent.iter().cloned());
        messages.push(ChatMessage::new("user", question));
        self.chat_completion_with_generation(&messages, patch).await
    }

    /// RAG 流式版本：检索上下文 + 历史组装后以 SSE 逐块输出。
    pub async fn rag_chat_stream<F>(
        &self,
        question: &str,
        contexts: &[String],
        history: &[ChatMessage],
        patch: GenerationParamsPatch,
        on_chunk: F,
    ) -> Result<String>
    where
        F: FnMut(&str),
    {
        let context_text = if contexts.is_empty() {
            String::new()
        } else {
            let mut context = String::from("Relevant document context:\n\n");
            for (index, value) in contexts.iter().enumerate() {
                context.push_str(&format!("[{}] {}\n\n", index + 1, value));
            }
            context
        };
        let mut messages = vec![ChatMessage::new(
            "system",
            format!(
                "{}\n\n{}If the context does not contain the answer, say 'I don't have enough information to answer this question.'.",
                self.config.system_prompt, context_text
            ),
        )];
        let recent = if history.len() > 10 {
            &history[history.len() - 10..]
        } else {
            history
        };
        messages.extend(recent.iter().cloned());
        messages.push(ChatMessage::new("user", question));
        self.chat_stream(&messages, patch, on_chunk).await
    }

    /// Backward-compatible content-only RAG adapter.
    pub async fn rag_chat(
        &self,
        question: &str,
        contexts: &[String],
        history: &[ChatMessage],
    ) -> Result<String> {
        Ok(self
            .rag_chat_completion(question, contexts, history)
            .await?
            .content)
    }
}

fn build_chat_request_body(
    config: &LlmConfig,
    messages: Vec<serde_json::Value>,
    generation: GenerationParams,
) -> serde_json::Value {
    let model = config.model.as_str();
    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "max_tokens": generation.max_tokens,
        "temperature": generation.temperature,
        "top_p": generation.top_p,
        "frequency_penalty": generation.frequency_penalty,
        "presence_penalty": generation.presence_penalty,
    });
    // RAGFlow `_apply_model_family_policies`: the Qwen3 family disables
    // thinking via extra_body on non-stream chat requests so the raw
    // reasoning_content thinking chain is not emitted inline.
    //
    // llama.cpp (the local GPU backend) does not understand `extra_body`;
    // it honors `chat_template_kwargs.enable_thinking` instead. Sending both
    // forms keeps every endpoint working: DashScope/OpenAI-compatible APIs
    // merge extra_body into the request, llama.cpp applies the chat template
    // kwarg, and endpoints that ignore unknown fields simply keep thinking
    // on (content still arrives; only the chain differs).
    //
    // RAGFlow 0.26.4 prompt_config.reasoning toggle: when true, thinking is
    // explicitly enabled for the Qwen3 family; for any other model the
    // chat_template_kwargs form is emitted so llama.cpp-served reasoning
    // models (DeepSeek-R1 style) honor the switch too.
    if model.to_ascii_lowercase().contains("qwen3") {
        body["extra_body"] = serde_json::json!({"enable_thinking": generation.reasoning});
        body["chat_template_kwargs"] = serde_json::json!({"enable_thinking": generation.reasoning});
    } else if generation.reasoning {
        body["chat_template_kwargs"] = serde_json::json!({"enable_thinking": true});
    }
    apply_deepseek_options(&mut body, config, generation);
    body
}

/// Add DeepSeek's server tools and safety/thinking switches.
///
/// These fields only exist on the DeepSeek application models; forwarding them
/// to another provider (or the legacy DeepSeek names) would be an unknown-field
/// error. The tools are service-side: the model runs the search/fetch itself and
/// returns the answer (with citations in annotations/server_tool_calls), so no
/// client tool round follows.
fn apply_deepseek_options(
    body: &mut serde_json::Value,
    config: &LlmConfig,
    generation: GenerationParams,
) {
    if !is_deepseek_endpoint(&config.api_base) || !is_deepseek_app_model(&config.model) {
        return;
    }
    let options = &config.deepseek;
    let mut server_tools = Vec::new();
    if options.web_search {
        server_tools.push(serde_json::json!({"type": "deepseek_web_search"}));
    }
    if options.read_url {
        server_tools.push(serde_json::json!({"type": "deepseek_read_url"}));
    }
    if !server_tools.is_empty() {
        let object = body
            .as_object_mut()
            .expect("the static chat request body is an object");
        // tool_choice=none means "no tools at all", including URL reading.
        let tool_choice_is_none = object
            .get("tool_choice")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == "none");
        if !tool_choice_is_none {
            match object.get_mut("tools") {
                Some(serde_json::Value::Array(existing)) => existing.extend(server_tools),
                _ => {
                    object.insert("tools".into(), serde_json::Value::Array(server_tools));
                }
            }
        }
    }
    if let Some(level) = &options.safety_check {
        body["safety_check"] = serde_json::json!({"level": level});
    }
    // RAGFlow's reasoning toggle maps onto DeepSeek's thinking switch; an
    // explicit DEEPSEEK_THINKING value wins.
    let thinking = options.thinking.clone().unwrap_or_else(|| {
        if generation.reasoning {
            "enabled".to_string()
        } else {
            "disabled".to_string()
        }
    });
    body["thinking"] = serde_json::json!({"type": thinking});
}

/// Shown when DeepSeek's safety check refuses a request. The provider returns
/// an empty content string with finish_reason=content_filter, so without this
/// the UI would display "(empty response)" and the refusal would be silent.
const CONTENT_REFUSAL_MESSAGE: &str =
    "The model declined this request because it did not pass the provider's content safety check.";

fn finish_reason_is_content_filter(json: &serde_json::Value) -> bool {
    json.pointer("/choices/0/finish_reason")
        .and_then(serde_json::Value::as_str)
        == Some("content_filter")
}

fn message_refusal(json: &serde_json::Value) -> Option<String> {
    json.pointer("/choices/0/message/refusal")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_openai_chat_completion(json: &serde_json::Value) -> Result<ChatCompletion> {
    let mut content = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    if let Some(refusal) = message_refusal(json) {
        content = format!("{CONTENT_REFUSAL_MESSAGE} {refusal}");
    } else if finish_reason_is_content_filter(json) && content.is_empty() {
        content = CONTENT_REFUSAL_MESSAGE.to_string();
    }
    if content.is_empty() {
        content = "(empty response)".to_string();
    }
    // RAGFlow async_chat / _async_chat: reasoning_content (or its alias
    // `reasoning`) from thinking-capable models is surfaced as a
    // <think>...</think> prefix, matching Qwen3 / DeepSeek-R1 / Kimi-K2.5.
    if let Some(reasoning) = json["choices"][0]["message"]["reasoning_content"]
        .as_str()
        .or_else(|| json["choices"][0]["message"]["reasoning"].as_str())
        .filter(|value| !value.is_empty())
    {
        content = format!("<think>{reasoning}</think>") + &content;
    }
    let usage = parse_token_usage(json);
    Ok(ChatCompletion { content, usage })
}

fn parse_openai_tool_chat_completion(json: &serde_json::Value) -> Result<ToolChatCompletion> {
    let message = json
        .pointer("/choices/0/message")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("LLM tool response has no choices[0].message object"))?;
    let mut content = message
        .get("content")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if let Some(refusal) = message
        .get("refusal")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        content = format!("{CONTENT_REFUSAL_MESSAGE} {refusal}");
    } else if finish_reason_is_content_filter(json) && content.is_empty() {
        content = CONTENT_REFUSAL_MESSAGE.to_string();
    }
    // Same <think> prefix contract as async_chat_with_tools: reasoning is only
    // attached when the model made no tool calls.
    let has_tool_calls = message
        .get("tool_calls")
        .filter(|value| !value.is_null())
        .is_some_and(|value| !value.as_array().is_none_or(|calls| calls.is_empty()));
    if !has_tool_calls
        && let Some(reasoning) = message
            .get("reasoning_content")
            .and_then(serde_json::Value::as_str)
            .or_else(|| message.get("reasoning").and_then(serde_json::Value::as_str))
            .filter(|value| !value.is_empty())
    {
        content = format!("<think>{reasoning}</think>") + &content;
    }
    let tool_calls = message
        .get("tool_calls")
        .filter(|value| !value.is_null())
        .map(|value| {
            serde_json::from_value::<Vec<ToolCall>>(value.clone())
                .map_err(|error| anyhow::anyhow!("invalid LLM tool_calls payload: {error}"))
        })
        .transpose()?
        .unwrap_or_default();
    Ok(ToolChatCompletion {
        content,
        tool_calls,
        usage: parse_token_usage(json),
    })
}

fn parse_token_usage(json: &serde_json::Value) -> Option<TokenUsage> {
    json.get("usage").and_then(|usage| {
        TokenUsage {
            prompt_tokens: usage
                .get("prompt_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            completion_tokens: usage
                .get("completion_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
            total_tokens: usage
                .get("total_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
        }
        .normalized()
    })
}

/// Conversation manager with JSON persistence.
pub struct ConvStore {
    conversations: RwLock<HashMap<String, Conversation>>,
    file_path: String,
    save_lock: Mutex<()>,
}

impl ConvStore {
    /// Load or create conversation store.
    pub fn new(file_path: &str) -> Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(file_path))?;
        let conversations = if std::path::Path::new(file_path).exists() {
            let data = std::fs::read_to_string(file_path)?;
            let list: Vec<Conversation> = serde_json::from_str(&data)?;
            let list = list
                .into_iter()
                .map(|mut conversation| {
                    if conversation.tenant_id.is_empty() {
                        conversation.tenant_id = conversation.owner_id.clone();
                    }
                    if conversation.source.is_empty() {
                        conversation.source = default_conversation_source();
                    }
                    conversation
                })
                .collect::<Vec<_>>();
            list.into_iter().map(|c| (c.id.clone(), c)).collect()
        } else {
            HashMap::new()
        };

        let store = Self {
            conversations: RwLock::new(conversations),
            file_path: file_path.to_string(),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    /// List all conversations.
    pub fn list(&self) -> Vec<Conversation> {
        self.conversations
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect()
    }

    /// List conversations owned by one user, newest first.
    pub fn list_for(&self, owner_id: &str) -> Vec<Conversation> {
        let mut conversations: Vec<Conversation> = self
            .conversations
            .read()
            .unwrap()
            .values()
            .filter(|conversation| {
                conversation.owner_id == owner_id && conversation.source == "chat"
            })
            .cloned()
            .collect();
        conversations.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        conversations
    }

    /// Count conversations owned by one user.
    pub fn count_for(&self, owner_id: &str) -> usize {
        self.conversations
            .read()
            .unwrap()
            .values()
            .filter(|conversation| {
                conversation.owner_id == owner_id && conversation.source == "chat"
            })
            .count()
    }

    /// List sessions belonging to a tenant, newest first.
    pub fn list_for_tenant(&self, tenant_id: &str) -> Vec<Conversation> {
        let mut conversations: Vec<Conversation> = self
            .conversations
            .read()
            .unwrap()
            .values()
            .filter(|conversation| conversation.tenant_id == tenant_id)
            .cloned()
            .collect();
        conversations.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        conversations
    }

    /// List one tenant's sessions for the requested source and optional canvas.
    pub fn list_for_tenant_source(
        &self,
        tenant_id: &str,
        source: &str,
        canvas_id: Option<&str>,
    ) -> Vec<Conversation> {
        let mut conversations: Vec<_> = self
            .conversations
            .read()
            .unwrap()
            .values()
            .filter(|conversation| {
                conversation.tenant_id == tenant_id
                    && conversation.source == source
                    && canvas_id.is_none_or(|id| conversation.canvas_id.as_deref() == Some(id))
            })
            .cloned()
            .collect();
        conversations.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        conversations
    }

    /// Create a new conversation.
    pub fn create(&self, name: &str) -> Result<Conversation> {
        self.create_for("", name)
    }

    /// Create a new conversation scoped to one owner.
    pub fn create_for(&self, owner_id: &str, name: &str) -> Result<Conversation> {
        self.create_for_tenant_settings(owner_id, owner_id, name, Vec::new(), None, None)
    }

    pub fn create_for_settings(
        &self,
        owner_id: &str,
        name: &str,
        kb_ids: Vec<String>,
        chat_model: Option<String>,
        embedding_model: Option<String>,
    ) -> Result<Conversation> {
        self.create_for_tenant_settings(
            owner_id,
            owner_id,
            name,
            kb_ids,
            chat_model,
            embedding_model,
        )
    }

    pub fn create_for_tenant_settings(
        &self,
        owner_id: &str,
        tenant_id: &str,
        name: &str,
        kb_ids: Vec<String>,
        chat_model: Option<String>,
        embedding_model: Option<String>,
    ) -> Result<Conversation> {
        let now = now_ms();
        let conv = Conversation {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            owner_id: owner_id.to_string(),
            tenant_id: tenant_id.to_string(),
            source: default_conversation_source(),
            canvas_id: None,
            app_id: None,
            kb_ids,
            chat_model,
            embedding_model,
            messages: vec![],
            duration_ms: 0,
            dsl: None,
            errors: None,
            version_title: None,
            created_at: now,
            updated_at: now,
        };
        self.mutate(|convs| {
            convs.insert(conv.id.clone(), conv.clone());
            Ok(conv)
        })
    }

    /// Bind a conversation to a chat app (RAGFlow 0.26.4 chat-app model).
    pub fn set_app_id(&self, id: &str, app_id: &str) -> Result<()> {
        self.mutate(|convs| {
            if let Some(c) = convs.get_mut(id) {
                c.app_id = Some(app_id.to_string());
            }
            Ok(())
        })
    }

    /// Agent sessions belonging to one canvas, in id order. The agent log list
    /// (`GET /api/v1/canvas/{id}/sessions`) sorts and paginates them itself,
    /// mirroring `API4ConversationService.get_list` filtering on `dialog_id`.
    pub fn list_agent_sessions(&self, canvas_id: &str) -> Vec<Conversation> {
        let mut sessions: Vec<Conversation> = self
            .conversations
            .read()
            .unwrap()
            .values()
            .filter(|conversation| {
                conversation.source == "agent"
                    && conversation.canvas_id.as_deref() == Some(canvas_id)
            })
            .cloned()
            .collect();
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        sessions
    }

    /// Record one agent run's DSL snapshot and error state (RAGFlow
    /// `Completion.save`: `conv.dsl = str(canvas)` and
    /// `conv.errors = canvas.error`). `errors` replaces the previous value, so
    /// a clean run clears an earlier failure.
    pub fn record_agent_run(
        &self,
        id: &str,
        dsl: &serde_json::Value,
        errors: Option<&str>,
    ) -> Result<()> {
        self.mutate(|conversations| {
            if let Some(conversation) = conversations.get_mut(id) {
                conversation.dsl = Some(dsl.clone());
                conversation.errors = errors.map(|error| error.to_string());
            }
            Ok(())
        })
    }

    /// Capture the canvas version title at session creation (RAGFlow
    /// `API4ConversationService.get_latest_version_title`).
    pub fn set_agent_version_title(&self, id: &str, title: &str) -> Result<()> {
        self.mutate(|conversations| {
            if let Some(conversation) = conversations.get_mut(id) {
                conversation.version_title = Some(title.to_string());
            }
            Ok(())
        })
    }

    /// Create a persisted agent session owned by the invoking user.
    pub fn create_agent_for(
        &self,
        owner_id: &str,
        tenant_id: &str,
        canvas_id: &str,
        name: &str,
        kb_ids: Vec<String>,
    ) -> Result<Conversation> {
        let now = now_ms();
        let conversation = Conversation {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            owner_id: owner_id.to_string(),
            tenant_id: tenant_id.to_string(),
            source: "agent".into(),
            canvas_id: Some(canvas_id.to_string()),
            app_id: None,
            kb_ids,
            chat_model: None,
            embedding_model: None,
            messages: Vec::new(),
            duration_ms: 0,
            dsl: None,
            errors: None,
            version_title: None,
            created_at: now,
            updated_at: now,
        };
        self.mutate(|conversations| {
            conversations.insert(conversation.id.clone(), conversation.clone());
            Ok(conversation)
        })
    }

    /// Create the row that upstream `agent_api.py::create_agent_session` writes:
    /// the request's name, the canvas DSL snapshot, the canvas prologue seeded as
    /// the first assistant message and the latest version title.
    ///
    /// Upstream seeds `message: [{"role": "assistant", "content":
    /// canvas.get_prologue()}]` unconditionally, so an empty prologue still
    /// produces one empty assistant entry; `reference` starts empty and
    /// `source` is `agent`.
    pub fn create_agent_session(&self, seed: NewAgentSession<'_>) -> Result<Conversation> {
        let now = now_ms();
        let conversation = Conversation {
            id: uuid::Uuid::new_v4().to_string(),
            name: seed.name.to_string(),
            owner_id: seed.owner_id.to_string(),
            tenant_id: seed.tenant_id.to_string(),
            source: "agent".into(),
            canvas_id: Some(seed.canvas_id.to_string()),
            app_id: None,
            kb_ids: seed.kb_ids,
            chat_model: None,
            embedding_model: None,
            messages: vec![ChatMessage::new("assistant", seed.prologue)],
            duration_ms: 0,
            dsl: seed.dsl,
            errors: None,
            version_title: seed.version_title,
            created_at: now,
            updated_at: now,
        };
        self.mutate(|conversations| {
            conversations.insert(conversation.id.clone(), conversation.clone());
            Ok(conversation)
        })
    }

    /// Resume only the caller's agent session for exactly the same tenant/canvas.
    pub fn get_agent_for(
        &self,
        id: &str,
        owner_id: &str,
        tenant_id: &str,
        canvas_id: &str,
    ) -> Option<Conversation> {
        self.conversations
            .read()
            .unwrap()
            .get(id)
            .filter(|conversation| {
                conversation.owner_id == owner_id
                    && conversation.tenant_id == tenant_id
                    && conversation.source == "agent"
                    && conversation.canvas_id.as_deref() == Some(canvas_id)
            })
            .cloned()
    }

    /// Get a conversation by ID.
    pub fn get(&self, id: &str) -> Option<Conversation> {
        self.conversations.read().unwrap().get(id).cloned()
    }

    /// Get a conversation only when it belongs to the requested owner.
    pub fn get_for(&self, id: &str, owner_id: &str) -> Option<Conversation> {
        self.conversations
            .read()
            .unwrap()
            .get(id)
            .filter(|conversation| {
                conversation.owner_id == owner_id && conversation.source == "chat"
            })
            .cloned()
    }

    /// Add a message to a conversation.
    /// 清空会话消息。
    pub fn clear_messages(&self, id: &str) -> Result<bool> {
        self.mutate_if_changed(|conversations| {
            let Some(conversation) = conversations.get_mut(id) else {
                return Ok((false, false));
            };
            conversation.messages.clear();
            Ok((true, true))
        })
    }

    pub fn add_message(&self, id: &str, role: &str, content: &str) -> Result<()> {
        self.mutate_if_changed(|convs| {
            let Some(conv) = convs.get_mut(id) else {
                return Ok(((), false));
            };
            conv.messages.push(ChatMessage::new(role, content));
            conv.updated_at = now_ms();
            Ok(((), true))
        })
    }

    /// Atomically append a user/assistant exchange so persistence failure cannot
    /// leave half of a turn in conversation history.
    pub fn append_exchange(
        &self,
        id: &str,
        owner_id: &str,
        question: &str,
        answer: &str,
        citations: Vec<String>,
    ) -> Result<bool> {
        self.append_exchange_with_references(id, owner_id, question, answer, citations, Vec::new())
            .map(|message_id| message_id.is_some())
    }

    pub fn append_exchange_with_references(
        &self,
        id: &str,
        owner_id: &str,
        question: &str,
        answer: &str,
        citations: Vec<String>,
        references: Vec<ChunkReference>,
    ) -> Result<Option<String>> {
        self.append_exchange_with_settings(
            id,
            owner_id,
            ConversationExchange {
                question: question.into(),
                answer: answer.into(),
                citations,
                references,
                settings: None,
                duration_ms: 0,
                usage: None,
            },
        )
    }

    pub fn append_exchange_with_settings(
        &self,
        id: &str,
        owner_id: &str,
        exchange: ConversationExchange,
    ) -> Result<Option<String>> {
        self.append_exchange_with_settings_id(id, owner_id, exchange, new_message_id())
    }

    /// Persist an exchange using the run-scoped message id already exposed in
    /// Agent SSE frames.
    pub fn append_exchange_with_settings_id(
        &self,
        id: &str,
        owner_id: &str,
        exchange: ConversationExchange,
        message_id: String,
    ) -> Result<Option<String>> {
        let created_at = now_ms();
        self.mutate_if_changed(|conversations| {
            let Some(conversation) = conversations
                .get_mut(id)
                .filter(|conversation| conversation.owner_id == owner_id)
            else {
                return Ok((None, false));
            };
            let mut user_message = ChatMessage::new("user", exchange.question);
            user_message.id = message_id.clone();
            user_message.created_at = created_at;
            conversation.messages.push(user_message);
            let mut assistant_message = ChatMessage::new("assistant", exchange.answer);
            assistant_message.id = message_id.clone();
            assistant_message.citations = Some(exchange.citations);
            assistant_message.references = exchange.references;
            assistant_message.usage = exchange.usage.and_then(TokenUsage::normalized);
            assistant_message.created_at = created_at;
            conversation.messages.push(assistant_message);
            if let Some((kb_ids, chat_model, embedding_model)) = exchange.settings {
                conversation.kb_ids = kb_ids;
                conversation.chat_model = chat_model;
                conversation.embedding_model = embedding_model;
            }
            conversation.duration_ms = conversation
                .duration_ms
                .saturating_add(exchange.duration_ms);
            conversation.updated_at = now_ms();
            Ok((Some(message_id), true))
        })
    }

    pub fn feedback_target(
        &self,
        conversation_id: &str,
        owner_id: &str,
        message_id: &str,
    ) -> Option<MessageFeedbackTarget> {
        let conversations = self.conversations.read().unwrap();
        let conversation = conversations.get(conversation_id).filter(|conversation| {
            conversation.owner_id == owner_id && conversation.source == "chat"
        })?;
        let message = conversation
            .messages
            .iter()
            .find(|message| message.id == message_id && message.role == "assistant")?;
        Some(MessageFeedbackTarget {
            prior_thumb: message.thumbup,
            prior_feedback: message.feedback.clone(),
            references: message.references.clone(),
        })
    }

    pub fn update_message_feedback(
        &self,
        conversation_id: &str,
        owner_id: &str,
        message_id: &str,
        expected_prior: Option<bool>,
        thumbup: bool,
        feedback: Option<String>,
    ) -> Result<bool> {
        self.mutate_if_changed(|conversations| {
            let Some(conversation) =
                conversations
                    .get_mut(conversation_id)
                    .filter(|conversation| {
                        conversation.owner_id == owner_id && conversation.source == "chat"
                    })
            else {
                return Ok((false, false));
            };
            let Some(message) = conversation
                .messages
                .iter_mut()
                .find(|message| message.id == message_id && message.role == "assistant")
            else {
                return Ok((false, false));
            };
            if message.thumbup != expected_prior {
                anyhow::bail!("Message feedback changed concurrently");
            }
            let normalized_feedback = if thumbup {
                None
            } else {
                feedback.filter(|value| !value.trim().is_empty())
            };
            let changed =
                message.thumbup != Some(thumbup) || message.feedback != normalized_feedback;
            message.thumbup = Some(thumbup);
            message.feedback = normalized_feedback;
            if changed {
                conversation.updated_at = now_ms();
            }
            Ok((true, changed))
        })
    }

    pub fn rename_for(
        &self,
        conversation_id: &str,
        owner_id: &str,
        name: &str,
    ) -> Result<Option<Conversation>> {
        let name = name.trim();
        if name.is_empty() {
            anyhow::bail!("Conversation name cannot be empty");
        }
        let name: String = name.chars().take(255).collect();
        self.mutate_if_changed(|conversations| {
            let Some(conversation) =
                conversations
                    .get_mut(conversation_id)
                    .filter(|conversation| {
                        conversation.owner_id == owner_id && conversation.source == "chat"
                    })
            else {
                return Ok((None, false));
            };
            let changed = conversation.name != name;
            if changed {
                conversation.name = name;
                conversation.updated_at = now_ms();
            }
            Ok((Some(conversation.clone()), changed))
        })
    }

    pub fn bind_settings_for(
        &self,
        conversation_id: &str,
        owner_id: &str,
        kb_ids: Vec<String>,
        chat_model: Option<String>,
        embedding_model: Option<String>,
    ) -> Result<Option<Conversation>> {
        self.mutate_if_changed(|conversations| {
            let Some(conversation) =
                conversations
                    .get_mut(conversation_id)
                    .filter(|conversation| {
                        conversation.owner_id == owner_id && conversation.source == "chat"
                    })
            else {
                return Ok((None, false));
            };
            let changed = conversation.kb_ids != kb_ids
                || conversation.chat_model != chat_model
                || conversation.embedding_model != embedding_model;
            if changed {
                conversation.kb_ids = kb_ids;
                conversation.chat_model = chat_model;
                conversation.embedding_model = embedding_model;
                conversation.updated_at = now_ms();
            }
            Ok((Some(conversation.clone()), changed))
        })
    }

    pub fn regeneration_target(
        &self,
        conversation_id: &str,
        owner_id: &str,
        message_id: &str,
    ) -> Option<RegenerationTarget> {
        let conversations = self.conversations.read().unwrap();
        let conversation = conversations.get(conversation_id).filter(|conversation| {
            conversation.owner_id == owner_id && conversation.source == "chat"
        })?;
        let assistant_index = conversation
            .messages
            .iter()
            .position(|message| message.id == message_id && message.role == "assistant")?;
        if assistant_index == 0 {
            return None;
        }
        let question = &conversation.messages[assistant_index - 1];
        if question.role != "user" || question.id != message_id {
            return None;
        }
        Some(RegenerationTarget {
            question: question.content.clone(),
            prior_answer: conversation.messages[assistant_index].content.clone(),
            history: conversation.messages[..assistant_index - 1].to_vec(),
            kb_ids: conversation.kb_ids.clone(),
            chat_model: conversation.chat_model.clone(),
            embedding_model: conversation.embedding_model.clone(),
        })
    }

    pub fn replace_assistant_message(
        &self,
        conversation_id: &str,
        owner_id: &str,
        message_id: &str,
        replacement: AssistantMessageReplacement,
    ) -> Result<bool> {
        self.mutate_if_changed(|conversations| {
            let Some(conversation) =
                conversations
                    .get_mut(conversation_id)
                    .filter(|conversation| {
                        conversation.owner_id == owner_id && conversation.source == "chat"
                    })
            else {
                return Ok((false, false));
            };
            let Some(message) = conversation
                .messages
                .iter_mut()
                .find(|message| message.id == message_id && message.role == "assistant")
            else {
                return Ok((false, false));
            };
            if message.content != replacement.expected_prior_answer {
                anyhow::bail!("Assistant message changed concurrently");
            }
            message.content = replacement.answer;
            message.citations = Some(replacement.citations);
            message.references = replacement.references;
            message.thumbup = None;
            message.feedback = None;
            message.usage = replacement.usage.and_then(TokenUsage::normalized);
            message.created_at = now_ms();
            conversation.kb_ids = replacement.kb_ids;
            conversation.chat_model = replacement.chat_model;
            conversation.embedding_model = replacement.embedding_model;
            conversation.updated_at = now_ms();
            Ok((true, true))
        })
    }

    pub fn delete_message_pair_for(
        &self,
        conversation_id: &str,
        owner_id: &str,
        message_id: &str,
    ) -> Result<Option<Conversation>> {
        self.mutate_if_changed(|conversations| {
            let Some(conversation) =
                conversations
                    .get_mut(conversation_id)
                    .filter(|conversation| {
                        conversation.owner_id == owner_id && conversation.source == "chat"
                    })
            else {
                return Ok((None, false));
            };
            let Some(user_index) = conversation
                .messages
                .iter()
                .position(|message| message.id == message_id && message.role == "user")
            else {
                return Ok((None, false));
            };
            let assistant_index = user_index + 1;
            if conversation
                .messages
                .get(assistant_index)
                .is_none_or(|message| message.id != message_id || message.role != "assistant")
            {
                anyhow::bail!("Conversation message pair is inconsistent");
            }
            conversation.messages.drain(user_index..=assistant_index);
            conversation.updated_at = now_ms();
            Ok((Some(conversation.clone()), true))
        })
    }

    /// Delete an owned conversation atomically.
    pub fn delete_for(&self, id: &str, owner_id: &str) -> Result<bool> {
        self.mutate_if_changed(|convs| {
            let removed = convs.get(id).is_some_and(|conversation| {
                conversation.owner_id == owner_id
                    && matches!(conversation.source.as_str(), "chat" | "agent")
            });
            if removed {
                convs.remove(id);
            }
            Ok((removed, removed))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, Conversation>) -> Result<T>,
    ) -> Result<T> {
        self.mutate_if_changed(|conversations| mutation(conversations).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, Conversation>) -> Result<(T, bool)>,
    ) -> Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut conversations = self.conversations.write().unwrap();
        let previous = conversations.clone();
        let (value, changed) = mutation(&mut conversations)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<Conversation> = conversations.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *conversations = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        let conversations: Vec<Conversation> = self
            .conversations
            .read()
            .unwrap()
            .values()
            .cloned()
            .collect();
        self.persist(&conversations)
    }

    fn persist(&self, conversations: &[Conversation]) -> Result<()> {
        let data = serde_json::to_vec_pretty(conversations)?;
        crate::persistence::atomic_write(std::path::Path::new(&self.file_path), &data)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn new_message_id() -> String {
    uuid::Uuid::new_v4().to_string()
}



// ── Model factory (rag/llm/__init__.py + module Base abstractions) ──

/// Model capability kinds — mirrors RAGFlow `MODULE_MAPPING` keys
/// (`chat_model`, `embedding_model`, `rerank_model`, `ocr_model`,
/// `cv_model`, `sequence2txt_model`, `tts_model`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Chat,
    Embedding,
    Rerank,
    Ocr,
    Cv,
    Seq2txt,
    Tts,
}

impl ModelKind {
    /// RAGFlow `MODULE_MAPPING` module name for this kind.
    pub const fn module_name(self) -> &'static str {
        match self {
            Self::Chat => "chat_model",
            Self::Embedding => "embedding_model",
            Self::Rerank => "rerank_model",
            Self::Ocr => "ocr_model",
            Self::Cv => "cv_model",
            Self::Seq2txt => "sequence2txt_model",
            Self::Tts => "tts_model",
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Embedding => "embedding",
            Self::Rerank => "rerank",
            Self::Ocr => "ocr",
            Self::Cv => "cv",
            Self::Seq2txt => "seq2txt",
            Self::Tts => "tts",
        }
    }
}

/// All model kinds, in RAGFlow module order.
pub const MODEL_KINDS: [ModelKind; 7] = [
    ModelKind::Chat,
    ModelKind::Embedding,
    ModelKind::Rerank,
    ModelKind::Seq2txt,
    ModelKind::Tts,
    ModelKind::Ocr,
    ModelKind::Cv,
];

/// Chat model abstraction — RAGFlow `chat_model.Base` (async_chat /
/// async_chat_streamly). RayRAG's chat backends are all OpenAI-compatible.
#[async_trait::async_trait]
pub trait ChatModel: Send + Sync {
    /// Model name (RAGFlow `Base.model_name`).
    fn model_name(&self) -> &str;
    /// Max context length in tokens (RAGFlow `Base.max_length`, default 8192).
    fn max_length(&self) -> usize {
        8192
    }
    /// One-shot chat: system prompt + history → answer text.
    async fn chat(&self, system: &str, history: &[ChatMessage]) -> Result<String>;
    /// One-shot chat with per-call generation overrides.
    ///
    /// The default deliberately delegates to [`ChatModel::chat`] and ignores
    /// the patch so existing non-OpenAI/fake implementations remain source
    /// compatible. Backends that support request-scoped generation settings
    /// override this method; [`LlmClient`] forwards the patch to its
    /// OpenAI-compatible completion request.
    async fn chat_with_generation(
        &self,
        system: &str,
        history: &[ChatMessage],
        _generation: GenerationParamsPatch,
    ) -> Result<String> {
        self.chat(system, history).await
    }
    /// Streaming chat, delivering each SSE delta to `on_chunk`.
    async fn chat_stream(
        &self,
        system: &str,
        history: &[ChatMessage],
        on_chunk: Box<dyn for<'a> FnMut(&'a str) + Send>,
    ) -> Result<String>;
}

#[async_trait::async_trait]
impl ChatModel for LlmClient {
    fn model_name(&self) -> &str {
        LlmClient::model_name(self)
    }

    async fn chat(&self, system: &str, history: &[ChatMessage]) -> Result<String> {
        let mut messages = Vec::with_capacity(history.len() + 1);
        messages.push(ChatMessage::new("system", system));
        messages.extend(history.iter().cloned());
        Ok(self.chat_completion(&messages).await?.content)
    }

    async fn chat_with_generation(
        &self,
        system: &str,
        history: &[ChatMessage],
        generation: GenerationParamsPatch,
    ) -> Result<String> {
        let mut messages = Vec::with_capacity(history.len() + 1);
        messages.push(ChatMessage::new("system", system));
        messages.extend(history.iter().cloned());
        Ok(self
            .chat_completion_with_generation(&messages, generation)
            .await?
            .content)
    }

    async fn chat_stream(
        &self,
        system: &str,
        history: &[ChatMessage],
        on_chunk: Box<dyn for<'a> FnMut(&'a str) + Send>,
    ) -> Result<String> {
        let mut messages = Vec::with_capacity(history.len() + 1);
        messages.push(ChatMessage::new("system", system));
        messages.extend(history.iter().cloned());
        let mut on_chunk = on_chunk;
        self.chat_stream_impl(&messages, GenerationParamsPatch::default(), &mut on_chunk)
            .await
    }
}

/// OCR model abstraction — RAGFlow `ocr_model.Base.parse_pdf`.
#[async_trait::async_trait]
pub trait OcrModel: Send + Sync {
    fn model_name(&self) -> &str;
    /// Parse a PDF file into extracted text.
    async fn parse_pdf(&self, filepath: &str) -> Result<String>;
}

/// Computer-vision model abstraction — RAGFlow `cv_model.Base`.
#[async_trait::async_trait]
pub trait CvModel: Send + Sync {
    fn model_name(&self) -> &str;
    /// Describe an image (raw bytes) in natural language.
    async fn describe(&self, image: &[u8]) -> Result<String>;
    /// Describe an image with a custom prompt.
    async fn describe_with_prompt(&self, image: &[u8], prompt: &str) -> Result<String>;
}

/// Speech-to-text model abstraction — RAGFlow `sequence2txt_model.Base`.
#[async_trait::async_trait]
pub trait Seq2txtModel: Send + Sync {
    fn model_name(&self) -> &str;
    /// Transcribe an audio file into text.
    async fn transcription(&self, audio_path: &str) -> Result<String>;
}

/// Text-to-speech model abstraction — RAGFlow `tts_model.Base`.
#[async_trait::async_trait]
pub trait TtsModel: Send + Sync {
    fn model_name(&self) -> &str;
    /// Synthesize speech; returns audio bytes.
    async fn tts(&self, text: &str) -> Result<Vec<u8>>;
}

/// OpenAI-compatible computer-vision model (chat completions with
/// image_url content parts) — mirrors `cv_model.GptV4` family.
pub struct OpenAiCvModel {
    client: LlmClient,
    lang: String,
}

/// `vision_llm_describe_prompt.md` — default PDF page transcription prompt
/// used by RAGFlow `cv_model.Base.describe` (GptV4 family). The
/// `deepdoc/vision` local-ONNX recognizers only produce raw text runs; this
/// LLM prompt is what turns a rendered page image into clean Markdown, and
/// is the CvModel counterpart of the same constant in `crate::vision`.
pub const VISION_LLM_DESCRIBE_PROMPT: &str = r#"## INSTRUCTION
Transcribe the content from the provided PDF page image into clean Markdown format.

- Only output the content transcribed from the image.
- Do NOT output this instruction or any other explanation.
- If the content is missing or you do not understand the input, return an empty string.

## RULES
1. Do NOT generate examples, demonstrations, or templates.
2. Do NOT output any extra text such as 'Example', 'Example Output', or similar.
3. Do NOT generate any tables, headings, or content that is not explicitly present in the image.
4. Transcribe content word-for-word. Do NOT modify, translate, or omit any content.
5. Do NOT explain Markdown or mention that you are using Markdown.
6. Do NOT wrap the output in ```markdown or ``` blocks.
7. Only apply Markdown structure to headings, paragraphs, lists, and tables, strictly based on the layout of the image. Do NOT create tables unless an actual table exists in the image.
8. Preserve the original language, information, and order exactly as shown in the image.

> If you do not detect valid content in the image, return an empty string.
"#;

impl OpenAiCvModel {
    pub fn new(client: LlmClient, lang: &str) -> Self {
        Self {
            client,
            lang: lang.to_string(),
        }
    }

    fn vision_messages(&self, prompt: &str, image_b64: &str) -> serde_json::Value {
        serde_json::json!([{
            "role": "user",
            "content": [
                {"type": "text", "text": prompt},
                {"type": "image_url", "image_url": {
                    "url": format!("data:image/png;base64,{image_b64}")
                }}
            ]
        }])
    }

    /// Describe an image as a rendered document page — RAGFlow
    /// `cv_model.Base.describe` semantics with the full
    /// `vision_llm_describe_prompt.md` transcription prompt (the
    /// `deepdoc/vision` layout/OCR recognizers already extracted the text
    /// runs; this is the page-level Markdown pass).
    pub async fn describe_page(&self, image: &[u8]) -> Result<String> {
        self.describe_with_prompt(image, VISION_LLM_DESCRIBE_PROMPT)
            .await
    }

    /// Describe a figure (chart / screenshot) — RAGFlow
    /// `vision_llm_figure_describe_prompt.md`, the CvModel-side entry of
    /// the `deepdoc/vision` figure semantics (the figure parser routes
    /// cropped `figure` layout blocks through this prompt).
    pub async fn describe_figure(&self, image: &[u8]) -> Result<String> {
        let prompt = crate::parser::figure::figure_describe_prompt();
        self.describe_with_prompt(image, &prompt).await
    }

    /// Describe a figure with surrounding document context — RAGFlow
    /// `vision_llm_figure_describe_prompt_with_context.md`. Context is only
    /// used to disambiguate terms visible in the image; when both sides are
    /// empty this falls back to the plain figure prompt, mirroring
    /// `figure_parser.py VisionFigureParser.process`.
    pub async fn describe_figure_with_context(
        &self,
        image: &[u8],
        context_above: &str,
        context_below: &str,
    ) -> Result<String> {
        let prompt = if context_above.is_empty() && context_below.is_empty() {
            crate::parser::figure::figure_describe_prompt()
        } else {
            crate::parser::figure::figure_describe_prompt_with_context(context_above, context_below)
        };
        self.describe_with_prompt(image, &prompt).await
    }
}

#[async_trait::async_trait]
impl CvModel for OpenAiCvModel {
    fn model_name(&self) -> &str {
        self.client.model_name()
    }

    async fn describe(&self, image: &[u8]) -> Result<String> {
        self.describe_with_prompt(image, "").await
    }

    async fn describe_with_prompt(&self, image: &[u8], prompt: &str) -> Result<String> {
        let prompt = if prompt.trim().is_empty() {
            format!("Describe this image in {}.", self.lang)
        } else {
            prompt.to_string()
        };
        let body = serde_json::json!({
            "model": self.client.model_name(),
            "messages": self.vision_messages(&prompt, &base64_encode(image)),
            "max_tokens": 512,
        });
        let json = self.client.chat_raw(body).await?;
        Ok(json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string())
    }
}

/// OpenAI-compatible speech-to-text model (`/audio/transcriptions`) —
/// mirrors `sequence2txt_model.GPTSeq2txt` (whisper family).
pub struct OpenAiSeq2txtModel {
    client: LlmClient,
}

impl OpenAiSeq2txtModel {
    pub fn new(client: LlmClient) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Seq2txtModel for OpenAiSeq2txtModel {
    fn model_name(&self) -> &str {
        self.client.model_name()
    }

    async fn transcription(&self, audio_path: &str) -> Result<String> {
        let bytes = std::fs::read(audio_path)?;
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name("audio.mp3")
            .mime_str("audio/mpeg")?;
        let form = reqwest::multipart::Form::new()
            .text("model", self.client.model_name().to_string())
            .part("file", part);
        let url = format!("{}/audio/transcriptions", self.client.api_base());
        let json = self.client.raw_post_multipart(&url, form).await?;
        Ok(json["text"].as_str().unwrap_or_default().to_string())
    }
}

/// OpenAI-compatible text-to-speech model (`/audio/speech`) — mirrors
/// `tts_model.OpenAITTS`.
pub struct OpenAiTtsModel {
    client: LlmClient,
    voice: String,
}

impl OpenAiTtsModel {
    pub fn new(client: LlmClient, voice: &str) -> Self {
        Self {
            client,
            voice: voice.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl TtsModel for OpenAiTtsModel {
    fn model_name(&self) -> &str {
        self.client.model_name()
    }

    async fn tts(&self, text: &str) -> Result<Vec<u8>> {
        let body = serde_json::json!({
            "model": self.client.model_name(),
            "input": text,
            "voice": self.voice,
        });
        let url = format!("{}/audio/speech", self.client.api_base());
        self.client.raw_post_bytes(&url, &body).await
    }
}

/// Stub for model kinds with no Rust backend yet (OCR needs Python-side
/// MinerU/Paddle). Implements the remaining traits and fails with a clear
/// error, mirroring RAGFlow's factory returning no class for unknown kinds.
pub struct UnsupportedModel {
    kind: &'static str,
    model: String,
}

impl UnsupportedModel {
    pub fn new(kind: &'static str, model: &str) -> Self {
        Self {
            kind,
            model: model.to_string(),
        }
    }

    fn error(&self) -> anyhow::Error {
        anyhow::anyhow!(
            "model kind '{}' (model '{}') has no Rust backend yet",
            self.kind,
            self.model
        )
    }
}

#[async_trait::async_trait]
impl OcrModel for UnsupportedModel {
    fn model_name(&self) -> &str {
        &self.model
    }
    async fn parse_pdf(&self, _filepath: &str) -> Result<String> {
        Err(self.error())
    }
}

#[async_trait::async_trait]
impl CvModel for UnsupportedModel {
    fn model_name(&self) -> &str {
        &self.model
    }
    async fn describe(&self, _image: &[u8]) -> Result<String> {
        Err(self.error())
    }
    async fn describe_with_prompt(&self, _image: &[u8], _prompt: &str) -> Result<String> {
        Err(self.error())
    }
}

#[async_trait::async_trait]
impl Seq2txtModel for UnsupportedModel {
    fn model_name(&self) -> &str {
        &self.model
    }
    async fn transcription(&self, _audio_path: &str) -> Result<String> {
        Err(self.error())
    }
}

#[async_trait::async_trait]
impl TtsModel for UnsupportedModel {
    fn model_name(&self) -> &str {
        &self.model
    }
    async fn tts(&self, _text: &str) -> Result<Vec<u8>> {
        Err(self.error())
    }
}

/// Resolve a model API base URL: explicit override > provider default
/// (RAGFlow `FACTORY_DEFAULT_BASE_URL`) > global fallback.
fn resolve_base_url(provider: &str, api_base: Option<&str>, fallback: &str) -> String {
    api_base
        .map(str::to_string)
        .or_else(|| crate::providers::provider_default_base(provider, provider).map(str::to_string))
        .unwrap_or_else(|| fallback.to_string())
}

/// Create a chat model for a provider — RAGFlow `ChatModel[_FACTORY_NAME]`.
/// All RayRAG chat backends are OpenAI-compatible; the provider only selects
/// the default base URL when none is given.
pub fn chat_model_for(
    provider: &str,
    api_key: &str,
    model: &str,
    api_base: Option<&str>,
) -> Box<dyn ChatModel> {
    let config = LlmConfig {
        api_base: resolve_base_url(provider, api_base, "https://api.openai.com/v1"),
        api_key: api_key.to_string(),
        model: model.to_string(),
        ..LlmConfig::default()
    };
    Box::new(LlmClient::new(config))
}

/// Create an embedding model for a provider — RAGFlow
/// `EmbeddingModel[_FACTORY_NAME]` (OpenAI-compatible family).
pub fn embedding_model_for(
    provider: &str,
    api_key: &str,
    model: &str,
    api_base: Option<&str>,
) -> Result<Box<dyn crate::embed::Embedder>> {
    if provider.eq_ignore_ascii_case("builtin") {
        anyhow::bail!(
            "Builtin embedding requires a local model path; use embed::local_embedder() instead"
        );
    }
    let base = resolve_base_url(provider, api_base, "https://api.openai.com/v1");
    Ok(Box::new(crate::embed::OpenAIEmbedder::new(
        &base, api_key, model,
    )))
}

/// Create a rerank model for a provider — RAGFlow
/// `RerankModel[_FACTORY_NAME]` (Cohere-compatible `/v1/rerank` family).
pub fn rerank_model_for(
    provider: &str,
    api_key: &str,
    model: &str,
    api_base: Option<&str>,
) -> Result<Box<dyn crate::rerank::Reranker>> {
    // `model` used to be discarded (`let _ = model;`): the instance's rerank model
    // reached the endpoint only as a name the endpoint had to guess.
    let base = resolve_base_url(provider, api_base, "http://127.0.0.1:8899");
    let mut reranker = crate::rerank::RemoteReranker::new(&base);
    if !api_key.is_empty() {
        reranker = reranker.with_api_key(api_key);
    }
    if !model.trim().is_empty() {
        reranker = reranker.with_model(model);
    }
    Ok(Box::new(reranker))
}

/// Create a computer-vision model for a provider — RAGFlow
/// `CvModel[_FACTORY_NAME]` (GptV4 family).
pub fn cv_model_for(
    provider: &str,
    api_key: &str,
    model: &str,
    api_base: Option<&str>,
) -> Box<dyn CvModel> {
    let base = resolve_base_url(provider, api_base, "https://api.openai.com/v1");
    let client = LlmClient::new(LlmConfig {
        api_base: base,
        api_key: api_key.to_string(),
        model: model.to_string(),
        ..LlmConfig::default()
    });
    Box::new(OpenAiCvModel::new(client, "Chinese"))
}

/// Create a speech-to-text model for a provider — RAGFlow
/// `Seq2txtModel[_FACTORY_NAME]` (GPTSeq2txt family).
pub fn seq2txt_model_for(
    provider: &str,
    api_key: &str,
    model: &str,
    api_base: Option<&str>,
) -> Box<dyn Seq2txtModel> {
    let base = resolve_base_url(provider, api_base, "https://api.openai.com/v1");
    let client = LlmClient::new(LlmConfig {
        api_base: base,
        api_key: api_key.to_string(),
        model: model.to_string(),
        ..LlmConfig::default()
    });
    Box::new(OpenAiSeq2txtModel::new(client))
}

/// Create a text-to-speech model for a provider — RAGFlow
/// `TTSModel[_FACTORY_NAME]` (OpenAITTS family).
pub fn tts_model_for(
    provider: &str,
    api_key: &str,
    model: &str,
    api_base: Option<&str>,
) -> Box<dyn TtsModel> {
    let base = resolve_base_url(provider, api_base, "https://api.openai.com/v1");
    let client = LlmClient::new(LlmConfig {
        api_base: base,
        api_key: api_key.to_string(),
        model: model.to_string(),
        ..LlmConfig::default()
    });
    Box::new(OpenAiTtsModel::new(client, "alloy"))
}

/// Create an OCR model for a provider — RAGFlow `OcrModel[_FACTORY_NAME]`.
/// No Rust OCR backend exists yet (upstream uses Python MinerU/Paddle), so
/// this always returns the explicit-error stub.
pub fn ocr_model_for(
    provider: &str,
    api_key: &str,
    model: &str,
    api_base: Option<&str>,
) -> Box<dyn OcrModel> {
    let _ = (provider, api_key, api_base);
    Box::new(UnsupportedModel::new("ocr", model))
}

/// Whether a factory can construct a working (non-stub) model of `kind` for
/// `provider`. OCR is the only kind with no Rust backend.
pub fn model_kind_supported(kind: ModelKind, provider: &str) -> bool {
    match kind {
        ModelKind::Ocr => false,
        ModelKind::Chat
        | ModelKind::Embedding
        | ModelKind::Rerank
        | ModelKind::Cv
        | ModelKind::Seq2txt
        | ModelKind::Tts => !provider.is_empty(),
    }
}

/// Minimal standard base64 encoder (RFC 4648, padded) — avoids pulling the
/// `base64` crate for image data URLs and audio payloads.
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}



/// Block on an async future in a sync test (small local helper mirroring
/// tokio::test's runtime without the attribute).
fn tokio_test_block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build test runtime")
        .block_on(future)
}

/// The production [`crate::harness::HarnessChat`] over RayRAG's chat client.
///
/// The ported generators (dataset navigation, wiki/artifact compilation, structure
/// compilation) all drive `crate::harness::HarnessChat`, but until now the trait had **only
/// test doubles** — which is why those engines could be built and unit-tested yet never run
/// in production. This is the adapter that connects them to a real model.
///
/// OpenAI-shaped `history` entries (`{"role": …, "content": …}`) are mapped onto
/// [`ChatMessage`]s; an empty system prompt is omitted rather than sent as an empty message.
/// A failure is returned as `Err(String)` — never as an empty `Ok`, which would let a
/// generator "succeed" while producing nothing.
pub struct LlmHarnessChat {
    client: std::sync::Arc<LlmClient>,
}

impl LlmHarnessChat {
    pub fn new(client: std::sync::Arc<LlmClient>) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl crate::harness::HarnessChat for LlmHarnessChat {
    async fn chat(
        &self,
        system: &str,
        history: &[serde_json::Value],
        gen_conf: &serde_json::Value,
    ) -> std::result::Result<String, String> {
        let mut messages: Vec<ChatMessage> = Vec::new();
        if !system.trim().is_empty() {
            messages.push(ChatMessage::new("system", system.trim()));
        }
        for entry in history {
            let role = entry
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("user");
            let content = entry
                .get("content")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if content.trim().is_empty() {
                continue;
            }
            messages.push(ChatMessage::new(role, content));
        }
        if messages.is_empty() {
            return Err("harness chat called with no system prompt and no history".to_string());
        }
        if gen_conf
            .as_object()
            .map(|object| !object.is_empty())
            .unwrap_or(false)
        {
            // Stated rather than silently dropped: generation parameters (temperature, top_p)
            // are not forwarded yet, so a caller that sets them gets the model defaults.
            tracing::debug!(
                model = %self.client.model_selector(),
                "HarnessChat generation parameters are not forwarded yet; using model defaults"
            );
        }
        self.client
            .chat(&messages)
            .await
            .map_err(|error| error.to_string())
    }

    /// `chat_mdl.max_length` — the ported generators use it to trim history.
    fn max_length(&self) -> usize {
        self.client.max_context_tokens()
    }

    fn model_name(&self) -> String {
        self.client.model_selector()
    }
}


