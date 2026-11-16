//! Compiled-product expansion for hybrid search — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/tools/compiled_expansion.py` (zero-LLM).
//!
//! When `hybrid_search` runs with `use_compiled=True` this module layers the
//! dataset's *compiled* products on top of the retrieved chunks: per-kind
//! compiled structure rows (page_index / timeline / mind_map /
//! knowledge_graph / tree), the synthesised pages (wiki / artifact / essence),
//! and wiki page drill-downs. Datasets without compiled products are unaffected
//! — every strategy short-circuits when its rows are absent.
//!
//! The store search (dense/keyword expr construction) and chunk loads are
//! injected through [`CompiledExpansionHost`], mirroring the search-leg and
//! navigation contracts.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use crate::harness::orchestrator::direct::Kbinfos;
use crate::harness::tools::exploration::KgScope;

/// HNSW ef_search floor (below the requested `top_n` the ANN search is bounded
/// by the candidate list instead of by relevance).
pub const VECTOR_NUM_CANDIDATES: usize = 256;
pub const VECTOR_SIMILARITY: f64 = 0.1;

/// The injected compiled-row store surface.
#[allow(clippy::too_many_arguments)]
#[async_trait]
pub trait CompiledExpansionHost: Send + Sync {
    /// `_search_compiled_rows`: `knowledge_graph_kwd` rows of one KB.
    async fn search_compiled_rows(
        &self,
        kb_id: &str,
        tenant_id: &str,
        doc_ids: Option<&[String]>,
        kind: &str,
        text: &str,
        top_n: usize,
        extra: &Map<String, Value>,
        compile_kwd: Option<&str>,
        template_kind: Option<&str>,
    ) -> Vec<Value>;

    /// `_search_synthesis_pages`: `compile_kwd` page rows with `available_int=1`.
    async fn search_synthesis_pages(
        &self,
        kb_id: &str,
        tenant_id: &str,
        doc_ids: Option<&[String]>,
        text: &str,
        compile_kwd: &str,
        top_n: usize,
    ) -> Vec<Value>;

    /// `_load_chunks_for_doc`.
    async fn load_chunks_for_doc(&self, doc_id: &str, chunk_ids: &[String]) -> Vec<Value>;
}

fn row_id(row: &Value) -> String {
    row.get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| row.to_string())
}

fn chunk_key(chunk: &Value) -> String {
    chunk
        .get("chunk_id")
        .or_else(|| chunk.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default()
}

/// Both name spellings — merged dataset rows lowercase endpoints while
/// per-doc rows keep original case.
fn endpoint_name_list(names: &HashSet<String>) -> Vec<String> {
    let mut list: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        list.insert(trimmed.to_string());
        list.insert(trimmed.to_lowercase());
    }
    list.into_iter().collect()
}

