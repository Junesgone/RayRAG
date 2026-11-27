//! Wiki synthesis host adapter — binds the structure-compile runner's
//! synthesis contract (`StructureCompileBackend::wiki_plan_from_reduction` /
//! `wiki_refine_from_plan`) to the ported `knowlege_wiki` pipeline.
//!
//! The runner only calls these two methods during its optional synthesis
//! phase (template `synthesis.enabled` + non-empty `example`); a full host
//! backend can delegate here instead of re-implementing the wiki phases.

use crate::doc_store::DocStore;
use crate::embed::Embedder;
use crate::harness::HarnessChat;
use crate::harness::knowlege_wiki;
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Host handle bundle for wiki synthesis calls.
pub struct WikiSynthHost {
    pub store: Arc<dyn DocStore>,
    pub chat: Arc<dyn HarnessChat>,
    pub embd: Option<Arc<dyn Embedder>>,
    /// Disabled documents (host-resolved; upstream fetches them internally).
    pub disabled_doc_ids: BTreeSet<String>,
    pub kb_name: Option<String>,
    pub kb_description: Option<String>,
}

impl WikiSynthHost {
    pub fn new(store: Arc<dyn DocStore>, chat: Arc<dyn HarnessChat>) -> Self {
        Self {
            store,
            chat,
            embd: None,
            disabled_doc_ids: BTreeSet::new(),
            kb_name: None,
            kb_description: None,
        }
    }

    pub fn with_embedder(mut self, embd: Arc<dyn Embedder>) -> Self {
        self.embd = Some(embd);
        self
    }

    /// `wiki_plan_from_reduction(tenant_id, kb_id)` — PLAN phase over the
    /// cached REDUCE result (thresholds/timeouts at ported defaults).
    pub async fn wiki_plan_from_reduction(
        &self,
        tenant_id: &str,
        kb_id: &str,
    ) -> std::result::Result<Value, String> {
        Ok(knowlege_wiki::wiki_plan_from_reduction(
            self.store.as_ref(),
            self.chat.as_ref(),
            self.embd.as_deref(),
            tenant_id,
            kb_id,
            &self.disabled_doc_ids,
            self.kb_name.as_deref(),
            self.kb_description.as_deref(),
            knowlege_wiki::DEFAULT_WIKI_PLAN_UPDATE_THRESHOLD,
            knowlege_wiki::DEFAULT_WIKI_PLAN_MAYBE_THRESHOLD,
            knowlege_wiki::DEFAULT_WIKI_PLAN_RECONCILE_BATCH,
            knowlege_wiki::default_wiki_plan_timeout(),
            false,
            None,
        )
        .await)
    }

    /// `wiki_refine_from_plan(tenant_id, kb_id, example)` — REFINE phase; the
    /// runner's synthesis `example` feeds the writer template override.
    pub async fn wiki_refine_from_plan(
        &self,
        tenant_id: &str,
        kb_id: &str,
        example: &str,
    ) -> std::result::Result<Vec<Value>, String> {
        let example = if example.is_empty() {
            None
        } else {
            Some(example)
        };
        Ok(knowlege_wiki::wiki_refine_from_plan(
            self.store.as_ref(),
            self.chat.as_ref(),
            self.embd.as_deref(),
            tenant_id,
            kb_id,
            knowlege_wiki::default_wiki_refine_workers(),
            knowlege_wiki::default_wiki_refine_timeout(),
            knowlege_wiki::WIKI_REFINE_SOURCE_BUDGET_CHARS,
            knowlege_wiki::WIKI_MERGE_BODY_SHRINK_THRESHOLD,
            false,
            None,
            None,
            example,
        )
        .await)
    }
}

#[cfg(test)]
mod wiki_synth_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::knowlege_wiki::persist_reduce;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct SequentialChat {
        replies: Mutex<VecDeque<String>>,
    }

    #[async_trait::async_trait]
    impl HarnessChat for SequentialChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            Ok(self.replies.lock().unwrap().pop_front().unwrap_or_default())
        }

        fn max_length(&self) -> usize {
            100_000
        }
    }

    #[tokio::test]
    async fn host_wires_plan_and_refine_end_to_end() {
        let store = Arc::new(MemoryDocStore::new());
        persist_reduce(
            store.as_ref(),
            "t1",
            "kb1",
            &serde_json::json!({
                "entities": [{"name": "Alpha", "type": "entity", "mention_count": 5, "chunk_ids": ["c1"]}],
                "concepts": [],
                "claims": [],
                "relations": [],
                "topics": ["t"]
            }),
            "H1",
            &[],
        );
        let chat = Arc::new(SequentialChat {
            replies: Mutex::new(
                [
                    r#"{"pages": [{"action": "CREATE", "slug": "entity/alpha", "title": "Alpha", "page_type": "entity", "topic": "alpha", "entity_names": ["Alpha"], "priority": 1}]}"#,
                    "TOPIC: alpha\n# Body\n\nGrounded text.",
                ]
                .iter()
                .map(|reply| (*reply).to_string())
                .collect(),
            ),
        });
        let host = WikiSynthHost::new(store.clone(), chat.clone());
        let plan = host
            .wiki_plan_from_reduction("t1", "kb1")
            .await
            .expect("plan");
        assert_eq!(plan["pages"].as_array().unwrap().len(), 1);
        let pages = host
            .wiki_refine_from_plan("t1", "kb1", "Example body")
            .await
            .expect("refine");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["slug"], serde_json::json!("entity/alpha"));
        // Old-mode REFINE persists resume drafts (searchable wiki_page rows are written by the task handler, matching upstream).
        let drafts =
            crate::harness::knowlege_wiki::wiki_load_refine_resume(store.as_ref(), "t1", "kb1");
        assert!(drafts.contains_key("entity/alpha"));
    }
}
