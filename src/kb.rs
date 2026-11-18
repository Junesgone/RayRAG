//! Knowledge Base manager — CRUD operations with JSON persistence.
//!
//! Replaces RAGFlow's `api/db/services/knowledgebase_service.py`.
//! Stores knowledge bases as JSON files on disk.

use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, RwLock};

/// A dataset <-> data-source link (RAGFlow's Connector2Kb row joined with the
/// Connector record for the display fields). Stored on the knowledge base so
/// the Configuration -> Link data source section survives a reload.
///
/// auto_parse is the upstream switch that decides whether the documents the
/// connector syncs into the dataset are parsed automatically ("1") or kept
/// unparsed ("0"). RAGFlow stores it as the string "0" / "1"; the same wire
/// form is kept here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectorLink {
    /// The linked Connector id (the data_sources record id).
    pub id: String,
    #[serde(default = "default_auto_parse")]
    pub auto_parse: String,
}

/// Upstream Connector2Kb.auto_parse default: parse the synced documents.
pub fn default_auto_parse() -> String {
    "1".into()
}

/// A knowledge base (dataset in RAGFlow terms).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBase {
    /// Unique KB ID
    pub id: String,
    /// Display name
    pub name: String,
    /// RAGFlow `Knowledgebase.language` -- a *top-level* column, not a
    /// `parser_config` key. Upstream defaults it to `Chinese` when `LANG`
    /// mentions `zh_CN` and to `English` otherwise, and the dataset
    /// configuration form's Language field reads and writes it.
    #[serde(default = "default_language")]
    pub language: String,
    /// Description
    pub description: String,
    /// Owning user/tenant. Empty only for legacy records created before ownership existed.
    #[serde(default)]
    pub owner_id: String,
    /// Visibility: `private` for the owner only, `team` for joined tenant members.
    #[serde(default = "default_permission")]
    pub permission: String,
    /// RAGFlow `Knowledgebase.pipeline_id` — the ingestion-pipeline binding.
    /// The Artifacts tab's `canGenerateWiki` gate requires it alongside a
    /// non-zero chunk count (upstream `wiki-generation-eligibility.ts`).
    #[serde(default)]
    pub pipeline_id: Option<String>,
    /// Linked data sources (RAGFlow Connector2Kb rows): the dataset
    /// Configuration -> Link data source section reads and writes this column.
    #[serde(default)]
    pub connectors: Vec<ConnectorLink>,
    /// Chunk count
    pub chunk_count: usize,
    /// Document count
    pub doc_count: usize,
    /// Embedding model name
    pub embd_id: String,
    /// Parser config (JSON string)
    pub parser_config: String,
    /// KB tag sets (comma-joined; RAGFlow tag_kwd parity).
    #[serde(default)]
    pub tag_sets: String,
    /// Avatar image (data URL or empty for the letter avatar).
    #[serde(default)]
    pub avatar: String,
    /// RAGFlow `prompt_config` — JSON settings applied to RAG chats over this
    /// KB (top_k / similarity_threshold / cross_languages / keyword / etc.).
    /// Stored as a JSON object; empty object = defaults.
    #[serde(default = "default_prompt_config")]
    pub prompt_config: serde_json::Value,
    /// RAGFlow `Knowledgebase.pagerank` column (0 disables page-rank boost).
    #[serde(default)]
    pub pagerank: u32,
    /// RAGFlow `Knowledgebase.similarity_threshold` column (retrieval default).
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,
    /// RAGFlow `Knowledgebase.vector_similarity_weight` column.
    #[serde(default = "default_vector_similarity_weight")]
    pub vector_similarity_weight: f64,
    /// Creation timestamp (Unix ms)
    pub created_at: u64,
    /// Last update timestamp (Unix ms)
    pub updated_at: u64,
}

fn default_similarity_threshold() -> f64 {
    0.2
}

fn default_vector_similarity_weight() -> f64 {
    0.3
}

/// Upstream `Knowledgebase.language` default: `Chinese` when the process locale
/// mentions `zh_CN`, `English` otherwise.
pub fn default_language() -> String {
    match std::env::var("LANG") {
        Ok(lang) if lang.contains("zh_CN") => "Chinese".to_string(),
        _ => "English".to_string(),
    }
}

