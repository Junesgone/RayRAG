//! Joint service layer — ported from RAGFlow `api/db/joint_services/`:
//!
//! - `memory_message_service.py`: message-level memory read/write — raw
//!   message construction, size accounting, FIFO eviction and queries.
//! - `tenant_model_service.py`: tenant model binding — resolve a tenant model
//!   by id, by type+name, and the tenant's default model per capability.
//! - `user_account_service.py`: user account lifecycle — the
//!   `create_new_user` record plan (user + tenant + owner relation + root file
//!   folder + initial tenant-LLM bindings) and the `delete_user_data`
//!   guard/step semantics.
//!
//! RayRAG stores these records in its own JSON/Postgres stores, so the ports
//! below are the *semantics*: pure decision functions plus a message store
//! that mirrors the existing `MemoryStore` persistence pattern.

use crate::api::common::ApiError;
use crate::api::tenant_models::TenantModelInstance;
use crate::generation_params::GenerationParamsPatch;
use crate::llm::{ChatMessage, ChatModel};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Mutex, RwLock};

// ── Shared enums (common/constants.py, api/db/__init__.py) ────────────────

/// `common/constants.py::LLMType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LLMType {
    Chat,
    Embedding,
    Speech2Text,
    Image2Text,
    Rerank,
    Tts,
    Ocr,
}

impl LLMType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Embedding => "embedding",
            Self::Speech2Text => "speech2text",
            Self::Image2Text => "image2text",
            Self::Rerank => "rerank",
            Self::Tts => "tts",
            Self::Ocr => "ocr",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "chat" => Some(Self::Chat),
            "embedding" => Some(Self::Embedding),
            "speech2text" => Some(Self::Speech2Text),
            "image2text" => Some(Self::Image2Text),
            "rerank" => Some(Self::Rerank),
            "tts" => Some(Self::Tts),
            "ocr" => Some(Self::Ocr),
            _ => None,
        }
    }
}

/// `api/db/__init__.py::UserTenantRole`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserTenantRole {
    Owner,
    Admin,
    Normal,
    Invite,
}

impl UserTenantRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Normal => "normal",
            Self::Invite => "invite",
        }
    }
}

// ── MemoryMessageService (memory_message_service.py + memory/services/messages.py) ──

/// CPython `sys.getsizeof(str)` object overhead used by
/// `MessageService.calculate_message_size` (49 bytes for a short str).
pub const MESSAGE_CONTENT_OVERHEAD: usize = 49;
/// CPython `sys.getsizeof(float)` — per embedding dimension.
pub const MESSAGE_EMBEDDING_DIM_SIZE: usize = 24;
const MEMORY_BUDGET_ERROR: &str = "Failed to insert message into memory. Memory size reached limit and cannot decide which to delete.";

/// `common/constants.py::MemoryType` bit flags.
pub mod memory_type {
    pub const RAW: u8 = 0b0001;
    pub const SEMANTIC: u8 = 0b0010;
    pub const EPISODIC: u8 = 0b0100;
    pub const PROCEDURAL: u8 = 0b1000;

    /// Lowercase names of the set bits (RAGFlow stores `message_type` as
    /// `MemoryType.<X>.name.lower()`).
    pub fn names(bits: u8) -> Vec<&'static str> {
        let mut names = Vec::new();
        if bits & RAW != 0 {
            names.push("raw");
        }
        if bits & SEMANTIC != 0 {
            names.push("semantic");
        }
        if bits & EPISODIC != 0 {
            names.push("episodic");
        }
        if bits & PROCEDURAL != 0 {
            names.push("procedural");
        }
        names
    }
}

/// A single memory message row (RAGFlow `memory/services/messages.py`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryMessage {
    pub message_id: i64,
    /// `"raw"` | `"semantic"` | `"episodic"` | `"procedural"`.
    pub message_type: String,
    /// Raw message id this extracted message derives from (0 for raw rows).
    pub source_id: i64,
    pub memory_id: String,
    pub user_id: String,
    pub agent_id: String,
    pub session_id: String,
    pub content: String,
    /// `yyyy-MM-dd HH:mm:ss` (lexicographic order == chronological order).
    pub valid_at: String,
    pub invalid_at: Option<String>,
    pub forget_at: Option<String>,
    pub status: bool,
    /// Fixed Infinity `zone_id` column; RAGFlow currently relies on its zero
    /// default but it remains part of the physical storage contract.
    #[serde(default)]
    pub zone_id: i64,
    #[serde(default, skip_serializing)]
    pub content_embed: Vec<f32>,
}

impl MemoryMessage {
    /// `MessageService.calculate_message_size`:
    /// `sys.getsizeof(content) + sys.getsizeof(content_embed[0]) * len(content_embed)`.
    pub fn calculate_size(&self) -> usize {
        self.content.len()
            + MESSAGE_CONTENT_OVERHEAD
            + MESSAGE_EMBEDDING_DIM_SIZE * self.content_embed.len()
    }

    /// RAGFlow/Infinity physical document id.
    pub fn storage_id(&self) -> String {
        format!("{}_{}", self.memory_id, self.message_id)
    }
}

/// Filters applied by the Infinity-compatible message query path.
pub struct MemoryMessageQuery<'a> {
    pub memory_ids: &'a [String],
    pub agent_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub user_id: Option<&'a str>,
    pub status: Option<bool>,
    pub top_n: Option<usize>,
    pub hide_forgotten: bool,
}

/// Inputs for the in-process full-text + vector replacement search.
pub struct MemoryMessageSearch<'a> {
    pub memory_ids: &'a [String],
    pub agent_id: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub user_id: Option<&'a str>,
    pub question: &'a str,
    pub query_vector: &'a [f32],
    pub similarity_threshold: f64,
    pub keywords_similarity_weight: f64,
    pub top_n: usize,
}

/// `memory_message_service.py::save_to_memory` raw message content format:
/// `"User Input: {user_input}\nAgent Response: {agent_response}"`.
pub fn build_raw_message_content(user_input: &str, agent_response: &str) -> String {
    format!("User Input: {user_input}\nAgent Response: {agent_response}")
}

// ── Memory extraction (memory/utils/prompt_util.py + msg_util.py) ────────

/// A flattened item returned by RAGFlow's
/// `memory_message_service.extract_by_llm` list comprehension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractedMemory {
    pub content: String,
    pub valid_at: String,
    pub invalid_at: Option<String>,
    /// The LLM response's top-level key. Unknown keys are intentionally kept,
    /// matching the fixed Python service's lack of post-response filtering.
    pub message_type: String,
}

/// RAGFlow `get_json_result_from_llm_response` followed by
/// `extract_by_llm`'s flattening list comprehension.
///
/// JSON syntax errors are the fixed service's soft-empty case. Once valid JSON
/// is present, an incompatible shape is a worker error rather than an empty
/// extraction result.
pub fn parse_memory_extraction_response(response: &str) -> anyhow::Result<Vec<ExtractedMemory>> {
    let value = crate::memory::get_json_result_from_llm_response(response);
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("memory extraction response must be a JSON object"))?;
    let mut extracted = Vec::new();
    for (message_type, items) in object {
        let items = items.as_array().ok_or_else(|| {
            anyhow::anyhow!("memory extraction field {message_type:?} must be an array")
        })?;
        for (index, item) in items.iter().enumerate() {
            let item = item.as_object().ok_or_else(|| {
                anyhow::anyhow!(
                    "memory extraction field {message_type:?} item {index} must be an object"
                )
            })?;
            let content = required_memory_extraction_string(item, message_type, index, "content")?;
            let valid_at =
                required_memory_extraction_string(item, message_type, index, "valid_at")?;
            let invalid_at = match item.get("invalid_at") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) if value.is_empty() => None,
                Some(Value::String(value)) => {
                    Some(crate::common::time_utils::format_iso_8601_to_ymd_hms(value))
                }
                Some(_) => {
                    anyhow::bail!(
                        "memory extraction field {message_type:?} item {index} field \"invalid_at\" must be a string or null"
                    )
                }
            };
            extracted.push(ExtractedMemory {
                content: content.to_owned(),
                valid_at: crate::common::time_utils::format_iso_8601_to_ymd_hms(valid_at),
                invalid_at,
                message_type: message_type.clone(),
            });
        }
    }
    Ok(extracted)
}

/// Fixed-v0.26.4 Memory LLM extraction call, separated from persistence so the
/// API/task worker can later batch-embed and save the returned items.
#[allow(clippy::too_many_arguments)]
pub async fn extract_memory_by_llm(
    model: &dyn ChatModel,
    memory_types: &[String],
    user_input: &str,
    agent_response: &str,
    system_prompt: Option<&str>,
    user_prompt: Option<&str>,
    conversation_time: &str,
    temperature: f32,
) -> anyhow::Result<Vec<ExtractedMemory>> {
    if !memory_types
        .iter()
        .any(|kind| matches!(kind.as_str(), "semantic" | "episodic" | "procedural"))
    {
        return Ok(Vec::new());
    }

    let default_system;
    let system = match system_prompt.filter(|prompt| !prompt.is_empty()) {
        Some(prompt) => prompt,
        None => {
            default_system = crate::memory::PromptAssembler::assemble_system_prompt(memory_types);
            &default_system
        }
    };
    let conversation = build_raw_message_content(user_input, agent_response);
    let history = match user_prompt.filter(|prompt| !prompt.is_empty()) {
        Some(prompt) => vec![
            ChatMessage::new("user", prompt),
            ChatMessage::new(
                "user",
                format!(
                    "Conversation: {conversation}\nConversation Time: {conversation_time}\nCurrent Time: {conversation_time}"
                ),
            ),
        ],
        None => vec![ChatMessage::new(
            "user",
            crate::memory::PromptAssembler::assemble_user_prompt(
                &conversation,
                Some(conversation_time),
                Some(conversation_time),
            ),
        )],
    };
    let response = model
        .chat_with_generation(
            system,
            &history,
            GenerationParamsPatch {
                temperature: Some(temperature),
                ..GenerationParamsPatch::default()
            },
        )
        .await?;
    parse_memory_extraction_response(&response)
}

fn required_memory_extraction_string<'a>(
    item: &'a Map<String, Value>,
    message_type: &str,
    index: usize,
    field: &str,
) -> anyhow::Result<&'a str> {
    item.get(field).and_then(Value::as_str).ok_or_else(|| {
        anyhow::anyhow!(
            "memory extraction field {message_type:?} item {index} requires string field {field:?}"
        )
    })
}

/// `MessageService.pick_messages_to_delete_by_fifo`: walk messages oldest
/// first (ascending `valid_at`), accumulating size until `size_to_delete` is
/// freed. Returns the message ids to remove and the total freed size.
pub fn pick_messages_to_delete_by_fifo<'a>(
    messages: impl IntoIterator<Item = &'a MemoryMessage>,
    size_to_delete: usize,
) -> (Vec<i64>, usize) {
    let mut freed = 0usize;
    let mut ids = Vec::new();
    for message in messages {
        if freed >= size_to_delete {
            break;
        }
        freed += message.calculate_size();
        ids.push(message.message_id);
    }
    (ids, freed)
}

/// In-process message store following the `MemoryStore` persistence pattern
/// (JSON file mirror; `RAYRAG_POSTGRES_URL` snapshots via `persistence`).
pub struct MemoryMessageService {
    messages: RwLock<Vec<MemoryMessage>>,
    next_message_id: AtomicI64,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
    /// Native index bookkeeping — `messages.py::MessageService.has_index` /
    /// `create_index` / `delete_index`. The in-process engine materializes an
    /// index lazily on first save (mirroring `embed_and_save`'s
    /// `has_index → create_index` gate); rows alone are not an index, but
    /// legacy snapshots restored without bookkeeping count as indexed when
    /// they already hold rows so delete flows still reach them.
    indexes: RwLock<std::collections::HashSet<(String, String)>>,
}

