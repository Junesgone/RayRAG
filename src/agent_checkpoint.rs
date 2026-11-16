//! Durable, authenticated checkpoints for interruptible Agent workflows.
//!
//! Checkpoints are never accepted from the client. The public resume token is
//! only an opaque lookup guard; the serialized scheduler/runtime state remains
//! in RayRAG's crash-resistant JSON store (and therefore participates in the
//! optional PostgreSQL snapshot mirror).

use crate::agent::{AgentWorkflowCheckpoint, normalize_agent_dsl_for_run};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;
use xxhash_rust::xxh3::xxh3_64;

const CLAIM_LEASE_MS: u64 = 15 * 60 * 1_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCheckpointRecord {
    pub checkpoint_id: String,
    pub conversation_id: String,
    pub owner_id: String,
    pub tenant_id: String,
    pub agent_id: String,
    pub dsl_fingerprint: String,
    pub checkpoint: AgentWorkflowCheckpoint,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    claim_expires_at: u64,
}

#[derive(Debug, Clone)]
pub struct AgentCheckpointClaim {
    pub record: AgentCheckpointRecord,
    claim_id: String,
}

#[derive(Debug, Clone)]
pub enum AgentCheckpointClaimResult {
    Claimed(Box<AgentCheckpointClaim>),
    Missing,
    Busy,
    ResumeTokenMismatch,
}

#[derive(Debug)]
pub struct AgentCheckpointStore {
    records: Mutex<BTreeMap<String, AgentCheckpointRecord>>,
    path: Option<PathBuf>,
}

impl AgentCheckpointStore {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let mut records = if path.exists() {
            serde_json::from_slice::<Vec<AgentCheckpointRecord>>(
                &std::fs::read(&path)
                    .with_context(|| format!("Failed to read {}", path.display()))?,
            )
            .with_context(|| format!("Failed to parse {}", path.display()))?
        } else {
            Vec::new()
        };
        // No in-flight workflow future survives a process restart. Clear
        // persisted leases so a crash during resume cannot make an otherwise
        // valid waiting form unavailable for the remainder of the lease.
        for record in &mut records {
            record.claim_id = None;
            record.claim_expires_at = 0;
        }
        validate_records(&records)?;
        Ok(Self {
            records: Mutex::new(
                records
                    .into_iter()
                    .map(|record| (record.conversation_id.clone(), record))
                    .collect(),
            ),
            path: Some(path),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            records: Mutex::new(BTreeMap::new()),
            path: None,
        }
    }

    pub fn save_waiting(
        &self,
        conversation_id: &str,
        owner_id: &str,
        tenant_id: &str,
        agent_id: &str,
        dsl_fingerprint: &str,
        checkpoint: AgentWorkflowCheckpoint,
    ) -> Result<AgentCheckpointRecord> {
        let now = now_ms();
        let record = AgentCheckpointRecord {
            checkpoint_id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_owned(),
            owner_id: owner_id.to_owned(),
            tenant_id: tenant_id.to_owned(),
            agent_id: agent_id.to_owned(),
            dsl_fingerprint: dsl_fingerprint.to_owned(),
            checkpoint,
            created_at: now,
            updated_at: now,
            claim_id: None,
            claim_expires_at: 0,
        };
        self.mutate(|records| {
            if records.contains_key(conversation_id) {
                bail!(
                    "Conversation '{}' already has a pending Agent checkpoint",
                    conversation_id
                );
            }
            records.insert(conversation_id.to_owned(), record.clone());
            Ok((record, true))
        })
    }

    pub fn claim(
        &self,
        conversation_id: &str,
        owner_id: &str,
        tenant_id: &str,
        agent_id: &str,
        resume_token: Option<&str>,
    ) -> Result<AgentCheckpointClaimResult> {
        self.mutate(|records| {
            let Some(record) = records.get_mut(conversation_id) else {
                return Ok((AgentCheckpointClaimResult::Missing, false));
            };
            if record.owner_id != owner_id
                || record.tenant_id != tenant_id
                || record.agent_id != agent_id
            {
                // Do not reveal the existence of another principal's state.
                return Ok((AgentCheckpointClaimResult::Missing, false));
            }
            if resume_token.is_some_and(|token| token != record.checkpoint_id) {
                return Ok((AgentCheckpointClaimResult::ResumeTokenMismatch, false));
            }
            let now = now_ms();
            if record.claim_id.is_some() && record.claim_expires_at > now {
                return Ok((AgentCheckpointClaimResult::Busy, false));
            }
            let claim_id = Uuid::new_v4().to_string();
            record.claim_id = Some(claim_id.clone());
            record.claim_expires_at = now.saturating_add(CLAIM_LEASE_MS);
            record.updated_at = now;
            Ok((
                AgentCheckpointClaimResult::Claimed(Box::new(AgentCheckpointClaim {
                    record: record.clone(),
                    claim_id,
                })),
                true,
            ))
        })
    }

