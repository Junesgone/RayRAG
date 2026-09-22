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
        }
    }
}

/// LLM client for OpenAI-compatible APIs.
#[derive(Clone)]
pub struct LlmClient {
    config: LlmConfig,
    client: reqwest::Client,
}

impl LlmClient {
    /// Create a new LLM client.
    pub fn new(config: LlmConfig) -> Self {
        Self {
            config,
            client: crate::common::cmd_timeout::http_client(),
        }
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
        let public_messages: Vec<_> = messages
            .iter()
            .map(|message| {
                serde_json::json!({
                    "role": message.role,
                    "content": message.content,
                })
            })
            .collect();
        let body = build_chat_request_body(&self.config.model, public_messages, generation);

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
        let mut body = serde_json::json!({
            "model": self.config.model,
            "messages": messages
                .iter()
                .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
                .collect::<Vec<_>>(),
            "stream": true,
        });
        let generation = self.config.generation.merged(patch);
        body["max_tokens"] = serde_json::json!(generation.max_tokens);
        body["temperature"] = serde_json::json!(generation.temperature);
        body["top_p"] = serde_json::json!(generation.top_p);
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
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(data)
                        && let Some(delta) = value
                            .pointer("/choices/0/delta/content")
                            .and_then(|content| content.as_str())
                    {
                        full.push_str(delta);
                        on_chunk(delta);
                    }
                }
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
        let public_messages = messages
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut body = build_chat_request_body(&self.config.model, public_messages, generation);
        if !tools.is_empty() {
            let object = body
                .as_object_mut()
                .expect("the static chat request body is an object");
            object.insert("tools".into(), serde_json::Value::Array(tools.to_vec()));
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
        Ok(resp.bytes().await?.to_vec())
    }

    /// Backward-compatible content-only adapter.
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
    model: &str,
    messages: Vec<serde_json::Value>,
    generation: GenerationParams,
) -> serde_json::Value {
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
    body
}