impl MemoryMessageService {
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let messages: Vec<MemoryMessage> = if path.exists() {
            decode_message_snapshot(&std::fs::read(&path)?)?
        } else {
            Vec::new()
        };
        let max_id = messages
            .iter()
            .map(|message| message.message_id)
            .max()
            .unwrap_or(0);
        let service = Self {
            messages: RwLock::new(messages),
            // `init_message_id_sequence`: seed with max+1 (or 1 when empty).
            next_message_id: AtomicI64::new(if max_id > 0 { max_id + 1 } else { 1 }),
            path: Some(path),
            save_lock: Mutex::new(()),
            indexes: RwLock::new(std::collections::HashSet::new()),
        };
        service.persist()?;
        Ok(service)
    }

    pub fn in_memory() -> Self {
        Self {
            messages: RwLock::new(Vec::new()),
            next_message_id: AtomicI64::new(1),
            path: None,
            save_lock: Mutex::new(()),
            indexes: RwLock::new(std::collections::HashSet::new()),
        }
    }

    /// `REDIS_CONN.generate_auto_increment_id(namespace="memory")`.
    pub fn next_message_id(&self) -> i64 {
        self.next_message_id.fetch_add(1, Ordering::SeqCst)
    }

    pub fn insert_messages(&self, messages: Vec<MemoryMessage>) -> anyhow::Result<()> {
        let highest_id = messages
            .iter()
            .map(|message| message.message_id)
            .max()
            .unwrap_or(0);
        self.mutate(|all| {
            for message in messages {
                if let Some(existing) = all.iter_mut().find(|existing| {
                    existing.memory_id == message.memory_id
                        && existing.message_id == message.message_id
                }) {
                    *existing = message;
                } else {
                    all.push(message);
                }
            }
            Ok(())
        })?;
        self.next_message_id
            .fetch_max(highest_id.saturating_add(1), Ordering::SeqCst);
        Ok(())
    }

    /// `search_message` semantics: filter (status defaults to active) and
    /// order by `valid_at` descending, capped at `top_n`.
    pub fn query(
        &self,
        memory_ids: &[String],
        agent_id: Option<&str>,
        session_id: Option<&str>,
        user_id: Option<&str>,
        status: Option<bool>,
        top_n: Option<usize>,
    ) -> Vec<MemoryMessage> {
        self.query_with_options(MemoryMessageQuery {
            memory_ids,
            agent_id,
            session_id,
            user_id,
            status: Some(status.unwrap_or(true)),
            top_n,
            hide_forgotten: true,
        })
    }

    /// Lower-level Infinity search filter. `status=None` means no status
    /// predicate; `hide_forgotten=true` applies the connector's default
    /// `NOT exists(forget_at_flt)` condition.
    pub fn query_with_options(&self, query: MemoryMessageQuery<'_>) -> Vec<MemoryMessage> {
        let mut results: Vec<_> = self
            .messages
            .read()
            .expect("message store lock poisoned")
            .iter()
            .filter(|message| query.memory_ids.contains(&message.memory_id))
            .filter(|message| query.agent_id.is_none_or(|value| message.agent_id == value))
            .filter(|message| {
                query
                    .session_id
                    .is_none_or(|value| message.session_id == value)
            })
            .filter(|message| query.user_id.is_none_or(|value| message.user_id == value))
            .filter(|message| query.status.is_none_or(|value| message.status == value))
            .filter(|message| !query.hide_forgotten || message.forget_at.is_none())
            .cloned()
            .collect();
        // Order by valid_at desc — RAGFlow's OrderByExpr(desc).
        results.sort_by(|left, right| right.valid_at.cmp(&left.valid_at));
        if let Some(top_n) = query.top_n {
            results.truncate(top_n);
        }
        results
    }

    pub fn get_max_message_id(&self, memory_ids: &[String]) -> i64 {
        self.messages
            .read()
            .expect("message store lock poisoned")
            .iter()
            .filter(|message| memory_ids.contains(&message.memory_id))
            .map(|message| message.message_id)
            .max()
            .unwrap_or(0)
    }

    /// In-process replacement for Infinity's content full-text + dense-vector
    /// weighted-sum search. Search defaults to active and non-forgotten rows.
    pub fn search_hybrid(&self, search: MemoryMessageSearch<'_>) -> Vec<MemoryMessage> {
        let candidates = self.query_with_options(MemoryMessageQuery {
            memory_ids: search.memory_ids,
            agent_id: search.agent_id,
            session_id: search.session_id,
            user_id: search.user_id,
            status: Some(true),
            top_n: None,
            hide_forgotten: true,
        });
        let (_, query_tokens) = crate::memory::MsgTextQuery::new()
            .question(search.question, search.similarity_threshold);
        let document_tokens: Vec<Vec<String>> = candidates
            .iter()
            .map(|message| {
                crate::nlp::rag_fine_grained_tokenize(&crate::nlp::rag_tokenize(&message.content))
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect()
            })
            .collect();
        let document_vectors: Vec<Vec<f32>> = candidates
            .iter()
            .map(|message| message.content_embed.clone())
            .collect();
        let keyword_weight = search.keywords_similarity_weight.clamp(0.0, 1.0);
        let (scores, _, _) = crate::nlp::TermWeightComputer::new().hybrid_similarity(
            search.query_vector,
            &document_vectors,
            &query_tokens,
            &document_tokens,
            keyword_weight,
            1.0 - keyword_weight,
        );
        let mut ranked: Vec<_> = candidates
            .into_iter()
            .zip(scores)
            .filter(|(_, score)| *score >= search.similarity_threshold)
            .collect();
        ranked.sort_by(|(left_message, left_score), (right_message, right_score)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| right_message.valid_at.cmp(&left_message.valid_at))
        });
        ranked
            .into_iter()
            .take(search.top_n)
            .map(|(message, _)| message)
            .collect()
    }

    /// `MessageService.calculate_memory_size` for one memory.
    pub fn calculate_memory_size(&self, memory_id: &str) -> usize {
        self.messages
            .read()
            .expect("message store lock poisoned")
            .iter()
            .filter(|message| message.memory_id == memory_id)
            .map(MemoryMessage::calculate_size)
            .sum()
    }

    /// Delete message ids inside one memory. Message ids normally come from a
    /// process-wide sequence, but the physical identity is the composite
    /// `{memory_id}_{message_id}` and imported/legacy snapshots may reuse an
    /// integer in another memory.
    pub fn delete_messages(&self, memory_id: &str, ids: &[i64]) -> anyhow::Result<usize> {
        self.mutate(|all| {
            let before = all.len();
            all.retain(|message| {
                message.memory_id != memory_id || !ids.contains(&message.message_id)
            });
            Ok(before - all.len())
        })
    }

    pub fn delete_by_memory(&self, memory_id: &str) -> anyhow::Result<usize> {
        self.mutate(|all| {
            let before = all.len();
            all.retain(|message| message.memory_id != memory_id);
            Ok(before - all.len())
        })
    }

    /// `memory_message_service.py::embed_and_save` size-budget block: when the
    /// new messages overflow `memory_size`, FIFO-evict the oldest messages;
    /// any other forgetting policy is an error, mirroring the RAGFlow message.
    pub fn save_messages_with_budget(
        &self,
        memory_id: &str,
        forgetting_policy: &str,
        memory_size: usize,
        new_messages: Vec<MemoryMessage>,
    ) -> Result<(), ApiError> {
        let new_msg_size: usize = new_messages.iter().map(MemoryMessage::calculate_size).sum();
        let highest_id = new_messages
            .iter()
            .map(|message| message.message_id)
            .max()
            .unwrap_or(0);
        self.mutate(|all| {
            let current_memory_size: usize = all
                .iter()
                .filter(|message| message.memory_id == memory_id)
                .map(MemoryMessage::calculate_size)
                .sum();
            let projected_size = new_msg_size.saturating_add(current_memory_size);
            if projected_size > memory_size {
                if forgetting_policy != "FIFO" {
                    anyhow::bail!(MEMORY_BUDGET_ERROR);
                }
                let size_to_delete = projected_size - memory_size;
                let mut ordered: Vec<MemoryMessage> = all
                    .iter()
                    .filter(|message| message.memory_id == memory_id)
                    .cloned()
                    .collect();
                // Infinity first physically removes rows already soft-forgotten,
                // then the oldest remaining rows by `valid_at_flt`.
                ordered.sort_by(|left, right| match (&left.forget_at, &right.forget_at) {
                    (Some(left), Some(right)) => left.cmp(right),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => left.valid_at.cmp(&right.valid_at),
                });
                let (ids, _freed) = pick_messages_to_delete_by_fifo(ordered.iter(), size_to_delete);
                all.retain(|message| {
                    message.memory_id != memory_id || !ids.contains(&message.message_id)
                });
            }
            for message in new_messages {
                if let Some(existing) = all.iter_mut().find(|existing| {
                    existing.memory_id == message.memory_id
                        && existing.message_id == message.message_id
                }) {
                    *existing = message;
                } else {
                    all.push(message);
                }
            }
            Ok(())
        })
        .map_err(|error| {
            if error.to_string() == MEMORY_BUDGET_ERROR {
                ApiError::admin(MEMORY_BUDGET_ERROR)
            } else {
                ApiError::from(error)
            }
        })?;
        self.next_message_id
            .fetch_max(highest_id.saturating_add(1), Ordering::SeqCst);
        Ok(())
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut Vec<MemoryMessage>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _guard = self.save_lock.lock().expect("save lock poisoned");
        let mut messages = self.messages.write().expect("message store lock poisoned");
        let previous = messages.clone();
        let value = match mutation(&mut messages) {
            Ok(value) => value,
            Err(error) => {
                *messages = previous;
                return Err(error);
            }
        };
        if let Err(error) = self.persist_snapshot(&messages) {
            *messages = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist(&self) -> anyhow::Result<()> {
        let _guard = self.save_lock.lock().expect("save lock poisoned");
        let snapshot = self
            .messages
            .read()
            .expect("message store lock poisoned")
            .clone();
        self.persist_snapshot(&snapshot)
    }

    fn persist_snapshot(&self, snapshot: &[MemoryMessage]) -> anyhow::Result<()> {
        let data = encode_message_snapshot(snapshot)?;
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(path, &data)
    }
}

fn encode_message_snapshot(messages: &[MemoryMessage]) -> anyhow::Result<Vec<u8>> {
    let mut ordered = messages.to_vec();
    ordered.sort_by_key(MemoryMessage::storage_id);
    let rows = ordered
        .iter()
        .map(message_to_storage_value)
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(serde_json::to_vec_pretty(&rows)?)
}

fn decode_message_snapshot(bytes: &[u8]) -> anyhow::Result<Vec<MemoryMessage>> {
    let rows: Vec<Value> = serde_json::from_slice(bytes)?;
    rows.into_iter()
        .map(|row| {
            let physical = row.as_object().is_some_and(|object| {
                object.contains_key("message_type_kwd")
                    || object.contains_key("status_int")
                    || object.keys().any(|field| vector_dimension(field).is_some())
            });
            if physical {
                storage_value_to_message(row)
            } else {
                let message: MemoryMessage = serde_json::from_value(row)?;
                validate_message(&message)?;
                Ok(message)
            }
        })
        .collect()
}

fn message_to_storage_value(message: &MemoryMessage) -> anyhow::Result<Value> {
    validate_message(message)?;
    let invalid_at = message.invalid_at.as_deref().unwrap_or_default();
    let forget_at = message.forget_at.as_deref().unwrap_or_default();
    let mut row = Map::new();
    row.insert("id".into(), Value::String(message.storage_id()));
    row.insert("message_id".into(), Value::from(message.message_id));
    row.insert(
        "message_type_kwd".into(),
        Value::String(message.message_type.clone()),
    );
    row.insert("source_id".into(), Value::from(message.source_id));
    row.insert("memory_id".into(), Value::String(message.memory_id.clone()));
    row.insert("user_id".into(), Value::String(message.user_id.clone()));
    row.insert("agent_id".into(), Value::String(message.agent_id.clone()));
    row.insert(
        "session_id".into(),
        Value::String(message.session_id.clone()),
    );
    insert_storage_date(&mut row, "valid_at", &message.valid_at)?;
    insert_storage_date(&mut row, "invalid_at", invalid_at)?;
    insert_storage_date(&mut row, "forget_at", forget_at)?;
    row.insert(
        "status_int".into(),
        Value::from(if message.status { 1 } else { 0 }),
    );
    row.insert("zone_id".into(), Value::from(message.zone_id));
    row.insert("content".into(), Value::String(message.content.clone()));
    row.insert(
        format!("q_{}_vec", message.content_embed.len()),
        serde_json::to_value(&message.content_embed)?,
    );
    Ok(Value::Object(row))
}

fn insert_storage_date(
    row: &mut Map<String, Value>,
    field: &str,
    value: &str,
) -> anyhow::Result<()> {
    row.insert(field.into(), Value::String(value.into()));
    row.insert(
        format!("{field}_flt"),
        Value::from(storage_timestamp(value)? as f64),
    );
    Ok(())
}

fn storage_value_to_message(row: Value) -> anyhow::Result<MemoryMessage> {
    let object = row
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("memory message snapshot row must be an object"))?;
    let allowed: std::collections::HashSet<&str> = crate::settings::MESSAGE_INFINITY_FIELDS
        .iter()
        .map(|field| field.name)
        .collect();
    for field in object.keys() {
        if !allowed.contains(field.as_str()) && vector_dimension(field).is_none() {
            anyhow::bail!("unexpected memory message storage field {field:?}");
        }
    }

    let memory_id = storage_string(object, "memory_id")?;
    let message_id = storage_i64(object, "message_id", 0)?;
    let id = storage_string(object, "id")?;
    let expected_id = format!("{memory_id}_{message_id}");
    if id != expected_id {
        anyhow::bail!("memory message storage id {id:?} does not match {expected_id:?}");
    }

    let valid_at = storage_date(object, "valid_at")?;
    let invalid_at = storage_optional_date(object, "invalid_at")?;
    let forget_at = storage_optional_date(object, "forget_at")?;
    let status = match storage_i64(object, "status_int", 1)? {
        0 => false,
        1 => true,
        other => anyhow::bail!("memory message status_int must be 0 or 1, got {other}"),
    };

    let mut vector = None;
    for (field, value) in object {
        let Some(dimension) = vector_dimension(field) else {
            continue;
        };
        if vector.is_some() {
            anyhow::bail!("memory message snapshot contains multiple vector fields");
        }
        let values: Vec<f32> = serde_json::from_value(value.clone())?;
        if values.len() != dimension {
            anyhow::bail!(
                "memory message vector field {field:?} declares {dimension} values but stores {}",
                values.len()
            );
        }
        vector = Some(values);
    }

    let message = MemoryMessage {
        message_id,
        message_type: storage_string(object, "message_type_kwd")?,
        source_id: storage_i64(object, "source_id", 0)?,
        memory_id,
        user_id: storage_string(object, "user_id")?,
        agent_id: storage_string(object, "agent_id")?,
        session_id: storage_string(object, "session_id")?,
        content: storage_string(object, "content")?,
        valid_at,
        invalid_at,
        forget_at,
        status,
        zone_id: storage_i64(object, "zone_id", 0)?,
        content_embed: vector.unwrap_or_default(),
    };
    validate_message(&message)?;
    Ok(message)
}

fn storage_string(object: &Map<String, Value>, field: &str) -> anyhow::Result<String> {
    match object.get(field) {
        None => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => anyhow::bail!("memory message storage field {field:?} must be a string"),
    }
}

fn storage_i64(object: &Map<String, Value>, field: &str, default: i64) -> anyhow::Result<i64> {
    match object.get(field) {
        None => Ok(default),
        Some(value) => value.as_i64().ok_or_else(|| {
            anyhow::anyhow!("memory message storage field {field:?} must be an integer")
        }),
    }
}

fn storage_date(object: &Map<String, Value>, field: &str) -> anyhow::Result<String> {
    let value = storage_string(object, field)?;
    let expected = storage_timestamp(&value)? as f64;
    let physical_field = format!("{field}_flt");
    let actual = object
        .get(&physical_field)
        .map(|value| {
            value.as_f64().ok_or_else(|| {
                anyhow::anyhow!("memory message storage field {physical_field:?} must be numeric")
            })
        })
        .transpose()?
        .unwrap_or(0.0);
    if actual != expected {
        anyhow::bail!(
            "memory message storage field {physical_field:?} is {actual}, expected {expected}"
        );
    }
    Ok(value)
}

fn storage_optional_date(
    object: &Map<String, Value>,
    field: &str,
) -> anyhow::Result<Option<String>> {
    let value = storage_date(object, field)?;
    Ok((!value.is_empty()).then_some(value))
}

fn storage_timestamp(value: &str) -> anyhow::Result<i64> {
    if value.is_empty() {
        return Ok(0);
    }
    crate::common::time_utils::date_string_to_timestamp(
        value,
        crate::common::time_utils::DEFAULT_TIME_FORMAT,
    )
}

fn vector_dimension(field: &str) -> Option<usize> {
    field
        .strip_prefix(crate::settings::MESSAGE_INFINITY_VECTOR_FIELD_PREFIX)?
        .strip_suffix(crate::settings::MESSAGE_INFINITY_VECTOR_FIELD_SUFFIX)?
        .parse()
        .ok()
}

fn validate_message(message: &MemoryMessage) -> anyhow::Result<()> {
    if message.memory_id.is_empty() {
        anyhow::bail!("memory message memory_id must not be empty");
    }
    if message.message_id <= 0 {
        anyhow::bail!("memory message message_id must be positive");
    }
    if message.content_embed.is_empty() {
        anyhow::bail!("memory message content_embed must not be empty");
    }
    storage_timestamp(&message.valid_at)?;
    storage_timestamp(message.invalid_at.as_deref().unwrap_or_default())?;
    storage_timestamp(message.forget_at.as_deref().unwrap_or_default())?;
    Ok(())
}

// ── TenantModelService (tenant_model_service.py) ───────────────────────────

/// Tenant field holding the default model name for a capability
/// (`get_tenant_default_model_by_type` match arm). OCR and unknown types are
/// handled by the caller with RAGFlow's exceptions.
pub fn default_model_field_for_type(model_type: &str) -> Option<&'static str> {
    match model_type {
        "embedding" => Some("embd_id"),
        "speech2text" => Some("asr_id"),
        "image2text" => Some("img2txt_id"),
        "chat" => Some("llm_id"),
        "rerank" => Some("rerank_id"),
        "tts" => Some("tts_id"),
        _ => None,
    }
}

/// The tenant's default model ids (`user_service.py::Tenant` columns).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TenantDefaultModels {
    pub llm_id: String,
    pub embd_id: String,
    pub asr_id: String,
    pub img2txt_id: String,
    pub rerank_id: String,
    pub tts_id: String,
}

impl TenantDefaultModels {
    /// `get_tenant_default_model_by_type` — resolve the tenant's default model
    /// name for a capability, raising RAGFlow's exceptions verbatim.
    pub fn model_name_for_type(&self, model_type: &str) -> Result<&str, ApiError> {
        match model_type {
            "embedding" => required_default(model_type, &self.embd_id),
            "speech2text" => required_default(model_type, &self.asr_id),
            "image2text" => required_default(model_type, &self.img2txt_id),
            "chat" => required_default(model_type, &self.llm_id),
            "rerank" => required_default(model_type, &self.rerank_id),
            "tts" => required_default(model_type, &self.tts_id),
            "ocr" => Err(ApiError::admin("OCR model name is required")),
            other => Err(ApiError::admin(format!("Unknown model type {other}"))),
        }
    }
}

/// `get_tenant_default_model_by_type` — a default model id that is empty means
/// `"No default {model_type} model is set."`.
fn required_default<'a>(model_type: &str, field: &'a str) -> Result<&'a str, ApiError> {
    if field.is_empty() {
        Err(ApiError::admin(format!(
            "No default {model_type} model is set."
        )))
    } else {
        Ok(field)
    }
}

/// `get_model_config_by_id` lookup semantic — find a tenant model instance by
/// its database id, raising the RAGFlow `LookupError` message as an
/// `ApiError` (surfaced as HTTP 400 by the API layer).
pub fn find_tenant_model_by_id<'a>(
    instances: &'a [TenantModelInstance],
    tenant_model_id: &str,
) -> Result<&'a TenantModelInstance, ApiError> {
    instances
        .iter()
        .find(|instance| instance.instance_id == tenant_model_id)
        .ok_or_else(|| ApiError::admin(format!("Tenant Model with id {tenant_model_id} not found")))
}