    pub fn release(&self, claim: &AgentCheckpointClaim) -> Result<()> {
        self.mutate(|records| {
            let record = claimed_record_mut(records, claim)?;
            record.claim_id = None;
            record.claim_expires_at = 0;
            record.updated_at = now_ms();
            Ok(((), true))
        })
    }

    pub fn replace_claimed(
        &self,
        claim: &AgentCheckpointClaim,
        dsl_fingerprint: &str,
        checkpoint: AgentWorkflowCheckpoint,
    ) -> Result<AgentCheckpointRecord> {
        self.mutate(|records| {
            let record = claimed_record_mut(records, claim)?;
            record.checkpoint_id = Uuid::new_v4().to_string();
            record.dsl_fingerprint = dsl_fingerprint.to_owned();
            record.checkpoint = checkpoint;
            record.claim_id = None;
            record.claim_expires_at = 0;
            record.updated_at = now_ms();
            Ok((record.clone(), true))
        })
    }

    pub fn delete_claimed(&self, claim: &AgentCheckpointClaim) -> Result<()> {
        self.mutate(|records| {
            claimed_record_mut(records, claim)?;
            records.remove(&claim.record.conversation_id);
            Ok(((), true))
        })
    }

    pub fn delete_unclaimed(&self, record: &AgentCheckpointRecord) -> Result<()> {
        self.mutate(|records| {
            let current = records
                .get(&record.conversation_id)
                .ok_or_else(|| anyhow!("Agent checkpoint is no longer present"))?;
            if current.checkpoint_id != record.checkpoint_id || current.claim_id.is_some() {
                bail!("Agent checkpoint changed before cleanup");
            }
            records.remove(&record.conversation_id);
            Ok(((), true))
        })
    }

    #[cfg(test)]
    pub fn get(&self, conversation_id: &str) -> Option<AgentCheckpointRecord> {
        self.records.lock().unwrap().get(conversation_id).cloned()
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut BTreeMap<String, AgentCheckpointRecord>) -> Result<(T, bool)>,
    ) -> Result<T> {
        let mut records = self.records.lock().unwrap();
        let mut proposed = records.clone();
        let (result, changed) = operation(&mut proposed)?;
        if !changed {
            return Ok(result);
        }
        validate_records(&proposed.values().cloned().collect::<Vec<_>>())?;
        if let Some(path) = &self.path {
            let data = serde_json::to_vec_pretty(&proposed.values().collect::<Vec<_>>())?;
            crate::persistence::atomic_write(path, &data)?;
        }
        *records = proposed;
        Ok(result)
    }
}

pub fn agent_dsl_fingerprint(dsl: &Value) -> Result<String> {
    let normalized = normalize_agent_dsl_for_run(dsl);
    let bytes = serde_json::to_vec(&normalized)?;
    Ok(format!("{:016x}", xxh3_64(&bytes)))
}

fn claimed_record_mut<'a>(
    records: &'a mut BTreeMap<String, AgentCheckpointRecord>,
    claim: &AgentCheckpointClaim,
) -> Result<&'a mut AgentCheckpointRecord> {
    let record = records
        .get_mut(&claim.record.conversation_id)
        .ok_or_else(|| anyhow!("Agent checkpoint claim is no longer present"))?;
    if record.claim_id.as_deref() != Some(claim.claim_id.as_str()) {
        bail!("Agent checkpoint claim is stale");
    }
    Ok(record)
}

