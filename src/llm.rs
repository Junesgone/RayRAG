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
    pub created_at: u64,
    pub updated_at: u64,
}

fn default_conversation_source() -> String {
    "chat".into()
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
            client: reqwest::Client::new(),
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
    if model.to_ascii_lowercase().contains("qwen3") {
        body["extra_body"] = serde_json::json!({"enable_thinking": false});
        body["chat_template_kwargs"] = serde_json::json!({"enable_thinking": false});
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
    if !has_tool_calls {
        if let Some(reasoning) = message
            .get("reasoning_content")
            .and_then(serde_json::Value::as_str)
            .or_else(|| message.get("reasoning").and_then(serde_json::Value::as_str))
            .filter(|value| !value.is_empty())
        {
            content = format!("<think>{reasoning}</think>") + &content;
        }
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
            kb_ids,
            chat_model,
            embedding_model,
            messages: vec![],
            duration_ms: 0,
            created_at: now,
            updated_at: now,
        };
        self.mutate(|convs| {
            convs.insert(conv.id.clone(), conv.clone());
            Ok(conv)
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
            kb_ids,
            chat_model: None,
            embedding_model: None,
            messages: Vec::new(),
            duration_ms: 0,
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
                conversation.owner_id == owner_id && conversation.source == "chat"
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