/// `get_model_config_by_type_and_name` type-compatibility check: a chat model
/// may serve IMAGE2TEXT and vice versa; any other mismatch is an error.
pub fn validate_model_type_match(
    configured: &str,
    expected: &str,
    model_name: &str,
) -> Result<(), ApiError> {
    let compatible = configured == expected
        || (expected == "chat" && configured == "image2text")
        || (expected == "image2text" && configured == "chat");
    if compatible {
        Ok(())
    } else {
        Err(ApiError::admin(format!(
            "Tenant Model with name {model_name} has type {configured}, expected {expected}"
        )))
    }
}

// ── UserAccountService (user_account_service.py) ───────────────────────────

/// `user_service.py::User` record (subset).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInfo {
    pub id: String,
    pub email: String,
    pub nickname: String,
    pub password: String,
    pub login_channel: String,
    pub is_superuser: bool,
    pub access_token: String,
}

/// `user_service.py::Tenant` record (subset).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantRecord {
    pub id: String,
    pub name: String,
    pub llm_id: String,
    pub embd_id: String,
    pub asr_id: String,
    pub parser_ids: String,
    pub img2txt_id: String,
    pub rerank_id: String,
}

/// `user_service.py::UserTenant` relation record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserTenantRecord {
    pub tenant_id: String,
    pub user_id: String,
    pub invited_by: String,
    pub role: UserTenantRole,
}

/// `file_service.py::File` record — the root folder created for a new user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: String,
    pub parent_id: String,
    pub tenant_id: String,
    pub created_by: String,
    pub name: String,
    pub file_type: String,
    pub size: u64,
    pub location: String,
}

/// `tenant_llm_service.py::TenantLLM` record (subset) — one initial binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantLlmRecord {
    pub tenant_id: String,
    pub llm_factory: String,
    pub llm_name: String,
    pub model_type: String,
    pub api_key: String,
    pub api_base: String,
    pub max_tokens: u64,
}

/// One model endpoint used to seed a new tenant
/// (`common/settings.py::*_CFG` shapes).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelEndpoint {
    pub factory: String,
    pub api_key: String,
    pub base_url: String,
}

/// `settings.CHAT_MDL/EMBEDDING_MDL/...` defaults for a new tenant.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelDefaults {
    pub chat_mdl: String,
    pub embedding_mdl: String,
    pub asr_mdl: String,
    pub image2text_mdl: String,
    pub rerank_mdl: String,
    pub parsers: String,
    pub chat: ModelEndpoint,
    pub embedding: ModelEndpoint,
    pub asr: ModelEndpoint,
    pub image2text: ModelEndpoint,
    pub rerank: ModelEndpoint,
}

/// `create_new_user` record plan. Insert order: user → tenant → user_tenant →
/// tenant_llms → root file; rollback runs in the reverse order (RAGFlow
/// deletes metadata index, tenant, user-tenant, tenant-llms, file, user).
#[derive(Debug, Clone)]
pub struct NewUserPlan {
    pub user: UserInfo,
    pub tenant: TenantRecord,
    pub user_tenant: UserTenantRecord,
    pub root_file: FileRecord,
    pub tenant_llms: Vec<TenantLlmRecord>,
}

impl NewUserPlan {
    /// Port of `create_new_user`: generate `user_id` / `access_token`, the
    /// `"{nickname}‘s Kingdom"` tenant, the OWNER relation, the self-rooted
    /// `/` folder and the initial tenant-LLM bindings.
    pub fn build(
        nickname: &str,
        email: &str,
        password: &str,
        login_channel: &str,
        is_superuser: bool,
        defaults: &ModelDefaults,
        tenant_llms: Vec<TenantLlmRecord>,
    ) -> Self {
        let user_id = uuid::Uuid::new_v4().simple().to_string();
        let access_token = uuid::Uuid::new_v4().simple().to_string();
        let file_id = uuid::Uuid::new_v4().simple().to_string();
        let user = UserInfo {
            id: user_id.clone(),
            email: email.into(),
            nickname: nickname.into(),
            password: password.into(),
            login_channel: login_channel.into(),
            is_superuser,
            access_token,
        };
        let tenant = TenantRecord {
            id: user_id.clone(),
            name: format!("{nickname}‘s Kingdom"),
            llm_id: defaults.chat_mdl.clone(),
            embd_id: defaults.embedding_mdl.clone(),
            asr_id: defaults.asr_mdl.clone(),
            parser_ids: defaults.parsers.clone(),
            img2txt_id: defaults.image2text_mdl.clone(),
            rerank_id: defaults.rerank_mdl.clone(),
        };
        let user_tenant = UserTenantRecord {
            tenant_id: user_id.clone(),
            user_id: user_id.clone(),
            invited_by: user_id.clone(),
            role: UserTenantRole::Owner,
        };
        let root_file = FileRecord {
            id: file_id.clone(),
            parent_id: file_id,
            tenant_id: user_id.clone(),
            created_by: user_id.clone(),
            name: "/".into(),
            file_type: "folder".into(),
            size: 0,
            location: String::new(),
        };
        Self {
            user,
            tenant,
            user_tenant,
            root_file,
            tenant_llms,
        }
    }
}

/// `llm_service.py::get_init_tenant_llm` — one binding per unique factory over
/// the five default endpoints (RAGFlow expands each factory against the LLM
/// catalog; RayRAG records the endpoint defaults). Factories are deduplicated
/// in the same first-seen order.
pub fn init_tenant_llms(tenant_id: &str, defaults: &ModelDefaults) -> Vec<TenantLlmRecord> {
    let endpoints = [
        ("chat", &defaults.chat),
        ("embedding", &defaults.embedding),
        ("speech2text", &defaults.asr),
        ("image2text", &defaults.image2text),
        ("rerank", &defaults.rerank),
    ];
    let mut seen = HashMap::new();
    let mut records = Vec::new();
    for (model_type, endpoint) in endpoints {
        if endpoint.factory.is_empty() || seen.contains_key(&endpoint.factory) {
            continue;
        }
        seen.insert(endpoint.factory.clone(), ());
        records.push(TenantLlmRecord {
            tenant_id: tenant_id.into(),
            llm_factory: endpoint.factory.clone(),
            llm_name: endpoint.factory.clone(),
            model_type: model_type.into(),
            api_key: endpoint.api_key.clone(),
            api_base: endpoint.base_url.clone(),
            max_tokens: 8192,
        });
    }
    records
}

/// `delete_user_data` guards: an active user or the super user cannot be
/// deleted.
pub fn can_delete_user(is_active: bool, is_superuser: bool, user_id: &str) -> Result<(), ApiError> {
    if is_active {
        return Err(ApiError::admin(format!(
            "{user_id} is active and can't be deleted."
        )));
    }
    if is_superuser {
        return Err(ApiError::admin("Can't delete the super user."));
    }
    Ok(())
}

/// `delete_user_data` — the user's owned tenant (first OWNER relation).
pub fn owned_tenant(relations: &[UserTenantRecord]) -> Option<&UserTenantRecord> {
    relations
        .iter()
        .find(|relation| relation.role == UserTenantRole::Owner)
}

/// `delete_user_data` — tenants the user joined as a NORMAL member.
pub fn joined_tenants(relations: &[UserTenantRecord]) -> Vec<&UserTenantRecord> {
    relations
        .iter()
        .filter(|relation| relation.role == UserTenantRole::Normal)
        .collect()
}

/// Deletion counts collected by the caller, mirroring `delete_user_data`'s
/// per-step tallies.
#[derive(Debug, Clone, Default)]
pub struct DeletionCounts {
    pub kb_buckets: usize,
    pub documents: usize,
    pub tasks: usize,
    pub files: usize,
    pub file2docs: usize,
    pub chunks: usize,
    pub knowledgebases: usize,
    pub agents: usize,
    pub agent_versions: usize,
    pub dialogs: usize,
    pub conversations: usize,
    pub api_tokens: usize,
    pub api4conversations: usize,
    pub mcp_servers: usize,
    pub searches: usize,
    pub tenant_llms: usize,
    pub langfuse: usize,
    pub memories: usize,
    pub tenants: usize,
    pub user_tenant_records: usize,
}

/// Ordered `delete_user_data` steps (owned tenant then relations then user),
/// each with the count the caller observed — the `done_msg` structure of
/// RAGFlow's report.
pub fn deletion_steps(counts: &DeletionCounts) -> Vec<(&'static str, usize)> {
    vec![
        ("dataset's buckets", counts.kb_buckets),
        ("document records", counts.documents),
        ("task records", counts.tasks),
        ("file records", counts.files),
        ("document-file relation records", counts.file2docs),
        ("chunk records", counts.chunks),
        ("dataset records", counts.knowledgebases),
        ("agent records", counts.agents),
        ("agent version records", counts.agent_versions),
        ("dialogs", counts.dialogs),
        ("conversations", counts.conversations),
        ("api tokens", counts.api_tokens),
        ("api4conversations", counts.api4conversations),
        ("MCP servers", counts.mcp_servers),
        ("search records", counts.searches),
        ("tenant-LLM records", counts.tenant_llms),
        ("langfuse records", counts.langfuse),
        ("memory datasets", counts.memories),
        ("tenants", counts.tenants),
        ("user-tenant records", counts.user_tenant_records),
    ]
}

/// Format the deletion steps into RAGFlow's `done_msg` report.
pub fn deletion_report(steps: &[(&'static str, usize)]) -> String {
    let mut report = String::from("Start to delete owned tenant.\n");
    for (label, count) in steps {
        report.push_str(&format!("- Deleted {count} {label}.\n"));
    }
    report.push_str("Delete done!");
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(message_id: i64, memory_id: &str, valid_at: &str, content: &str) -> MemoryMessage {
        MemoryMessage {
            message_id,
            message_type: "raw".into(),
            source_id: 0,
            memory_id: memory_id.into(),
            user_id: "u1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            content: content.into(),
            valid_at: valid_at.into(),
            invalid_at: None,
            forget_at: None,
            status: true,
            zone_id: 0,
            content_embed: vec![0.1, 0.2],
        }
    }

    #[test]
    fn fifo_eviction_frees_oldest_messages_first() {
        let messages = [
            message(1, "m1", "2026-08-01 00:00:00", "first"),
            message(2, "m1", "2026-08-02 00:00:00", "second"),
            message(3, "m1", "2026-08-03 00:00:00", "third"),
        ];
        // Each message ~= len(content) + 49 + 24*2 bytes; size_to_delete only
        // the first message frees.
        let target = messages[0].calculate_size();
        let (ids, freed) = pick_messages_to_delete_by_fifo(messages.iter(), target);
        assert_eq!(ids, vec![1]);
        assert!(freed >= target);
        // A huge budget takes everything.
        let (ids, _) = pick_messages_to_delete_by_fifo(messages.iter(), usize::MAX);
        assert_eq!(ids, vec![1, 2, 3]);
        // Zero budget deletes nothing (RAGFlow's `current_size < size_to_delete` gate).
        let (ids, freed) = pick_messages_to_delete_by_fifo(messages.iter(), 0);
        assert!(ids.is_empty());
        assert_eq!(freed, 0);
    }

    #[test]
    fn message_size_accounting_matches_ragflow_formula() {
        let msg = message(1, "m1", "2026-08-01 00:00:00", "hello");
        assert_eq!(
            msg.calculate_size(),
            5 + MESSAGE_CONTENT_OVERHEAD + MESSAGE_EMBEDDING_DIM_SIZE * 2
        );
        assert_eq!(
            build_raw_message_content("问", "答"),
            "User Input: 问\nAgent Response: 答"
        );
    }

    #[test]
    fn message_store_budget_evicts_fifo_and_queries_latest_first() {
        let service = MemoryMessageService::in_memory();
        let raw = MemoryMessage {
            message_id: service.next_message_id(),
            ..message(0, "m1", "2026-08-01 00:00:00", "old")
        };
        let budget = 3 * raw.calculate_size();
        service.insert_messages(vec![raw]).unwrap();
        // Overflow: insert two more messages, forcing one FIFO eviction.
        let second = MemoryMessage {
            message_id: service.next_message_id(),
            ..message(0, "m1", "2026-08-02 00:00:00", "second")
        };
        let third = MemoryMessage {
            message_id: service.next_message_id(),
            ..message(0, "m1", "2026-08-03 00:00:00", "third")
        };
        service
            .save_messages_with_budget("m1", "FIFO", budget, vec![second, third])
            .unwrap();
        let remaining = service.query(&["m1".into()], None, None, None, None, None);
        assert_eq!(remaining.len(), 2);
        // Query orders by valid_at desc → newest first.
        assert_eq!(remaining[0].message_id, 3);
        assert_eq!(remaining[1].message_id, 2);
        // Non-FIFO policy refuses to evict.
        let fourth = MemoryMessage {
            message_id: service.next_message_id(),
            ..message(0, "m1", "2026-08-04 00:00:00", "fourth")
        };
        let err = service
            .save_messages_with_budget("m1", "LRU", budget, vec![fourth])
            .unwrap_err();
        assert!(
            err.message
                .contains("Memory size reached limit and cannot decide which to delete")
        );
    }

    #[test]
    fn fifo_eviction_is_scoped_to_the_composite_message_identity() {
        let service = MemoryMessageService::in_memory();
        let old = message(1, "m1", "2026-08-01 00:00:00", "old");
        let same_integer_other_memory = message(1, "m2", "2026-08-01 00:00:00", "keep");
        service
            .insert_messages(vec![old, same_integer_other_memory.clone()])
            .unwrap();

        let replacement = message(2, "m1", "2026-08-02 00:00:00", "new");
        let budget = replacement.calculate_size();
        service
            .save_messages_with_budget("m1", "FIFO", budget, vec![replacement])
            .unwrap();

        assert!(service.get_by_message_id("m1", 1).is_none());
        assert!(service.get_by_message_id("m1", 2).is_some());
        assert_eq!(
            service.get_by_message_id("m2", 1),
            Some(same_integer_other_memory)
        );
    }

    #[test]
    fn invalid_budget_write_rolls_back_the_planned_fifo_eviction() {
        let service = MemoryMessageService::in_memory();
        let stable = message(1, "m1", "2026-08-01 00:00:00", "stable");
        service.insert_messages(vec![stable.clone()]).unwrap();
        let invalid = MemoryMessage {
            message_id: 2,
            content_embed: Vec::new(),
            ..message(0, "m1", "2026-08-02 00:00:00", "invalid")
        };

        let result = service.save_messages_with_budget(
            "m1",
            "FIFO",
            invalid.calculate_size(),
            vec![invalid],
        );
        assert!(result.is_err());
        assert_eq!(service.get_by_message_id("m1", 1), Some(stable));
        assert!(service.get_by_message_id("m1", 2).is_none());
    }

    #[test]
    fn tenant_default_model_resolution_matches_ragflow_errors() {
        let defaults = TenantDefaultModels {
            llm_id: "deepseek-chat@deepseek".into(),
            embd_id: "bge-m3@builtin".into(),
            ..Default::default()
        };
        assert_eq!(
            defaults.model_name_for_type("chat").unwrap(),
            "deepseek-chat@deepseek"
        );
        assert_eq!(
            defaults.model_name_for_type("embedding").unwrap(),
            "bge-m3@builtin"
        );
        assert_eq!(
            defaults.model_name_for_type("tts").unwrap_err().message,
            "No default tts model is set."
        );
        assert_eq!(
            defaults.model_name_for_type("ocr").unwrap_err().message,
            "OCR model name is required"
        );
        assert_eq!(
            defaults.model_name_for_type("vision").unwrap_err().message,
            "Unknown model type vision"
        );
        // Chat ↔ IMAGE2TEXT are interchangeable.
        assert!(validate_model_type_match("image2text", "chat", "gpt-4o").is_ok());
        assert!(validate_model_type_match("chat", "image2text", "gpt-4o").is_ok());
        assert!(validate_model_type_match("chat", "embedding", "gpt-4o").is_err());
    }

    #[test]
    fn new_user_plan_builds_kingdom_tenant_and_owner_relation() {
        let defaults = ModelDefaults {
            chat_mdl: "deepseek-chat@deepseek".into(),
            embedding_mdl: "bge-m3@builtin".into(),
            parsers: "naive".into(),
            chat: ModelEndpoint {
                factory: "deepseek".into(),
                api_key: "k1".into(),
                base_url: "https://api.deepseek.com".into(),
            },
            embedding: ModelEndpoint {
                factory: "builtin".into(),
                api_key: "k2".into(),
                base_url: "http://embed:9380".into(),
            },
            ..Default::default()
        };
        // Seed tenant-LLM bindings with the plan's generated user id, the way
        // `create_new_user` wires `get_init_tenant_llm(user_id)`.
        let mut plan = NewUserPlan::build(
            "Alice",
            "alice@example.com",
            "secret",
            "password",
            false,
            &defaults,
            Vec::new(),
        );
        plan.tenant_llms = init_tenant_llms(&plan.user.id, &defaults);
        // user id == tenant id == owner relation tenant (RAGFlow uuid1().hex).
        assert_eq!(plan.user.id, plan.tenant.id);
        assert_eq!(plan.user_tenant.tenant_id, plan.user.id);
        assert_eq!(plan.user_tenant.user_id, plan.user.id);
        assert_eq!(plan.user_tenant.role, UserTenantRole::Owner);
        assert_eq!(plan.tenant.name, "Alice‘s Kingdom");
        assert_eq!(plan.tenant.llm_id, "deepseek-chat@deepseek");
        // Root folder is self-rooted and owned by the new user.
        assert_eq!(plan.root_file.parent_id, plan.root_file.id);
        assert_eq!(plan.root_file.tenant_id, plan.user.id);
        assert_eq!(plan.root_file.name, "/");
        assert_eq!(plan.root_file.file_type, "folder");
        // Tenant-LLM bindings carry the new tenant id.
        assert!(!plan.tenant_llms.is_empty());
        assert!(
            plan.tenant_llms
                .iter()
                .all(|record| record.tenant_id == plan.user.id)
        );
    }

    #[test]
    fn delete_user_guards_and_relation_filtering() {
        assert!(can_delete_user(false, false, "u1").is_ok());
        assert_eq!(
            can_delete_user(true, false, "u1").unwrap_err().message,
            "u1 is active and can't be deleted."
        );
        assert_eq!(
            can_delete_user(false, true, "u1").unwrap_err().message,
            "Can't delete the super user."
        );
        let relations = vec![
            UserTenantRecord {
                tenant_id: "t-own".into(),
                user_id: "u1".into(),
                invited_by: "u1".into(),
                role: UserTenantRole::Owner,
            },
            UserTenantRecord {
                tenant_id: "t-join".into(),
                user_id: "u1".into(),
                invited_by: "t-join".into(),
                role: UserTenantRole::Normal,
            },
        ];
        assert_eq!(owned_tenant(&relations).unwrap().tenant_id, "t-own");
        let joined = joined_tenants(&relations);
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].tenant_id, "t-join");
        // Deletion report mirrors RAGFlow's done_msg shape.
        let counts = DeletionCounts {
            knowledgebases: 2,
            documents: 5,
            ..Default::default()
        };
        let report = deletion_report(&deletion_steps(&counts));
        assert!(report.contains("- Deleted 2 dataset records."));
        assert!(report.contains("- Deleted 5 document records."));
        assert!(report.ends_with("Delete done!"));
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// init_data semantics — ported from RAGFlow `api/db/init_data.py`.
//
// RayRAG keeps its records in JSON/Postgres stores, so these ports are the
// *semantics*: pure decision functions plus a small seed store. Mirrored
// behaviours:
//
// - `init_table()`: seed `system_settings` rows from
//   `conf/system_settings.json`, inserting only names that are missing.
// - `init_superuser()`: build the bootstrap user plan (user + tenant +
//   user-tenant OWNER relation + initial tenant-LLM bindings); skip when the
//   email already exists.
// - `init_llm_factory()`: normalize `FACTORY_LLM_INFOS` (factory record with
//   the `llm` list hoisted out, each model stamped with its `fid`) and apply
//   the idempotent cleanup rules (drop `Local` / `novita.ai` / `QAnything`,
//   migrate `QAnything`→`Youdao`, `cohere`→`Cohere`, drop the legacy
//   `qwen-vl-max` and Moonshot `flag-embedding` rows).
// - `fix_empty_tenant_model_id()`: group rows with a NULL tenant-model id by
//   `(tenant_id, model_id)`, resolve each group through the tenant-LLM
//   lookup and plan UPDATEs for knowledgebase / dialog / memory / tenant.
// ═══════════════════════════════════════════════════════════════════════════

/// `api/db/init_data.py` env defaults for the bootstrap superuser.
pub const DEFAULT_SUPERUSER_NICKNAME: &str = "admin";
pub const DEFAULT_SUPERUSER_EMAIL: &str = "admin@ragflow.io";
pub const DEFAULT_SUPERUSER_PASSWORD: &str = "admin";

/// `SystemSettings` row (`conf/system_settings.json` record).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemSetting {
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub data_type: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub create_time: u64,
    #[serde(default)]
    pub update_time: u64,
}

/// `system_settings_service.py::SystemSettingsService` semantics: upsert only
/// the settings whose `name` is not already present (RAGFlow `init_table`
/// inserts only `to_save` records). Returns the number of rows seeded.
pub fn seed_system_settings(
    existing: &[SystemSetting],
    records_from_file: &[SystemSetting],
    now: u64,
) -> (Vec<SystemSetting>, usize) {
    let existing_names: std::collections::HashSet<&str> =
        existing.iter().map(|r| r.name.as_str()).collect();
    let mut to_save = Vec::new();
    for mut record in records_from_file.iter().cloned() {
        if existing_names.contains(record.name.as_str()) {
            continue;
        }
        record.create_time = now;
        record.update_time = now;
        to_save.push(record);
    }
    let count = to_save.len();
    (to_save, count)
}

/// `init_superuser()` record plan — a bootstrap user with its own tenant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuperuserSeedPlan {
    pub user_id: String,
    /// Base64-encoded password (`encode_to_base64`).
    pub password: String,
    pub nickname: String,
    pub email: String,
    pub is_superuser: bool,
    pub creator: String,
    pub status: String,
    pub tenant_id: String,
    pub tenant_name: String,
    pub llm_id: String,
    pub embd_id: String,
    pub asr_id: String,
    pub img2txt_id: String,
    pub rerank_id: String,
    pub parser_ids: String,
}

