//! Exploration tools: knowledge-graph walks and wiki page drill-downs —
//! RAGFlow v0.27.2 `rag/advanced_rag/harness/tools/exploration.py`.
//!
//! 1. [`graph_explore`] — seeds entities by dense similarity to the question,
//!    expands BFS over compiled `relation` rows for a bounded number of hops,
//!    then asks the model whether the resulting subgraph answers the question;
//!    when it does not, the subgraph becomes evidence passages.
//! 2. [`wiki_query`] — asks a question of one dataset's compiled wiki pages.

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use crate::harness::tools::navigation::doc_aggs;
use crate::harness::tools::text_processing::narrow_by_keywords;

/// Scope of the compiled KG rows we search.
pub const SCOPE_KWD_DATASET: &str = "dataset";
pub const SCOPE_KWD_DOC: &str = "doc";

pub const KG_SEEDS: usize = 2; // top-N entities matched directly to the question
pub const KG_SEED_POOL: usize = 64; // KNN candidate pool before the mention_count re-sort
pub const KG_SEED_SIM: f64 = 0.8; // dense-similarity floor for seed entities
pub const KG_HOPS: usize = 2; // relation hops out from the seeds
pub const KG_NEIGHBORS: usize = 128; // cap on neighbour entity rows resolved per hop
pub const KG_REL_LIMIT: usize = 32; // relations fetched per endpoint filter

pub const WIKI_DRAFT_COMPILE_KWD: &str = "wiki_page_draft";
pub const WIKI_QUERY_TOP_N: usize = 12;

/// One exploration scope: `(kb_id, tenant_id, doc_ids)` (`doc_ids = None`
/// means the dataset-merged graph).
pub type KgScope = (String, String, Option<Vec<String>>);

/// `_kg_scopes`: resolve the scopes to search. With a `doc_scope` the graph is
/// limited to those docs (grouped by their KB); otherwise the whole bound KB
/// graph is explored.
pub fn kg_scopes(
    scoped_doc_ids: Option<&(dyn Fn(Option<Vec<String>>) -> Vec<String> + Send + Sync)>,
    doc_scope: Option<Vec<String>>,
    resolve_doc_tenant: &(dyn Fn(&str) -> Option<(String, String)> + Send + Sync),
    kbs: &[(String, String)],
) -> Vec<KgScope> {
    let doc_scope = match scoped_doc_ids {
        Some(hook) => hook(doc_scope),
        None => doc_scope.unwrap_or_default(),
    };
    if !doc_scope.is_empty() {
        let mut order: Vec<(String, String)> = Vec::new();
        let mut by_kb: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for doc_id in doc_scope {
            let Some(resolved) = resolve_doc_tenant(&doc_id) else {
                continue;
            };
            if !by_kb.contains_key(&resolved) {
                order.push(resolved.clone());
            }
            by_kb.entry(resolved).or_default().push(doc_id);
        }
        return order
            .into_iter()
            .map(|key| {
                let docs = by_kb.remove(&key).unwrap_or_default();
                (key.0, key.1, Some(docs))
            })
            .collect();
    }
    kbs.iter()
        .map(|(kb_id, tenant_id)| (kb_id.clone(), tenant_id.clone(), None))
        .collect()
}

/// `_kg_parse_entity`.
pub fn kg_parse_entity(row: &Value) -> Option<Value> {
    let payload: Value = serde_json::from_str(
        row.get("content_with_weight")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )
    .unwrap_or_else(|_| json!({}));
    let name = ["name", "term", "title"]
        .iter()
        .find_map(|key| payload.get(*key).and_then(Value::as_str))
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let aliases: Vec<String> = payload
        .get("aliases")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Some(json!({
        "name": name,
        "type": payload.get("type").and_then(Value::as_str).unwrap_or("other"),
        "description": payload.get("description").and_then(Value::as_str).unwrap_or(""),
        "aliases": aliases,
        "source_chunk_ids": row.get("source_chunk_ids").cloned().unwrap_or(json!([])),
        "doc_id": row.get("doc_id").and_then(Value::as_str).unwrap_or(""),
        "docnm_kwd": row.get("docnm_kwd").and_then(Value::as_str).unwrap_or(""),
    }))
}