/// `_expand_compiled_strategy`: entity search → relation nav → chunk load.
#[allow(clippy::too_many_arguments)]
pub async fn expand_compiled_strategy(
    host: &dyn CompiledExpansionHost,
    kb_id: &str,
    tenant_id: &str,
    doc_ids: Option<&[String]>,
    query: &str,
    seen_ids: &mut HashSet<String>,
    compile_kwd: Option<&str>,
    template_kind: Option<&str>,
    max_chunks: usize,
) -> Vec<Value> {
    // 1. Seed entities.
    let seed_rows = host
        .search_compiled_rows(
            kb_id,
            tenant_id,
            doc_ids,
            "entity",
            query,
            5,
            &Map::new(),
            compile_kwd,
            template_kind,
        )
        .await;
    if seed_rows.is_empty() {
        return Vec::new();
    }
    let mut seed_names: HashSet<String> = HashSet::new();
    for row in &seed_rows {
        let payload: Value = serde_json::from_str(
            row.get("content_with_weight")
                .and_then(Value::as_str)
                .unwrap_or(""),
        )
        .unwrap_or_else(|_| json!({}));
        let name = payload
            .get("name")
            .or_else(|| payload.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !name.is_empty() {
            seed_names.insert(name);
        }
    }
    if seed_names.is_empty() {
        return Vec::new();
    }

    // 2. Adjacent relations (outgoing + incoming).
    let seed_list = endpoint_name_list(&seed_names);
    let mut from_extra = Map::new();
    from_extra.insert("from_entity_kwd".to_string(), json!(seed_list));
    let fwd = host
        .search_compiled_rows(
            kb_id,
            tenant_id,
            doc_ids,
            "relation",
            "",
            50,
            &from_extra,
            compile_kwd,
            template_kind,
        )
        .await;
    let mut to_extra = Map::new();
    to_extra.insert("to_entity_kwd".to_string(), json!(seed_list));
    let bwd = host
        .search_compiled_rows(
            kb_id,
            tenant_id,
            doc_ids,
            "relation",
            "",
            50,
            &to_extra,
            compile_kwd,
            template_kind,
        )
        .await;
    let mut rel_seen: HashSet<String> = HashSet::new();
    let mut all_rels: Vec<&Value> = Vec::new();
    for row in fwd.iter().chain(bwd.iter()) {
        if rel_seen.insert(row_id(row)) {
            all_rels.push(row);
        }
    }

    // 3. Neighbour names (1-hop, excluding seeds).
    let seed_lower: HashSet<String> = seed_names.iter().map(|name| name.to_lowercase()).collect();
    let mut neighbour_names: HashSet<String> = HashSet::new();
    for row in &all_rels {
        let from = row
            .get("from_entity_kwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let to = row
            .get("to_entity_kwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if seed_lower.contains(&from.to_lowercase())
            && !to.is_empty()
            && !seed_lower.contains(&to.to_lowercase())
        {
            neighbour_names.insert(to.clone());
        }
        if seed_lower.contains(&to.to_lowercase())
            && !from.is_empty()
            && !seed_lower.contains(&from.to_lowercase())
        {
            neighbour_names.insert(from.clone());
        }
    }
    if neighbour_names.is_empty() {
        return Vec::new();
    }

    // 4. Neighbour entity source_chunk_ids.
    let mut neigh_list = endpoint_name_list(&neighbour_names);
    neigh_list.truncate(100);
    let mut name_extra = Map::new();
    name_extra.insert("name_kwd".to_string(), json!(neigh_list));
    let neigh_rows = host
        .search_compiled_rows(
            kb_id,
            tenant_id,
            doc_ids,
            "entity",
            "",
            neigh_list.len().max(1),
            &name_extra,
            compile_kwd,
            template_kind,
        )
        .await;

    // Group chunk ids by doc (insertion order preserved).
    let mut doc_order: Vec<String> = Vec::new();
    let mut by_doc: HashMap<String, Vec<String>> = HashMap::new();
    for row in &neigh_rows {
        let doc_id = row
            .get("doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(ids) = row.get("source_chunk_ids").and_then(Value::as_array) {
            for id in ids {
                let Some(id) = id.as_str() else {
                    continue;
                };
                if id.is_empty() || seen_ids.contains(id) {
                    continue;
                }
                if !by_doc.contains_key(&doc_id) {
                    doc_order.push(doc_id.clone());
                }
                let entry = by_doc.entry(doc_id.clone()).or_default();
                if !entry.iter().any(|existing| existing == id) {
                    entry.push(id.to_string());
                }
            }
        }
    }

    // 5. Load and return.
    let mut new_chunks: Vec<Value> = Vec::new();
    for doc_id in doc_order {
        if new_chunks.len() >= max_chunks {
            break;
        }
        let limit = max_chunks - new_chunks.len();
        let cids: Vec<String> = by_doc
            .get(&doc_id)
            .map(|ids| ids.iter().take(limit).cloned().collect())
            .unwrap_or_default();
        for chunk in host.load_chunks_for_doc(&doc_id, &cids).await {
            let cid = chunk_key(&chunk);
            if !cid.is_empty() && seen_ids.insert(cid) {
                new_chunks.push(chunk);
            }
        }
    }
    new_chunks
}

/// `_expand_wiki_page_strategy`: synthesis pages → load referenced chunks.
pub async fn expand_wiki_page_strategy(
    host: &dyn CompiledExpansionHost,
    kb_id: &str,
    tenant_id: &str,
    doc_ids: Option<&[String]>,
    query: &str,
    seen_ids: &mut HashSet<String>,
    compile_kwd: &str,
    max_chunks: usize,
) -> Vec<Value> {
    let wiki_rows = host
        .search_synthesis_pages(kb_id, tenant_id, doc_ids, query, compile_kwd, 5)
        .await;
    if wiki_rows.is_empty() {
        return Vec::new();
    }
    let mut doc_order: Vec<String> = Vec::new();
    let mut by_doc: HashMap<String, Vec<String>> = HashMap::new();
    for row in &wiki_rows {
        let doc_id = row
            .get("doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(ids) = row.get("source_chunk_ids").and_then(Value::as_array) {
            for id in ids {
                let Some(id) = id.as_str() else {
                    continue;
                };
                if id.is_empty() || seen_ids.contains(id) {
                    continue;
                }
                if !by_doc.contains_key(&doc_id) {
                    doc_order.push(doc_id.clone());
                }
                let entry = by_doc.entry(doc_id.clone()).or_default();
                if !entry.iter().any(|existing| existing == id) {
                    entry.push(id.to_string());
                }
            }
        }
    }
    let mut new_chunks: Vec<Value> = Vec::new();
    for doc_id in doc_order {
        if new_chunks.len() >= max_chunks {
            break;
        }
        let limit = max_chunks - new_chunks.len();
        let cids: Vec<String> = by_doc
            .get(&doc_id)
            .map(|ids| ids.iter().take(limit).cloned().collect())
            .unwrap_or_default();
        for mut chunk in host.load_chunks_for_doc(&doc_id, &cids).await {
            let cid = chunk_key(&chunk);
            if !cid.is_empty() && seen_ids.insert(cid) {
                if let Some(object) = chunk.as_object_mut() {
                    object
                        .entry("similarity".to_string())
                        .or_insert_with(|| json!(0.9));
                }
                new_chunks.push(chunk);
            }
        }
    }
    new_chunks
}

/// `_expand_with_compiled`: zero-LLM compiled-product expansion on top of the
/// hybrid result. Returns how many chunks were added.
pub async fn expand_with_compiled(
    host: &dyn CompiledExpansionHost,
    scopes: &[KgScope],
    query: &str,
    _keywords: &str,
    kbinfos: &mut Kbinfos,
) -> usize {
    let before = kbinfos.chunks.len();
    let mut seen_ids: HashSet<String> = kbinfos
        .chunks
        .iter()
        .map(chunk_key)
        .filter(|id| !id.is_empty())
        .collect();
    if scopes.is_empty() {
        return 0;
    }
    for (kb_id, tenant_id, doc_ids) in scopes {
        let docs = doc_ids.as_deref();
        // 1-hop entity-graph expansion per template kind.
        for template_kind in ["knowledge_graph", "mind_map", "timeline", "page_index"] {
            let chunks = expand_compiled_strategy(
                host,
                kb_id,
                tenant_id,
                docs,
                query,
                &mut seen_ids,
                None,
                Some(template_kind),
                5,
            )
            .await;
            kbinfos.chunks.extend(chunks);
        }
        // Tree structure graph (uses compile_kwd, not template kind).
        let chunks = expand_compiled_strategy(
            host,
            kb_id,
            tenant_id,
            docs,
            query,
            &mut seen_ids,
            Some("tree"),
            None,
            5,
        )
        .await;
        kbinfos.chunks.extend(chunks);
        // Synthesis pages.
        for compile_kwd in ["wiki_page", "artifact_page", "essence"] {
            let chunks = expand_wiki_page_strategy(
                host,
                kb_id,
                tenant_id,
                docs,
                query,
                &mut seen_ids,
                compile_kwd,
                5,
            )
            .await;
            kbinfos.chunks.extend(chunks);
        }
    }
    // Re-sort so compiled-expansion chunks blend by similarity.
    kbinfos.chunks.sort_by(|left, right| {
        let left_score = left
            .get("similarity")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let right_score = right
            .get("similarity")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        right_score
            .partial_cmp(&left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    kbinfos.chunks.len().saturating_sub(before)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockHost {
        seeds: Vec<Value>,
        fwd: Vec<Value>,
        bwd: Vec<Value>,
        neighbours: Vec<Value>,
        pages: Vec<Value>,
        chunks: Vec<Value>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl MockHost {
        fn new() -> Self {
            Self {
                seeds: Vec::new(),
                fwd: Vec::new(),
                bwd: Vec::new(),
                neighbours: Vec::new(),
                pages: Vec::new(),
                chunks: Vec::new(),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    fn entity_row(name: &str, doc: &str, chunks: &[&str]) -> Value {
        json!({
            "id": format!("e:{name}"),
            "content_with_weight": json!({"name": name}).to_string(),
            "doc_id": doc,
            "source_chunk_ids": chunks,
        })
    }

    fn relation_row(from: &str, to: &str) -> Value {
        json!({
            "id": format!("r:{from}:{to}"),
            "from_entity_kwd": from,
            "to_entity_kwd": to,
            "source_chunk_ids": ["rc"],
            "doc_id": "d1",
        })
    }

    #[async_trait]
    impl CompiledExpansionHost for MockHost {
        async fn search_compiled_rows(
            &self,
            _kb_id: &str,
            _tenant_id: &str,
            _doc_ids: Option<&[String]>,
            kind: &str,
            _text: &str,
            _top_n: usize,
            extra: &Map<String, Value>,
            compile_kwd: Option<&str>,
            template_kind: Option<&str>,
        ) -> Vec<Value> {
            self.calls.lock().unwrap().push(format!(
                "{kind}:{extra:?}:{compile_kwd:?}:{template_kind:?}"
            ));
            if kind == "entity" {
                if extra.contains_key("name_kwd") {
                    return self.neighbours.clone();
                }
                return self.seeds.clone();
            }
            if extra.contains_key("from_entity_kwd") {
                return self.fwd.clone();
            }
            if extra.contains_key("to_entity_kwd") {
                return self.bwd.clone();
            }
            Vec::new()
        }

        async fn search_synthesis_pages(
            &self,
            _kb_id: &str,
            _tenant_id: &str,
            _doc_ids: Option<&[String]>,
            _text: &str,
            compile_kwd: &str,
            _top_n: usize,
        ) -> Vec<Value> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("pages:{compile_kwd}"));
            self.pages.clone()
        }

        async fn load_chunks_for_doc(&self, _doc_id: &str, chunk_ids: &[String]) -> Vec<Value> {
            self.chunks
                .iter()
                .filter(|chunk| {
                    chunk
                        .get("chunk_id")
                        .and_then(Value::as_str)
                        .map(|id| chunk_ids.iter().any(|wanted| wanted == id))
                        .unwrap_or(false)
                })
                .cloned()
                .collect()
        }
    }

    #[tokio::test]
    async fn one_hop_expansion_loads_neighbour_chunks() {
        let host = MockHost {
            seeds: vec![entity_row("Alpha", "d1", &["s1"])],
            fwd: vec![relation_row("Alpha", "Beta")],
            neighbours: vec![entity_row("Beta", "d2", &["c1", "c2", "c3"])],
            chunks: vec![
                json!({"chunk_id": "c1", "content_with_weight": "one", "similarity": 0.5}),
                json!({"chunk_id": "c2", "content_with_weight": "two", "similarity": 0.4}),
                json!({"chunk_id": "c3", "content_with_weight": "three", "similarity": 0.3}),
            ],
            ..MockHost::new()
        };
        let mut seen: HashSet<String> = ["s1".to_string()].into_iter().collect();
        let chunks = expand_compiled_strategy(
            &host,
            "kb1",
            "t1",
            None,
            "alpha query",
            &mut seen,
            Some("tree"),
            None,
            2,
        )
        .await;
        assert_eq!(chunks.len(), 2, "max_chunks cap");
        assert!(seen.contains("c1") && seen.contains("c2"));
    }

    #[tokio::test]
    async fn synthesis_pages_get_priority_similarity() {
        let host = MockHost {
            pages: vec![json!({"id": "w1", "doc_id": "d1", "source_chunk_ids": ["c1"]})],
            chunks: vec![json!({"chunk_id": "c1", "content_with_weight": "wiki text"})],
            ..MockHost::new()
        };
        let mut seen: HashSet<String> = HashSet::new();
        let chunks =
            expand_wiki_page_strategy(&host, "kb1", "t1", None, "q", &mut seen, "wiki_page", 5)
                .await;
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["similarity"], json!(0.9));
    }

    #[tokio::test]
    async fn expand_with_compiled_sorts_and_counts() {
        let host = MockHost {
            seeds: vec![entity_row("Alpha", "d1", &[])],
            fwd: vec![relation_row("Alpha", "Beta")],
            neighbours: vec![entity_row("Beta", "d2", &["c2"])],
            chunks: vec![json!({"chunk_id": "c2", "content_with_weight": "x", "similarity": 0.95})],
            ..MockHost::new()
        };
        let mut kbinfos = Kbinfos {
            chunks: vec![
                json!({"chunk_id": "base", "content_with_weight": "base", "similarity": 0.2}),
            ],
            doc_aggs: vec![],
            pre_summary: None,
        };
        let scopes: Vec<KgScope> = vec![("kb1".to_string(), "t1".to_string(), None)];
        let added = expand_with_compiled(&host, &scopes, "q", "", &mut kbinfos).await;
        assert_eq!(added, 1);
        assert_eq!(
            kbinfos.chunks[0]["chunk_id"],
            json!("c2"),
            "compiled chunk sorts by similarity"
        );
        // All template kinds + tree + three synthesis kinds were searched.
        let calls = host.calls.lock().unwrap().clone();
        assert!(calls.iter().any(|call| call.starts_with("pages:essence")));
        assert!(
            calls
                .iter()
                .filter(|call| call.contains("Some(\"knowledge_graph\")"))
                .count()
                >= 1
        );

        // Empty scopes short-circuit.
        let added = expand_with_compiled(&host, &[], "q", "", &mut kbinfos).await;
        assert_eq!(added, 0);
    }
}