fn validate_records(records: &[AgentCheckpointRecord]) -> Result<()> {
    let mut conversations = HashSet::new();
    let mut checkpoint_ids = HashSet::new();
    for record in records {
        if record.checkpoint_id.trim().is_empty()
            || record.conversation_id.trim().is_empty()
            || record.owner_id.trim().is_empty()
            || record.tenant_id.trim().is_empty()
            || record.agent_id.trim().is_empty()
            || record.dsl_fingerprint.trim().is_empty()
        {
            bail!("Agent checkpoint identity fields must not be empty");
        }
        if !conversations.insert(record.conversation_id.as_str()) {
            bail!(
                "Duplicate Agent checkpoint conversation: {}",
                record.conversation_id
            );
        }
        if !checkpoint_ids.insert(record.checkpoint_id.as_str()) {
            bail!(
                "Duplicate Agent checkpoint resume token: {}",
                record.checkpoint_id
            );
        }
        if record.claim_id.is_none() && record.claim_expires_at != 0 {
            bail!(
                "Agent checkpoint '{}' has a lease without a claim",
                record.checkpoint_id
            );
        }
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checkpoint() -> AgentWorkflowCheckpoint {
        serde_json::from_value(serde_json::json!({
            "version": 1,
            "waiting_component_id": "fill",
            "next_execution_index": 1,
            "selected": ["begin", "fill"],
            "runtime": {},
            "transient": {
                "last_message": null,
                "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
                "provider_calls": 0,
                "references": []
            },
            "trace": []
        }))
        .unwrap()
    }

    #[test]
    fn durable_store_roundtrips_and_consumes_one_claim() {
        let root =
            std::env::temp_dir().join(format!("rayrag-agent-checkpoints-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("agent-checkpoints.json");
        let store = AgentCheckpointStore::new(&path).unwrap();
        let saved = store
            .save_waiting(
                "conversation",
                "owner",
                "tenant",
                "agent",
                "fingerprint",
                checkpoint(),
            )
            .unwrap();
        drop(store);

        let restored = AgentCheckpointStore::new(&path).unwrap();
        let _claim = match restored
            .claim(
                "conversation",
                "owner",
                "tenant",
                "agent",
                Some(&saved.checkpoint_id),
            )
            .unwrap()
        {
            AgentCheckpointClaimResult::Claimed(claim) => claim,
            result => panic!("unexpected claim result: {result:?}"),
        };
        assert!(matches!(
            restored
                .claim("conversation", "owner", "tenant", "agent", None)
                .unwrap(),
            AgentCheckpointClaimResult::Busy
        ));
        drop(restored);

        let recovered_after_claim_crash = AgentCheckpointStore::new(&path).unwrap();
        let claim = match recovered_after_claim_crash
            .claim(
                "conversation",
                "owner",
                "tenant",
                "agent",
                Some(&saved.checkpoint_id),
            )
            .unwrap()
        {
            AgentCheckpointClaimResult::Claimed(claim) => claim,
            result => panic!("unexpected post-restart claim result: {result:?}"),
        };
        recovered_after_claim_crash.delete_claimed(&claim).unwrap();
        assert!(matches!(
            recovered_after_claim_crash
                .claim("conversation", "owner", "tenant", "agent", None)
                .unwrap(),
            AgentCheckpointClaimResult::Missing
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checkpoint_is_scoped_and_resume_token_is_checked() {
        let store = AgentCheckpointStore::in_memory();
        let saved = store
            .save_waiting(
                "conversation",
                "owner",
                "tenant",
                "agent",
                "fingerprint",
                checkpoint(),
            )
            .unwrap();
        assert!(matches!(
            store
                .claim("conversation", "intruder", "tenant", "agent", None)
                .unwrap(),
            AgentCheckpointClaimResult::Missing
        ));
        assert!(matches!(
            store
                .claim("conversation", "owner", "tenant", "agent", Some("wrong"))
                .unwrap(),
            AgentCheckpointClaimResult::ResumeTokenMismatch
        ));
        let claim = match store
            .claim(
                "conversation",
                "owner",
                "tenant",
                "agent",
                Some(&saved.checkpoint_id),
            )
            .unwrap()
        {
            AgentCheckpointClaimResult::Claimed(claim) => claim,
            result => panic!("unexpected claim result: {result:?}"),
        };
        store.release(&claim).unwrap();
        assert!(matches!(
            store
                .claim("conversation", "owner", "tenant", "agent", None)
                .unwrap(),
            AgentCheckpointClaimResult::Claimed(_)
        ));
    }

    #[test]
    fn normalized_dsl_fingerprint_is_stable() {
        let dsl = serde_json::json!({
            "components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}}
            }
        });
        assert_eq!(
            agent_dsl_fingerprint(&dsl).unwrap(),
            agent_dsl_fingerprint(&normalize_agent_dsl_for_run(&dsl)).unwrap()
        );
    }
}