fn parse_openai_chat_completion(json: &serde_json::Value) -> Result<ChatCompletion> {
    let mut content = json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("(empty response)")
        .to_string();
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

#[cfg(test)]
mod conversation_store_tests {
    use super::*;

    #[test]
    fn completion_payload_serializes_all_generation_parameters() {
        let generation = GenerationParams::default().merged(GenerationParamsPatch {
            temperature: Some(0.8),
            top_p: Some(0.9),
            frequency_penalty: Some(0.2),
            presence_penalty: Some(0.3),
            max_tokens: Some(2048),
            reasoning: None,
        });
        let body = build_chat_request_body(
            "chat-model",
            vec![serde_json::json!({"role": "user", "content": "hello"})],
            generation,
        );
        assert_eq!(body["model"], "chat-model");
        for (key, expected) in [
            ("temperature", 0.8),
            ("top_p", 0.9),
            ("frequency_penalty", 0.2),
            ("presence_penalty", 0.3),
        ] {
            let actual = body[key].as_f64().unwrap();
            assert!((actual - expected).abs() < 1e-6, "{key}: {actual}");
        }
        assert_eq!(body["max_tokens"], 2048);
    }

    #[test]
    fn failed_persistence_rolls_back_conversation_mutations() {
        let root =
            std::env::temp_dir().join(format!("rayrag-conv-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = store.create("First").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .add_message(&conversation.id, "user", "hello")
                .is_err()
        );
        assert!(store.get(&conversation.id).unwrap().messages.is_empty());
        assert!(store.delete_for(&conversation.id, "").is_err());
        assert!(store.get(&conversation.id).is_some());
        assert!(store.create("Second").is_err());
        assert_eq!(store.list().len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn openai_completion_parses_usage_and_infers_missing_total() {
        let exact = parse_openai_chat_completion(&serde_json::json!({
            "choices": [{"message": {"content": "answer"}}],
            "usage": {"prompt_tokens": 7, "completion_tokens": 3, "total_tokens": 10}
        }))
        .unwrap();
        assert_eq!(exact.content, "answer");
        assert_eq!(
            exact.usage,
            Some(TokenUsage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
            })
        );

        let inferred = parse_openai_chat_completion(&serde_json::json!({
            "choices": [{"message": {"content": "answer"}}],
            "usage": {"prompt_tokens": 4, "completion_tokens": 2}
        }))
        .unwrap();
        assert_eq!(inferred.usage.unwrap().total_tokens, 6);

        let legacy = parse_openai_chat_completion(&serde_json::json!({
            "choices": [{"message": {"content": "answer"}}]
        }))
        .unwrap();
        assert_eq!(legacy.usage, None);
    }

    #[test]
    fn openai_completion_surfaces_reasoning_content_as_think_prefix() {
        // Qwen3 / DeepSeek-R1 style: reasoning_content is prefixed <think>.
        let qwen = parse_openai_chat_completion(&serde_json::json!({
            "choices": [{"message": {
                "content": "RAG 是检索增强生成。",
                "reasoning_content": "用户询问 RAG 定义。"
            }}]
        }))
        .unwrap();
        assert_eq!(
            qwen.content,
            "<think>用户询问 RAG 定义。</think>RAG 是检索增强生成。"
        );

        // `reasoning` alias (Kimi-K2.5 style) is honoured too.
        let kimi = parse_openai_chat_completion(&serde_json::json!({
            "choices": [{"message": {
                "content": "答案",
                "reasoning": "思考过程"
            }}]
        }))
        .unwrap();
        assert_eq!(kimi.content, "<think>思考过程</think>答案");

        // Empty reasoning must not wrap an empty <think></think>.
        let empty = parse_openai_chat_completion(&serde_json::json!({
            "choices": [{"message": {
                "content": "answer",
                "reasoning_content": ""
            }}]
        }))
        .unwrap();
        assert_eq!(empty.content, "answer");
    }

    #[test]
    fn qwen3_requests_disable_thinking_via_extra_body() {
        let generation = GenerationParams {
            max_tokens: 128,
            temperature: 0.7,
            top_p: 0.9,
            frequency_penalty: 0.0,
            presence_penalty: 0.0,
            reasoning: false,
        };
        let qwen = build_chat_request_body(
            "Qwen3-8B",
            vec![serde_json::json!({"role": "user", "content": "hi"})],
            generation,
        );
        assert_eq!(
            qwen["extra_body"]["enable_thinking"],
            serde_json::json!(false)
        );

        // Non-Qwen3 models keep the body untouched (no extra_body key).
        let plain = build_chat_request_body(
            "deepseek-chat",
            vec![serde_json::json!({"role": "user", "content": "hi"})],
            generation,
        );
        assert!(plain.get("extra_body").is_none());
    }

    #[test]
    fn openai_tool_completion_skips_think_prefix_when_calling_tools() {
        let completion = parse_openai_tool_chat_completion(&serde_json::json!({
            "choices": [{"message": {
                "content": null,
                "reasoning_content": "先搜索再回答。",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "search_my_dateset_0", "arguments": "{}"}
                }]
            }}]
        }))
        .unwrap();
        assert_eq!(completion.content, "");
        assert_eq!(completion.tool_calls.len(), 1);

        // Without tool calls the reasoning prefix is attached, matching
        // async_chat_with_tools.
        let reasoning = parse_openai_tool_chat_completion(&serde_json::json!({
            "choices": [{"message": {
                "content": "最终答案",
                "reasoning_content": "思考"
            }}]
        }))
        .unwrap();
        assert_eq!(reasoning.content, "<think>思考</think>最终答案");
        assert!(reasoning.tool_calls.is_empty());
    }

    #[test]
    fn openai_tool_completion_preserves_calls_null_content_and_usage() {
        let completion = parse_openai_tool_chat_completion(&serde_json::json!({
            "choices": [{"message": {
                "content": null,
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "search_my_dateset_0",
                        "arguments": "{\"query\":\"rust ownership\"}"
                    }
                }]
            }}],
            "usage": {"prompt_tokens": 8, "completion_tokens": 4}
        }))
        .unwrap();

        assert_eq!(completion.content, "");
        assert_eq!(completion.tool_calls.len(), 1);
        assert_eq!(completion.tool_calls[0].id, "call_1");
        assert_eq!(
            completion.tool_calls[0].function.name,
            "search_my_dateset_0"
        );
        assert_eq!(completion.usage.unwrap().total_tokens, 12);

        let assistant = ToolChatMessage::assistant(completion.tool_calls);
        let encoded = serde_json::to_value(assistant).unwrap();
        assert_eq!(encoded["role"], "assistant");
        assert!(encoded.get("content").is_none());
        assert_eq!(encoded["tool_calls"][0]["type"], "function");
    }

    #[test]
    fn malformed_tool_call_payload_is_rejected() {
        let error = parse_openai_tool_chat_completion(&serde_json::json!({
            "choices": [{"message": {
                "content": null,
                "tool_calls": [{"id": "call_1", "type": "function"}]
            }}]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("invalid LLM tool_calls payload"));
    }

    #[test]
    fn exact_usage_survives_exchange_persistence_and_regeneration() {
        let root =
            std::env::temp_dir().join(format!("rayrag-conv-token-usage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = store.create_for("owner-1", "Usage").unwrap();
        let first_usage = TokenUsage {
            prompt_tokens: 11,
            completion_tokens: 5,
            total_tokens: 16,
        };
        let message_id = store
            .append_exchange_with_settings(
                &conversation.id,
                "owner-1",
                ConversationExchange {
                    question: "question".into(),
                    answer: "answer".into(),
                    citations: Vec::new(),
                    references: Vec::new(),
                    settings: None,
                    duration_ms: 10,
                    usage: Some(first_usage),
                },
            )
            .unwrap()
            .unwrap();
        drop(store);

        let restored = ConvStore::new(path.to_str().unwrap()).unwrap();
        let assistant = &restored.get(&conversation.id).unwrap().messages[1];
        assert_eq!(assistant.usage, Some(first_usage));
        let replacement_usage = TokenUsage {
            prompt_tokens: 13,
            completion_tokens: 7,
            total_tokens: 20,
        };
        assert!(
            restored
                .replace_assistant_message(
                    &conversation.id,
                    "owner-1",
                    &message_id,
                    AssistantMessageReplacement {
                        expected_prior_answer: "answer".into(),
                        answer: "replacement".into(),
                        citations: Vec::new(),
                        references: Vec::new(),
                        kb_ids: Vec::new(),
                        chat_model: None,
                        embedding_model: None,
                        usage: Some(replacement_usage),
                    },
                )
                .unwrap()
        );
        assert_eq!(
            restored.get(&conversation.id).unwrap().messages[1].usage,
            Some(replacement_usage)
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn owner_scoping_and_exchange_survive_restart() {
        let root = std::env::temp_dir().join(format!("rayrag-conv-owner-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = store.create_for("owner-1", "First").unwrap();

        assert!(store.get_for(&conversation.id, "owner-2").is_none());
        assert!(
            store
                .append_exchange(
                    &conversation.id,
                    "owner-1",
                    "question",
                    "answer",
                    vec!["source".into()],
                )
                .unwrap()
        );
        drop(store);

        let restored = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = restored.get_for(&conversation.id, "owner-1").unwrap();
        assert_eq!(conversation.messages.len(), 2);
        assert_eq!(
            conversation.messages[1].citations.as_deref(),
            Some(&["source".to_string()][..])
        );
        assert_eq!(restored.count_for("owner-1"), 1);
        assert!(restored.list_for("owner-2").is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tenant_scoping_includes_all_tenant_sessions_without_cross_tenant_records() {
        let root =
            std::env::temp_dir().join(format!("rayrag-conv-tenant-scope-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let alice = store
            .create_for_tenant_settings("alice", "shared", "Alice shared", Vec::new(), None, None)
            .unwrap();
        let bob = store
            .create_for_tenant_settings("bob", "shared", "Bob shared", Vec::new(), None, None)
            .unwrap();
        store.create_for("alice", "Alice private").unwrap();

        let shared = store.list_for_tenant("shared");
        assert_eq!(shared.len(), 2);
        assert!(shared.iter().any(|entry| entry.id == alice.id));
        assert!(shared.iter().any(|entry| entry.id == bob.id));
        assert_eq!(store.list_for_tenant("alice").len(), 1);
        assert!(store.list_for_tenant("missing").is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn agent_sessions_persist_and_are_isolated_by_owner_tenant_canvas_and_source() {
        let root =
            std::env::temp_dir().join(format!("rayrag-conv-agent-scope-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let agent = store
            .create_agent_for("member", "tenant-a", "canvas-a", "Agent", vec![])
            .unwrap();
        store
            .create_agent_for("member", "tenant-a", "canvas-b", "Other", vec![])
            .unwrap();
        store
            .create_agent_for("member", "tenant-b", "canvas-a", "Foreign", vec![])
            .unwrap();
        store
            .create_for_tenant_settings("member", "tenant-a", "Chat", vec![], None, None)
            .unwrap();

        assert!(
            store
                .get_agent_for(&agent.id, "member", "tenant-a", "canvas-a")
                .is_some()
        );
        assert!(
            store
                .get_agent_for(&agent.id, "other", "tenant-a", "canvas-a")
                .is_none()
        );
        assert!(
            store
                .get_agent_for(&agent.id, "member", "tenant-b", "canvas-a")
                .is_none()
        );
        assert!(
            store
                .get_agent_for(&agent.id, "member", "tenant-a", "canvas-b")
                .is_none()
        );
        assert_eq!(
            store
                .list_for_tenant_source("tenant-a", "agent", Some("canvas-a"))
                .len(),
            1
        );
        assert_eq!(
            store.list_for_tenant_source("tenant-a", "chat", None).len(),
            1
        );
        drop(store);

        let restored = ConvStore::new(path.to_str().unwrap()).unwrap();
        let restored = restored.get(&agent.id).unwrap();
        assert_eq!(restored.source, "agent");
        assert_eq!(restored.canvas_id.as_deref(), Some("canvas-a"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_exchange_persistence_rolls_back_both_messages() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-conv-exchange-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = store.create_for("owner-1", "First").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .append_exchange(&conversation.id, "owner-1", "question", "answer", vec![],)
                .is_err()
        );
        assert!(store.get(&conversation.id).unwrap().messages.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_messages_gain_ids_and_feedback_defaults_on_load() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-conv-legacy-message-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!([{
                "id": "legacy-conversation",
                "name": "Legacy",
                "owner_id": "owner-1",
                "messages": [{
                    "role": "assistant",
                    "content": "legacy answer",
                    "citations": ["source"]
                }],
                "created_at": 1,
                "updated_at": 1
            }]))
            .unwrap(),
        )
        .unwrap();

        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let message = &store.get("legacy-conversation").unwrap().messages[0];
        assert!(!message.id.is_empty());
        assert!(message.created_at > 0);
        assert!(message.references.is_empty());
        assert_eq!(message.thumbup, None);
        assert_eq!(message.feedback, None);
        assert_eq!(message.usage, None);
        let conversation = store.get("legacy-conversation").unwrap();
        assert_eq!(conversation.tenant_id, "owner-1");
        assert_eq!(conversation.source, "chat");
        assert_eq!(conversation.canvas_id, None);
        assert!(conversation.kb_ids.is_empty());
        assert_eq!(conversation.chat_model, None);
        assert_eq!(conversation.embedding_model, None);
        let persisted: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted[0]["tenant_id"], "owner-1");
        assert_eq!(persisted[0]["source"], "chat");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn structured_references_and_feedback_survive_restart() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-conv-feedback-restart-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = store.create_for("owner-1", "Feedback").unwrap();
        let reference = ChunkReference {
            id: "chunk-a".into(),
            kb_id: "kb-a".into(),
            content: "water quality".into(),
            similarity: Some(0.9),
            vector_similarity: Some(0.8),
            term_similarity: Some(0.7),
        };
        let message_id = store
            .append_exchange_with_references(
                &conversation.id,
                "owner-1",
                "question",
                "answer",
                vec![reference.content.clone()],
                vec![reference.clone()],
            )
            .unwrap()
            .unwrap();
        assert!(
            store
                .update_message_feedback(
                    &conversation.id,
                    "owner-1",
                    &message_id,
                    None,
                    false,
                    Some("not relevant".into()),
                )
                .unwrap()
        );
        drop(store);

        let restored = ConvStore::new(path.to_str().unwrap()).unwrap();
        let target = restored
            .feedback_target(&conversation.id, "owner-1", &message_id)
            .unwrap();
        assert_eq!(target.prior_thumb, Some(false));
        assert_eq!(target.prior_feedback.as_deref(), Some("not relevant"));
        assert_eq!(target.references, vec![reference]);
        assert!(
            restored
                .update_message_feedback(
                    &conversation.id,
                    "owner-1",
                    &message_id,
                    Some(false),
                    false,
                    Some("not relevant".into()),
                )
                .unwrap()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn feedback_compare_and_set_rejects_stale_prior_state() {
        let root =
            std::env::temp_dir().join(format!("rayrag-conv-feedback-cas-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = store.create_for("owner-1", "Feedback").unwrap();
        let message_id = store
            .append_exchange_with_references(
                &conversation.id,
                "owner-1",
                "question",
                "answer",
                Vec::new(),
                Vec::new(),
            )
            .unwrap()
            .unwrap();
        store
            .update_message_feedback(&conversation.id, "owner-1", &message_id, None, true, None)
            .unwrap();
        let error = store
            .update_message_feedback(
                &conversation.id,
                "owner-1",
                &message_id,
                None,
                false,
                Some("stale".into()),
            )
            .unwrap_err();
        assert!(error.to_string().contains("concurrently"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn conversation_lifecycle_rename_regenerate_and_delete_pair_are_atomic() {
        let root =
            std::env::temp_dir().join(format!("rayrag-conv-lifecycle-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = store.create_for("owner-1", " Old ").unwrap();
        let message_id = store
            .append_exchange_with_references(
                &conversation.id,
                "owner-1",
                "How is pond oxygen?",
                "Old answer",
                vec!["old citation".into()],
                vec![ChunkReference {
                    id: "chunk-old".into(),
                    kb_id: "kb-a".into(),
                    content: "old context".into(),
                    similarity: Some(0.5),
                    vector_similarity: Some(0.5),
                    term_similarity: None,
                }],
            )
            .unwrap()
            .unwrap();
        store
            .update_message_feedback(
                &conversation.id,
                "owner-1",
                &message_id,
                None,
                false,
                Some("old feedback".into()),
            )
            .unwrap();

        let renamed = store
            .rename_for(&conversation.id, "owner-1", "  Oxygen Session  ")
            .unwrap()
            .unwrap();
        assert_eq!(renamed.name, "Oxygen Session");
        assert!(
            store
                .rename_for(&conversation.id, "owner-2", "Forbidden")
                .unwrap()
                .is_none()
        );

        let target = store
            .regeneration_target(&conversation.id, "owner-1", &message_id)
            .unwrap();
        assert_eq!(target.question, "How is pond oxygen?");
        assert_eq!(target.prior_answer, "Old answer");
        assert!(target.history.is_empty());
        assert!(
            store
                .replace_assistant_message(
                    &conversation.id,
                    "owner-1",
                    &message_id,
                    AssistantMessageReplacement {
                        expected_prior_answer: "Old answer".into(),
                        answer: "New answer".into(),
                        citations: vec!["new citation".into()],
                        references: vec![ChunkReference {
                            id: "chunk-new".into(),
                            kb_id: "kb-a".into(),
                            content: "new context".into(),
                            similarity: Some(0.9),
                            vector_similarity: Some(0.9),
                            term_similarity: None,
                        }],
                        kb_ids: vec!["kb-a".into()],
                        chat_model: Some("chat-a".into()),
                        embedding_model: Some("embed-a".into()),
                        usage: None,
                    },
                )
                .unwrap()
        );
        let regenerated = store.get_for(&conversation.id, "owner-1").unwrap();
        assert_eq!(regenerated.messages.len(), 2);
        assert_eq!(regenerated.messages[0].id, message_id);
        assert_eq!(regenerated.messages[1].id, message_id);
        assert_eq!(regenerated.messages[1].content, "New answer");
        assert_eq!(regenerated.messages[1].thumbup, None);
        assert_eq!(regenerated.messages[1].feedback, None);
        assert_eq!(regenerated.kb_ids, vec!["kb-a"]);
        assert_eq!(regenerated.chat_model.as_deref(), Some("chat-a"));
        assert_eq!(regenerated.embedding_model.as_deref(), Some("embed-a"));

        let error = store
            .replace_assistant_message(
                &conversation.id,
                "owner-1",
                &message_id,
                AssistantMessageReplacement {
                    expected_prior_answer: "Old answer".into(),
                    answer: "Stale overwrite".into(),
                    citations: Vec::new(),
                    references: Vec::new(),
                    kb_ids: Vec::new(),
                    chat_model: None,
                    embedding_model: None,
                    usage: None,
                },
            )
            .unwrap_err();
        assert!(error.to_string().contains("concurrently"));

        let emptied = store
            .delete_message_pair_for(&conversation.id, "owner-1", &message_id)
            .unwrap()
            .unwrap();
        assert!(emptied.messages.is_empty());
        assert!(!store.delete_for(&conversation.id, "owner-2").unwrap());
        assert!(store.delete_for(&conversation.id, "owner-1").unwrap());
        assert!(store.get(&conversation.id).is_none());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn lifecycle_persistence_failure_restores_previous_conversation() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-conv-lifecycle-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("conversations.json");
        let store = ConvStore::new(path.to_str().unwrap()).unwrap();
        let conversation = store.create_for("owner-1", "Original").unwrap();
        let message_id = store
            .append_exchange_with_references(
                &conversation.id,
                "owner-1",
                "Question",
                "Answer",
                Vec::new(),
                Vec::new(),
            )
            .unwrap()
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .rename_for(&conversation.id, "owner-1", "Changed")
                .is_err()
        );
        assert_eq!(store.get(&conversation.id).unwrap().name, "Original");
        assert!(
            store
                .replace_assistant_message(
                    &conversation.id,
                    "owner-1",
                    &message_id,
                    AssistantMessageReplacement {
                        expected_prior_answer: "Answer".into(),
                        answer: "Changed answer".into(),
                        citations: Vec::new(),
                        references: Vec::new(),
                        kb_ids: Vec::new(),
                        chat_model: None,
                        embedding_model: None,
                        usage: None,
                    },
                )
                .is_err()
        );
        assert_eq!(
            store.get(&conversation.id).unwrap().messages[1].content,
            "Answer"
        );
        assert!(
            store
                .delete_message_pair_for(&conversation.id, "owner-1", &message_id)
                .is_err()
        );
        assert_eq!(store.get(&conversation.id).unwrap().messages.len(), 2);
        std::fs::remove_dir_all(root).ok();
    }
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
    let _ = model;
    let base = resolve_base_url(provider, api_base, "http://127.0.0.1:8899");
    let mut reranker = crate::rerank::RemoteReranker::new(&base);
    if !api_key.is_empty() {
        reranker = reranker.with_api_key(api_key);
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

#[cfg(test)]
mod model_factory_tests {
    use super::*;

    #[test]
    fn model_kind_module_names_match_ragflow() {
        assert_eq!(ModelKind::Chat.module_name(), "chat_model");
        assert_eq!(ModelKind::Embedding.module_name(), "embedding_model");
        assert_eq!(ModelKind::Rerank.module_name(), "rerank_model");
        assert_eq!(ModelKind::Ocr.module_name(), "ocr_model");
        assert_eq!(ModelKind::Cv.module_name(), "cv_model");
        assert_eq!(ModelKind::Seq2txt.module_name(), "sequence2txt_model");
        assert_eq!(ModelKind::Tts.module_name(), "tts_model");
        assert_eq!(MODEL_KINDS.len(), 7);
    }

    #[test]
    fn chat_model_for_returns_working_client() {
        let chat = chat_model_for("OpenAI", "sk-test", "gpt-4o", None);
        assert_eq!(chat.model_name(), "gpt-4o");
        assert_eq!(chat.max_length(), 8192);
        assert!(model_kind_supported(ModelKind::Chat, "OpenAI"));
    }

    #[tokio::test]
    async fn llm_client_chat_model_forwards_generation_patch_and_prompt_envelope() {
        use axum::{Json, Router, routing::post};
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let requests: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().await.push(body);
                    Json(serde_json::json!({
                        "choices": [{"message": {"content": "extracted"}}],
                        "usage": {"total_tokens": 3}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = LlmClient::new(LlmConfig {
            api_base: format!("http://{addr}/v1"),
            api_key: "key".into(),
            model: "memory-model".into(),
            ..LlmConfig::default()
        });
        let answer = client
            .chat_with_generation(
                "memory system",
                &[ChatMessage::new("user", "memory history")],
                GenerationParamsPatch {
                    temperature: Some(0.73),
                    ..GenerationParamsPatch::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(answer, "extracted");

        let requests = requests.lock().await;
        let body = &requests[0];
        assert!(
            (body["temperature"].as_f64().unwrap() - 0.73).abs() < 1e-6,
            "temperature patch must be serialized into the provider request: {body}"
        );
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][0]["content"], "memory system");
        assert_eq!(body["messages"][1]["role"], "user");
        assert_eq!(body["messages"][1]["content"], "memory history");
    }

    #[test]
    fn embedding_model_for_rejects_builtin_without_path() {
        let err = embedding_model_for("Builtin", "", "bge-m3", None);
        assert!(err.is_err());
        let ok = embedding_model_for("OpenAI", "sk-test", "text-embedding-3-small", None);
        assert!(ok.is_ok());
    }

    #[test]
    fn rerank_model_for_returns_remote_reranker() {
        let reranker = rerank_model_for("Cohere", "key", "rerank-model", None);
        assert!(reranker.is_ok());
    }

    #[test]
    fn ocr_factory_returns_explicit_error_stub() {
        let ocr = ocr_model_for("MinerU", "", "mineru", None);
        assert_eq!(ocr.model_name(), "mineru");
        assert!(!model_kind_supported(ModelKind::Ocr, "MinerU"));
        let err = tokio_test_block_on(ocr.parse_pdf("/tmp/nonexistent.pdf"));
        assert!(err.is_err());
        let msg = format!("{}", err.unwrap_err());
        assert!(msg.contains("ocr"), "stub error mentions kind: {msg}");
    }

    #[test]
    fn cv_model_builds_vision_payload() {
        let client = LlmClient::new(LlmConfig {
            api_base: "https://api.openai.com/v1".into(),
            api_key: "k".into(),
            model: "gpt-4o".into(),
            ..LlmConfig::default()
        });
        let cv = OpenAiCvModel::new(client, "Chinese");
        assert_eq!(cv.model_name(), "gpt-4o");
        let msgs = cv.vision_messages("描述", "QUJD");
        assert_eq!(msgs[0]["content"][1]["type"], "image_url");
        assert_eq!(
            msgs[0]["content"][1]["image_url"]["url"],
            "data:image/png;base64,QUJD"
        );
    }

    #[test]
    fn vision_describe_prompt_constant_is_ragflow_pdf_transcription() {
        // Port of `vision_llm_describe_prompt.md` — the default prompt of
        // RAGFlow `cv_model.Base.describe` (GptV4 family).
        assert!(VISION_LLM_DESCRIBE_PROMPT.contains("PDF page image"));
        assert!(VISION_LLM_DESCRIBE_PROMPT.contains("clean Markdown"));
        assert!(VISION_LLM_DESCRIBE_PROMPT.contains("return an empty string"));
    }

    #[tokio::test]
    async fn describe_figure_with_context_replaces_placeholders_and_falls_back() {
        use axum::{Json, Router, routing::post};
        use std::sync::Arc;
        use tokio::sync::Mutex;

        // Mock server that records the text prompt of the vision request.
        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = prompts.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |body: Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    let text = body["messages"][0]["content"][0]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    captured.lock().await.push(text);
                    Json(serde_json::json!({
                        "choices": [{"message": {"content": "figure text"}}],
                        "usage": {"total_tokens": 7}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = LlmClient::new(LlmConfig {
            api_base: format!("http://{addr}/v1"),
            api_key: "k".into(),
            model: "gpt-4o".into(),
            ..LlmConfig::default()
        });
        let cv = OpenAiCvModel::new(client, "Chinese");

        // With context: the with-context prompt with placeholders replaced.
        let out = cv
            .describe_figure_with_context(b"img", "上文", "下文")
            .await
            .unwrap();
        assert_eq!(out, "figure text");
        let sent = prompts.lock().await;
        let with_context = &sent[0];
        assert!(with_context.contains("{{ context_above }}") == false);
        assert!(with_context.contains("上文"));
        assert!(with_context.contains("下文"));
        assert!(with_context.contains("expert visual data analyst"));
        drop(sent);

        // Without context: the plain figure prompt, no placeholders left.
        let _ = cv
            .describe_figure_with_context(b"img", "", "")
            .await
            .unwrap();
        let sent = prompts.lock().await;
        let plain = &sent[1];
        assert!(!plain.contains("{{ context_above }}"));
        assert!(plain.contains("## ROLE"));
        assert!(!plain.contains("上文"));
    }

    #[tokio::test]
    async fn describe_page_uses_full_transcription_prompt() {
        use axum::{Json, Router, routing::post};
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let prompts: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured = prompts.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |body: Json<serde_json::Value>| {
                let captured = captured.clone();
                async move {
                    let text = body["messages"][0]["content"][0]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                    captured.lock().await.push(text);
                    Json(serde_json::json!({
                        "choices": [{"message": {"content": "transcribed page"}}],
                        "usage": {"total_tokens": 9}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = LlmClient::new(LlmConfig {
            api_base: format!("http://{addr}/v1"),
            api_key: "k".into(),
            model: "gpt-4o".into(),
            ..LlmConfig::default()
        });
        let cv = OpenAiCvModel::new(client, "Chinese");
        let out = cv.describe_page(b"page-image").await.unwrap();
        assert_eq!(out, "transcribed page");
        let sent = prompts.lock().await;
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], VISION_LLM_DESCRIBE_PROMPT);
        // Image part still travels as an image_url data URL.
    }

    #[test]
    fn base64_encode_matches_rfc4648() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }
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