impl SuperuserSeedPlan {
    /// Build the plan (RAGFlow `init_superuser` field assembly). The caller
    /// decides whether to persist it — the service skips when a user with the
    /// same email already exists.
    pub fn build(
        user_id: String,
        password: String,
        nickname: &str,
        email: &str,
        llm_id: &str,
        embd_id: &str,
        asr_id: &str,
        img2txt_id: &str,
        rerank_id: &str,
        parser_ids: &str,
    ) -> Self {
        let tenant_id = user_id.clone();
        Self {
            tenant_name: format!("{nickname}‘s Kingdom"),
            tenant_id,
            user_id,
            password,
            nickname: nickname.to_owned(),
            email: email.to_owned(),
            is_superuser: true,
            creator: "system".to_owned(),
            status: "1".to_owned(),
            llm_id: llm_id.to_owned(),
            embd_id: embd_id.to_owned(),
            asr_id: asr_id.to_owned(),
            img2txt_id: img2txt_id.to_owned(),
            rerank_id: rerank_id.to_owned(),
            parser_ids: parser_ids.to_owned(),
        }
    }
}

/// One LLM factory row from `settings.FACTORY_LLM_INFOS`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmFactoryRecord {
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub llm: Vec<LlmRecord>,
}

/// One LLM model row inside a factory (`init_llm_factory` hoists the `llm`
/// list out and stamps each model with its `fid`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRecord {
    pub llm_name: String,
    #[serde(default)]
    pub fid: String,
    #[serde(default)]
    pub model_type: String,
    #[serde(default)]
    pub max_tokens: Option<i64>,
}

/// `init_llm_factory()` normalization: `info.pop("llm")` — the factory record
/// is the dict minus the `llm` key, and every model gets `fid = factory.name`.
pub fn normalize_factory_llm_infos(
    factories: &[LlmFactoryRecord],
) -> (Vec<LlmFactoryRecord>, Vec<LlmRecord>) {
    let mut factory_rows = Vec::new();
    let mut llm_rows = Vec::new();
    for factory in factories {
        let mut row = factory.clone();
        row.llm.clear();
        factory_rows.push(row);
        for mut llm in factory.llm.clone() {
            // RAGFlow: `llm_info["fid"] = factory_llm_info["name"]`.
            llm.fid = factory.name.clone();
            llm_rows.push(llm);
        }
    }
    (factory_rows, llm_rows)
}

/// Idempotent cleanup rules from `init_llm_factory()`'s tail — which
/// (factory, model) rows are dropped and which factories are renamed.
#[derive(Debug, Clone, PartialEq)]
pub struct FactoryCleanupPlan {
    /// (factory_name, model_name) rows to delete.
    pub delete_llm_rows: Vec<(String, String)>,
    /// Factory names to delete entirely.
    pub delete_factories: Vec<String>,
    /// (old_factory, new_factory) renames.
    pub rename_factories: Vec<(String, String)>,
}

/// Apply RAGFlow's fixed cleanup rules to a set of (factory, model) rows.
pub fn apply_factory_cleanup_rules(rows: &[(String, String)]) -> FactoryCleanupPlan {
    let mut delete_llm_rows = Vec::new();
    let mut delete_factories = Vec::new();
    let mut rename_factories = Vec::new();

    for (factory, model) in rows {
        match (factory.as_str(), model.as_str()) {
            ("Local", _) | ("novita.ai", _) => {
                delete_llm_rows.push((factory.clone(), model.clone()))
            }
            ("QAnything", _) => rename_factories.push(("QAnything".into(), "Youdao".into())),
            ("cohere", _) => rename_factories.push(("cohere".into(), "Cohere".into())),
            ("Moonshot", "flag-embedding") => {
                delete_llm_rows.push((factory.clone(), model.clone()))
            }
            (_, "qwen-vl-max") => delete_llm_rows.push((factory.clone(), model.clone())),
            _ => {}
        }
    }
    // Deduplicate rename pairs; factories fully dropped are removed outright.
    rename_factories.sort();
    rename_factories.dedup();
    if rows.iter().any(|(f, _)| f == "Local") {
        delete_factories.push("Local".into());
    }
    if rows.iter().any(|(f, _)| f == "novita.ai") {
        delete_factories.push("novita.ai".into());
    }
    if rows.iter().any(|(f, _)| f == "QAnything") {
        delete_factories.push("QAnything".into());
    }
    delete_llm_rows.sort();
    delete_llm_rows.dedup();
    FactoryCleanupPlan {
        delete_llm_rows,
        delete_factories,
        rename_factories,
    }
}

/// A row with a NULL `tenant_{model}_id` that needs resolution
/// (`fix_empty_tenant_model_id`).
#[derive(Debug, Clone, PartialEq)]
pub struct NullTenantModelRow {
    pub id: String,
    pub tenant_id: String,
    pub model_id: String,
}

/// `fix_empty_tenant_model_id()` planning: group NULL-tenant-model-id rows by
/// `(tenant_id, model_id)` and resolve each group through the tenant-LLM
/// lookup. Returns per-row UPDATE plans `(row_id, tenant_model_id)`.
pub fn plan_fix_empty_tenant_model_ids(
    rows: &[NullTenantModelRow],
    resolve_tenant_llm: impl Fn(&str, &str) -> Option<String>,
) -> Vec<(String, String)> {
    // Group by (tenant_id, model_id) preserving first-seen order.
    let mut groups: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for row in rows {
        if let Some((_, _, ids)) = groups
            .iter_mut()
            .find(|(t, m, _)| *t == row.tenant_id && *m == row.model_id)
        {
            ids.push(row.id.as_str());
        } else {
            groups.push((
                row.tenant_id.as_str(),
                row.model_id.as_str(),
                vec![row.id.as_str()],
            ));
        }
    }
    let mut updates = Vec::new();
    for (tenant_id, model_id, ids) in groups {
        if let Some(tenant_llm_id) = resolve_tenant_llm(tenant_id, model_id) {
            for id in ids {
                updates.push((id.to_owned(), tenant_llm_id.clone()));
            }
        }
    }
    updates
}

/// `tenant_model_id` columns fixed per table kind
/// (`fix_empty_tenant_model_id` — kb/dialog/memory/tenant).
pub const TENANT_MODEL_ID_COLUMNS: &[&str] = &[
    "llm_id",
    "embd_id",
    "asr_id",
    "img2txt_id",
    "rerank_id",
    "tts_id",
];

/// `template_utils.py::normalize_canvas_template_categories` — collect
/// `canvas_type` + `canvas_types` into a deduplicated list; `canvas_type`
/// becomes the first entry (or None). Pure, matches the Python exactly.
pub fn normalize_canvas_template_categories(template: &serde_json::Value) -> serde_json::Value {
    let mut normalized = template.clone();
    let mut categories: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let push = |category: &str,
                categories: &mut Vec<String>,
                seen: &mut std::collections::HashSet<String>| {
        let category = category.trim();
        if category.is_empty() || seen.contains(category) {
            return;
        }
        seen.insert(category.to_owned());
        categories.push(category.to_owned());
    };

    if let Some(serde_json::Value::String(canvas_type)) = normalized.get("canvas_type") {
        push(canvas_type, &mut categories, &mut seen);
    }
    match normalized.get("canvas_types") {
        Some(serde_json::Value::Array(items)) => {
            for item in items {
                if let serde_json::Value::String(category) = item {
                    push(category, &mut categories, &mut seen);
                }
            }
        }
        Some(serde_json::Value::String(single)) => {
            push(single, &mut categories, &mut seen);
        }
        _ => {}
    }
    normalized["canvas_types"] = serde_json::Value::Array(
        categories
            .iter()
            .map(|c| serde_json::Value::String(c.clone()))
            .collect(),
    );
    normalized["canvas_type"] = categories
        .first()
        .map(|c| serde_json::Value::String(c.clone()))
        .unwrap_or(serde_json::Value::Null);
    normalized
}

/// A seeded canvas template row (`CanvasTemplateService.save` payload).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanvasTemplateRecord {
    pub id: String,
    pub title: String,
    pub description: String,
    pub canvas_type: Option<String>,
    pub canvas_types: Vec<String>,
    pub dsl: serde_json::Value,
}

/// Build a `CanvasTemplateRecord` from a raw template JSON, applying the
/// `template_utils` category normalization (RAGFlow `add_graph_templates`
/// normalizes every template file before saving).
pub fn canvas_template_record(id: String, raw: &serde_json::Value) -> CanvasTemplateRecord {
    let normalized = normalize_canvas_template_categories(raw);
    CanvasTemplateRecord {
        id,
        title: normalized
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned(),
        description: normalized
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned(),
        canvas_type: normalized
            .get("canvas_type")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
        canvas_types: normalized
            .get("canvas_types")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        dsl: normalized
            .get("dsl")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// langfuse_service.py + mcp_server_service.py semantics.
// ═══════════════════════════════════════════════════════════════════════════

/// `TenantLangfuse` row (`langfuse_service.py::TenantLangfuseService`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TenantLangfuseRecord {
    pub tenant_id: String,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub secret_key: String,
    #[serde(default)]
    pub public_key: String,
    #[serde(default)]
    pub create_time: u64,
    #[serde(default)]
    pub update_time: u64,
}

/// `MCPServer` row (`mcp_server_service.py::MCPServerService`) — the fields
/// exposed by `get_servers` for list display.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct McpServerRecord {
    pub id: String,
    pub name: String,
    pub server_type: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub variables: serde_json::Value,
    pub tenant_id: String,
    #[serde(default)]
    pub create_time: u64,
    #[serde(default)]
    pub update_time: u64,
}

