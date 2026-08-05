//! Knowledge Base manager — CRUD operations with JSON persistence.
//!
//! Replaces RAGFlow's `api/db/services/knowledgebase_service.py`.
//! Stores knowledge bases as JSON files on disk.

use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, RwLock};

/// A knowledge base (dataset in RAGFlow terms).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBase {
    /// Unique KB ID
    pub id: String,
    /// Display name
    pub name: String,
    /// Description
    pub description: String,
    /// Owning user/tenant. Empty only for legacy records created before ownership existed.
    #[serde(default)]
    pub owner_id: String,
    /// Visibility: `private` for the owner only, `team` for joined tenant members.
    #[serde(default = "default_permission")]
    pub permission: String,
    /// Chunk count
    pub chunk_count: usize,
    /// Document count
    pub doc_count: usize,
    /// Embedding model name
    pub embd_id: String,
    /// Parser config (JSON string)
    pub parser_config: String,
    /// Creation timestamp (Unix ms)
    pub created_at: u64,
    /// Last update timestamp (Unix ms)
    pub updated_at: u64,
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
            list.into_iter().map(|kb| (kb.id.clone(), kb)).collect()
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
        self.create_for_with_config(owner_id, name, description, permission, "default")
    }

    pub fn create_for_with_config(
        &self,
        owner_id: &str,
        name: &str,
        description: &str,
        permission: &str,
        embd_id: &str,
    ) -> Result<KnowledgeBase> {
        validate_permission(permission)?;
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
                description: description.to_string(),
                owner_id: owner_id.to_string(),
                permission: permission.into(),
                chunk_count: 0,
                doc_count: 0,
                embd_id: embd_id.into(),
                parser_config: r#"{"chunk_token_num":2048,"overlapped_percent":0.05}"#.into(),
                created_at: now,
                updated_at: now,
            };
            kbs.insert(kb.id.clone(), kb.clone());
            Ok(kb)
        })
    }

    pub fn update_permission(&self, id: &str, permission: &str) -> Result<Option<KnowledgeBase>> {
        self.update_config(id, Some(permission), None, None)
    }

    pub fn update_config(
        &self,
        id: &str,
        permission: Option<&str>,
        embd_id: Option<&str>,
        parser_config: Option<&str>,
    ) -> Result<Option<KnowledgeBase>> {
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
            if !permission_changed && !embedding_changed && !config_changed {
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
        self.mutate_if_changed(|kbs| {
            let Some(kb) = kbs.get_mut(id) else {
                return Ok((None, false));
            };
            let name_changed =
                name.is_some_and(|value| kb.name != value.trim());
            let desc_changed = description.is_some_and(|value| kb.description != value);
            if !name_changed && !desc_changed {
                return Ok((Some(kb.clone()), false));
            }
            if let Some(name) = name {
                kb.name = name.trim().to_string();
            }
            if let Some(description) = description {
                kb.description = description.to_string();
            }
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

fn default_permission() -> String {
    "private".into()
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

fn validate_permission(permission: &str) -> Result<()> {
    if matches!(permission, "private" | "team") {
        Ok(())
    } else {
        anyhow::bail!("Knowledge base permission must be 'private' or 'team'")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenant_invitation_is_unique_persistent_accepted_and_revocable() {
        let root = std::env::temp_dir().join(format!("rayrag-tenant-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tenants.json");
        let store = TenantStore::new(path.to_str().unwrap()).unwrap();
        assert!(store.is_member("owner", "owner"));
        assert!(store.can_manage("owner", "owner"));
        assert!(store.invite_member("owner", "member", "owner").unwrap());
        assert!(!store.invite_member("owner", "member", "owner").unwrap());
        assert!(!store.is_member("owner", "member"));
        assert_eq!(store.role("owner", "member"), Some(TenantRole::Invite));
        assert_eq!(store.list_members("owner").len(), 1);
        drop(store);

        let reloaded = TenantStore::new(path.to_str().unwrap()).unwrap();
        assert!(!reloaded.is_member("owner", "member"));
        assert!(reloaded.accept_invitation("owner", "member").unwrap());
        assert!(reloaded.is_member("owner", "member"));
        assert!(!reloaded.can_manage("owner", "member"));
        assert!(
            reloaded
                .update_role("owner", "member", TenantRole::Admin)
                .unwrap()
        );
        assert!(reloaded.can_manage("owner", "member"));
        assert!(reloaded.remove_member("owner", "member").unwrap());
        assert!(!reloaded.is_member("owner", "member"));
        assert!(reloaded.remove_member("owner", "owner").is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn legacy_member_roles_load_as_normal_members() {
        let root =
            std::env::temp_dir().join(format!("rayrag-tenant-legacy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tenants.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!([
                { "tenant_id": "owner", "user_id": "member", "role": "member" },
                { "tenant_id": "owner", "user_id": "user", "role": "user" }
            ]))
            .unwrap(),
        )
        .unwrap();

        let store = TenantStore::new(path.to_str().unwrap()).unwrap();
        assert_eq!(store.role("owner", "member"), Some(TenantRole::Normal));
        assert_eq!(store.role("owner", "user"), Some(TenantRole::Normal));
        assert!(store.is_member("owner", "member"));
        assert!(store.is_member("owner", "user"));
        let tenants = store.list_for_user("member");
        assert!(
            tenants
                .iter()
                .any(|entry| { entry.tenant_id == "owner" && entry.role == TenantRole::Normal })
        );
        assert!(
            tenants
                .iter()
                .any(|entry| { entry.tenant_id == "member" && entry.role == TenantRole::Owner })
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn tenant_persistence_failure_rolls_back_memory() {
        let root =
            std::env::temp_dir().join(format!("rayrag-tenant-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tenants.json");
        let store = TenantStore::new(path.to_str().unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(store.invite_member("owner", "member", "owner").is_err());
        assert!(!store.is_member("owner", "member"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn old_kb_json_defaults_to_private() {
        let root = std::env::temp_dir().join(format!("rayrag-kb-old-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("kbs.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!([{
                "id": "legacy",
                "name": "Legacy",
                "description": "",
                "owner_id": "owner",
                "chunk_count": 0,
                "doc_count": 0,
                "embd_id": "",
                "parser_config": "{}",
                "created_at": 1,
                "updated_at": 1
            }]))
            .unwrap(),
        )
        .unwrap();

        let store = KbStore::new(path.to_str().unwrap()).unwrap();
        assert_eq!(store.get("legacy").unwrap().permission, "private");
        assert!(!store.can_read("legacy", "member", false, |_, _| true));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn team_kb_members_can_read_but_not_manage() {
        let root = std::env::temp_dir().join(format!("rayrag-kb-acl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("kbs.json");
        let store = KbStore::new(path.to_str().unwrap()).unwrap();
        let kb = store
            .create_for_with_permission("owner", "Team", "", "team")
            .unwrap();

        assert!(store.can_read(&kb.id, "member", false, |tenant, user| {
            tenant == "owner" && user == "member"
        }));
        assert!(!store.can_manage(&kb.id, "member", false));
        assert!(store.update_permission(&kb.id, "public").is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_persistence_rolls_back_kb_mutations() {
        let root =
            std::env::temp_dir().join(format!("rayrag-kb-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("kbs.json");
        let store = KbStore::new(path.to_str().unwrap()).unwrap();
        let kb = store.create_for("owner-1", "KB", "").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(store.update_counts(&kb.id, 5, 1).is_err());
        let unchanged = store.get(&kb.id).unwrap();
        assert_eq!(unchanged.chunk_count, 0);
        assert_eq!(unchanged.doc_count, 0);

        assert!(store.delete(&kb.id).is_err());
        assert!(store.get(&kb.id).is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn embedding_selector_is_created_and_updated_atomically() {
        let root =
            std::env::temp_dir().join(format!("rayrag-kb-embedding-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("kbs.json");
        let store = KbStore::new(path.to_str().unwrap()).unwrap();
        let kb = store
            .create_for_with_config(
                "owner",
                "Configured",
                "",
                "private",
                "local/primary/embed-a",
            )
            .unwrap();
        assert_eq!(kb.embd_id, "local/primary/embed-a");

        let updated = store
            .update_config(&kb.id, Some("team"), Some("local/primary/embed-b"), None)
            .unwrap()
            .unwrap();
        assert_eq!(updated.permission, "team");
        assert_eq!(updated.embd_id, "local/primary/embed-b");

        drop(store);
        let restored = KbStore::new(path.to_str().unwrap()).unwrap();
        assert_eq!(
            restored.get(&kb.id).unwrap().embd_id,
            "local/primary/embed-b"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