/// `_kg_parse_relation`.
pub fn kg_parse_relation(row: &Value) -> Option<Value> {
    let source = row
        .get("from_entity_kwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let target = row
        .get("to_entity_kwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if source.is_empty() || target.is_empty() {
        return None;
    }
    let payload: Value = serde_json::from_str(
        row.get("content_with_weight")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )
    .unwrap_or_else(|_| json!({}));
    let relation_type = payload
        .get("type")
        .or_else(|| payload.get("relation"))
        .and_then(Value::as_str)
        .unwrap_or("related");
    Some(json!({
        "from": source,
        "to": target,
        "type": relation_type,
        "source_chunk_ids": row.get("source_chunk_ids").cloned().unwrap_or(json!([])),
        "doc_id": row.get("doc_id").and_then(Value::as_str).unwrap_or(""),
    }))
}

/// `_endpoint_terms`: case variants for matching relation endpoints (merged
/// rows lowercase endpoints while entity names keep their original case).
pub fn endpoint_terms(names: &[String]) -> Vec<String> {
    let mut terms: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for name in names {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            continue;
        }
        terms.insert(trimmed.to_string());
        terms.insert(trimmed.to_lowercase());
    }
    terms.into_iter().collect()
}

/// `_collect_evidence_ids`: group the source_chunk_ids of the relevant
/// entities AND relations by doc (insertion order preserved).
pub fn collect_evidence_ids(
    entities: &[Value],
    relations: &[Value],
    relevant_names: &[String],
) -> Vec<(String, Vec<String>)> {
    let wanted: std::collections::HashSet<String> = relevant_names
        .iter()
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .collect();
    let mut order: Vec<String> = Vec::new();
    let mut by_doc: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    let mut add = |doc_id: String, ids: Option<&Vec<Value>>| {
        for id in ids.into_iter().flatten() {
            let Some(id) = id.as_str() else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            if !seen.insert((doc_id.clone(), id.to_string())) {
                continue;
            }
            if !by_doc.contains_key(&doc_id) {
                order.push(doc_id.clone());
            }
            by_doc
                .entry(doc_id.clone())
                .or_default()
                .push(id.to_string());
        }
    };
    for entity in entities {
        let entity_name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
        names.insert(entity_name);
        if let Some(aliases) = entity.get("aliases").and_then(Value::as_array) {
            for alias in aliases {
                if let Some(alias) = alias.as_str() {
                    names.insert(alias.trim().to_lowercase());
                }
            }
        }
        if names.intersection(&wanted).next().is_some() {
            add(
                entity
                    .get("doc_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                entity.get("source_chunk_ids").and_then(Value::as_array),
            );
        }
    }
    for relation in relations {
        let mut endpoints: std::collections::HashSet<String> = std::collections::HashSet::new();
        for field in ["from", "to"] {
            if let Some(endpoint) = relation.get(field).and_then(Value::as_str) {
                endpoints.insert(endpoint.trim().to_lowercase());
            }
        }
        if endpoints.intersection(&wanted).next().is_some() {
            add(
                relation
                    .get("doc_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                relation.get("source_chunk_ids").and_then(Value::as_array),
            );
        }
    }
    order
        .into_iter()
        .map(|doc_id| {
            let ids = by_doc.remove(&doc_id).unwrap_or_default();
            (doc_id, ids)
        })
        .collect()
}

/// The injected store/model surface `graph_explore` / `wiki_query` drive.
#[allow(clippy::too_many_arguments)]
#[async_trait]
pub trait ExplorationHost: Send + Sync {
    /// One compiled-KG row search (`settings.docStoreConn.search` over
    /// `knowledge_graph_kwd` rows); returns the raw rows.
    async fn kg_search(
        &self,
        kb_id: &str,
        tenant_id: &str,
        doc_ids: Option<&[String]>,
        kind: &str,
        text: &str,
        top_n: usize,
        extra: &Map<String, Value>,
        scope_kwd: Option<&str>,
        order_desc: Option<&str>,
        pool: usize,
        similarity: f64,
    ) -> Vec<Value>;

    /// `_ask_structure`: does the subgraph answer the question?
    async fn ask_structure(
        &self,
        query: &str,
        entities: &[Value],
        relations: &[Value],
    ) -> (String, Vec<String>);

    /// `_load_chunks_by_ids`.
    async fn load_chunks_by_ids(&self, doc_id: &str, chunk_ids: &[String]) -> Vec<Value>;

    /// Wiki row search (`compile_kwd = wiki_page_draft`).
    async fn wiki_search(&self, kb_id: &str, tenant_id: &str, text: &str) -> Vec<Value>;
}

fn add_entities(
    new: Vec<Value>,
    scope_key: &str,
    entities: &mut Vec<Value>,
    ent_names: &mut std::collections::HashSet<String>,
) -> Vec<String> {
    let mut added: Vec<String> = Vec::new();
    for entity in new {
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let key = format!("{}:{}", scope_key, name.to_lowercase());
        if !ent_names.insert(key) {
            continue;
        }
        added.push(name.clone());
        entities.push(entity);
    }
    added
}

/// `graph_explore`: explore the compiled knowledge graph to answer `query`.
pub async fn graph_explore(
    host: &dyn ExplorationHost,
    scopes: &[KgScope],
    doc_scoped: bool,
    query: &str,
    keywords: &str,
) -> Value {
    let empty = json!({"answer": "", "chunks": [], "doc_aggs": []});
    if scopes.is_empty() {
        return empty;
    }
    let scope_kwd = if doc_scoped {
        SCOPE_KWD_DOC
    } else {
        SCOPE_KWD_DATASET
    };
    let text = format!("{query} {keywords}").trim().to_string();
    let mut entities: Vec<Value> = Vec::new();
    let mut relations: Vec<Value> = Vec::new();
    let mut ent_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (kb_id, tenant_id, doc_ids) in scopes {
        // (1) Seeds: dense match over the scoped entity rows, ranked by
        // mention_count_int desc, top KG_SEEDS.
        let seed_rows = host
            .kg_search(
                kb_id,
                tenant_id,
                doc_ids.as_deref(),
                "entity",
                &text,
                KG_SEEDS,
                &Map::new(),
                Some(scope_kwd),
                Some("mention_count_int"),
                KG_SEED_POOL,
                KG_SEED_SIM,
            )
            .await;
        let seeds: Vec<Value> = seed_rows.iter().filter_map(kg_parse_entity).collect();
        let mut frontier = add_entities(seeds, kb_id, &mut entities, &mut ent_names);

        // (2) Expand KG_HOPS out, collecting relations and neighbour entities.
        for _hop in 0..KG_HOPS {
            if frontier.is_empty() {
                break;
            }
            let terms = endpoint_terms(&frontier);
            let mut from_extra = Map::new();
            from_extra.insert("from_entity_kwd".to_string(), json!(terms));
            let mut rel_rows = host
                .kg_search(
                    kb_id,
                    tenant_id,
                    doc_ids.as_deref(),
                    "relation",
                    "",
                    KG_REL_LIMIT,
                    &from_extra,
                    Some(scope_kwd),
                    None,
                    0,
                    0.6,
                )
                .await;
            let mut to_extra = Map::new();
            to_extra.insert("to_entity_kwd".to_string(), json!(terms));
            rel_rows.extend(
                host.kg_search(
                    kb_id,
                    tenant_id,
                    doc_ids.as_deref(),
                    "relation",
                    "",
                    KG_REL_LIMIT,
                    &to_extra,
                    Some(scope_kwd),
                    None,
                    0,
                    0.6,
                )
                .await,
            );
            let hop_relations: Vec<Value> = rel_rows.iter().filter_map(kg_parse_relation).collect();
            relations.extend(hop_relations.clone());

            let prefix = format!("{kb_id}:");
            let seen_lower: std::collections::HashSet<String> = ent_names
                .iter()
                .filter_map(|key| key.strip_prefix(&prefix).map(str::to_string))
                .collect();
            let mut neigh_names: Vec<String> = Vec::new();
            for relation in &hop_relations {
                for field in ["from", "to"] {
                    if let Some(name) = relation.get(field).and_then(Value::as_str) {
                        let trimmed = name.trim();
                        if !trimmed.is_empty() && !neigh_names.iter().any(|n| n == trimmed) {
                            neigh_names.push(trimmed.to_string());
                        }
                    }
                }
            }
            let neigh_filtered: Vec<String> = neigh_names
                .iter()
                .filter(|name| !seen_lower.contains(&name.to_lowercase()))
                .cloned()
                .collect();
            if neigh_filtered.is_empty() {
                break;
            }
            let limit = neigh_filtered.len().max(1).min(KG_NEIGHBORS);
            let mut name_extra = Map::new();
            name_extra.insert(
                "name_kwd".to_string(),
                json!(endpoint_terms(&neigh_filtered)),
            );
            let neigh_rows = host
                .kg_search(
                    kb_id,
                    tenant_id,
                    doc_ids.as_deref(),
                    "entity",
                    "",
                    limit,
                    &name_extra,
                    Some(scope_kwd),
                    None,
                    0,
                    0.6,
                )
                .await;
            let neighbours: Vec<Value> = neigh_rows.iter().filter_map(kg_parse_entity).collect();
            frontier = add_entities(neighbours, kb_id, &mut entities, &mut ent_names);
        }
    }

    if entities.is_empty() && relations.is_empty() {
        return empty;
    }

    // (3) Does the subgraph answer the question?
    let (answer, relevant) = host.ask_structure(query, &entities, &relations).await;
    if !answer.is_empty() {
        return json!({"answer": answer, "chunks": [], "doc_aggs": []});
    }

    // (4) Insufficient — return the source passages behind the relevant nodes.
    let mut chunks: Vec<Value> = Vec::new();
    for (doc_id, ids) in collect_evidence_ids(&entities, &relations, &relevant) {
        if !doc_id.is_empty() && !ids.is_empty() {
            chunks.extend(host.load_chunks_by_ids(&doc_id, &ids).await);
        }
    }
    let narrowed = narrow_by_keywords(&chunks, keywords);
    json!({"answer": "", "chunks": narrowed, "doc_aggs": doc_aggs(&narrowed)})
}

/// `wiki_query`: search the compiled wiki (`wiki_page_draft` rows), parse the
/// page markdown out of each row and return the pages as chunks.
pub async fn wiki_query(
    host: &dyn ExplorationHost,
    kbs: &[(String, String)],
    query: &str,
    keywords: &str,
) -> Value {
    let text = format!("{query} {keywords}").trim().to_string();
    if kbs.is_empty() || text.is_empty() {
        return json!({"answer": "", "chunks": [], "doc_aggs": []});
    }
    let mut chunks: Vec<Value> = Vec::new();
    for (kb_id, tenant_id) in kbs {
        let rows = host.wiki_search(kb_id, tenant_id, &text).await;
        for row in rows {
            let page: Value = serde_json::from_str(
                row.get("content_with_weight")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
            )
            .unwrap_or_else(|_| json!({}));
            let content = page
                .get("content_md_rendered")
                .or_else(|| page.get("content_md"))
                .or_else(|| page.get("content_md_raw"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if content.is_empty() {
                continue;
            }
            let title = row
                .get("docnm_kwd")
                .and_then(Value::as_str)
                .or_else(|| page.get("title").and_then(Value::as_str))
                .or_else(|| row.get("title_kwd").and_then(Value::as_str))
                .unwrap_or("");
            let slug = row
                .get("wiki_slug_kwd")
                .and_then(Value::as_str)
                .or_else(|| page.get("slug").and_then(Value::as_str))
                .unwrap_or("");
            let doc_id = if !slug.is_empty() {
                slug.to_string()
            } else {
                row.get("doc_id")
                    .and_then(Value::as_str)
                    .unwrap_or(kb_id)
                    .to_string()
            };
            chunks.push(json!({
                "chunk_id": row.get("id").cloned().unwrap_or(json!("")),
                "content_with_weight": content,
                "docnm_kwd": title,
                "doc_id": doc_id,
                "wiki_slug_kwd": slug,
            }));
        }
    }
    let narrowed = narrow_by_keywords(&chunks, keywords);
    json!({"answer": "", "chunks": narrowed, "doc_aggs": doc_aggs(&narrowed)})
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[allow(clippy::too_many_arguments)]
    struct MockHost {
        seed_rows: Vec<Value>,
        from_rows: Vec<Value>,
        to_rows: Vec<Value>,
        name_rows: Vec<Value>,
        answer: String,
        relevant: Vec<String>,
        evidence: Vec<Value>,
        calls: Mutex<Vec<String>>,
    }

    impl Default for MockHost {
        fn default() -> Self {
            Self {
                seed_rows: vec![],
                from_rows: vec![],
                to_rows: vec![],
                name_rows: vec![],
                answer: String::new(),
                relevant: vec![],
                evidence: vec![],
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ExplorationHost for MockHost {
        async fn kg_search(
            &self,
            _kb_id: &str,
            _tenant_id: &str,
            _doc_ids: Option<&[String]>,
            kind: &str,
            _text: &str,
            _top_n: usize,
            extra: &Map<String, Value>,
            _scope_kwd: Option<&str>,
            _order_desc: Option<&str>,
            _pool: usize,
            _similarity: f64,
        ) -> Vec<Value> {
            self.calls.lock().unwrap().push(format!("{kind}:{extra:?}"));
            if extra.contains_key("from_entity_kwd") {
                return self.from_rows.clone();
            }
            if extra.contains_key("to_entity_kwd") {
                return self.to_rows.clone();
            }
            if extra.contains_key("name_kwd") {
                return self.name_rows.clone();
            }
            self.seed_rows.clone()
        }

        async fn ask_structure(
            &self,
            _query: &str,
            _entities: &[Value],
            _relations: &[Value],
        ) -> (String, Vec<String>) {
            (self.answer.clone(), self.relevant.clone())
        }

        async fn load_chunks_by_ids(&self, _doc_id: &str, _chunk_ids: &[String]) -> Vec<Value> {
            self.evidence.clone()
        }

        async fn wiki_search(&self, _kb_id: &str, _tenant_id: &str, _text: &str) -> Vec<Value> {
            self.calls.lock().unwrap().push("wiki".to_string());
            self.seed_rows.clone()
        }
    }

    fn entity_row(name: &str) -> Value {
        json!({
            "content_with_weight": json!({"name": name, "type": "Person", "description": "d"}).to_string(),
            "source_chunk_ids": ["c1"],
            "doc_id": "d1",
        })
    }

    fn relation_row(from: &str, to: &str) -> Value {
        json!({
            "from_entity_kwd": from,
            "to_entity_kwd": to,
            "content_with_weight": json!({"type": "knows"}).to_string(),
            "source_chunk_ids": ["c2"],
            "doc_id": "d1",
        })
    }

    #[test]
    fn parse_helpers_mirror_upstream() {
        let entity = kg_parse_entity(&entity_row("Alpha")).unwrap();
        assert_eq!(entity["name"], json!("Alpha"));
        assert_eq!(entity["type"], json!("Person"));
        assert_eq!(entity["doc_id"], json!("d1"));
        assert!(kg_parse_entity(&json!({"content_with_weight": "{}"})).is_none());

        let relation = kg_parse_relation(&relation_row("Alpha", "Beta")).unwrap();
        assert_eq!(relation["from"], json!("Alpha"));
        assert_eq!(relation["type"], json!("knows"));

        assert_eq!(
            endpoint_terms(&["Alpha".to_string()]),
            vec!["Alpha".to_string(), "alpha".to_string()]
        );
    }

    #[test]
    fn evidence_collection_groups_by_doc() {
        let entities = vec![kg_parse_entity(&entity_row("Alpha")).unwrap()];
        let relations = vec![kg_parse_relation(&relation_row("Alpha", "Beta")).unwrap()];
        let grouped = collect_evidence_ids(&entities, &relations, &["alpha".to_string()]);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].0, "d1");
        assert_eq!(grouped[0].1, vec!["c1".to_string(), "c2".to_string()]);
    }

    #[tokio::test]
    async fn graph_explore_answer_and_evidence_paths() {
        let scopes = vec![("kb1".to_string(), "t1".to_string(), None)];

        // Answered directly from the subgraph.
        let host = MockHost {
            seed_rows: vec![entity_row("Alpha")],
            from_rows: vec![relation_row("Alpha", "Beta")],
            name_rows: vec![entity_row("Beta")],
            answer: "42".to_string(),
            ..Default::default()
        };
        let result = graph_explore(&host, &scopes, false, "q", "").await;
        assert_eq!(result["answer"], json!("42"));
        assert!(result["chunks"].as_array().unwrap().is_empty());

        // Insufficient: relevant nodes' evidence chunks come back narrowed.
        let host = MockHost {
            seed_rows: vec![entity_row("Alpha")],
            from_rows: vec![relation_row("Alpha", "Beta")],
            name_rows: vec![entity_row("Beta")],
            relevant: vec!["Alpha".to_string()],
            evidence: vec![
                json!({"chunk_id": "c1", "content_with_weight": "alpha evidence about the topic", "doc_id": "d1"}),
            ],
            ..Default::default()
        };
        // Keyword narrowing needs 3+ comma terms (upstream bigram behaviour).
        let result = graph_explore(&host, &scopes, false, "q", "alpha, evidence, topic").await;
        assert_eq!(result["answer"], json!(""));
        assert_eq!(result["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(result["doc_aggs"][0]["doc_id"], json!("d1"));

        // No subgraph in scope -> empty contract.
        let host = MockHost::default();
        let result = graph_explore(&host, &scopes, true, "q", "").await;
        assert_eq!(result["answer"], json!(""));
        assert!(result["chunks"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn wiki_query_renders_pages() {
        let host = MockHost {
            seed_rows: vec![json!({
                "id": "w1",
                "content_with_weight": json!({"content_md": "# Alpha\nwiki body about alpha", "slug": "alpha-page"}).to_string(),
                "docnm_kwd": "Alpha Page",
            })],
            ..Default::default()
        };
        let kbs = vec![("kb1".to_string(), "t1".to_string())];
        let result = wiki_query(&host, &kbs, "alpha", "").await;
        assert_eq!(result["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(result["chunks"][0]["doc_id"], json!("alpha-page"));
        assert_eq!(result["chunks"][0]["docnm_kwd"], json!("Alpha Page"));
        assert!(
            wiki_query(&host, &[], "alpha", "").await["chunks"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
}
