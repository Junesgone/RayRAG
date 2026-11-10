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
