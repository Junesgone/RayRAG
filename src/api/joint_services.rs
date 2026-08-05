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
use serde::{Deserialize, Serialize};
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default)]
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
}

/// `memory_message_service.py::save_to_memory` raw message content format:
/// `"User Input: {user_input}\nAgent Response: {agent_response}"`.
pub fn build_raw_message_content(user_input: &str, agent_response: &str) -> String {
    format!("User Input: {user_input}\nAgent Response: {agent_response}")
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
}

impl MemoryMessageService {
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let messages: Vec<MemoryMessage> = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
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
        }
    }

    /// `REDIS_CONN.generate_auto_increment_id(namespace="memory")`.
    pub fn next_message_id(&self) -> i64 {
        self.next_message_id.fetch_add(1, Ordering::SeqCst)
    }

    pub fn insert_messages(&self, messages: Vec<MemoryMessage>) -> anyhow::Result<()> {
        {
            let mut all = self.messages.write().expect("message store lock poisoned");
            all.extend(messages);
        }
        self.persist()
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
        let mut results: Vec<_> = self
            .messages
            .read()
            .expect("message store lock poisoned")
            .iter()
            .filter(|message| memory_ids.contains(&message.memory_id))
            .filter(|message| agent_id.is_none_or(|value| message.agent_id == value))
            .filter(|message| session_id.is_none_or(|value| message.session_id == value))
            .filter(|message| user_id.is_none_or(|value| message.user_id == value))
            .filter(|message| status.unwrap_or(true) == message.status)
            .cloned()
            .collect();
        // Order by valid_at desc — RAGFlow's OrderByExpr(desc).
        results.sort_by(|left, right| right.valid_at.cmp(&left.valid_at));
        if let Some(top_n) = top_n {
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

    pub fn delete_messages(&self, ids: &[i64]) -> usize {
        let deleted = {
            let mut all = self.messages.write().expect("message store lock poisoned");
            let before = all.len();
            all.retain(|message| !ids.contains(&message.message_id));
            before - all.len()
        };
        if deleted > 0 {
            let _ = self.persist();
        }
        deleted
    }

    pub fn delete_by_memory(&self, memory_id: &str) -> usize {
        let deleted = {
            let mut all = self.messages.write().expect("message store lock poisoned");
            let before = all.len();
            all.retain(|message| message.memory_id != memory_id);
            before - all.len()
        };
        if deleted > 0 {
            let _ = self.persist();
        }
        deleted
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
        let current_memory_size = self.calculate_memory_size(memory_id);
        if new_msg_size + current_memory_size > memory_size {
            let size_to_delete = current_memory_size + new_msg_size - memory_size;
            if forgetting_policy != "FIFO" {
                return Err(ApiError::admin(
                    "Failed to insert message into memory. Memory size reached limit and cannot decide which to delete.",
                ));
            }
            let mut ordered: Vec<MemoryMessage> = {
                let guard = self.messages.read().expect("message store lock poisoned");
                guard
                    .iter()
                    .filter(|message| message.memory_id == memory_id)
                    .cloned()
                    .collect()
            };
            ordered.sort_by(|left, right| left.valid_at.cmp(&right.valid_at));
            let (ids, _freed) = pick_messages_to_delete_by_fifo(ordered.iter(), size_to_delete);
            if !ids.is_empty() {
                self.delete_messages(&ids);
            }
        }
        self.insert_messages(new_messages)
            .map_err(ApiError::from)?;
        Ok(())
    }

    fn persist(&self) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let snapshot = self
            .messages
            .read()
            .expect("message store lock poisoned")
            .clone();
        let data = serde_json::to_vec_pretty(&snapshot)?;
        let _guard = self.save_lock.lock().expect("save lock poisoned");
        crate::persistence::atomic_write(path, &data)
    }
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
        .ok_or_else(|| {
            ApiError::admin(format!(
                "Tenant Model with id {tenant_model_id} not found"
            ))
        })
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
            content_embed: vec![0.1, 0.2],
        }
    }

    #[test]
    fn fifo_eviction_frees_oldest_messages_first() {
        let messages = vec![
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
        assert!(err
            .message
            .contains("Memory size reached limit and cannot decide which to delete"));
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
        assert!(plan
            .tenant_llms
            .iter()
            .all(|record| record.tenant_id == plan.user.id));
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