/// Upstream `language = CharField(max_length=32)`: the stored value is the raw
/// `LanguageTranslationMap` key, so only the length and emptiness are checked.
pub fn validate_language(language: &str) -> Result<()> {
    let language = language.trim();
    if language.is_empty() {
        anyhow::bail!("Language is required");
    }
    if language.chars().count() > 32 {
        anyhow::bail!("Language must not exceed 32 characters");
    }
    Ok(())
}

/// Knowledge base store with file persistence.
pub struct KbStore {
    kbs: RwLock<HashMap<String, KnowledgeBase>>,
    file_path: String,
    save_lock: Mutex<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TenantRole {
    Owner,
    Admin,
    #[serde(alias = "member", alias = "user")]
    Normal,
    Invite,
}

impl TenantRole {
    pub fn is_member(self) -> bool {
        matches!(self, Self::Owner | Self::Admin | Self::Normal)
    }

    pub fn can_manage(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantMembership {
    pub tenant_id: String,
    pub user_id: String,
    #[serde(default = "default_member_role")]
    pub role: TenantRole,
    #[serde(default)]
    pub invited_by: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

impl PartialEq for TenantMembership {
    fn eq(&self, other: &Self) -> bool {
        self.tenant_id == other.tenant_id && self.user_id == other.user_id
    }
}

impl Eq for TenantMembership {}

impl std::hash::Hash for TenantMembership {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.tenant_id.hash(state);
        self.user_id.hash(state);
    }
}

/// Persistent membership list for user-owned tenants.
pub struct TenantStore {
    memberships: RwLock<HashSet<TenantMembership>>,
    file_path: String,
    save_lock: Mutex<()>,
}

impl TenantStore {
    pub fn new(file_path: &str) -> Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(file_path))?;
        let memberships = if std::path::Path::new(file_path).exists() {
            let data = std::fs::read_to_string(file_path)?;
            serde_json::from_str::<Vec<TenantMembership>>(&data)?
                .into_iter()
                .collect()
        } else {
            HashSet::new()
        };
        let store = Self {
            memberships: RwLock::new(memberships),
            file_path: file_path.into(),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    /// Personal tenant IDs equal their owner user IDs, so owners are implicit members.
    pub fn role(&self, tenant_id: &str, user_id: &str) -> Option<TenantRole> {
        if tenant_id == user_id {
            return Some(TenantRole::Owner);
        }
        self.memberships
            .read()
            .unwrap()
            .iter()
            .find(|entry| entry.tenant_id == tenant_id && entry.user_id == user_id)
            .map(|entry| entry.role)
    }

    pub fn is_member(&self, tenant_id: &str, user_id: &str) -> bool {
        self.role(tenant_id, user_id)
            .is_some_and(TenantRole::is_member)
    }

    pub fn can_manage(&self, tenant_id: &str, user_id: &str) -> bool {
        self.role(tenant_id, user_id)
            .is_some_and(TenantRole::can_manage)
    }

    pub fn list_members(&self, tenant_id: &str) -> Vec<TenantMembership> {
        self.memberships
            .read()
            .unwrap()
            .iter()
            .filter(|entry| entry.tenant_id == tenant_id)
            .cloned()
            .collect()
    }

    pub fn list_for_user(&self, user_id: &str) -> Vec<TenantMembership> {
        let mut memberships: Vec<TenantMembership> = self
            .memberships
            .read()
            .unwrap()
            .iter()
            .filter(|entry| entry.user_id == user_id)
            .cloned()
            .collect();
        memberships.push(TenantMembership {
            tenant_id: user_id.into(),
            user_id: user_id.into(),
            role: TenantRole::Owner,
            invited_by: user_id.into(),
            created_at: 0,
            updated_at: 0,
        });
        memberships
    }

    pub fn invite_member(&self, tenant_id: &str, user_id: &str, invited_by: &str) -> Result<bool> {
        if tenant_id == user_id {
            return Ok(false);
        }
        self.mutate(|memberships| {
            if let Some(existing) = memberships
                .iter()
                .find(|entry| entry.tenant_id == tenant_id && entry.user_id == user_id)
            {
                if existing.role.is_member() {
                    anyhow::bail!("User is already a tenant member");
                }
                return Ok((false, false));
            }
            let now = now_ms();
            let entry = TenantMembership {
                tenant_id: tenant_id.into(),
                user_id: user_id.into(),
                role: TenantRole::Invite,
                invited_by: invited_by.into(),
                created_at: now,
                updated_at: now,
            };
            memberships.insert(entry);
            Ok((true, true))
        })
    }

    pub fn accept_invitation(&self, tenant_id: &str, user_id: &str) -> Result<bool> {
        self.mutate(|memberships| {
            let Some(mut entry) = memberships.take(&membership_key(tenant_id, user_id)) else {
                return Ok((false, false));
            };
            if entry.role != TenantRole::Invite {
                memberships.insert(entry);
                return Ok((false, false));
            }
            entry.role = TenantRole::Normal;
            entry.updated_at = now_ms();
            memberships.insert(entry);
            Ok((true, true))
        })
    }

    pub fn update_role(&self, tenant_id: &str, user_id: &str, role: TenantRole) -> Result<bool> {
        if tenant_id == user_id || !matches!(role, TenantRole::Admin | TenantRole::Normal) {
            anyhow::bail!("Tenant member role must be 'admin' or 'normal'");
        }
        self.mutate(|memberships| {
            let Some(mut entry) = memberships.take(&membership_key(tenant_id, user_id)) else {
                return Ok((false, false));
            };
            if !entry.role.is_member() {
                memberships.insert(entry);
                anyhow::bail!("Invitation must be accepted before assigning a role");
            }
            if entry.role == role {
                memberships.insert(entry);
                return Ok((false, false));
            }
            entry.role = role;
            entry.updated_at = now_ms();
            memberships.insert(entry);
            Ok((true, true))
        })
    }

    pub fn remove_member(&self, tenant_id: &str, user_id: &str) -> Result<bool> {
        if tenant_id == user_id {
            anyhow::bail!("Tenant owner cannot be removed");
        }
        self.mutate(|memberships| {
            let before = memberships.len();
            memberships.retain(|entry| entry.tenant_id != tenant_id || entry.user_id != user_id);
            let changed = memberships.len() != before;
            Ok((changed, changed))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashSet<TenantMembership>) -> Result<(T, bool)>,
    ) -> Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut memberships = self.memberships.write().unwrap();
        let previous = memberships.clone();
        let (value, changed) = mutation(&mut memberships)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<TenantMembership> = memberships.iter().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *memberships = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> Result<()> {
        let memberships: Vec<TenantMembership> =
            self.memberships.read().unwrap().iter().cloned().collect();
        self.persist(&memberships)
    }

    fn persist(&self, memberships: &[TenantMembership]) -> Result<()> {
        let data = serde_json::to_vec_pretty(memberships)?;
        crate::persistence::atomic_write(std::path::Path::new(&self.file_path), &data)
    }
}

impl KbStore {
    /// Load existing Kbs from a JSON file, or create empty store.
    pub fn new(file_path: &str) -> Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(file_path))?;
        let kbs = if std::path::Path::new(file_path).exists() {
            let data = std::fs::read_to_string(file_path)?;
            let list: Vec<KnowledgeBase> = serde_json::from_str(&data)?;
            list.into_iter()
                .map(|mut kb| {
                    if kb.permission != "team" {
                        kb.permission = "me".to_string();
                    }
                    (kb.id.clone(), kb)
                })
                .collect()
        } else {
            HashMap::new()
        };

        let store = Self {
            kbs: RwLock::new(kbs),
            file_path: file_path.to_string(),
            save_lock: Mutex::new(()),
        };
        store.persist(&store.list())?;
        Ok(store)
    }

    /// List knowledge bases visible to a user. Administrators may recover legacy records.
    pub fn list_accessible(
        &self,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
    ) -> Vec<KnowledgeBase> {
        self.kbs
            .read()
            .unwrap()
            .values()
            .filter(|kb| {
                kb.owner_id == user_id
                    || (kb.permission == "team" && is_tenant_member(&kb.owner_id, user_id))
                    || (is_admin && kb.owner_id.is_empty())
            })
            .cloned()
            .collect()
    }

    /// List all knowledge bases for system-level statistics and maintenance.
    pub fn list(&self) -> Vec<KnowledgeBase> {
        self.kbs.read().unwrap().values().cloned().collect()
    }

    /// Get a KB by ID without applying authorization.
    pub fn get(&self, id: &str) -> Option<KnowledgeBase> {
        self.kbs.read().unwrap().get(id).cloned()
    }

    pub fn can_read(
        &self,
        id: &str,
        user_id: &str,
        is_admin: bool,
        is_tenant_member: impl Fn(&str, &str) -> bool,
    ) -> bool {
        self.get(id).is_some_and(|kb| {
            kb.owner_id == user_id
                || (kb.permission == "team" && is_tenant_member(&kb.owner_id, user_id))
                || (is_admin && kb.owner_id.is_empty())
        })
    }

    pub fn can_manage(&self, id: &str, user_id: &str, is_admin: bool) -> bool {
        self.get(id)
            .is_some_and(|kb| kb.owner_id == user_id || (is_admin && kb.owner_id.is_empty()))
    }

    /// Create a new private knowledge base for one owner.
    pub fn create_for(
        &self,
        owner_id: &str,
        name: &str,
        description: &str,
    ) -> Result<KnowledgeBase> {
        self.create_for_with_permission(owner_id, name, description, "private")
    }

    pub fn create_for_with_permission(
        &self,
        owner_id: &str,
        name: &str,
        description: &str,
        permission: &str,
    ) -> Result<KnowledgeBase> {
        self.create_for_with_config(
            owner_id,
            name,
            description,
            normalize_permission(permission),
            "default",
        )
    }

    pub fn create_for_with_config(
        &self,
        owner_id: &str,
        name: &str,
        description: &str,
        permission: &str,
        embd_id: &str,
    ) -> Result<KnowledgeBase> {
        self.create_for_with_parser_config(
            owner_id,
            name,
            description,
            permission,
            embd_id,
            r#"{"chunk_token_num":2048,"overlapped_percent":0.05}"#,
        )
    }

    /// Like `create_for_with_config` but seeds a caller-supplied
    /// `parser_config` (mirrors RAGFlow create_dataset: the create dialog
    /// sends chunk_method + parse_type up front).
    pub fn create_for_with_parser_config(
        &self,
        owner_id: &str,
        name: &str,
        description: &str,
        permission: &str,
        embd_id: &str,
        parser_config: &str,
    ) -> Result<KnowledgeBase> {
        self.create_for_with_parser_config_and_language(
            owner_id,
            name,
            description,
            permission,
            embd_id,
            parser_config,
            &default_language(),
        )
    }

    /// Creation with an explicit `language` (upstream `POST /api/v1/datasets`
    /// accepts the column, and the dataset form's Language field is stored here).
    #[allow(clippy::too_many_arguments)]
    pub fn create_for_with_parser_config_and_language(
        &self,
        owner_id: &str,
        name: &str,
        description: &str,
        permission: &str,
        embd_id: &str,
        parser_config: &str,
        language: &str,
    ) -> Result<KnowledgeBase> {
        validate_permission(permission)?;
        validate_language(language)?;
        let permission = normalize_permission(permission);
        validate_embedding_selector(embd_id)?;
        self.mutate(|kbs| {
            if kbs
                .values()
                .any(|kb| kb.owner_id == owner_id && kb.name == name)
            {
                anyhow::bail!("Knowledge base '{}' already exists", name);
            }
            let now = now_ms();
            let kb = KnowledgeBase {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.to_string(),
                language: language.trim().to_string(),
                description: description.to_string(),
                owner_id: owner_id.to_string(),
                permission: normalize_permission(permission).to_string(),
                pipeline_id: None,
                connectors: Vec::new(),
                chunk_count: 0,
                doc_count: 0,
                embd_id: embd_id.into(),
                parser_config: parser_config.to_string(),
                tag_sets: String::new(),
                avatar: String::new(),
                prompt_config: serde_json::json!({}),
                pagerank: 0,
                similarity_threshold: 0.2,
                vector_similarity_weight: 0.3,
                created_at: now,
                updated_at: now,
            };
            kbs.insert(kb.id.clone(), kb.clone());
            Ok(kb)
        })
    }

    /// Upstream `Knowledgebase.pipeline_id` binding update: a value binds the
    /// dataset to an ingestion pipeline, `None`/empty clears it.
    pub fn set_pipeline_id(
        &self,
        id: &str,
        pipeline_id: Option<String>,
    ) -> Result<Option<KnowledgeBase>> {
        let pipeline_id = pipeline_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.mutate_if_changed(|kbs| {
            let Some(kb) = kbs.get_mut(id) else {
                return Ok((None, false));
            };
            if kb.pipeline_id == pipeline_id {
                return Ok((Some(kb.clone()), false));
            }
            kb.pipeline_id = pipeline_id;
            Ok((Some(kb.clone()), true))
        })
    }

    /// Replace the dataset's linked data sources (RAGFlow Connector2Kb rows).
    ///
    /// The links are canonicalised by (id, auto_parse) before the comparison so
    /// a client that only reorders the array does not rewrite the record (and
    /// its updated_at timestamp). A no-op returns the current row untouched.
    pub fn set_connectors(
        &self,
        id: &str,
        connectors: Vec<ConnectorLink>,
    ) -> Result<Option<KnowledgeBase>> {
        let mut canonical = connectors;
        canonical.sort_by(|a, b| {
            (a.id.as_str(), a.auto_parse.as_str()).cmp(&(b.id.as_str(), b.auto_parse.as_str()))
        });
        canonical.dedup_by(|a, b| a.id == b.id);
        self.mutate_if_changed(move |kbs| {
            let Some(kb) = kbs.get_mut(id) else {
                return Ok((None, false));
            };
            if kb.connectors == canonical {
                return Ok((Some(kb.clone()), false));
            }
            kb.connectors = canonical;
            kb.updated_at = now_ms();
            Ok((Some(kb.clone()), true))
        })
    }

    /// Upstream `KnowledgebaseService.update_by_id(kb.id, {"language": ...})`.
    pub fn set_language(&self, id: &str, language: &str) -> Result<Option<KnowledgeBase>> {
        validate_language(language)?;
        let language = language.trim().to_string();
        self.mutate_if_changed(|kbs| {
            let Some(kb) = kbs.get_mut(id) else {
                return Ok((None, false));
            };
            if kb.language == language {
                return Ok((Some(kb.clone()), false));
            }
            kb.language = language;
            Ok((Some(kb.clone()), true))
        })
    }

    pub fn update_permission(&self, id: &str, permission: &str) -> Result<Option<KnowledgeBase>> {
        self.update_config(id, Some(permission), None, None, None)
    }

    pub fn update_config(
        &self,
        id: &str,
        permission: Option<&str>,
        embd_id: Option<&str>,
        parser_config: Option<&str>,
        prompt_config: Option<&serde_json::Value>,
    ) -> Result<Option<KnowledgeBase>> {
        let permission = permission.map(normalize_permission);
        if let Some(permission) = permission {
            validate_permission(permission)?;
        }
        if let Some(embd_id) = embd_id {
            validate_embedding_selector(embd_id)?;
        }
        self.mutate_if_changed(|kbs| {
            let Some(kb) = kbs.get_mut(id) else {
                return Ok((None, false));
            };
            let permission_changed = permission.is_some_and(|value| kb.permission != value);
            let embedding_changed = embd_id.is_some_and(|value| kb.embd_id != value);
            let config_changed = parser_config.is_some_and(|value| kb.parser_config != value);
            let prompt_changed = prompt_config.is_some_and(|value| kb.prompt_config != *value);
            if !permission_changed && !embedding_changed && !config_changed && !prompt_changed {
                return Ok((Some(kb.clone()), false));
            }
            if let Some(permission) = permission {
                kb.permission = permission.into();
            }
            if let Some(embd_id) = embd_id {
                kb.embd_id = embd_id.into();
            }
            if let Some(parser_config) = parser_config {
                kb.parser_config = parser_config.into();
            }
            if let Some(prompt_config) = prompt_config {
                kb.prompt_config = prompt_config.clone();
            }
            kb.updated_at = now_ms();
            Ok((Some(kb.clone()), true))
        })
    }

    /// Rename a knowledge base and/or update its description (RAGFlow rename flow).
    pub fn update_name_description(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<Option<KnowledgeBase>> {
        self.update_identity(id, name, description, None, None)
    }

    /// Update KB identity fields: name, description, tag_sets, avatar.
    pub fn update_identity(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        tag_sets: Option<&str>,
        avatar: Option<&str>,
    ) -> Result<Option<KnowledgeBase>> {
        self.mutate_if_changed(|kbs| {
            let Some(kb) = kbs.get_mut(id) else {
                return Ok((None, false));
            };
            let name_changed = name.is_some_and(|value| kb.name != value.trim());
            let desc_changed = description.is_some_and(|value| kb.description != value);
            let tags_changed = tag_sets.is_some_and(|value| kb.tag_sets != value);
            let avatar_changed = avatar.is_some_and(|value| kb.avatar != value);
            if !name_changed && !desc_changed && !tags_changed && !avatar_changed {
                return Ok((Some(kb.clone()), false));
            }
            if let Some(name) = name {
                kb.name = name.trim().to_string();
            }
            if let Some(description) = description {
                kb.description = description.to_string();
            }
            if let Some(tag_sets) = tag_sets {
                kb.tag_sets = tag_sets.to_string();
            }
            if let Some(avatar) = avatar {
                kb.avatar = avatar.to_string();
            }
            kb.updated_at = now_ms();
            Ok((Some(kb.clone()), true))
        })
    }

    /// Upstream `Knowledgebase.pagerank` update (the update API accepts 0..=100).
    pub fn set_pagerank(&self, id: &str, pagerank: u32) -> Result<Option<KnowledgeBase>> {
        self.mutate_if_changed(|kbs| {
            let Some(kb) = kbs.get_mut(id) else {
                return Ok((None, false));
            };
            if kb.pagerank == pagerank {
                return Ok((Some(kb.clone()), false));
            }
            kb.pagerank = pagerank;
            kb.updated_at = now_ms();
            Ok((Some(kb.clone()), true))
        })
    }

    /// Delete a knowledge base by ID.
    pub fn delete(&self, id: &str) -> Result<bool> {
        self.mutate_if_changed(|kbs| {
            let removed = kbs.remove(id).is_some();
            Ok((removed, removed))
        })
    }

    /// Update chunk/document counts for a KB.
    pub fn update_counts(&self, id: &str, chunk_delta: isize, doc_delta: isize) -> Result<()> {
        self.mutate_if_changed(|kbs| {
            let Some(kb) = kbs.get_mut(id) else {
                return Ok(((), false));
            };
            kb.chunk_count = (kb.chunk_count as isize + chunk_delta).max(0) as usize;
            kb.doc_count = (kb.doc_count as isize + doc_delta).max(0) as usize;
            kb.updated_at = now_ms();
            Ok(((), true))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, KnowledgeBase>) -> Result<T>,
    ) -> Result<T> {
        self.mutate_if_changed(|kbs| mutation(kbs).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, KnowledgeBase>) -> Result<(T, bool)>,
    ) -> Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut kbs = self.kbs.write().unwrap();
        let previous = kbs.clone();
        let (value, changed) = mutation(&mut kbs)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<KnowledgeBase> = kbs.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *kbs = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist(&self, list: &[KnowledgeBase]) -> Result<()> {
        let data = serde_json::to_vec_pretty(&list)?;
        crate::persistence::atomic_write(std::path::Path::new(&self.file_path), &data)
    }
}

fn default_prompt_config() -> serde_json::Value {
    serde_json::json!({})
}

fn default_permission() -> String {
    "me".into()
}

fn default_member_role() -> TenantRole {
    TenantRole::Normal
}

fn membership_key(tenant_id: &str, user_id: &str) -> TenantMembership {
    TenantMembership {
        tenant_id: tenant_id.into(),
        user_id: user_id.into(),
        role: TenantRole::Normal,
        invited_by: String::new(),
        created_at: 0,
        updated_at: 0,
    }
}

/// Upstream `PermissionRole` uses `me` / `team`; RayRAG historically stored
/// `private`, so both spellings are accepted and normalised to `me` on write so
/// the API payload matches RAGFlow.
pub fn normalize_permission(permission: &str) -> &str {
    match permission.trim() {
        "team" => "team",
        "private" | "me" => "me",
        other => other,
    }
}

fn validate_permission(permission: &str) -> Result<()> {
    if matches!(normalize_permission(permission), "me" | "team") {
        Ok(())
    } else {
        anyhow::bail!("Knowledge base permission must be 'me' or 'team'")
    }
}

fn validate_embedding_selector(selector: &str) -> Result<()> {
    if selector.trim().is_empty() {
        anyhow::bail!("Knowledge base embedding model is required")
    }
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}










