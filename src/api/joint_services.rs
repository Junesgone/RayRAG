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
        self.insert_messages(new_messages).map_err(ApiError::from)?;
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
        assert!(
            err.message
                .contains("Memory size reached limit and cannot decide which to delete")
        );
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

    let mut push = |category: &str,
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
        .filter(|s| id_list.map_or(true, |ids| ids.iter().any(|id| id == &s.id)))
        .filter(|s| {
            keywords.map_or(true, |kw| {
                s.name.to_lowercase().contains(&kw.to_lowercase())
            })
        })
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
        .filter(|s| id.map_or(true, |v| v == s.id))
        .filter(|s| name.map_or(true, |v| v == s.name))
        .filter(|s| user_id.map_or(true, |v| v == s.user_id))
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
    format!("memory_{uid}")
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
        self.memory_id.as_ref().is_none_or(|value| value == &message.memory_id)
            && self.message_id.is_none_or(|value| value == message.message_id)
            && self.agent_id.as_ref().is_none_or(|value| value == &message.agent_id)
            && self.session_id.as_ref().is_none_or(|value| value == &message.session_id)
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
    pub fn update_messages(&self, condition: &MessageCondition, update: &MessageUpdate) -> usize {
        let mut updated = 0usize;
        {
            let mut all = self.messages.write().expect("message store lock poisoned");
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
        }
        if updated > 0 {
            let _ = self.persist();
        }
        updated
    }

    /// `MessageService.delete_message(condition)` — remove every message
    /// matching the condition. Returns the number of rows deleted.
    pub fn delete_messages_by_condition(&self, condition: &MessageCondition) -> usize {
        let ids: Vec<i64> = self
            .messages
            .read()
            .expect("message store lock poisoned")
            .iter()
            .filter(|message| condition.matches(message))
            .map(|message| message.message_id)
            .collect();
        self.delete_messages(&ids)
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
            .filter(|message| keywords.is_none_or(|kw| message.session_id.contains(kw)))
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
        self.query(memory_ids, Some(agent_id), Some(session_id), None, None, Some(limit))
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
        let updated = service.update_messages(
            &MessageCondition {
                agent_id: Some("a1".into()),
                ..Default::default()
            },
            &MessageUpdate {
                status: Some(false),
                forget_at: Some("2026-08-10 00:00:00".into()),
                ..Default::default()
            },
        );
        assert_eq!(updated, 1);
        let row = service.get_by_message_id("m1", 1).unwrap();
        assert!(!row.status);
        assert_eq!(row.forget_at.as_deref(), Some("2026-08-10 00:00:00"));
        let untouched = service.get_by_message_id("m1", 2).unwrap();
        assert!(untouched.status);

        // message_id condition + content rewrite.
        let updated = service.update_messages(
            &MessageCondition {
                message_id: Some(2),
                ..Default::default()
            },
            &MessageUpdate {
                content: Some("updated".into()),
                ..Default::default()
            },
        );
        assert_eq!(updated, 1);
        assert_eq!(service.get_by_message_id("m1", 2).unwrap().content, "updated");
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
        let (page, total) = service.list_message("m1", &[], Some("s1"), 1, 50);
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
        let deleted = service.delete_messages_by_condition(&MessageCondition {
            agent_id: Some("a2".into()),
            ..Default::default()
        });
        assert_eq!(deleted, 1);
        assert!(service.get_by_message_id("m1", 4).is_none());
        assert!(service.get_by_message_id("m1", 1).is_some());

        // index_name mirrors messages.py.
        assert_eq!(message_index_name("u1"), "memory_u1");
    }
}
