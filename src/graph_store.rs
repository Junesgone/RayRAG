//! Persistent GraphRAG document checkpoints and derived KB graphs.

use crate::graphrag_enhanced::{EntityGraph, NerExtractor};
use crate::search::IndexedChunk;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphCheckpoint {
    pub doc_id: String,
    pub kb_id: String,
    pub content_hash: String,
    pub method: String,
    pub entity_types: Vec<String>,
    pub graph: EntityGraph,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphArtifacts {
    pub checkpoints: HashMap<String, GraphCheckpoint>,
    pub kb_graphs: HashMap<String, EntityGraph>,
    /// Community reports keyed by knowledge base then report title. Persisted
    /// so Microsoft-GraphRAG-style global search does not re-run the expensive
    /// LLM community-report pass on every query.
    #[serde(default)]
    pub community_reports: HashMap<String, HashMap<String, String>>,
    /// Optional embedding per community report title, used by global/DRIFT
    /// search when an embedding model is configured.
    #[serde(default)]
    pub report_embeddings: HashMap<String, HashMap<String, Vec<f32>>>,
    /// Optional embedding per entity (name -> vector), used to complement
    /// lexical KgSearch local retrieval.
    #[serde(default)]
    pub entity_embeddings: HashMap<String, HashMap<String, Vec<f32>>>,
}

pub struct GraphStore {
    artifacts: RwLock<GraphArtifacts>,
    file_path: String,
    save_lock: Mutex<()>,
}

impl GraphStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        let artifacts = if std::path::Path::new(path).exists() {
            serde_json::from_slice(&std::fs::read(path)?)?
        } else {
            GraphArtifacts::default()
        };
        Ok(Self {
            artifacts: RwLock::new(artifacts),
            file_path: path.into(),
            save_lock: Mutex::new(()),
        })
    }

    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            artifacts: RwLock::new(GraphArtifacts::default()),
            file_path: String::new(),
            save_lock: Mutex::new(()),
        }
    }

    pub fn build_checkpoint(
        doc_id: &str,
        kb_id: &str,
        content_hash: &str,
        method: &str,
        entity_types: &[String],
        chunks: &[IndexedChunk],
    ) -> GraphCheckpoint {
        let extractor = NerExtractor::new();
        let mut graph = EntityGraph::new();
        for chunk in chunks {
            let entities = extractor.extract(&chunk.content);
            graph.add_entities(&entities);
            for pair in entities.windows(2) {
                graph.add_relation(&pair[0].name, &pair[1].name, 1.0);
            }
        }
        GraphCheckpoint {
            doc_id: doc_id.into(),
            kb_id: kb_id.into(),
            content_hash: content_hash.into(),
            method: method.into(),
            entity_types: entity_types.to_vec(),
            graph,
        }
    }

    pub fn snapshot(&self) -> GraphArtifacts {
        self.artifacts.read().unwrap().clone()
    }

    pub fn replace_document(
        &self,
        doc_id: &str,
        checkpoint: Option<GraphCheckpoint>,
    ) -> anyhow::Result<()> {
        let mut next = self.snapshot();
        let affected_kb = checkpoint
            .as_ref()
            .map(|value| value.kb_id.clone())
            .or_else(|| {
                next.checkpoints
                    .get(doc_id)
                    .map(|value| value.kb_id.clone())
            });
        next.checkpoints.remove(doc_id);
        if let Some(checkpoint) = checkpoint {
            next.checkpoints.insert(doc_id.into(), checkpoint);
        }
        if let Some(kb_id) = affected_kb {
            rebuild_kb_graph(&mut next, &kb_id);
        }
        self.replace_snapshot(next)
    }

    pub fn replace_snapshot(&self, snapshot: GraphArtifacts) -> anyhow::Result<()> {
        let _save_guard = self.save_lock.lock().unwrap();
        if !self.file_path.is_empty() {
            let data = serde_json::to_vec_pretty(&snapshot)?;
            crate::persistence::atomic_write(std::path::Path::new(&self.file_path), &data)?;
        }
        *self.artifacts.write().unwrap() = snapshot;
        Ok(())
    }

    pub fn community_reports_for(&self, kb_id: &str) -> HashMap<String, String> {
        self.artifacts
            .read()
            .unwrap()
            .community_reports
            .get(kb_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn report_embeddings_for(&self, kb_id: &str) -> HashMap<String, Vec<f32>> {
        self.artifacts
            .read()
            .unwrap()
            .report_embeddings
            .get(kb_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn entity_embeddings_for(&self, kb_id: &str) -> HashMap<String, Vec<f32>> {
        self.artifacts
            .read()
            .unwrap()
            .entity_embeddings
            .get(kb_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_entity_embeddings(
        &self,
        kb_id: &str,
        embeddings: HashMap<String, Vec<f32>>,
    ) -> anyhow::Result<()> {
        let mut next = self.snapshot();
        if embeddings.is_empty() {
            next.entity_embeddings.remove(kb_id);
        } else {
            next.entity_embeddings.insert(kb_id.to_string(), embeddings);
        }
        self.replace_snapshot(next)
    }

    pub fn set_report_embeddings(
        &self,
        kb_id: &str,
        embeddings: HashMap<String, Vec<f32>>,
    ) -> anyhow::Result<()> {
        let mut next = self.snapshot();
        if embeddings.is_empty() {
            next.report_embeddings.remove(kb_id);
        } else {
            next.report_embeddings.insert(kb_id.to_string(), embeddings);
        }
        self.replace_snapshot(next)
    }

    pub fn set_community_reports(
        &self,
        kb_id: &str,
        reports: HashMap<String, String>,
    ) -> anyhow::Result<()> {
        let mut next = self.snapshot();
        if reports.is_empty() {
            next.community_reports.remove(kb_id);
        } else {
            next.community_reports.insert(kb_id.to_string(), reports);
        }
        self.replace_snapshot(next)
    }

    pub fn context_for_query(&self, kb_ids: &[String], query: &str) -> Vec<String> {
        let artifacts = self.artifacts.read().unwrap();
        let mut context = Vec::new();
        for kb_id in kb_ids {
            if let Some(graph) = artifacts.kb_graphs.get(kb_id) {
                context.extend(graph.context_for_query(query, 8));
            }
        }
        context.sort();
        context.dedup();
        context.truncate(16);
        context
    }
}

fn rebuild_kb_graph(artifacts: &mut GraphArtifacts, kb_id: &str) {
    let mut merged = EntityGraph::new();
    for checkpoint in artifacts
        .checkpoints
        .values()
        .filter(|checkpoint| checkpoint.kb_id == kb_id)
    {
        merged.merge(&checkpoint.graph);
    }
    artifacts.kb_graphs.insert(kb_id.into(), merged);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, doc_id: &str, kb_id: &str, content: &str) -> IndexedChunk {
        IndexedChunk {
            id: id.into(),
            doc_name: format!("{doc_id}.txt"),
            content: content.into(),
            embedding: vec![1.0, 0.0],
            token_count: 4,
            position: 0,
            metadata: HashMap::from([
                ("doc_id".into(), doc_id.into()),
                ("kb_id".into(), kb_id.into()),
            ]),
        }
    }

    #[test]
    fn document_replacement_rebuilds_only_its_kb_graph() {
        let store = GraphStore::in_memory();
        let first = GraphStore::build_checkpoint(
            "doc-a",
            "kb-a",
            "hash-a",
            "light",
            &[],
            &[chunk("a", "doc-a", "kb-a", "John Smith uses Rust")],
        );
        let second = GraphStore::build_checkpoint(
            "doc-b",
            "kb-b",
            "hash-b",
            "light",
            &[],
            &[chunk("b", "doc-b", "kb-b", "Google Inc uses Docker")],
        );
        store.replace_document("doc-a", Some(first)).unwrap();
        store.replace_document("doc-b", Some(second)).unwrap();
        assert!(
            !store
                .context_for_query(&["kb-a".into()], "John Smith")
                .is_empty()
        );
        assert!(
            store
                .context_for_query(&["kb-a".into()], "Google Inc")
                .is_empty()
        );

        store.replace_document("doc-a", None).unwrap();
        assert!(
            store
                .context_for_query(&["kb-a".into()], "John Smith")
                .is_empty()
        );
        assert!(
            !store
                .context_for_query(&["kb-b".into()], "Google Inc")
                .is_empty()
        );
    }

    #[test]
    fn community_reports_round_trip_and_legacy_snapshots_default_empty() {
        let legacy: GraphArtifacts = serde_json::from_str(r#"{"checkpoints":{},"kb_graphs":{}}"#)
            .expect("older graphrag.json must still load");
        assert!(legacy.community_reports.is_empty());
        let store = GraphStore::in_memory();
        let reports = HashMap::from([(
            "Battery recycling network".to_string(),
            "# Battery recycling network\nPartners collect batteries.".to_string(),
        )]);
        store
            .set_community_reports("kb-a", reports.clone())
            .unwrap();
        assert_eq!(store.community_reports_for("kb-a"), reports);
        let vectors = HashMap::from([("Battery recycling network".to_string(), vec![0.1, 0.9])]);
        store
            .set_report_embeddings("kb-a", vectors.clone())
            .unwrap();
        assert_eq!(store.report_embeddings_for("kb-a"), vectors);
        store.set_report_embeddings("kb-a", HashMap::new()).unwrap();
        assert!(store.report_embeddings_for("kb-a").is_empty());
        let entities = HashMap::from([("Battery Co".to_string(), vec![0.2, 0.8])]);
        store
            .set_entity_embeddings("kb-a", entities.clone())
            .unwrap();
        assert_eq!(store.entity_embeddings_for("kb-a"), entities);
        store.set_entity_embeddings("kb-a", HashMap::new()).unwrap();
        assert!(store.entity_embeddings_for("kb-a").is_empty());
        store.set_community_reports("kb-a", HashMap::new()).unwrap();
        assert!(store.community_reports_for("kb-a").is_empty());
    }

    #[test]
    fn failed_persistence_keeps_previous_graph_snapshot() {
        let root = std::env::temp_dir().join(format!("rayrag-graph-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("graphrag.json");
        let store = GraphStore::new(path.to_str().unwrap()).unwrap();
        let checkpoint = GraphStore::build_checkpoint(
            "doc-a",
            "kb-a",
            "hash-a",
            "light",
            &[],
            &[chunk("a", "doc-a", "kb-a", "John Smith uses Rust")],
        );
        store.replace_document("doc-a", Some(checkpoint)).unwrap();
        let before = store.snapshot();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(store.replace_document("doc-a", None).is_err());
        assert_eq!(store.snapshot().checkpoints.len(), before.checkpoints.len());
        std::fs::remove_dir_all(root).ok();
    }
}
