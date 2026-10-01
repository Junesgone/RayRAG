//! Navigation tools over a dataset's compiled structures — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/tools/navigation.py`.
//!
//! Slice 1 (this batch): the shared constants, [`NavResult`], the kind
//! normalizer and the two helpers `graph_explore` consumes —
//! [`load_chunks_by_ids`] and [`doc_aggs`]. The dataset-navigation-tree and
//! document-structure routers land in the next batch on top of these.

use async_trait::async_trait;
use serde_json::{Value, json};

/// Compiled-structure kinds that describe a document's *layout*.
pub const CATALOG_KINDS: [&str; 5] = ["tree", "timeline", "raptor", "page_index", "pageindex"];
/// Compiled-structure kinds that describe the document's *concepts*.
pub const MINDMAP_KINDS: [&str; 2] = ["mindmap", "mind_map"];
/// Cap on evidence chunks pulled from a compiled-structure outline.
pub const MAX_EVIDENCE_CHUNKS: usize = 24;
/// Cap on entities offered to the nav-tree entity selector.
pub const MAX_ENTITIES: usize = 300;

/// Structured outcome of ONE compiled-navigation call (orchestrator signals:
/// did this dataset have the structure, did the query reach anything, was the
/// result worth using).
#[derive(Debug, Clone, Default)]
pub struct NavResult {
    pub text: String,
    pub doc_ids: Vec<String>,
    pub routed_docs: Vec<(String, String)>,
    pub entities: usize,
    pub chunk_ptrs: usize,
    pub top_score: f64,
    pub chunk_paths: std::collections::HashMap<String, String>,
    /// `""` when usable, else `no_structure` / `no_doc` / `infra` / `bad_args`.
    pub empty_reason: String,
}

/// `_normalize_kind`: mirror the API's normalization
/// (`page_index` / `knowledge_graph` -> `timeline`).
pub fn normalize_kind(kind: &Value) -> String {
    let Value::String(text) = kind else {
        return String::new();
    };
    let normalized = text.trim().to_lowercase().replace('-', "_");
    match normalized.as_str() {
        "pageindex" | "page_index" | "knowledge_graph" => "timeline".to_string(),
        _ => normalized,
    }
}

/// The doc-store surface this module reads.
#[async_trait]
pub trait NavigationStore: Send + Sync {
    /// Rows for the id-filtered chunk query (`id` + `content_with_weight`,
    /// `docnm_kwd`, `doc_id` fields).
    async fn chunks_by_ids(&self, doc_id: &str, chunk_ids: &[String])
    -> Result<Vec<Value>, String>;
}

/// `_load_chunks_by_ids`: fetch chunks by their ids from the doc store.
pub async fn load_chunks_by_ids(
    store: &dyn NavigationStore,
    doc_id: &str,
    chunk_ids: &[String],
) -> Vec<Value> {
    if chunk_ids.is_empty() {
        return Vec::new();
    }
    let capped: Vec<String> = chunk_ids
        .iter()
        .take(MAX_EVIDENCE_CHUNKS)
        .cloned()
        .collect();
    let Ok(rows) = store.chunks_by_ids(doc_id, &capped).await else {
        return Vec::new();
    };
    rows.into_iter()
        .map(|row| {
            let id = row
                .get("id")
                .or_else(|| row.get("chunk_id"))
                .cloned()
                .unwrap_or(Value::String(String::new()));
            json!({
                "chunk_id": id,
                "content_with_weight": row
                    .get("content_with_weight")
                    .cloned()
                    .unwrap_or(Value::String(String::new())),
                "docnm_kwd": row
                    .get("docnm_kwd")
                    .cloned()
                    .unwrap_or(Value::String(String::new())),
                "doc_id": row
                    .get("doc_id")
                    .cloned()
                    .unwrap_or(Value::String(doc_id.to_string())),
            })
        })
        .collect()
}

/// `_doc_aggs`: ordered document aggregates for the returned chunks.
pub fn doc_aggs(chunks: &[Value]) -> Vec<Value> {
    let mut aggs: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for chunk in chunks {
        let Some(doc_id) = chunk.get("doc_id").and_then(Value::as_str) else {
            continue;
        };
        if doc_id.is_empty() || !seen.insert(doc_id.to_string()) {
            continue;
        }
        aggs.push(json!({
            "doc_id": doc_id,
            "doc_name": chunk.get("docnm_kwd").and_then(Value::as_str).unwrap_or(""),
        }));
    }
    aggs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Store {
        rows: Vec<Value>,
        last: Mutex<Option<Vec<String>>>,
    }

    #[async_trait]
    impl NavigationStore for Store {
        async fn chunks_by_ids(
            &self,
            _doc_id: &str,
            chunk_ids: &[String],
        ) -> Result<Vec<Value>, String> {
            *self.last.lock().unwrap() = Some(chunk_ids.to_vec());
            Ok(self
                .rows
                .iter()
                .filter(|row| {
                    row.get("id")
                        .and_then(Value::as_str)
                        .map(|id| chunk_ids.iter().any(|wanted| wanted == id))
                        .unwrap_or(false)
                })
                .cloned()
                .collect())
        }
    }

    #[test]
    fn kind_normalization_matches_api() {
        assert_eq!(normalize_kind(&json!("Page-Index")), "timeline");
        assert_eq!(normalize_kind(&json!("KNOWLEDGE_GRAPH")), "timeline");
        assert_eq!(normalize_kind(&json!("mindmap")), "mindmap");
        assert_eq!(normalize_kind(&json!(42)), "");
    }

    #[tokio::test]
    async fn chunk_loading_caps_and_maps() {
        let store = Store {
            rows: vec![
                json!({"id": "c1", "content_with_weight": "alpha", "docnm_kwd": "Doc A"}),
                json!({"id": "c2", "content_with_weight": "beta", "docnm_kwd": "Doc A"}),
            ],
            last: Mutex::new(None),
        };
        let chunks = load_chunks_by_ids(&store, "d1", &["c1".to_string(), "c9".to_string()]).await;
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["chunk_id"], json!("c1"));
        assert_eq!(chunks[0]["doc_id"], json!("d1"));
        assert!(load_chunks_by_ids(&store, "d1", &[]).await.is_empty());

        let aggs = doc_aggs(&chunks);
        assert_eq!(aggs.len(), 1);
        assert_eq!(aggs[0]["doc_id"], json!("d1"));
        assert_eq!(aggs[0]["doc_name"], json!("Doc A"));
    }
}