/// `mcp_server_service.get_servers` semantics — filter + order + paginate a
/// tenant's MCP server rows. Pure; mirrors the peewee query chain.
pub fn mcp_server_get_servers(
    servers: &[McpServerRecord],
    tenant_id: &str,
    id_list: Option<&[String]>,
    keywords: Option<&str>,
    orderby: &str,
    desc: bool,
    page_number: Option<usize>,
    items_per_page: Option<usize>,
) -> Vec<McpServerRecord> {
    let mut rows: Vec<McpServerRecord> = servers
        .iter()
        .filter(|s| s.tenant_id == tenant_id)
        .filter(|s| id_list.is_none_or(|ids| ids.iter().any(|id| id == &s.id)))
        .filter(|s| keywords.is_none_or(|kw| s.name.to_lowercase().contains(&kw.to_lowercase())))
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        let key_a = match orderby {
            "name" => a.name.cmp(&b.name),
            "create_time" => a.create_time.cmp(&b.create_time),
            "update_time" => a.update_time.cmp(&b.update_time),
            _ => a.create_time.cmp(&b.create_time),
        };
        if desc { key_a.reverse() } else { key_a }
    });
    if let (Some(page), Some(per_page)) = (page_number, items_per_page) {
        let start = page.saturating_sub(1) * per_page;
        rows = rows.into_iter().skip(start).take(per_page).collect();
    }
    rows
}

/// `get_by_name_and_tenant` semantics: name uniqueness within a tenant.
pub fn mcp_server_exists(servers: &[McpServerRecord], name: &str, tenant_id: &str) -> bool {
    servers
        .iter()
        .any(|s| s.name == name && s.tenant_id == tenant_id)
}

// ═══════════════════════════════════════════════════════════════════════════
// conversation_service.py::ConversationService.get_list semantics.
// ═══════════════════════════════════════════════════════════════════════════

/// `Conversation` row subset used by `get_list`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConversationRow {
    pub id: String,
    pub dialog_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub create_time: u64,
    #[serde(default)]
    pub update_time: u64,
}

/// `conversation_service.get_list` — filter by dialog/id/name/user, order by
/// `orderby` asc/desc, paginate when `items_per_page > 0`.
pub fn conversation_get_list(
    sessions: &[ConversationRow],
    dialog_id: &str,
    page_number: usize,
    items_per_page: usize,
    orderby: &str,
    desc: bool,
    id: Option<&str>,
    name: Option<&str>,
    user_id: Option<&str>,
) -> Vec<ConversationRow> {
    let mut rows: Vec<ConversationRow> = sessions
        .iter()
        .filter(|s| s.dialog_id == dialog_id)
        .filter(|s| id.is_none_or(|v| v == s.id))
        .filter(|s| name.is_none_or(|v| v == s.name))
        .filter(|s| user_id.is_none_or(|v| v == s.user_id))
        .cloned()
        .collect();
    rows.sort_by(|a, b| {
        let key = match orderby {
            "name" => a.name.cmp(&b.name),
            "update_time" => a.update_time.cmp(&b.update_time),
            _ => a.create_time.cmp(&b.create_time),
        };
        if desc { key.reverse() } else { key }
    });
    if items_per_page > 0 {
        let start = page_number.saturating_sub(1) * items_per_page;
        rows = rows.into_iter().skip(start).take(items_per_page).collect();
    }
    rows
}

// ═══════════════════════════════════════════════════════════════════════════
// connector_service.py::SyncLogsService semantics.
// ═══════════════════════════════════════════════════════════════════════════

/// `SyncLogs` row (`connector_service.py::SyncLogsService`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncLogRow {
    pub id: String,
    pub connector_id: String,
    pub kb_id: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub poll_range_start: Option<String>,
    #[serde(default)]
    pub poll_range_end: Option<String>,
    #[serde(default)]
    pub new_docs_indexed: u64,
    #[serde(default)]
    pub total_docs_indexed: u64,
    #[serde(default)]
    pub docs_removed_from_index: u64,
    #[serde(default)]
    pub error_count: u64,
    #[serde(default)]
    pub error_msg: String,
    #[serde(default)]
    pub update_time: u64,
}

/// `SyncLogsService.increase_docs` — monotonic counters. The poll range
/// advances only forward (`GREATEST(current, max_update)`); doc counters
/// accumulate. Mirrors the Python update expression exactly.
pub fn sync_log_increase_docs(
    log: &mut SyncLogRow,
    max_update: &str,
    doc_num: u64,
    err_msg: &str,
    error_count: u64,
    now: u64,
) {
    log.new_docs_indexed += doc_num;
    log.total_docs_indexed += doc_num;
    let bump = |current: &Option<String>| -> Option<String> {
        match current {
            Some(cur) if cur.as_str() >= max_update => Some(cur.clone()),
            _ => Some(max_update.to_owned()),
        }
    };
    log.poll_range_start = bump(&log.poll_range_start);
    log.poll_range_end = bump(&log.poll_range_end);
    log.error_msg.push_str(err_msg);
    log.error_count += error_count;
    log.update_time = now;
}

/// `SyncLogsService.increase_removed_docs` — stale-document accounting.
pub fn sync_log_increase_removed_docs(
    log: &mut SyncLogRow,
    removed_count: u64,
    err_msg: &str,
    error_count: u64,
    now: u64,
) {
    log.docs_removed_from_index += removed_count;
    log.error_msg.push_str(err_msg);
    log.error_count += error_count;
    log.update_time = now;
}

/// `SyncLogsService.schedule` retention cap: keep at most 100 logs per
/// (connector, kb); when over, prune the 70 oldest by `update_time` asc.
/// Returns the ids to delete (empty when under the cap).
pub fn sync_log_prune_plan(logs: &[SyncLogRow], connector_id: &str, kb_id: &str) -> Vec<String> {
    let mut matching: Vec<&SyncLogRow> = logs
        .iter()
        .filter(|l| l.connector_id == connector_id && l.kb_id == kb_id)
        .collect();
    if matching.len() <= 100 {
        return Vec::new();
    }
    matching.sort_by_key(|l| l.update_time);
    matching.iter().take(70).map(|l| l.id.clone()).collect()
}

/// `ConnectorService.resume` status transition: a SCHEDULE request on top of
/// a DONE task starts a fresh sync from `poll_range_end`; otherwise the
/// existing task is re-marked with the target status.
#[derive(Debug, Clone, PartialEq)]
pub enum ResumeOutcome {
    /// No task existed — schedule a fresh one.
    ScheduleNew { connector_id: String, kb_id: String },
    /// A DONE task exists — schedule from the recorded poll range end.
    ScheduleFrom {
        connector_id: String,
        kb_id: String,
        poll_range_end: Option<String>,
        total_docs_indexed: u64,
    },
    /// Re-mark the existing task with the target status.
    Remark { task_id: String, status: String },
}

