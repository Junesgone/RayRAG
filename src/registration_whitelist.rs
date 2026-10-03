//! The registration whitelist behind `pages/admin/whitelist.tsx` and RAGFlow's
//! `enable_whitelist` variable.
//!
//! When the whitelist is disabled every registration is accepted, which is the default RayRAG
//! ships with. When it is enabled only listed addresses may register, and an entry may be either a
//! full address (`someone@example.com`) or a domain (`@example.com`) so an operator can admit a
//! whole company without listing every mailbox.

use std::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WhitelistState {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub entries: Vec<String>,
}

pub struct WhitelistStore {
    state: RwLock<WhitelistState>,
    path: Option<String>,
    save_lock: Mutex<()>,
}

impl WhitelistStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        if !path.is_empty() {
            crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        }
        let state = if !path.is_empty() && std::path::Path::new(path).exists() {
            let raw = std::fs::read_to_string(path)
                .map_err(|error| anyhow::anyhow!("cannot read whitelist '{path}': {error}"))?;
            serde_json::from_str::<WhitelistState>(&raw)
                .map_err(|error| anyhow::anyhow!("cannot parse whitelist '{path}': {error}"))?
        } else {
            WhitelistState::default()
        };
        Ok(Self {
            state: RwLock::new(state),
            path: if path.is_empty() {
                None
            } else {
                Some(path.to_string())
            },
            save_lock: Mutex::new(()),
        })
    }

    /// An in-memory store, for tests and for a deployment that only wants the environment
    /// variable to decide.
    pub fn in_memory() -> Self {
        Self {
            state: RwLock::new(WhitelistState::default()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> WhitelistState {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }

    fn persist(&self, state: &WhitelistState) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let _guard = self.save_lock.lock().unwrap_or_else(|e| e.into_inner());
        let json = serde_json::to_string_pretty(state)?;
        let temp = format!("{path}.tmp");
        std::fs::write(&temp, json.as_bytes())?;
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    pub fn set_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        state.enabled = enabled;
        self.persist(&state)
    }

    /// Adds an entry. Returns `false` when it was already present, so the caller can answer
    /// "unchanged" instead of silently claiming a new entry.
    pub fn add(&self, entry: &str) -> anyhow::Result<bool> {
        let entry = normalise(entry);
        if entry.is_empty() {
            anyhow::bail!("an email or @domain is required");
        }
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        if state.entries.iter().any(|existing| existing == &entry) {
            return Ok(false);
        }
        state.entries.push(entry);
        state.entries.sort();
        self.persist(&state)?;
        Ok(true)
    }

    /// Removes one or more entries and reports how many were actually there.
    pub fn remove(&self, entries: &[String]) -> anyhow::Result<usize> {
        let wanted: Vec<String> = entries.iter().map(|e| normalise(e)).collect();
        let mut state = self.state.write().unwrap_or_else(|e| e.into_inner());
        let before = state.entries.len();
        state.entries.retain(|entry| !wanted.contains(entry));
        let removed = before - state.entries.len();
        if removed > 0 {
            self.persist(&state)?;
        }
        Ok(removed)
    }

    /// Whether an address may register. A disabled whitelist admits everyone.
    pub fn allows(&self, email: &str) -> bool {
        let state = self.state.read().unwrap_or_else(|e| e.into_inner());
        if !state.enabled {
            return true;
        }
        let email = email.trim().to_ascii_lowercase();
        if email.is_empty() {
            return false;
        }
        let domain = format!("@{}", email.rsplit('@').next().unwrap_or_default());
        state
            .entries
            .iter()
            .any(|entry| entry == &email || entry == &domain)
    }
}

fn normalise(entry: &str) -> String {
    entry.trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default deployment must not lock anybody out: a disabled whitelist admits everyone.
    #[test]
    fn a_disabled_whitelist_admits_everyone() {
        let store = WhitelistStore::in_memory();
        assert!(!store.snapshot().enabled);
        assert!(store.allows("anyone@example.com"));
        assert!(store.allows(""));
    }

    /// Enabled means enabled: only listed addresses, and an `@domain` entry admits the domain.
    #[test]
    fn an_enabled_whitelist_admits_only_listed_addresses_and_domains() {
        let store = WhitelistStore::in_memory();
        store.set_enabled(true).unwrap();
        assert!(
            !store.allows("someone@example.com"),
            "empty list admits nobody"
        );
        assert!(
            store.add("Someone@Example.com").unwrap(),
            "normalised to lowercase"
        );
        assert!(store.allows("someone@example.com"));
        assert!(store.allows("SOMEONE@EXAMPLE.COM"));
        assert!(!store.allows("other@example.com"));
        assert!(store.add("@partner.cn").unwrap());
        assert!(store.allows("anyone@partner.cn"));
        assert!(
            !store.allows("anyone@notpartner.cn"),
            "a domain entry is not a suffix match"
        );
    }

    /// Adding the same entry twice must not silently duplicate it.
    #[test]
    fn duplicate_entries_are_reported_as_unchanged() {
        let store = WhitelistStore::in_memory();
        assert!(store.add("a@b.c").unwrap());
        assert!(!store.add(" a@b.c ").unwrap());
        assert_eq!(store.snapshot().entries, vec!["a@b.c".to_string()]);
        assert_eq!(store.remove(&["a@b.c".into()]).unwrap(), 1);
        assert_eq!(store.remove(&["a@b.c".into()]).unwrap(), 0);
        assert!(store.snapshot().entries.is_empty());
    }

    /// The whitelist survives a restart, which is the whole point of persisting it.
    #[test]
    fn the_whitelist_survives_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("whitelist.json");
        let path = path.to_string_lossy().into_owned();
        {
            let store = WhitelistStore::new(&path).unwrap();
            store.set_enabled(true).unwrap();
            store.add("ops@example.com").unwrap();
        }
        let reloaded = WhitelistStore::new(&path).unwrap();
        let snapshot = reloaded.snapshot();
        assert!(
            snapshot.enabled,
            "the toggle is persisted too: {snapshot:?}"
        );
        assert_eq!(snapshot.entries, vec!["ops@example.com".to_string()]);
        assert!(reloaded.allows("ops@example.com"));
        assert!(!reloaded.allows("nobody@example.com"));
    }
}