pub fn connector_resume(
    latest_task: Option<&SyncLogRow>,
    connector_id: &str,
    kb_id: &str,
    status: &str,
) -> ResumeOutcome {
    // TaskStatus (common/constants.py): DONE = "3", SCHEDULE = "5".
    const SCHEDULE: &str = "5";
    const DONE: &str = "3";
    match latest_task {
        None if status == SCHEDULE => ResumeOutcome::ScheduleNew {
            connector_id: connector_id.to_owned(),
            kb_id: kb_id.to_owned(),
        },
        Some(task) if task.status == DONE && status == SCHEDULE => ResumeOutcome::ScheduleFrom {
            connector_id: connector_id.to_owned(),
            kb_id: kb_id.to_owned(),
            poll_range_end: task.poll_range_end.clone(),
            total_docs_indexed: task.total_docs_indexed,
        },
        Some(task) => ResumeOutcome::Remark {
            task_id: task.id.clone(),
            status: status.to_owned(),
        },
        None => ResumeOutcome::Remark {
            task_id: String::new(),
            status: status.to_owned(),
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// task_service.py semantics.
// ═══════════════════════════════════════════════════════════════════════════

/// `TASK_MAX_LOG_LENGTH` env default — progress logs are trimmed to keep TEXT
/// columns under 64 KiB.
pub const TASK_MAX_LOG_LENGTH: usize = 3000;

/// `task_service.trim_header_by_lines` — drop leading lines until the tail
/// fits `max_length`; return the text unchanged when it already fits.
pub fn trim_header_by_lines(text: &str, max_length: usize) -> String {
    let len = text.chars().count();
    if len <= max_length {
        return text.to_owned();
    }
    for (i, ch) in text.char_indices() {
        if ch == '\n' && len - text[..i].chars().count() <= max_length {
            return text[i + 1..].to_owned();
        }
    }
    text.to_owned()
}

/// `TaskService.update_progress` progress gate: a new progress value is
/// written when (a) it is >= 1 (recovers from -1), or (b) the current value
/// is not -1 AND (the new value is -1 or strictly greater). Mirrors the
/// SQL WHERE clause.
pub fn progress_should_update(current: f64, new: f64) -> bool {
    new >= 1.0 || (current != -1.0 && (new == -1.0 || new > current))
}

/// `TaskService.get_task` retry semantics: a task is abandoned after 3
/// attempts (`retry_count >= 3` → progress -1, returns None).
pub fn task_attempt(retry_count: u32) -> Option<u32> {
    if retry_count >= 3 {
        return None;
    }
    Some(retry_count + 1)
}

/// `queue_tasks` page-range splitting: PDF tasks are split by
/// `task_page_size` (default 12; 22 for the `paper` parser; unbounded for
/// `one`/`knowledge_graph`, non-DeepDOC layout or toc extraction), Excel
/// `table` tasks by 3000 rows, everything else gets a single task.
/// Returns `(from_page, to_page)` ranges.
pub fn task_page_ranges(
    doc_type: &str,
    parser_id: &str,
    parser_config: &serde_json::Value,
    total_pages: usize,
) -> Vec<(usize, usize)> {
    let page_size_of = |default: usize| -> usize {
        parser_config
            .get("task_page_size")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(default)
    };
    if doc_type == "pdf" {
        let do_layout = parser_config
            .get("layout_recognize")
            .and_then(|v| v.as_str())
            .unwrap_or("DeepDOC");
        let toc = parser_config
            .get("toc_extraction")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let page_size = if parser_id == "paper" {
            page_size_of(22)
        } else if matches!(parser_id, "one" | "knowledge_graph") || do_layout != "DeepDOC" || toc {
            usize::MAX
        } else {
            page_size_of(12)
        };
        // RAGFlow default page range is (1, MAXIMUM_PAGE_NUMBER).
        let mut ranges = Vec::new();
        let mut start = 0usize;
        while start < total_pages {
            let end = start.saturating_add(page_size).min(total_pages);
            ranges.push((start, end));
            if end == start {
                break;
            }
            start = end;
        }
        if ranges.is_empty() {
            ranges.push((0, 0));
        }
        return ranges;
    }
    if parser_id == "table" {
        let mut ranges = Vec::new();
        let mut start = 0usize;
        while start < total_pages {
            let end = start.saturating_add(3000).min(total_pages);
            ranges.push((start, end));
            start = end;
        }
        if ranges.is_empty() {
            ranges.push((0, 0));
        }
        return ranges;
    }
    vec![(0, usize::MAX)]
}

/// `reuse_prev_task_chunks` — a previous task's chunks are reused when a task
/// with the same `from_page` and digest exists and finished (progress >= 1)
/// with chunk ids. Returns the reused chunk ids.
pub fn reuse_prev_task_chunks(
    from_page: usize,
    digest: &str,
    prev_tasks: &[(usize, String, f64, String)],
) -> Option<String> {
    for (prev_from_page, prev_digest, progress, chunk_ids) in prev_tasks {
        if *prev_from_page == from_page && prev_digest == digest {
            if *progress >= 1.0 && !chunk_ids.is_empty() {
                return Some(chunk_ids.clone());
            }
            return None;
        }
    }
    None
}

#[cfg(test)]
mod init_data_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn system_settings_seed_inserts_only_missing_names() {
        let existing = vec![SystemSetting {
            name: "enable_whitelist".into(),
            source: "variable".into(),
            data_type: "bool".into(),
            value: "true".into(),
            create_time: 1,
            update_time: 1,
        }];
        let from_file = vec![
            SystemSetting {
                name: "enable_whitelist".into(),
                source: "variable".into(),
                data_type: "bool".into(),
                value: "true".into(),
                create_time: 0,
                update_time: 0,
            },
            SystemSetting {
                name: "mail.server".into(),
                source: "variable".into(),
                data_type: "string".into(),
                value: "".into(),
                create_time: 0,
                update_time: 0,
            },
        ];
        let (to_save, count) = seed_system_settings(&existing, &from_file, 42);
        assert_eq!(count, 1);
        assert_eq!(to_save[0].name, "mail.server");
        assert_eq!(to_save[0].create_time, 42);
    }

    #[test]
    fn canvas_template_categories_normalize_and_dedupe() {
        let template = json!({
            "id": "t1",
            "title": "Deep Research",
            "canvas_type": "agent",
            "canvas_types": ["agent", " agent ", "chatflow"],
            "dsl": {"nodes": []}
        });
        let record = canvas_template_record("t1".into(), &template);
        assert_eq!(record.canvas_type.as_deref(), Some("agent"));
        assert_eq!(record.canvas_types, vec!["agent", "chatflow"]);
        // Missing categories collapse to an empty list, canvas_type to null.
        let bare = json!({"id": "t2", "title": "Bare"});
        let record = canvas_template_record("t2".into(), &bare);
        assert!(record.canvas_type.is_none());
        assert!(record.canvas_types.is_empty());
    }

    #[test]
    fn fix_empty_tenant_model_ids_groups_and_resolves_per_tenant() {
        let rows = vec![
            NullTenantModelRow {
                id: "kb1".into(),
                tenant_id: "t1".into(),
                model_id: "gpt-4o".into(),
            },
            NullTenantModelRow {
                id: "kb2".into(),
                tenant_id: "t1".into(),
                model_id: "gpt-4o".into(),
            },
            NullTenantModelRow {
                id: "kb3".into(),
                tenant_id: "t1".into(),
                model_id: "bge-m3".into(),
            },
            NullTenantModelRow {
                id: "kb4".into(),
                tenant_id: "t2".into(),
                model_id: "gpt-4o".into(),
            },
        ];
        let resolve = |tenant: &str, model: &str| -> Option<String> {
            match (tenant, model) {
                ("t1", "gpt-4o") => Some("tlm-1".into()),
                ("t2", "gpt-4o") => Some("tlm-2".into()),
                _ => None,
            }
        };
        let updates = plan_fix_empty_tenant_model_ids(&rows, resolve);
        assert_eq!(
            updates,
            vec![
                ("kb1".to_string(), "tlm-1".to_string()),
                ("kb2".to_string(), "tlm-1".to_string()),
                ("kb4".to_string(), "tlm-2".to_string()),
            ]
        );
    }

    #[test]
    fn sync_log_counters_are_monotonic_and_pruning_respects_the_cap() {
        let mut log = SyncLogRow {
            id: "task-1".into(),
            connector_id: "c1".into(),
            kb_id: "kb".into(),
            status: "5".into(),
            poll_range_start: Some("2026-08-01T00:00:00Z".into()),
            poll_range_end: Some("2026-08-01T00:00:00Z".into()),
            new_docs_indexed: 0,
            total_docs_indexed: 0,
            docs_removed_from_index: 0,
            error_count: 0,
            error_msg: String::new(),
            update_time: 0,
        };
        // A backwards poll marker must not regress the range.
        sync_log_increase_docs(&mut log, "2026-07-01T00:00:00Z", 5, "", 0, 10);
        assert_eq!(
            log.poll_range_start.as_deref(),
            Some("2026-08-01T00:00:00Z")
        );
        assert_eq!(log.new_docs_indexed, 5);
        // Forward marker advances.
        sync_log_increase_docs(&mut log, "2026-08-02T00:00:00Z", 3, "boom", 2, 20);
        assert_eq!(log.poll_range_end.as_deref(), Some("2026-08-02T00:00:00Z"));
        assert_eq!(log.total_docs_indexed, 8);
        assert_eq!(log.error_count, 2);

        // Prune plan: 101 matching logs -> 70 oldest ids dropped.
        let logs: Vec<SyncLogRow> = (0..101)
            .map(|i| SyncLogRow {
                id: format!("l{i}"),
                connector_id: "c1".into(),
                kb_id: "kb".into(),
                status: "5".into(),
                poll_range_start: None,
                poll_range_end: None,
                new_docs_indexed: 0,
                total_docs_indexed: 0,
                docs_removed_from_index: 0,
                error_count: 0,
                error_msg: String::new(),
                update_time: i as u64,
            })
            .collect();
        let prune = sync_log_prune_plan(&logs, "c1", "kb");
        assert_eq!(prune.len(), 70);
        assert!(prune.contains(&"l0".to_string()));
        assert!(!prune.contains(&"l70".to_string()));
        // Under the cap nothing is pruned.
        assert!(sync_log_prune_plan(&logs[..99], "c1", "kb").is_empty());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// memory/services/messages.py — remaining MessageService semantics.
//
// The `MemoryMessageService` above already ports insert / query / delete /
// size accounting / FIFO eviction. This section fills the gaps:
// `index_name`, `update_message`, `delete_message` (by condition),
// `list_message` (raw-only, paginated, with extract grouping),
// `get_by_message_id` and `get_recent_messages`.
// ═══════════════════════════════════════════════════════════════════════════

/// `messages.py::MessageService.index_name` — one message index per user.
pub fn message_index_name(uid: &str) -> String {
    let prefix = std::env::var("ES_INDEX_PREFIX").unwrap_or_default();
    message_index_name_with_prefix(uid, &prefix)
}

/// Pure form of the fixed index-name contract. Only the prefix is trimmed;
/// the uid is preserved byte-for-byte, including whitespace and punctuation.
pub fn message_index_name_with_prefix(uid: &str, raw_prefix: &str) -> String {
    let prefix = raw_prefix.trim();
    if prefix.is_empty() {
        format!("memory_{uid}")
    } else {
        format!("memory_{prefix}_{uid}")
    }
}

/// Condition accepted by `update_message` / `delete_message`
/// (`msgStoreConn.update(condition, …)`); `None` fields match everything.
#[derive(Debug, Clone, Default)]
pub struct MessageCondition {
    pub memory_id: Option<String>,
    pub message_id: Option<i64>,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub status: Option<bool>,
}

impl MessageCondition {
    pub fn matches(&self, message: &MemoryMessage) -> bool {
        self.memory_id
            .as_ref()
            .is_none_or(|value| value == &message.memory_id)
            && self
                .message_id
                .is_none_or(|value| value == message.message_id)
            && self
                .agent_id
                .as_ref()
                .is_none_or(|value| value == &message.agent_id)
            && self
                .session_id
                .as_ref()
                .is_none_or(|value| value == &message.session_id)
            && self.status.is_none_or(|value| value == message.status)
    }
}

/// Field updates accepted by `update_message`; the `status` bool↔0/1
/// coercion of messages.py is native in Rust.
#[derive(Debug, Clone, Default)]
pub struct MessageUpdate {
    pub status: Option<bool>,
    pub valid_at: Option<String>,
    pub invalid_at: Option<String>,
    pub forget_at: Option<String>,
    pub content: Option<String>,
}

impl MemoryMessageService {
    /// `MessageService.update_message(condition, update_dict)` — update every
    /// message matching the condition. Returns the number of rows changed.
    pub fn update_messages(
        &self,
        condition: &MessageCondition,
        update: &MessageUpdate,
    ) -> anyhow::Result<usize> {
        self.mutate(|all| {
            let mut updated = 0usize;
            for message in all.iter_mut() {
                if !condition.matches(message) {
                    continue;
                }
                if let Some(status) = update.status {
                    message.status = status;
                }
                if let Some(valid_at) = &update.valid_at {
                    message.valid_at = valid_at.clone();
                }
                if let Some(invalid_at) = &update.invalid_at {
                    message.invalid_at = Some(invalid_at.clone());
                }
                if let Some(forget_at) = &update.forget_at {
                    message.forget_at = Some(forget_at.clone());
                }
                if let Some(content) = &update.content {
                    message.content = content.clone();
                }
                updated += 1;
            }
            Ok(updated)
        })
    }

    /// `MessageService.delete_message(condition)` — remove every message
    /// matching the condition. Returns the number of rows deleted.
    pub fn delete_messages_by_condition(
        &self,
        condition: &MessageCondition,
    ) -> anyhow::Result<usize> {
        self.mutate(|all| {
            let before = all.len();
            all.retain(|message| !condition.matches(message));
            Ok(before - all.len())
        })
    }

    /// `MessageService.get_by_message_id` — the document id is
    /// `"{memory_id}_{message_id}"`; returns `None` when absent.
    pub fn get_by_message_id(&self, memory_id: &str, message_id: i64) -> Option<MemoryMessage> {
        self.messages
            .read()
            .expect("message store lock poisoned")
            .iter()
            .find(|message| message.memory_id == memory_id && message.message_id == message_id)
            .cloned()
    }

    /// `MessageService.list_message` — raw messages (`message_type ==
    /// "raw"`) of one memory, optionally filtered by `agent_ids` and by a
    /// session keyword (`keywords` maps to `session_id`), ordered by
    /// `valid_at` descending and paginated. Returns `(page, total_count)`.
    pub fn list_message(
        &self,
        memory_id: &str,
        agent_ids: &[String],
        keywords: Option<&str>,
        page: usize,
        page_size: usize,
    ) -> (Vec<MemoryMessage>, usize) {
        let mut matching: Vec<MemoryMessage> = self
            .messages
            .read()
            .expect("message store lock poisoned")
            .iter()
            .filter(|message| message.memory_id == memory_id)
            .filter(|message| message.message_type == "raw")
            .filter(|message| agent_ids.is_empty() || agent_ids.contains(&message.agent_id))
            .filter(|message| keywords.is_none_or(|kw| message.session_id == kw))
            .cloned()
            .collect();
        matching.sort_by(|left, right| right.valid_at.cmp(&left.valid_at));
        let total_count = matching.len();
        let start = page.saturating_sub(1) * page_size;
        let page_rows = matching.into_iter().skip(start).take(page_size).collect();
        (page_rows, total_count)
    }

    /// `MessageService.get_recent_messages` — the most recent messages of an
    /// agent+session across memories, `valid_at` descending, capped at
    /// `limit`.
    pub fn recent_messages(
        &self,
        memory_ids: &[String],
        agent_id: &str,
        session_id: &str,
        limit: usize,
    ) -> Vec<MemoryMessage> {
        self.query_with_options(MemoryMessageQuery {
            memory_ids,
            agent_id: (!agent_id.is_empty()).then_some(agent_id),
            session_id: (!session_id.is_empty()).then_some(session_id),
            user_id: None,
            status: None,
            top_n: Some(limit),
            hide_forgotten: true,
        })
    }

    /// `MessageService.has_index(uid, memory_id)` on the in-process engine.
    /// An index counts as present when it was created through the lifecycle
    /// (`create_index` / the save gate) or — for legacy snapshots restored
    /// without bookkeeping — when the store already holds rows for the memory.
    pub fn has_index(&self, uid: &str, memory_id: &str) -> bool {
        let key = (message_index_name(uid), memory_id.to_string());
        let bookkept = self
            .indexes
            .read()
            .expect("index lock poisoned")
            .contains(&key);
        if bookkept {
            return true;
        }
        self.messages
            .read()
            .expect("message store lock poisoned")
            .iter()
            .any(|message| message.memory_id == memory_id)
    }

    /// `MessageService.create_index` — materialize the per-tenant index for a
    /// memory. In-process this is bookkeeping; the row store is the index.
    pub fn create_index(
        &self,
        uid: &str,
        memory_id: &str,
        _vector_size: usize,
    ) -> anyhow::Result<()> {
        self.indexes
            .write()
            .expect("index lock poisoned")
            .insert((message_index_name(uid), memory_id.to_string()));
        Ok(())
    }

    /// `MessageService.delete_index` — drop the per-tenant index bookkeeping.
    /// Rows are not removed, mirroring the native connector where the index
    /// and the documents are separate resources.
    pub fn delete_index(&self, uid: &str, memory_id: &str) -> anyhow::Result<()> {
        self.indexes
            .write()
            .expect("index lock poisoned")
            .remove(&(message_index_name(uid), memory_id.to_string()));
        Ok(())
    }

    /// `embed_and_save` cross-store transactional sequence on the in-process
    /// engine: (1) ensure the native index exists (fail → abort before any
    /// insert), (2) check the size budget against the cache (overflow + FIFO
    /// evicts and decrements the cache; any other policy is an error), (3)
    /// insert the rows (fail → abort), (4) increase the size cache. The cache
    /// is the soft Redis mirror; a miss recomputes from this store.
    pub fn save_messages_with_index_and_cache(
        &self,
        uid: &str,
        memory_id: &str,
        forgetting_policy: &str,
        memory_size: usize,
        new_messages: Vec<MemoryMessage>,
        size_cache: &MemorySizeCache,
    ) -> Result<(), ApiError> {
        let vector_size = new_messages
            .iter()
            .map(|message| message.content_embed.len())
            .max()
            .unwrap_or(0);
        if !self.has_index(uid, memory_id)
            && self.create_index(uid, memory_id, vector_size).is_err()
        {
            return Err(ApiError::admin("Failed to create message index."));
        }

        let new_msg_size: usize = new_messages.iter().map(MemoryMessage::calculate_size).sum();
        let current_memory_size = size_cache.get(self, memory_id, uid) as usize;
        if new_msg_size + current_memory_size > memory_size {
            if forgetting_policy != "FIFO" {
                return Err(ApiError::admin(MEMORY_BUDGET_ERROR));
            }
            let size_to_delete = current_memory_size + new_msg_size - memory_size;
            let ordered = {
                let mut ordered: Vec<MemoryMessage> = self
                    .messages
                    .read()
                    .expect("message store lock poisoned")
                    .iter()
                    .filter(|message| message.memory_id == memory_id)
                    .cloned()
                    .collect();
                // Infinity first physically removes rows already
                // soft-forgotten, then the oldest remaining rows.
                ordered.sort_by(|left, right| match (&left.forget_at, &right.forget_at) {
                    (Some(left), Some(right)) => left.cmp(right),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => left.valid_at.cmp(&right.valid_at),
                });
                ordered
            };
            let (ids_to_delete, delete_size) =
                pick_messages_to_delete_by_fifo(ordered.iter(), size_to_delete);
            if let Err(error) = self.delete_messages(memory_id, &ids_to_delete) {
                return Err(ApiError::from(error));
            }
            size_cache.decrease(memory_id, delete_size as i64);
        }

        if let Err(error) = self.insert_messages(new_messages) {
            return Err(ApiError::from(error));
        }
        size_cache.increase(memory_id, new_msg_size as i64);
        Ok(())
    }
}

/// `MessageService.list_message` — group extracted messages by their
/// `source_id` (the raw message they were extracted from) so each raw
/// message can be returned with its `extract` list.
pub fn group_extract_by_source(
    messages: &[MemoryMessage],
) -> std::collections::BTreeMap<i64, Vec<MemoryMessage>> {
    let mut grouped: std::collections::BTreeMap<i64, Vec<MemoryMessage>> =
        std::collections::BTreeMap::new();
    for message in messages {
        if message.source_id > 0 {
            grouped
                .entry(message.source_id)
                .or_default()
                .push(message.clone());
        }
    }
    grouped
}

// ── Memory size cache (memory_message_service.py Redis helpers) ───────────

/// In-process mirror of the Redis `memory_{memory_id}` size cache used by
/// `embed_and_save` / `get_memory_size_cache` / `increase_memory_size_cache` /
/// `decrease_memory_size_cache` / `init_memory_size_cache`.
///
/// The key is always `memory_{memory_id}` (the first positional argument of
/// every upstream helper). NOTE: v0.26.4's `embed_and_save` calls
/// `get_memory_size_cache(memory.tenant_id, memory.id)` while
/// increase/decrease pass `memory.id` first — an upstream argument-order
/// quirk. The port passes the memory id first in every call site so the
/// cache stays self-consistent; a `get` miss recomputes from the message
/// store either way, which is what makes the upstream quirk benign.
#[derive(Default)]
pub struct MemorySizeCache {
    entries: RwLock<HashMap<String, i64>>,
}

impl MemorySizeCache {
    pub fn new() -> Self {
        Self::default()
    }

    fn key(memory_id: &str) -> String {
        format!("memory_{memory_id}")
    }

    /// `get_memory_size_cache(memory_id, uid)`: hit → cached size; miss →
    /// recompute via `MessageService.calculate_memory_size`, cache it, return.
    /// `uid` only participates in the native per-tenant index namespace and
    /// is unused by the in-process recomputation.
    pub fn get(&self, store: &MemoryMessageService, memory_id: &str, _uid: &str) -> i64 {
        let key = Self::key(memory_id);
        if let Some(size) = self
            .entries
            .read()
            .expect("size cache lock poisoned")
            .get(&key)
        {
            return *size;
        }
        let size = store.calculate_memory_size(memory_id) as i64;
        self.set(memory_id, size);
        size
    }

    /// `set_memory_size_cache(memory_id, size)`.
    pub fn set(&self, memory_id: &str, size: i64) {
        self.entries
            .write()
            .expect("size cache lock poisoned")
            .insert(Self::key(memory_id), size);
    }

    /// `increase_memory_size_cache` (Redis INCRBY — returns the new value).
    pub fn increase(&self, memory_id: &str, size: i64) -> i64 {
        let mut entries = self.entries.write().expect("size cache lock poisoned");
        let value = entries.entry(Self::key(memory_id)).or_insert(0);
        *value += size;
        *value
    }

    /// `decrease_memory_size_cache` (Redis DECRBY — returns the new value).
    pub fn decrease(&self, memory_id: &str, size: i64) -> i64 {
        self.increase(memory_id, -size)
    }

    /// `init_memory_size_cache`: warm the cache for every existing memory by
    /// forcing a get (recompute-on-miss). Returns the number warmed.
    pub fn init(&self, store: &MemoryMessageService, memories: &[(String, String)]) -> usize {
        for (memory_id, tenant_id) in memories {
            self.get(store, memory_id, tenant_id);
        }
        memories.len()
    }
}

// ── Native index lifecycle on the DocStore backend ─────────────────────────
//
// RAGFlow `messages.py::MessageService.has_index / create_index /
// delete_index` delegate to `settings.msgStoreConn` (Infinity/ES). RayRAG
// has no ES client: the equivalent lifecycle runs on the existing
// `src/doc_store.rs` `DocStore` trait backend (Memory / Postgres / Zvec),
// with the `ES_INDEX_PREFIX`-derived index name actually applied to the
// native resources.

/// Native-index lifecycle port — `memory/services/messages.py` index ops
/// re-expressed on the RayRAG `DocStore` backend abstraction.
pub struct MessageIndexLifecycle {
    store: std::sync::Arc<dyn crate::doc_store::DocStore>,
}

impl MessageIndexLifecycle {
    pub fn new(store: std::sync::Arc<dyn crate::doc_store::DocStore>) -> Self {
        Self { store }
    }

    /// `MessageService.has_index(uid, memory_id)` — prefix is applied to the
    /// native index resource before the existence check.
    pub fn has_index(&self, uid: &str, memory_id: &str) -> anyhow::Result<bool> {
        self.store.index_exist(&message_index_name(uid), memory_id)
    }

    /// `MessageService.create_index(uid, memory_id, vector_size)`.
    pub fn create_index(
        &self,
        uid: &str,
        memory_id: &str,
        vector_size: usize,
    ) -> anyhow::Result<()> {
        self.store
            .create_idx(&message_index_name(uid), memory_id, vector_size)
    }

    /// `MessageService.delete_index(uid, memory_id)`.
    pub fn delete_index(&self, uid: &str, memory_id: &str) -> anyhow::Result<()> {
        self.store.delete_idx(&message_index_name(uid), memory_id)
    }

    /// `MessageService.get_missing_field_messages` + `fix_missing_tokenized_memory`'s
    /// inner repair loop: find every row of the memory missing `field`, then
    /// re-apply its own content via `update_message` so the engine refreshes
    /// the tokenized field. Returns the number of rows repaired.
    pub fn repair_missing_field(
        &self,
        uid: &str,
        memory_id: &str,
        field: &str,
    ) -> anyhow::Result<usize> {
        let index_name = message_index_name(uid);
        let response = self.store.search(&crate::doc_store::SearchQuery {
            select_fields: vec!["message_id".into(), "content".into()],
            condition: [(
                "memory_id".to_string(),
                Value::String(memory_id.to_string()),
            )]
            .into_iter()
            .collect(),
            index_names: vec![index_name.clone()],
            dataset_ids: vec![memory_id.to_string()],
            ..Default::default()
        })?;
        let mut repaired = 0usize;
        for row in &response.docs {
            // Present-but-empty also counts as missing, matching the engine's
            // empty-string tokenization gap.
            let missing = match row.get(field) {
                None => true,
                Some(Value::Null) => true,
                Some(Value::String(value)) => value.is_empty(),
                Some(_) => false,
            };
            if !missing {
                continue;
            }
            let (Some(message_id), Some(content)) = (row.get("message_id"), row.get("content"))
            else {
                continue;
            };
            let condition: crate::doc_store::FilterCondition = [
                (
                    "memory_id".to_string(),
                    Value::String(memory_id.to_string()),
                ),
                ("message_id".to_string(), message_id.clone()),
            ]
            .into_iter()
            .collect();
            let mut new_value = crate::doc_store::DocRow::new();
            new_value.insert("content".into(), content.clone());
            self.store
                .update(&condition, &new_value, &index_name, memory_id)?;
            repaired += 1;
        }
        Ok(repaired)
    }
}

/// `memory_message_service.py::fix_missing_tokenized_memory` — only the
/// elasticsearch doc engine tokenizes content; on any other engine the
/// upstream helper logs and returns. `engine_tokenizes=false` reproduces that
/// no-op guard (RayRAG's DocStore never tokenizes); `true` runs the
/// per-memory repair loop and returns the total number of repaired rows.
pub fn fix_missing_tokenized_memory(
    engine_tokenizes: bool,
    memories: &[(String, String)],
    lifecycle: &MessageIndexLifecycle,
    field: &str,
) -> anyhow::Result<usize> {
    if !engine_tokenizes {
        return Ok(0);
    }
    let mut repaired = 0usize;
    for (memory_id, tenant_id) in memories {
        repaired += lifecycle.repair_missing_field(tenant_id, memory_id, field)?;
    }
    Ok(repaired)
}

#[cfg(test)]
mod message_service_extra_tests {
    use super::*;

    fn message(message_id: i64, memory_id: &str, valid_at: &str, content: &str) -> MemoryMessage {
        MemoryMessage {
            message_id,
            message_type: "raw".into(),
            source_id: 0,
            memory_id: memory_id.into(),
            user_id: "u1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            content: content.into(),
            valid_at: valid_at.into(),
            invalid_at: None,
            forget_at: None,
            status: true,
            zone_id: 0,
            content_embed: vec![0.1, 0.2],
        }
    }

    #[test]
    fn update_message_applies_status_and_timestamps_by_condition() {
        let service = MemoryMessageService::in_memory();
        let first = message(1, "m1", "2026-08-01 00:00:00", "hello");
        let second = MemoryMessage {
            message_id: 2,
            agent_id: "a2".into(),
            ..message(0, "m1", "2026-08-02 00:00:00", "world")
        };
        service.insert_messages(vec![first, second]).unwrap();

        // Forget only agent a1's messages.
        let updated = service
            .update_messages(
                &MessageCondition {
                    agent_id: Some("a1".into()),
                    ..Default::default()
                },
                &MessageUpdate {
                    status: Some(false),
                    forget_at: Some("2026-08-10 00:00:00".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated, 1);
        let row = service.get_by_message_id("m1", 1).unwrap();
        assert!(!row.status);
        assert_eq!(row.forget_at.as_deref(), Some("2026-08-10 00:00:00"));
        let untouched = service.get_by_message_id("m1", 2).unwrap();
        assert!(untouched.status);

        // message_id condition + content rewrite.
        let updated = service
            .update_messages(
                &MessageCondition {
                    message_id: Some(2),
                    ..Default::default()
                },
                &MessageUpdate {
                    content: Some("updated".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated, 1);
        assert_eq!(
            service.get_by_message_id("m1", 2).unwrap().content,
            "updated"
        );
    }

    #[test]
    fn list_message_filters_raw_paginates_and_groups_extracts() {
        let service = MemoryMessageService::in_memory();
        let mut rows = vec![
            message(1, "m1", "2026-08-01 00:00:00", "first"),
            message(2, "m1", "2026-08-02 00:00:00", "second"),
            message(3, "m1", "2026-08-03 00:00:00", "third"),
        ];
        // One extracted (semantic) message derived from raw message 1.
        rows.push(MemoryMessage {
            message_id: 4,
            message_type: "semantic".into(),
            source_id: 1,
            content: "extracted".into(),
            ..message(0, "m1", "2026-08-03 12:00:00", "ignored")
        });
        service.insert_messages(rows).unwrap();

        let (page, total) = service.list_message("m1", &[], None, 1, 2);
        assert_eq!(total, 3); // semantic rows are excluded
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].message_id, 3); // valid_at desc
        assert_eq!(page[1].message_id, 2);

        let (page, total) = service.list_message("m1", &[], None, 2, 2);
        assert_eq!(total, 3);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].message_id, 1);

        // keywords maps to the session_id filter.
        let (_page, total) = service.list_message("m1", &[], Some("s1"), 1, 50);
        assert_eq!(total, 3);
        let (page, total) = service.list_message("m1", &[], Some("nope"), 1, 50);
        assert_eq!(total, 0);
        assert!(page.is_empty());

        // Extract grouping: semantic message 4 derives from raw message 1.
        let all = service.query(&["m1".into()], None, None, None, None, None);
        let grouped = group_extract_by_source(&all);
        assert_eq!(grouped[&1].len(), 1);
        assert_eq!(grouped[&1][0].message_id, 4);
        assert!(!grouped.contains_key(&2));
    }

    #[test]
    fn delete_by_condition_and_recent_messages_match_ragflow_semantics() {
        let service = MemoryMessageService::in_memory();
        let mut rows = vec![
            message(1, "m1", "2026-08-01 00:00:00", "old"),
            message(2, "m1", "2026-08-02 00:00:00", "mid"),
            message(3, "m1", "2026-08-03 00:00:00", "new"),
        ];
        rows.push(MemoryMessage {
            message_id: 4,
            agent_id: "a2".into(),
            ..message(0, "m1", "2026-08-04 00:00:00", "other-agent")
        });
        service.insert_messages(rows).unwrap();

        // get_recent_messages: agent + session filter, newest first, capped.
        let recent = service.recent_messages(&["m1".into()], "a1", "s1", 2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].message_id, 3);
        assert_eq!(recent[1].message_id, 2);

        // delete_message(condition): drop agent a2's rows.
        let deleted = service
            .delete_messages_by_condition(&MessageCondition {
                agent_id: Some("a2".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(deleted, 1);
        assert!(service.get_by_message_id("m1", 4).is_none());
        assert!(service.get_by_message_id("m1", 1).is_some());

        // The pure helper avoids mutating the process environment in parallel
        // tests while exercising the exact dynamic ES_INDEX_PREFIX contract.
        assert_eq!(message_index_name_with_prefix("u1", ""), "memory_u1");
        assert_eq!(message_index_name_with_prefix("u1", "   "), "memory_u1");
        assert_eq!(
            message_index_name_with_prefix(" u1 ", "  cn prod  "),
            "memory_cn prod_ u1 "
        );
    }

    #[test]
    fn condition_delete_does_not_cross_memory_for_a_reused_integer_id() {
        let service = MemoryMessageService::in_memory();
        let first = message(7, "m1", "2026-08-01 00:00:00", "delete");
        let second = message(7, "m2", "2026-08-01 00:00:00", "keep");
        service
            .insert_messages(vec![first, second.clone()])
            .unwrap();

        assert_eq!(
            service
                .delete_messages_by_condition(&MessageCondition {
                    memory_id: Some("m1".into()),
                    message_id: Some(7),
                    ..Default::default()
                })
                .unwrap(),
            1
        );
        assert!(service.get_by_message_id("m1", 7).is_none());
        assert_eq!(service.get_by_message_id("m2", 7), Some(second));
    }

    #[test]
    fn canonical_snapshot_uses_exact_infinity_fields_and_restores() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-memory-message-snapshot-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("memory_messages.json");
        let service = MemoryMessageService::new(&path).unwrap();
        let mut stored = message(7, "memory-a", "2026-08-10 12:34:56", "remember this");
        stored.zone_id = 9;
        service.insert_messages(vec![stored.clone()]).unwrap();

        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let row = rows[0].as_object().unwrap();
        assert_eq!(row["id"], "memory-a_7");
        assert_eq!(row["message_type_kwd"], "raw");
        assert_eq!(row["status_int"], 1);
        assert_eq!(row["zone_id"], 9);
        assert_eq!(row["invalid_at"], "");
        assert_eq!(row["invalid_at_flt"], 0.0);
        assert_eq!(row["forget_at"], "");
        assert_eq!(row["forget_at_flt"], 0.0);
        assert_eq!(
            row["q_2_vec"],
            serde_json::to_value(vec![0.1_f32, 0.2_f32]).unwrap()
        );
        assert_eq!(
            row["valid_at_flt"],
            crate::common::time_utils::date_string_to_timestamp(
                "2026-08-10 12:34:56",
                crate::common::time_utils::DEFAULT_TIME_FORMAT,
            )
            .unwrap() as f64
        );
        for logical in ["message_type", "status", "content_embed"] {
            assert!(!row.contains_key(logical));
        }

        drop(service);
        let restored = MemoryMessageService::new(&path).unwrap();
        assert_eq!(restored.get_by_message_id("memory-a", 7), Some(stored));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_logical_snapshot_migrates_to_canonical_storage() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-memory-message-legacy-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("memory_messages.json");
        std::fs::write(
            &path,
            serde_json::to_vec_pretty(&serde_json::json!([{
                "message_id": 3,
                "message_type": "raw",
                "source_id": 0,
                "memory_id": "legacy-memory",
                "user_id": "u1",
                "agent_id": "a1",
                "session_id": "s1",
                "content": "legacy",
                "valid_at": "2026-08-01 00:00:00",
                "invalid_at": null,
                "forget_at": null,
                "status": true,
                "content_embed": [1.0, 0.0]
            }]))
            .unwrap(),
        )
        .unwrap();

        let service = MemoryMessageService::new(&path).unwrap();
        assert_eq!(
            service
                .get_by_message_id("legacy-memory", 3)
                .unwrap()
                .content_embed,
            vec![1.0, 0.0]
        );
        let rows: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let row = rows[0].as_object().unwrap();
        assert_eq!(row["id"], "legacy-memory_3");
        assert!(row.contains_key("message_type_kwd"));
        assert!(row.contains_key("q_2_vec"));
        assert!(!row.contains_key("message_type"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn search_recent_and_list_apply_distinct_status_and_forget_rules() {
        let service = MemoryMessageService::in_memory();
        let active = message(1, "m1", "2026-08-01 00:00:00", "active rust");
        let inactive = MemoryMessage {
            message_id: 2,
            status: false,
            valid_at: "2026-08-02 00:00:00".into(),
            content: "inactive rust".into(),
            ..active.clone()
        };
        let forgotten = MemoryMessage {
            message_id: 3,
            valid_at: "2026-08-03 00:00:00".into(),
            forget_at: Some("2026-08-04 00:00:00".into()),
            content: "forgotten rust".into(),
            ..active.clone()
        };
        service
            .insert_messages(vec![active, inactive, forgotten])
            .unwrap();

        let search = service.query(&["m1".into()], None, None, None, None, None);
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].message_id, 1);
        let recent = service.recent_messages(&["m1".into()], "a1", "s1", 10);
        assert_eq!(
            recent
                .iter()
                .map(|message| message.message_id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        let (listed, total) = service.list_message("m1", &[], None, 1, 10);
        assert_eq!(total, 3);
        assert_eq!(listed[0].message_id, 3);
    }

    #[test]
    fn persistence_failure_rolls_back_message_update() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-memory-message-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("memory_messages.json");
        let service = MemoryMessageService::new(&path).unwrap();
        service
            .insert_messages(vec![message(1, "m1", "2026-08-01 00:00:00", "stable")])
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            service
                .update_messages(
                    &MessageCondition {
                        memory_id: Some("m1".into()),
                        message_id: Some(1),
                        ..Default::default()
                    },
                    &MessageUpdate {
                        content: Some("must rollback".into()),
                        ..Default::default()
                    },
                )
                .is_err()
        );
        assert_eq!(
            service.get_by_message_id("m1", 1).unwrap().content,
            "stable"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn persistence_failure_rolls_back_fifo_and_insert_as_one_transaction() {
        let root = std::env::temp_dir().join(format!(
            "rayrag-memory-message-fifo-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("memory_messages.json");
        let service = MemoryMessageService::new(&path).unwrap();
        let stable = message(1, "m1", "2026-08-01 00:00:00", "stable");
        service.insert_messages(vec![stable.clone()]).unwrap();

        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let incoming = message(2, "m1", "2026-08-02 00:00:00", "incoming");
        let result = service.save_messages_with_budget(
            "m1",
            "FIFO",
            incoming.calculate_size(),
            vec![incoming],
        );
        assert!(result.is_err());
        assert_eq!(service.get_by_message_id("m1", 1), Some(stable));
        assert!(service.get_by_message_id("m1", 2).is_none());

        std::fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod memory_extraction_tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct RecordedChatCall {
        system: String,
        history: Vec<(String, String)>,
        temperature: Option<f32>,
    }

    struct FakeMemoryChatModel {
        response: String,
        fail: bool,
        calls: Mutex<Vec<RecordedChatCall>>,
    }

    impl FakeMemoryChatModel {
        fn returning(response: &str) -> Self {
            Self {
                response: response.into(),
                fail: false,
                calls: Mutex::new(Vec::new()),
            }
        }

        fn failing() -> Self {
            Self {
                response: String::new(),
                fail: true,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ChatModel for FakeMemoryChatModel {
        fn model_name(&self) -> &str {
            "fake-memory-chat"
        }

        async fn chat(&self, _system: &str, _history: &[ChatMessage]) -> crate::Result<String> {
            if self.fail {
                anyhow::bail!("fake memory LLM failed")
            }
            Ok(self.response.clone())
        }

        async fn chat_with_generation(
            &self,
            system: &str,
            history: &[ChatMessage],
            generation: GenerationParamsPatch,
        ) -> crate::Result<String> {
            self.calls.lock().unwrap().push(RecordedChatCall {
                system: system.into(),
                history: history
                    .iter()
                    .map(|message| (message.role.clone(), message.content.clone()))
                    .collect(),
                temperature: generation.temperature,
            });
            self.chat(system, history).await
        }

        async fn chat_stream(
            &self,
            system: &str,
            history: &[ChatMessage],
            _on_chunk: Box<dyn for<'a> FnMut(&'a str) + Send>,
        ) -> crate::Result<String> {
            self.chat(system, history).await
        }
    }

    struct LegacyFakeChatModel;

    #[async_trait::async_trait]
    impl ChatModel for LegacyFakeChatModel {
        fn model_name(&self) -> &str {
            "legacy"
        }

        async fn chat(&self, system: &str, history: &[ChatMessage]) -> crate::Result<String> {
            Ok(format!("{system}:{}", history.len()))
        }

        async fn chat_stream(
            &self,
            system: &str,
            history: &[ChatMessage],
            _on_chunk: Box<dyn for<'a> FnMut(&'a str) + Send>,
        ) -> crate::Result<String> {
            self.chat(system, history).await
        }
    }

    fn memory_types(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn extraction_helper_reuses_primary_prompt_assembler_contract() {
        let types = memory_types(&["raw", "semantic", "episodic", "procedural", "semantic"]);
        let prompt = crate::memory::PromptAssembler::assemble_system_prompt(&types);
        assert_eq!(
            prompt,
            crate::memory::PromptAssembler::assemble_system_prompt(&types)
        );
        assert!(prompt.starts_with("**Memory Extraction Specialist**"));
        assert_eq!(prompt.matches("**EXTRACT SEMANTIC KNOWLEDGE:**").count(), 1);
        assert_eq!(prompt.matches("**EXTRACT EPISODIC KNOWLEDGE:**").count(), 1);
        assert_eq!(
            prompt.matches("**EXTRACT PROCEDURAL KNOWLEDGE:**").count(),
            1
        );
        assert!(prompt.contains("Timestamps in ISO 8601 format"));
        assert!(prompt.contains("Maximum 5 items per type"));
        assert!(prompt.contains("\"semantic\": ["));
        assert!(prompt.contains("\"episodic\": ["));
        assert!(prompt.contains("\"procedural\": ["));
        assert!(!prompt.contains("\"raw\": ["));

        assert_eq!(
            crate::memory::PromptAssembler::assemble_user_prompt(
                "User Input: hello\nAgent Response: world",
                Some("2026-08-12 12:34:56"),
                Some("2026-08-12 12:34:56"),
            ),
            "\n**CONVERSATION:**\nUser Input: hello\nAgent Response: world\n\n**CONVERSATION TIME:** 2026-08-12 12:34:56\n**CURRENT TIME:** 2026-08-12 12:34:56\n"
        );

        let raw_prompt =
            crate::memory::PromptAssembler::assemble_system_prompt(&memory_types(&["raw"]));
        assert!(!raw_prompt.contains("**EXTRACT SEMANTIC KNOWLEDGE:**"));
        assert!(raw_prompt.contains("```json\n{\n\n}\n```"));
        assert!(!raw_prompt.contains("**EXAMPLES:**"));
    }

    #[test]
    fn response_parser_flattens_all_types_preserves_unknown_and_normalizes_time() {
        let response = r#"```json
        {
          "semantic": [{"content":"Paris is in France","valid_at":"2024-01-01T12:00:00Z","invalid_at":""}],
          "episodic": [{"content":"Deployed","valid_at":"2024-01-02T14:15:16+08:00"}],
          "procedural": [{"content":"Check logs","valid_at":"not-a-time","invalid_at":null}],
          "future_type": [{"content":"Keep this key","valid_at":"2024-01-03"}]
        }
        ```"#;
        let items = parse_memory_extraction_response(response).unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].message_type, "semantic");
        assert_eq!(items[0].valid_at, "2024-01-01 12:00:00");
        assert_eq!(items[0].invalid_at, None);
        assert_eq!(items[1].message_type, "episodic");
        assert_eq!(items[1].valid_at, "2024-01-02 14:15:16");
        assert_eq!(items[2].message_type, "procedural");
        assert_eq!(items[2].valid_at, "not-a-time");
        assert_eq!(items[3].message_type, "future_type");
        assert_eq!(items[3].valid_at, "2024-01-03 00:00:00");
    }

    #[test]
    fn response_parser_distinguishes_invalid_json_from_structural_errors() {
        assert!(
            parse_memory_extraction_response("not json")
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_memory_extraction_response("{broken")
                .unwrap()
                .is_empty()
        );
        assert!(parse_memory_extraction_response("[]").is_err());
        assert!(parse_memory_extraction_response(r#"{"semantic":{}}"#).is_err());
        assert!(parse_memory_extraction_response(r#"{"semantic":["fact"]}"#).is_err());
        assert!(
            parse_memory_extraction_response(r#"{"semantic":[{"valid_at":"2024-01-01"}]}"#)
                .is_err()
        );
        assert!(parse_memory_extraction_response(r#"{"semantic":[{"content":"fact"}]}"#).is_err());
    }

    #[tokio::test]
    async fn extraction_sends_default_system_history_and_temperature_then_flattens() {
        let model = FakeMemoryChatModel::returning(
            r#"{"semantic":[{"content":"fact","valid_at":"2026-08-12T10:00:00","invalid_at":""}],"episodic":[],"procedural":[]}"#,
        );
        let items = extract_memory_by_llm(
            &model,
            &memory_types(&["raw", "semantic", "episodic", "procedural"]),
            "hello",
            "world",
            None,
            None,
            "2026-08-12 12:34:56",
            0.7,
        )
        .await
        .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].message_type, "semantic");
        let calls = model.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].system,
            crate::memory::PromptAssembler::assemble_system_prompt(&memory_types(&[
                "raw",
                "semantic",
                "episodic",
                "procedural",
            ]))
        );
        assert!(calls[0].system.contains("**EXTRACT SEMANTIC KNOWLEDGE:**"));
        assert!(calls[0].system.contains("**EXTRACT EPISODIC KNOWLEDGE:**"));
        assert!(
            calls[0]
                .system
                .contains("**EXTRACT PROCEDURAL KNOWLEDGE:**")
        );
        assert_eq!(calls[0].temperature, Some(0.7));
        assert_eq!(calls[0].history.len(), 1);
        assert_eq!(calls[0].history[0].0, "user");
        assert_eq!(
            calls[0].history[0].1,
            crate::memory::PromptAssembler::assemble_user_prompt(
                "User Input: hello\nAgent Response: world",
                Some("2026-08-12 12:34:56"),
                Some("2026-08-12 12:34:56"),
            )
        );
    }

    #[tokio::test]
    async fn extraction_custom_prompts_raw_only_empty_and_failure_match_worker_boundaries() {
        let custom = FakeMemoryChatModel::returning("{}");
        let items = extract_memory_by_llm(
            &custom,
            &memory_types(&["semantic"]),
            "question",
            "answer",
            Some("custom system"),
            Some("custom user"),
            "2026-08-12 01:02:03",
            0.2,
        )
        .await
        .unwrap();
        assert!(items.is_empty());
        {
            let calls = custom.calls.lock().unwrap();
            assert_eq!(calls[0].system, "custom system");
            assert_eq!(calls[0].temperature, Some(0.2));
            assert_eq!(
                calls[0].history,
                vec![
                    ("user".into(), "custom user".into()),
                    (
                        "user".into(),
                        "Conversation: User Input: question\nAgent Response: answer\nConversation Time: 2026-08-12 01:02:03\nCurrent Time: 2026-08-12 01:02:03".into(),
                    ),
                ]
            );
        }

        let raw_only = FakeMemoryChatModel::failing();
        assert!(
            extract_memory_by_llm(
                &raw_only,
                &memory_types(&["raw"]),
                "question",
                "answer",
                None,
                None,
                "2026-08-12 01:02:03",
                0.5,
            )
            .await
            .unwrap()
            .is_empty()
        );
        assert!(raw_only.calls.lock().unwrap().is_empty());

        let invalid_json = FakeMemoryChatModel::returning("provider preamble");
        assert!(
            extract_memory_by_llm(
                &invalid_json,
                &memory_types(&["semantic"]),
                "question",
                "answer",
                None,
                None,
                "2026-08-12 01:02:03",
                0.5,
            )
            .await
            .unwrap()
            .is_empty()
        );

        let failing = FakeMemoryChatModel::failing();
        assert!(
            extract_memory_by_llm(
                &failing,
                &memory_types(&["semantic"]),
                "question",
                "answer",
                None,
                None,
                "2026-08-12 01:02:03",
                0.5,
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("fake memory LLM failed")
        );
    }

    #[tokio::test]
    async fn chat_model_generation_default_keeps_legacy_implementations_compatible() {
        let answer = LegacyFakeChatModel
            .chat_with_generation(
                "system",
                &[ChatMessage::new("user", "hello")],
                GenerationParamsPatch {
                    temperature: Some(0.9),
                    ..GenerationParamsPatch::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(answer, "system:1");
    }
}

#[cfg(test)]
mod memory_index_cache_tests {
    use super::*;
    // Trait methods (`index_exist` / `insert` / `get`) live on the `DocStore`
    // trait; bring it into scope for the `Arc<MemoryDocStore>` receivers.
    use crate::doc_store::DocStore;

    fn message(message_id: i64, memory_id: &str, content: &str) -> MemoryMessage {
        MemoryMessage {
            message_id,
            message_type: "raw".into(),
            source_id: 0,
            memory_id: memory_id.into(),
            user_id: "u1".into(),
            agent_id: "a1".into(),
            session_id: "s1".into(),
            content: content.into(),
            valid_at: format!("2026-08-0{message_id} 00:00:00"),
            invalid_at: None,
            forget_at: None,
            status: true,
            zone_id: 0,
            content_embed: vec![0.1, 0.2],
        }
    }

    // ── messages.py MessageService index lifecycle (DocStore backend) ──

    #[test]
    fn native_index_lifecycle_applies_prefix_and_tracks_creation() {
        let store = std::sync::Arc::new(crate::doc_store::MemoryDocStore::new());
        let lifecycle = MessageIndexLifecycle::new(store.clone());
        assert!(!lifecycle.has_index("tenant-1", "memory-1").unwrap());
        lifecycle.create_index("tenant-1", "memory-1", 768).unwrap();
        assert!(lifecycle.has_index("tenant-1", "memory-1").unwrap());
        // Index names are per-tenant: another uid does not see it.
        assert!(!lifecycle.has_index("tenant-2", "memory-1").unwrap());
        // The `memory_{uid}` name was applied to the native resource.
        assert!(
            store
                .index_exist(&message_index_name("tenant-1"), "memory-1")
                .unwrap()
        );
        lifecycle.delete_index("tenant-1", "memory-1").unwrap();
        assert!(!lifecycle.has_index("tenant-1", "memory-1").unwrap());
    }

    #[test]
    fn prefix_application_matches_upstream_index_name_contract() {
        // Pure form: trimmed prefix yields `memory_{prefix}_{uid}`.
        assert_eq!(
            message_index_name_with_prefix("u1", "rayrag"),
            "memory_rayrag_u1"
        );
        assert_eq!(
            message_index_name_with_prefix("u1", "  rayrag  "),
            "memory_rayrag_u1"
        );
        assert_eq!(message_index_name_with_prefix("u1", ""), "memory_u1");
        assert_eq!(message_index_name_with_prefix("u 1", "p"), "memory_p_u 1");
    }

    #[test]
    fn missing_field_repair_reapplies_content_and_counts_only_gaps() {
        let store = std::sync::Arc::new(crate::doc_store::MemoryDocStore::new());
        let lifecycle = MessageIndexLifecycle::new(store.clone());
        lifecycle.create_index("tenant-1", "memory-1", 768).unwrap();
        let mut complete = Map::new();
        complete.insert("id".into(), Value::String("memory-1_3".into()));
        complete.insert("memory_id".into(), Value::String("memory-1".into()));
        complete.insert("message_id".into(), Value::from(3i64));
        complete.insert("content".into(), Value::String("already tokenized".into()));
        complete.insert(
            "tokenized_content_ltks".into(),
            Value::String("tokens".into()),
        );
        let mut missing = Map::new();
        missing.insert("id".into(), Value::String("memory-1_1".into()));
        missing.insert("memory_id".into(), Value::String("memory-1".into()));
        missing.insert("message_id".into(), Value::from(1i64));
        missing.insert("content".into(), Value::String("first".into()));
        let mut empty = Map::new();
        empty.insert("id".into(), Value::String("memory-1_2".into()));
        empty.insert("memory_id".into(), Value::String("memory-1".into()));
        empty.insert("message_id".into(), Value::from(2i64));
        empty.insert("content".into(), Value::String("second".into()));
        empty.insert(
            "tokenized_content_ltks".into(),
            Value::String(String::new()),
        );
        store
            .insert(
                &[complete.clone(), missing, empty],
                &message_index_name("tenant-1"),
                "memory-1",
            )
            .unwrap();

        let repaired = lifecycle
            .repair_missing_field("tenant-1", "memory-1", "tokenized_content_ltks")
            .unwrap();
        assert_eq!(repaired, 2);
        // The complete row is untouched (content unchanged, field intact).
        let after = store
            .get(
                "memory-1_3",
                &message_index_name("tenant-1"),
                &["memory-1".into()],
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            after.get("tokenized_content_ltks"),
            Some(&Value::String("tokens".into()))
        );
    }

    #[test]
    fn fix_missing_tokenized_memory_is_a_noop_without_a_tokenizing_engine() {
        let store = std::sync::Arc::new(crate::doc_store::MemoryDocStore::new());
        let lifecycle = MessageIndexLifecycle::new(store);
        let memories = vec![("memory-1".to_string(), "tenant-1".to_string())];
        assert_eq!(
            fix_missing_tokenized_memory(false, &memories, &lifecycle, "tokenized_content_ltks")
                .unwrap(),
            0
        );
        assert_eq!(
            fix_missing_tokenized_memory(true, &memories, &lifecycle, "tokenized_content_ltks")
                .unwrap(),
            0
        );
    }

    // ── MemorySizeCache (memory_message_service.py Redis helpers) ──

    #[test]
    fn size_cache_recomputes_on_miss_and_tracks_increase_decrease() {
        let store = MemoryMessageService::in_memory();
        store
            .insert_messages(vec![message(1, "m1", "hello")])
            .unwrap();
        let cache = MemorySizeCache::new();
        assert_eq!(
            cache.get(&store, "m1", "tenant-1"),
            message(1, "m1", "hello").calculate_size() as i64
        );
        assert_eq!(
            cache.increase("m1", 100),
            message(1, "m1", "hello").calculate_size() as i64 + 100
        );
        assert_eq!(
            cache.decrease("m1", 30),
            message(1, "m1", "hello").calculate_size() as i64 + 70
        );
        cache.set("m2", 500);
        assert_eq!(cache.get(&store, "m2", "tenant-1"), 500);
        // Cache keys are per memory id, never per tenant.
        assert_eq!(cache.get(&store, "m3", "tenant-9"), 0);
        assert_eq!(cache.get(&store, "m3", "tenant-8"), 0);
    }

    #[test]
    fn init_size_cache_warms_every_memory() {
        let store = MemoryMessageService::in_memory();
        store
            .insert_messages(vec![message(1, "m1", "hello")])
            .unwrap();
        let cache = MemorySizeCache::new();
        let warmed = cache.init(
            &store,
            &[
                ("m1".to_string(), "tenant-1".to_string()),
                ("m2".to_string(), "tenant-2".to_string()),
            ],
        );
        assert_eq!(warmed, 2);
        assert!(cache.get(&store, "m1", "tenant-1") > 0);
        assert_eq!(cache.get(&store, "m2", "tenant-2"), 0);
    }

    // ── embed_and_save cross-store transactional sequence ──

    #[test]
    fn transactional_save_creates_index_evicts_fifo_and_keeps_cache_consistent() {
        let store = MemoryMessageService::in_memory();
        let cache = MemorySizeCache::new();
        let budget = 3 * message(1, "m1", "placeholder").calculate_size();
        let first = message(1, "m1", "oldest");
        let second = message(2, "m1", "middle");
        let two_row_size = first.calculate_size() + second.calculate_size();
        store
            .save_messages_with_index_and_cache(
                "tenant-1",
                "m1",
                "FIFO",
                budget,
                vec![first, second],
                &cache,
            )
            .unwrap();
        assert!(store.has_index("tenant-1", "m1"));
        let cached = cache.get(&store, "m1", "tenant-1");
        assert_eq!(cached, two_row_size as i64);

        // Overflow: FIFO evicts the oldest rows and decrements the cache.
        let bigger = "bigger".repeat(4);
        let replacement = message(3, "m1", &bigger);
        store
            .save_messages_with_index_and_cache(
                "tenant-1",
                "m1",
                "FIFO",
                budget,
                vec![replacement.clone()],
                &cache,
            )
            .unwrap();
        assert_eq!(
            store.calculate_memory_size("m1"),
            cache.get(&store, "m1", "tenant-1") as usize
        );
        assert!(store.calculate_memory_size("m1") <= budget);
        assert!(store.get_by_message_id("m1", 3).is_some());
    }

    #[test]
    fn transactional_save_rejects_non_fifo_overflow_and_leaves_store_untouched() {
        let store = MemoryMessageService::in_memory();
        let cache = MemorySizeCache::new();
        let budget = 10;
        let error = store
            .save_messages_with_index_and_cache(
                "tenant-1",
                "m1",
                "LRU",
                budget,
                vec![message(1, "m1", "content")],
                &cache,
            )
            .unwrap_err();
        assert_eq!(error.message, MEMORY_BUDGET_ERROR);
        // Nothing inserted; the index was already created before the budget
        // check, matching the upstream create-then-budget order.
        assert_eq!(store.calculate_memory_size("m1"), 0);
        assert!(store.has_index("tenant-1", "m1"));
    }

    #[test]
    fn legacy_rows_without_index_bookkeeping_still_count_as_indexed() {
        let store = MemoryMessageService::in_memory();
        store
            .insert_messages(vec![message(1, "m1", "legacy")])
            .unwrap();
        // No create_index call — rows alone must satisfy the has_index gate so
        // delete flows reach restored snapshots.
        assert!(store.has_index("tenant-1", "m1"));
        assert!(!store.has_index("tenant-1", "m2"));
    }
}
