//! Navigation tools over a dataset's compiled structures — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/tools/navigation.py`.
//!
//! Slice 1 (this batch): the shared constants, [`NavResult`], the kind
//! normalizer and the two helpers `graph_explore` consumes —
//! [`load_chunks_by_ids`] and [`doc_aggs`]. The dataset-navigation-tree and
//! document-structure routers land in the next batch on top of these.

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::harness::HarnessChat;
use crate::harness::tools::search::rank_chunks_by_terms;

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

// ── Dataset navigation (document router) — slice 2a ─────────────────────────

/// Documents the nav tree routes a query to.
pub const NAV_MAX_DOCS: usize = 8;
pub const NAV_MAX_HITS_PER_KB: usize = 8;
/// Tree-walk router tunables.
pub const NAV_MAX_CLUSTERS: usize = 500;
pub const NAV_CHILDREN_PAGE_SIZE: usize = 1000;
pub const NAV_TREE_MAX_DEPTH: usize = 6;
pub const NAV_TREE_MAX_LEAVES: usize = 300;
/// Chunk-level content recall (fallback) tunables.
pub const NAV_RECALL_TOP_N: usize = 40;
pub const NAV_RECALL_MAX_DOCS: usize = 4;
/// Dataset document search (hybrid, no LLM).
pub const NAV_SEARCH_MAX_DOCS: usize = 12;
pub const NAV_MIN_DOC_SCORE: f64 = 0.2;
/// Documents the navigate tools read per call (upstream `_NAV_TREE_MAX_DOCS`).
pub const NAV_TREE_MAX_DOCS: usize = 8;

/// `_NAV_SELECT_SYSTEM` (`{noun}` is filled by the caller).
pub const NAV_SELECT_SYSTEM: &str = r##"You are routing a question through a dataset's navigation tree.

You are given a QUESTION and a numbered list of {noun}, each with a name and a short description.
Choose the {noun} most likely to contain information relevant to answering the question.

Rules:
1. Judge only from the names and descriptions shown.
2. Be selective — include an item only if it is plausibly relevant. Include several when several are equally plausible.
3. If none are clearly relevant, return an empty list.
4. Return the bracketed index numbers of the chosen {noun}.

Output ONLY JSON, no prose, no code fences:
{{"relevant": [<index>, ...]}}"##;

/// The injected dataset-API surface the router reads.
#[async_trait]
pub trait NavRouterHost: Send + Sync {
    /// `list_nav_clusters(kb_id, tenant_id, page=1, page_size=NAV_MAX_CLUSTERS)`.
    async fn list_nav_clusters(&self, kb_id: &str, tenant_id: &str) -> Vec<Value>;
    /// `list_nav_children(kb_id, tenant_id, name, page=1, page_size=NAV_CHILDREN_PAGE_SIZE)`.
    async fn list_nav_children(&self, kb_id: &str, tenant_id: &str, name: &str) -> Vec<Value>;
    /// `settings.retriever.retrieval(...)` doc aggregates for the recall fallback.
    async fn content_recall(&self, query: &str, doc_scope: &[String]) -> Vec<Value>;
    /// `search_dataset_layers(..., "navigation_tree", top_k=NAV_SEARCH_MAX_DOCS)` items.
    async fn search_layers(
        &self,
        kb_id: &str,
        tenant_id: &str,
        query: &str,
        doc_scope: &[String],
    ) -> Vec<Value>;
}

/// `_ask_nav_select`: ask the chat model which of `items` are relevant.
/// Rendered as a numbered list; the model returns bracketed indices.
pub async fn ask_nav_select(
    chat: &dyn HarnessChat,
    query: &str,
    items: &[Value],
    noun: &str,
    max_items: usize,
) -> Vec<Value> {
    if items.is_empty() {
        return Vec::new();
    }
    let capped: Vec<&Value> = items.iter().take(max_items).collect();
    let mut lines: Vec<String> = Vec::new();
    for (index, item) in capped.iter().enumerate() {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let name = if name.is_empty() {
            format!("item-{index}")
        } else {
            name
        };
        let desc = item
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .replace('\n', " ");
        let extra = item
            .get("doc_count")
            .filter(|value| !value.is_null())
            .map(|value| format!(" [{} docs]", value))
            .unwrap_or_default();
        let tags = item
            .get("keywords")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .take(6)
                    .map(|value| match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let mut head = if tags.trim().is_empty() {
            String::new()
        } else {
            format!(" [tags: {}]", tags.trim())
        };
        let entities = item
            .get("entities")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .take(6)
                    .map(|value| match value {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        if !entities.trim().is_empty() {
            head.push_str(&format!(" [entities: {}]", entities.trim()));
        }
        let desc_capped: String = desc.chars().take(300).collect();
        lines.push(format!("[{index}] {name}{extra}{head}: {desc_capped}"));
    }
    let system = NAV_SELECT_SYSTEM.replace("{noun}", noun);
    let mut capitalized = noun.chars();
    let noun_head = match capitalized.next() {
        None => String::new(),
        Some(first) => {
            let mut out: String = first.to_uppercase().collect();
            out.extend(capitalized.flat_map(|ch| ch.to_lowercase()));
            out
        }
    };
    let user = format!(
        "Question:\n{query}\n\n{noun_head} (numbered):\n{}\n\nOutput JSON:",
        lines.join("\n")
    );
    let (_, messages) = crate::harness::message_fit_in(
        crate::harness::form_message(&system, &user),
        chat.max_length(),
    );
    let system_text = messages
        .first()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or(&system);
    let history = messages[1..].to_vec();
    let raw = chat
        .chat(
            system_text,
            &history,
            &serde_json::json!({"temperature": 0.2}),
        )
        .await
        .unwrap_or_default();
    let think = regex::Regex::new("(?s)^.*</think>").unwrap();
    let fences = regex::Regex::new(r"```(?:json)?\s*|\s*```").unwrap();
    let cleaned = fences
        .replace_all(&think.replace(&raw, ""), "")
        .trim()
        .to_string();
    let verdict: Value = serde_json::from_str(&cleaned).unwrap_or_else(|_| json!({}));
    let Some(raw_list) = verdict.get("relevant").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for entry in raw_list {
        let Some(index) = entry.as_i64().or_else(|| {
            entry
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        }) else {
            continue;
        };
        if index >= 0 && (index as usize) < capped.len() && seen.insert(index) {
            out.push(capped[index as usize].clone());
        }
    }
    out
}

/// `_collect_nav_leaves`: BFS from the selected clusters down to doc leaves.
pub async fn collect_nav_leaves(
    host: &dyn NavRouterHost,
    clusters: &[Value],
    doc_scope: Option<&[String]>,
) -> Vec<Value> {
    let mut leaves: Vec<Value> = Vec::new();
    let mut seen_docs: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut seen_nodes: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    let allowed: std::collections::HashSet<String> =
        doc_scope.unwrap_or(&[]).iter().cloned().collect();
    let mut frontier: std::collections::VecDeque<(String, String, String, usize)> =
        std::collections::VecDeque::new();
    for cluster in clusters {
        let Some(name) = cluster.get("name").and_then(Value::as_str) else {
            continue;
        };
        let kb_id = cluster.get("kb_id").and_then(Value::as_str).unwrap_or("");
        let tenant = cluster
            .get("tenant_id")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !name.is_empty() {
            frontier.push_back((kb_id.to_string(), tenant.to_string(), name.to_string(), 0));
        }
    }
    while !frontier.is_empty() && leaves.len() < NAV_TREE_MAX_LEAVES {
        let (kb_id, tenant, name, depth) = frontier.pop_front().unwrap();
        if !seen_nodes.insert((kb_id.clone(), name.clone())) {
            continue;
        }
        let items = host.list_nav_children(&kb_id, &tenant, &name).await;
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("doc") => {
                    let did = item
                        .get("doc_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !did.is_empty()
                        && (allowed.is_empty() || allowed.contains(&did))
                        && seen_docs.insert(did.clone())
                    {
                        let mut leaf = item.clone();
                        if let Some(object) = leaf.as_object_mut() {
                            object.insert("kb_id".to_string(), json!(kb_id));
                            object.insert("tenant_id".to_string(), json!(tenant));
                        }
                        leaves.push(leaf);
                        if leaves.len() >= NAV_TREE_MAX_LEAVES {
                            break;
                        }
                    }
                }
                Some("cluster") => {
                    let child = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !child.is_empty() && depth + 1 < NAV_TREE_MAX_DEPTH {
                        frontier.push_back((kb_id.clone(), tenant.clone(), child, depth + 1));
                    }
                }
                _ => {}
            }
        }
    }
    leaves
}

/// `_nav_cluster_names`.
pub fn nav_cluster_names(clusters: &[Value]) -> String {
    let names: Vec<String> = clusters
        .iter()
        .filter_map(|cluster| {
            cluster
                .get("name")
                .and_then(Value::as_str)
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
        })
        .collect();
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

/// `_content_recall_docs`: recall by chunk content, aggregated to docs.
pub async fn content_recall_docs(
    host: &dyn NavRouterHost,
    query: &str,
    doc_scope: &[String],
) -> Vec<String> {
    if query.is_empty() {
        return Vec::new();
    }
    let aggs = host.content_recall(query, doc_scope).await;
    let mut doc_ids: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for agg in aggs {
        let did = agg
            .get("doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !did.is_empty() && seen.insert(did.clone()) {
            doc_ids.push(did);
        }
    }
    doc_ids
}

/// `dataset_navigation_by_tree`: walk the dataset nav tree with the chat model.
pub async fn dataset_navigation_by_tree(
    host: &dyn NavRouterHost,
    chat: &dyn HarnessChat,
    kbs: &[(String, String)],
    topic: &str,
    keywords: &str,
    doc_scope: Option<Vec<String>>,
) -> Vec<String> {
    let query = format!("{} {}", topic.trim(), keywords.trim())
        .trim()
        .to_string();
    if query.is_empty() {
        return Vec::new();
    }
    let doc_scope = doc_scope.unwrap_or_default();

    // 1. List every top-level cluster across the bound KBs.
    let mut clusters: Vec<Value> = Vec::new();
    for (kb_id, tenant_id) in kbs {
        for item in host.list_nav_clusters(kb_id, tenant_id).await {
            if item.get("type").and_then(Value::as_str) == Some("cluster")
                && item
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| !name.trim().is_empty())
                    .unwrap_or(false)
            {
                let mut cluster = item.clone();
                if let Some(object) = cluster.as_object_mut() {
                    object.insert("kb_id".to_string(), json!(kb_id));
                    object.insert("tenant_id".to_string(), json!(tenant_id));
                }
                clusters.push(cluster);
            }
        }
    }
    if clusters.is_empty() {
        return content_recall_docs(host, &query, &doc_scope)
            .await
            .into_iter()
            .take(NAV_MAX_DOCS)
            .collect();
    }

    // 2. Ask the model which clusters are relevant.
    let selected_clusters =
        ask_nav_select(chat, &query, &clusters, "clusters", NAV_MAX_CLUSTERS).await;
    if selected_clusters.is_empty() {
        return content_recall_docs(host, &query, &doc_scope)
            .await
            .into_iter()
            .take(NAV_MAX_DOCS)
            .collect();
    }

    // 3. Descend the selected clusters to their document leaves.
    let leaves = collect_nav_leaves(host, &selected_clusters, Some(&doc_scope)).await;
    if leaves.is_empty() {
        return content_recall_docs(host, &query, &doc_scope)
            .await
            .into_iter()
            .take(NAV_MAX_DOCS)
            .collect();
    }

    // 4. Ask the model which documents to look into.
    let selected_docs =
        ask_nav_select(chat, &query, &leaves, "documents", NAV_TREE_MAX_LEAVES).await;
    if selected_docs.is_empty() {
        return content_recall_docs(host, &query, &doc_scope)
            .await
            .into_iter()
            .take(NAV_MAX_DOCS)
            .collect();
    }

    let mut routed: Vec<String> = Vec::new();
    let mut seen_docs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for doc in &selected_docs {
        let did = doc
            .get("doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if !did.is_empty() && seen_docs.insert(did.clone()) {
            routed.push(did);
        }
    }

    // 5. Content-recall fallback on top of the tree route.
    if routed.len() < NAV_MAX_DOCS {
        let fallback = content_recall_docs(host, &query, &doc_scope).await;
        let mut added: Vec<String> = Vec::new();
        for did in fallback {
            if !seen_docs.contains(&did) {
                seen_docs.insert(did.clone());
                added.push(did);
            }
        }
        routed.extend(added.into_iter().take(NAV_RECALL_MAX_DOCS));
    }
    routed.truncate(NAV_MAX_DOCS);
    routed
}

/// `_nav_search_titled`: hybrid nav-tree sweep returning `(doc_id, summary)`.
pub async fn nav_search_titled(
    host: &dyn NavRouterHost,
    kbs: &[(String, String)],
    topic: &str,
    keywords: &str,
    doc_scope: Option<Vec<String>>,
) -> Vec<(String, String)> {
    let query = format!("{} {}", topic.trim(), keywords.trim())
        .trim()
        .to_string();
    if query.is_empty() {
        return Vec::new();
    }
    let allowed: Vec<String> = doc_scope.unwrap_or_default();
    let mut order: Vec<String> = Vec::new();
    let mut candidates: std::collections::HashMap<String, (f64, String)> =
        std::collections::HashMap::new();
    for (kb_id, tenant_id) in kbs {
        for item in host.search_layers(kb_id, tenant_id, &query, &allowed).await {
            let score = item.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            if score < NAV_MIN_DOC_SCORE {
                continue;
            }
            let did = item
                .get("doc_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if did.is_empty() {
                continue;
            }
            let nav = item.get("_nav").cloned().unwrap_or_else(|| json!({}));
            let summary = nav
                .get("description")
                .and_then(Value::as_str)
                .or_else(|| nav.get("name").and_then(Value::as_str))
                .unwrap_or("")
                .trim()
                .to_string();
            match candidates.get(&did) {
                Some((previous, _)) if *previous >= score => {}
                _ => {
                    if !candidates.contains_key(&did) {
                        order.push(did.clone());
                    }
                    candidates.insert(did, (score, summary));
                }
            }
        }
    }
    let mut rows: Vec<(String, (f64, String))> = candidates.into_iter().collect();
    rows.sort_by(|left, right| {
        right
            .1
            .0
            .partial_cmp(&left.1.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    rows.truncate(NAV_SEARCH_MAX_DOCS);
    let _ = order;
    rows.into_iter()
        .map(|(did, (_, summary))| (did, summary))
        .collect()
}

/// `dataset_navigation_search`: routed doc_ids from the hybrid nav sweep.
pub async fn dataset_navigation_search(
    host: &dyn NavRouterHost,
    kbs: &[(String, String)],
    topic: &str,
    keywords: &str,
    doc_scope: Option<Vec<String>>,
) -> Vec<String> {
    nav_search_titled(host, kbs, topic, keywords, doc_scope)
        .await
        .into_iter()
        .map(|(did, _)| did)
        .collect()
}

// ── Compiled-structure navigation (slice 2b) ────────────────────────────────

/// `_structure_kinds_for`: map a `navigate_structure` kind to compiled kinds.
pub fn structure_kinds_for(kind: &str) -> Vec<String> {
    let normalized = kind.trim().to_lowercase();
    match normalized.as_str() {
        "mindmap" | "mind_map" | "concept" => {
            MINDMAP_KINDS.iter().map(|k| (*k).to_string()).collect()
        }
        "graph" | "kg" | "entity" | "ontology" => ["graph", "ontology", "entity", "raptor"]
            .iter()
            .map(|k| (*k).to_string())
            .collect(),
        _ => CATALOG_KINDS.iter().map(|k| (*k).to_string()).collect(),
    }
}

/// Drill-down caps (zero-LLM path).
pub const STRUCT_MAX_DEPTH: usize = 3;
pub const STRUCT_BRANCH_K: usize = 2;
pub const STRUCT_RELEVANCE_MIN: usize = 1;
pub const STRUCT_VEC_BEAM_RATIO: f64 = 0.5;
pub const STRUCT_MAX_NODES: usize = 10;
pub const STRUCT_MAX_CHUNKS: usize = 4;
pub const STRUCT_DESC_SNIPPET: usize = 180;
pub const STRUCT_RECALL_TOP_N: usize = 24;
pub const STRUCT_MAX_CHUNK_HITS: usize = 8;
/// Whole-TOC LLM selection — OFF by default (one chat call per document).
pub const STRUCT_TOC_LLM_SELECT: bool = false;
pub const STRUCT_TOC_MAX_NODES: usize = 120;
pub const STRUCT_TOC_DESC_SNIPPET: usize = 120;
pub const STRUCT_TOC_MAX_DEPTH: usize = 6;
pub const STRUCT_RELATED_SNIPPET_CHARS: usize = 300;
pub const STRUCT_RELATED_MAX_PER_DOC: usize = 4;

/// The injected structure store + embedding surface.
#[async_trait]
pub trait NavStructureHost: Send + Sync {
    /// `_embed_query` — `None` when no embedding model is available.
    async fn embed_query(&self, query: &str) -> Option<Vec<f64>>;
    /// `_load_entities_with_vectors` (entities carry `_vec`).
    async fn load_entities_with_vectors(
        &self,
        doc_id: &str,
        kinds: &[String],
        vec_field: &str,
    ) -> Vec<Value>;
    /// `_load_compiled_structure` → `(entities, relations)`.
    async fn load_compiled_structure(
        &self,
        doc_id: &str,
        kinds: &[String],
    ) -> (Vec<Value>, Vec<Value>);
    /// `_recall_chunk_ids_in_doc` → `[(chunk_id, score)]`.
    async fn recall_chunk_ids_in_doc(
        &self,
        query: &str,
        doc_id: &str,
        top_n: usize,
    ) -> Vec<(String, f64)>;
    /// `_load_chunks_for_ids` (across owning documents).
    async fn load_chunks_for_ids(&self, chunk_ids: &[String]) -> Vec<Value>;
}

/// Parent/child maps + roots for a compiled TOC (`_build_toc_tree`).
pub struct TocTree {
    pub by_name: std::collections::HashMap<String, Value>,
    pub order: Vec<String>,
    pub children: std::collections::HashMap<String, Vec<String>>,
    pub parents: std::collections::HashMap<String, String>,
    pub roots: Vec<String>,
}

/// `_build_toc_tree`: tree relations are interpreted as `from` = parent.
pub fn build_toc_tree(entities: &[Value], relations: &[Value]) -> TocTree {
    let mut by_name: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for entity in entities {
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() || by_name.contains_key(&name) {
            continue;
        }
        order.push(name.clone());
        by_name.insert(name, entity.clone());
    }
    let mut children: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut parents: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for relation in relations {
        let parent = relation
            .get("from")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let child = relation
            .get("to")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if parent.is_empty() || child.is_empty() || parent == child {
            continue;
        }
        let entry = children.entry(parent.clone()).or_default();
        if !entry.contains(&child) {
            entry.push(child.clone());
        }
        parents.insert(child, parent);
    }
    let mut roots: Vec<String> = order
        .iter()
        .filter(|name| !parents.contains_key(*name))
        .cloned()
        .collect();
    if roots.is_empty() {
        roots = order
            .iter()
            .filter(|name| !children.contains_key(*name))
            .cloned()
            .collect();
        if roots.is_empty() {
            roots = order.clone();
        }
    }
    TocTree {
        by_name,
        order,
        children,
        parents,
        roots,
    }
}

/// `_node_relevance`: keyword-overlap relevance of a TOC node.
pub fn node_relevance(query_terms: &[String], entity: &Value) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    let text = format!(
        "{} {}",
        entity.get("name").and_then(Value::as_str).unwrap_or(""),
        entity
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
    )
    .to_lowercase();
    query_terms
        .iter()
        .filter(|term| text.contains(term.as_str()))
        .count()
}

/// `_collect_chunk_ids`: union of source_chunk_ids across nodes, bounded.
pub fn collect_chunk_ids(nodes: &[Value], cap: usize) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for node in nodes {
        let Some(ids) = node.get("source_chunk_ids").and_then(Value::as_array) else {
            continue;
        };
        for id in ids {
            let Some(id) = id.as_str() else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            if !seen.iter().any(|existing| existing == id) {
                seen.push(id.to_string());
            }
            if seen.len() >= cap {
                return seen;
            }
        }
    }
    seen
}

/// `_cosine` (zero vectors score 0).
pub fn cosine(a: &[f64], b: &[f64]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f64 = a.iter().map(|x| x * x).sum::<f64>().sqrt();
    let nb: f64 = b.iter().map(|x| x * x).sum::<f64>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

fn value_vec(value: Option<&Value>) -> Option<Vec<f64>> {
    value.and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_f64())
            .collect::<Vec<f64>>()
    })
}

/// `_vecs_equal` (prefix compare on the first 16 entries, like upstream).
pub fn vecs_equal(a: &[f64], b: &[f64]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .take(16)
        .zip(b.iter().take(16))
        .all(|(x, y)| x == y)
}

/// `_has_distinct_node_vectors`: per-node embeddings rather than one shared one.
pub fn has_distinct_node_vectors(entities: &[Value]) -> bool {
    let mut first: Option<Vec<f64>> = None;
    for entity in entities {
        let Some(vector) = value_vec(entity.get("_vec")) else {
            continue;
        };
        match &first {
            None => first = Some(vector),
            Some(existing) => {
                if !vecs_equal(existing, &vector) {
                    return true;
                }
            }
        }
    }
    false
}

/// `_node_score`: cosine when the node and query carry vectors, else keyword overlap.
pub fn node_score(qvec: Option<&Vec<f64>>, query_terms: &[String], entity: &Value) -> f64 {
    if let Some(qvec) = qvec {
        if let Some(vector) = value_vec(entity.get("_vec")) {
            return cosine(qvec, &vector);
        }
    }
    node_relevance(query_terms, entity) as f64
}

/// `_nodes_covering_chunks`.
pub fn nodes_covering_chunks(
    entities: &[Value],
    chunk_ids: &std::collections::HashSet<String>,
) -> Vec<String> {
    if chunk_ids.is_empty() {
        return Vec::new();
    }
    let mut names: Vec<String> = Vec::new();
    for entity in entities {
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let ids: std::collections::HashSet<String> = entity
            .get("source_chunk_ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if !ids.is_disjoint(chunk_ids) {
            names.push(name);
        }
    }
    names
}

/// `_drill_kept_nodes`: vector-beam drill-down over the TOC hierarchy.
pub fn drill_kept_nodes(
    query_terms: &[String],
    qvec: Option<&Vec<f64>>,
    entities: &[Value],
    relations: &[Value],
) -> (
    Vec<Value>,
    std::collections::HashMap<String, String>,
    Vec<String>,
    f64,
) {
    let tree = build_toc_tree(entities, relations);
    let mut kept_order: Vec<String> = Vec::new();
    let mut kept: std::collections::HashSet<String> = std::collections::HashSet::new();
    if tree.roots.is_empty() {
        return (Vec::new(), tree.parents, kept_order, 0.0);
    }
    let mut frontier: Vec<String> = tree.roots.clone();
    let mut depth = 0usize;
    let mut best_overall = 0.0f64;
    while !frontier.is_empty() && depth <= STRUCT_MAX_DEPTH {
        let mut scored: Vec<(f64, String)> = frontier
            .iter()
            .filter_map(|name| {
                tree.by_name
                    .get(name)
                    .map(|entity| (node_score(qvec, query_terms, entity), name.clone()))
            })
            .collect();
        if scored.is_empty() {
            break;
        }
        scored.sort_by(|left, right| {
            right
                .0
                .partial_cmp(&left.0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let best = scored[0].0;
        if best > best_overall {
            best_overall = best;
        }
        if qvec.is_none() && best < STRUCT_RELEVANCE_MIN as f64 {
            break;
        }
        let top: Vec<(f64, String)> = if qvec.is_some() {
            scored
                .iter()
                .filter(|(score, _)| *score >= best * STRUCT_VEC_BEAM_RATIO)
                .take(STRUCT_BRANCH_K)
                .cloned()
                .collect()
        } else {
            scored
                .iter()
                .filter(|(score, _)| *score >= STRUCT_RELEVANCE_MIN as f64)
                .take(STRUCT_BRANCH_K)
                .cloned()
                .collect()
        };
        if top.is_empty() {
            break;
        }
        let mut new_frontier: Vec<String> = Vec::new();
        for (_, name) in top {
            if kept.insert(name.clone()) {
                kept_order.push(name.clone());
            }
            new_frontier.extend(tree.children.get(&name).cloned().unwrap_or_default());
        }
        frontier = new_frontier;
        depth += 1;
    }
    // Ancestor path so the outline shows the TOC chain.
    for name in kept_order.clone() {
        let mut cur = tree.parents.get(&name).cloned();
        let mut guard = 0usize;
        while let Some(parent) = cur {
            if kept.contains(&parent) || guard >= STRUCT_MAX_DEPTH {
                break;
            }
            kept.insert(parent.clone());
            kept_order.push(parent.clone());
            cur = tree.parents.get(&parent).cloned();
            guard += 1;
        }
    }
    let kept_nodes: Vec<Value> = kept_order
        .iter()
        .filter_map(|name| tree.by_name.get(name).cloned())
        .collect();
    (kept_nodes, tree.parents, kept_order, best_overall)
}

/// `_chunk_ptrs`: comma-join up to 8 deduped source_chunk_ids.
pub fn chunk_ptrs(item: &Value) -> String {
    let mut seen: Vec<String> = Vec::new();
    if let Some(ids) = item.get("source_chunk_ids").and_then(Value::as_array) {
        for id in ids {
            let Some(id) = id.as_str() else {
                continue;
            };
            if id.is_empty() {
                continue;
            }
            if !seen.iter().any(|existing| existing == id) {
                seen.push(id.to_string());
            }
            if seen.len() >= 8 {
                break;
            }
        }
    }
    seen.join(",")
}

/// `_render_outline`: compact flat outline (fallback when no query terms).
pub fn render_outline(entities: &[Value], relations: &[Value]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for entity in entities.iter().take(40) {
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let entity_type = entity
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .trim()
            .to_string();
        let description = entity
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        let chunks = chunk_ptrs(entity);
        let mut line = format!("- {name} ({entity_type})");
        if !description.is_empty() {
            line.push_str(&format!(
                ": {}",
                crate::harness::chunk_utils::snippet(description, STRUCT_DESC_SNIPPET)
            ));
        }
        if !chunks.is_empty() {
            line.push_str(&format!(" [chunks: {chunks}]"));
        }
        lines.push(line);
    }
    for relation in relations.iter().take(40) {
        let from = relation
            .get("from")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let to = relation
            .get("to")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if from.is_empty() || to.is_empty() {
            continue;
        }
        let relation_type = relation
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("related_to");
        let chunks = chunk_ptrs(relation);
        let mut line = format!("- {from} -[{relation_type}]-> {to}");
        if !chunks.is_empty() {
            line.push_str(&format!(" [chunks: {chunks}]"));
        }
        lines.push(line);
    }
    lines.join("\n")
}

/// `_outline_stats` for the flat fallback.
pub fn outline_stats(entities: &[Value]) -> Value {
    let mut ptrs = 0usize;
    for entity in entities.iter().take(STRUCT_MAX_NODES) {
        ptrs += collect_chunk_ids(std::slice::from_ref(entity), 32).len();
    }
    json!({
        "nodes": entities.len().min(STRUCT_MAX_NODES),
        "chunk_ptrs": ptrs,
        "top_score": 0.0,
    })
}

fn query_terms(query: &str) -> Vec<String> {
    regex::Regex::new(r"[A-Za-z0-9_]{2,}")
        .unwrap()
        .find_iter(&query.to_lowercase())
        .map(|found| found.as_str().to_string())
        .collect()
}

/// `_render_toc_drilldown`: query-focused outline of one document's structure.
#[allow(clippy::too_many_arguments)]
pub async fn render_toc_drilldown(
    host: &dyn NavStructureHost,
    query: &str,
    qvec: Option<&Vec<f64>>,
    entities: &[Value],
    relations: &[Value],
    chunk_hits: Option<&[(String, f64)]>,
    selected: Option<&[String]>,
) -> (String, Value, std::collections::HashMap<String, String>) {
    let terms = query_terms(query);
    let has_hits = chunk_hits.map(|hits| !hits.is_empty()).unwrap_or(false);
    let has_selected = selected.map(|names| !names.is_empty()).unwrap_or(false);
    if !has_hits && !has_selected && terms.is_empty() && qvec.is_none() {
        let fallback: Vec<Value> = entities.iter().take(STRUCT_MAX_NODES).cloned().collect();
        let relations_capped: Vec<Value> =
            relations.iter().take(STRUCT_MAX_NODES).cloned().collect();
        return (
            render_outline(&fallback, &relations_capped),
            outline_stats(entities),
            std::collections::HashMap::new(),
        );
    }
    let tree = build_toc_tree(entities, relations);
    let mut kept_names_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut kept_order: Vec<String> = Vec::new();
    let mut selected_ids: Vec<String> = Vec::new();
    let parents = tree.parents.clone();
    let mut best = 0.0f64;
    let selector: &str;
    if let Some(selected) = selected.filter(|names| !names.is_empty()) {
        for name in selected {
            if tree.by_name.contains_key(name) {
                if kept_names_set.insert(name.clone()) {
                    kept_order.push(name.clone());
                }
            }
        }
        // Ancestors.
        for name in kept_order.clone() {
            let mut cur = parents.get(&name).cloned();
            let mut guard = 0usize;
            while let Some(parent) = cur {
                if kept_names_set.contains(&parent) || guard >= STRUCT_MAX_DEPTH {
                    break;
                }
                kept_names_set.insert(parent.clone());
                kept_order.push(parent.clone());
                cur = parents.get(&parent).cloned();
                guard += 1;
            }
        }
        selector = "llm_toc";
    } else if let Some(hits) = chunk_hits.filter(|hits| !hits.is_empty()) {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (cid, _score) in hits {
            if !cid.is_empty() && seen.insert(cid.clone()) {
                selected_ids.push(cid.clone());
            }
        }
        let wanted: std::collections::HashSet<String> = selected_ids.iter().cloned().collect();
        let covering = nodes_covering_chunks(entities, &wanted);
        for name in covering {
            if tree.by_name.contains_key(&name) && kept_names_set.insert(name.clone()) {
                kept_order.push(name);
            }
        }
        for name in kept_order.clone() {
            let mut cur = parents.get(&name).cloned();
            let mut guard = 0usize;
            while let Some(parent) = cur {
                if kept_names_set.contains(&parent) || guard >= STRUCT_MAX_DEPTH {
                    break;
                }
                kept_names_set.insert(parent.clone());
                kept_order.push(parent.clone());
                cur = parents.get(&parent).cloned();
                guard += 1;
            }
        }
        best = hits.iter().map(|(_, score)| *score).fold(0.0f64, f64::max);
        selector = "chunk_retrieval";
    } else {
        let (kept_nodes, _parents, order, top) =
            drill_kept_nodes(&terms, qvec, entities, relations);
        let _ = kept_nodes;
        kept_order = order.clone();
        for name in &order {
            kept_names_set.insert(name.clone());
        }
        best = top;
        selector = "beam";
    }
    let kept_nodes: Vec<Value> = kept_order
        .iter()
        .filter_map(|name| tree.by_name.get(name).cloned())
        .collect();
    if kept_nodes.is_empty() && selected_ids.is_empty() {
        let fallback: Vec<Value> = entities.iter().take(STRUCT_MAX_NODES).cloned().collect();
        let relations_capped: Vec<Value> =
            relations.iter().take(STRUCT_MAX_NODES).cloned().collect();
        return (
            render_outline(&fallback, &relations_capped),
            outline_stats(entities),
            std::collections::HashMap::new(),
        );
    }
    // Depth by counting kept ancestors.
    let mut depth_of: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for name in &kept_order {
        let mut d = 0usize;
        let mut cur = parents.get(name);
        while let Some(parent) = cur {
            if !kept_names_set.contains(parent) {
                break;
            }
            d += 1;
            cur = parents.get(parent);
        }
        depth_of.insert(name.clone(), d);
    }
    // root -> ... -> node path per drilled node, mapped onto its chunks.
    let mut chunk_paths: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for entity in &kept_nodes {
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() {
            continue;
        }
        let mut segments: Vec<String> = vec![name.clone()];
        let mut cur = parents.get(&name).cloned();
        let mut guard = 0usize;
        while let Some(parent) = cur {
            if guard >= STRUCT_MAX_DEPTH {
                break;
            }
            segments.push(parent.clone());
            cur = parents.get(&parent).cloned();
            guard += 1;
        }
        segments.reverse();
        let path = segments.join(" -> ");
        if let Some(ids) = entity.get("source_chunk_ids").and_then(Value::as_array) {
            for id in ids {
                if let Some(id) = id.as_str() {
                    if !id.is_empty() {
                        chunk_paths
                            .entry(id.to_string())
                            .or_insert_with(|| path.clone());
                    }
                }
            }
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for entity in kept_nodes.iter().take(STRUCT_MAX_NODES) {
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let entity_type = entity
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("other")
            .trim()
            .to_string();
        let description = entity
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let chunks = chunk_ptrs(entity);
        let indent = "  ".repeat(*depth_of.get(&name).unwrap_or(&0));
        let mut line = format!("{indent}- {name} ({entity_type})");
        if !description.is_empty() {
            line.push_str(&format!(
                ": {}",
                crate::harness::chunk_utils::snippet(&description, STRUCT_DESC_SNIPPET)
            ));
        }
        if !chunks.is_empty() {
            line.push_str(&format!(" [chunks: {chunks}]"));
        }
        lines.push(line);
    }
    let wanted: Vec<String> = if !selected_ids.is_empty() {
        selected_ids.clone()
    } else {
        collect_chunk_ids(&kept_nodes, 32)
    };
    if !wanted.is_empty() {
        let chunks = host.load_chunks_for_ids(&wanted).await;
        if !chunks.is_empty() {
            let ranked: Vec<Value> = if !selected_ids.is_empty() {
                let mut ordered = chunks;
                ordered.sort_by_key(|chunk| {
                    let cid = crate::harness::chunk_utils::chunk_id(chunk);
                    selected_ids
                        .iter()
                        .position(|wanted| wanted == &cid)
                        .unwrap_or(usize::MAX)
                });
                ordered
            } else {
                rank_chunks_by_terms(&chunks, &[query.to_string()])
            };
            let limit = if !selected_ids.is_empty() {
                STRUCT_MAX_CHUNK_HITS
            } else {
                STRUCT_MAX_CHUNKS
            };
            for chunk in ranked.iter().take(limit) {
                let cid = crate::harness::chunk_utils::chunk_id(chunk);
                let text = crate::harness::chunk_utils::chunk_text(chunk);
                lines.push(format!(
                    "- [chunk {cid}]: {}",
                    crate::harness::chunk_utils::snippet(&text, 300)
                ));
            }
        }
    }
    let stats = json!({
        "nodes": kept_nodes.len(),
        "chunk_ptrs": wanted.len(),
        "top_score": (best * 10000.0).round() / 10000.0,
        "selector": selector,
    });
    (lines.join("\n"), stats, chunk_paths)
}

/// `_build_toc_items`: indented whole-TOC serialization for LLM selection.
pub fn build_toc_items(entities: &[Value], relations: &[Value]) -> Vec<Value> {
    let tree = build_toc_tree(entities, relations);
    if tree.by_name.is_empty() {
        return Vec::new();
    }
    let mut items: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    fn walk(
        name: &str,
        depth: usize,
        tree: &TocTree,
        seen: &mut std::collections::HashSet<String>,
        items: &mut Vec<Value>,
    ) {
        if seen.contains(name) || depth > STRUCT_TOC_MAX_DEPTH {
            return;
        }
        seen.insert(name.to_string());
        let Some(entity) = tree.by_name.get(name) else {
            return;
        };
        let desc = entity
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .replace('\n', " ");
        items.push(json!({
            "_node": name,
            "name": format!("{}{}", "  ".repeat(depth), name),
            "description": crate::harness::chunk_utils::snippet(&desc, STRUCT_TOC_DESC_SNIPPET),
        }));
        if let Some(children) = tree.children.get(name) {
            for child in children {
                walk(child, depth + 1, tree, seen, items);
            }
        }
    }
    for root in &tree.roots {
        walk(root, 0, &tree, &mut seen, &mut items);
    }
    for name in &tree.order {
        if !seen.contains(name) {
            walk(name, 0, &tree, &mut seen, &mut items);
        }
    }
    items
}

/// `_select_toc_nodes`: one-shot model selection over the whole TOC.
pub async fn select_toc_nodes(
    chat: &dyn HarnessChat,
    query: &str,
    entities: &[Value],
    relations: &[Value],
) -> Vec<String> {
    if entities.is_empty() {
        return Vec::new();
    }
    let items = build_toc_items(entities, relations);
    if items.is_empty() {
        return Vec::new();
    }
    let picked = ask_nav_select(chat, query, &items, "sections", STRUCT_TOC_MAX_NODES).await;
    picked
        .iter()
        .filter_map(|item| {
            item.get("_node")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// `_read_structures`: per-document structure read + selection + drill render.
pub async fn read_structures(
    host: &dyn NavStructureHost,
    chat: &dyn HarnessChat,
    query: &str,
    doc_ids: &[String],
    kinds: &[String],
) -> Vec<Value> {
    let qvec = host.embed_query(query).await;
    let vec_field = qvec
        .as_ref()
        .map(|vector| format!("q_{}_vec", vector.len()))
        .unwrap_or_default();
    let mut out: Vec<Value> = Vec::new();
    for doc_id in doc_ids {
        let mut entities: Vec<Value> = Vec::new();
        let mut relations: Vec<Value> = Vec::new();
        if !vec_field.is_empty() {
            entities = host
                .load_entities_with_vectors(doc_id, kinds, &vec_field)
                .await;
        }
        let structure = host.load_compiled_structure(doc_id, kinds).await;
        if entities.is_empty() {
            entities = structure.0;
        }
        relations = structure.1;
        let mut hits: Vec<(String, f64)> = Vec::new();
        let mut selected: Vec<String> = Vec::new();
        if !entities.is_empty() && has_distinct_node_vectors(&entities) {
            if STRUCT_TOC_LLM_SELECT {
                selected = select_toc_nodes(chat, query, &entities, &relations).await;
            }
        } else {
            hits = host
                .recall_chunk_ids_in_doc(query, doc_id, STRUCT_RECALL_TOP_N)
                .await;
        }
        let (outline, stats, chunk_paths) = render_toc_drilldown(
            host,
            query,
            qvec.as_ref(),
            &entities,
            &relations,
            if hits.is_empty() { None } else { Some(&hits) },
            if selected.is_empty() {
                None
            } else {
                Some(&selected)
            },
        )
        .await;
        out.push(json!({
            "doc_id": doc_id,
            "title": "",
            "entities": entities,
            "relations": relations,
            "outline": outline,
            "stats": stats,
            "chunk_paths": chunk_paths,
        }));
    }
    out
}

/// `_navigate_tree_impl`: route documents via the nav-tree sweep (XML result).
pub async fn navigate_tree_impl(
    router: &dyn NavRouterHost,
    kbs: &[(String, String)],
    query: &str,
    keywords: &str,
    doc_scope: Option<Vec<String>>,
) -> NavResult {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return NavResult {
            text: "<tree_navigation count=\"0\" error=\"query is required\">\n</tree_navigation>"
                .to_string(),
            empty_reason: "bad_args".to_string(),
            ..NavResult::default()
        };
    }
    let mut routed = nav_search_titled(router, kbs, trimmed, keywords, doc_scope).await;
    if routed.is_empty() {
        return NavResult {
            text: "<tree_navigation count=\"0\">\n</tree_navigation>".to_string(),
            empty_reason: "no_doc".to_string(),
            ..NavResult::default()
        };
    }
    routed.truncate(NAV_TREE_MAX_DOCS);
    let escape = |text: &str| crate::harness::chunk_utils::xml_escape(&json!(text));
    let mut parts: Vec<String> = vec![format!(
        "<tree_navigation count=\"{}\" query=\"{}\">",
        routed.len(),
        escape(trimmed)
    )];
    for (index, (doc_id, summary)) in routed.iter().enumerate() {
        if !summary.is_empty() {
            parts.push(format!(
                "  <doc rank=\"{}\" doc_id=\"{}\">",
                index + 1,
                escape(doc_id)
            ));
            parts.push(format!("    <summary>{}</summary>", escape(summary)));
            parts.push("  </doc>".to_string());
        } else {
            parts.push(format!(
                "  <doc rank=\"{}\" doc_id=\"{}\"/>",
                index + 1,
                escape(doc_id)
            ));
        }
    }
    parts.push("</tree_navigation>".to_string());
    NavResult {
        text: parts.join("\n"),
        doc_ids: routed.iter().map(|(doc_id, _)| doc_id.clone()).collect(),
        routed_docs: routed,
        ..NavResult::default()
    }
}

/// `_navigate_structure_impl`: compiled structure outline for routed documents.
#[allow(clippy::too_many_arguments)]
pub async fn navigate_structure_impl(
    host: &dyn NavStructureHost,
    router: &dyn NavRouterHost,
    chat: &dyn HarnessChat,
    kbs: &[(String, String)],
    query: &str,
    doc_id: &str,
    kind: &str,
    keywords: &str,
    doc_scope: Option<Vec<String>>,
) -> NavResult {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return NavResult {
            text: "<structure_navigation count=\"0\" error=\"query is required\">\n</structure_navigation>".to_string(),
            empty_reason: "bad_args".to_string(),
            ..NavResult::default()
        };
    }
    let kinds = structure_kinds_for(kind);
    let doc_ids: Vec<String> = if !doc_id.trim().is_empty() {
        vec![doc_id.trim().to_string()]
    } else {
        nav_search_titled(router, kbs, trimmed, keywords, doc_scope)
            .await
            .into_iter()
            .map(|(doc_id, _summary)| doc_id)
            .collect()
    };
    if doc_ids.is_empty() {
        return NavResult {
            text: "<structure_navigation count=\"0\" error=\"no document located\">\n</structure_navigation>".to_string(),
            empty_reason: "no_doc".to_string(),
            ..NavResult::default()
        };
    }
    let capped: Vec<String> = doc_ids.iter().take(NAV_TREE_MAX_DOCS).cloned().collect();
    let structures = read_structures(host, chat, trimmed, &capped, &kinds).await;
    let escape = |text: &str| crate::harness::chunk_utils::xml_escape(&json!(text));
    let mut parts: Vec<String> = vec![format!(
        "<structure_navigation count=\"{}\" query=\"{}\" kind=\"{}\">",
        structures.len(),
        escape(trimmed),
        escape(kind)
    )];
    let mut total_entities = 0usize;
    let mut total_ptrs = 0usize;
    let mut best_score = 0.0f64;
    let mut all_chunk_paths: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut structure_docs: Vec<String> = Vec::new();
    for (index, structure) in structures.iter().enumerate() {
        let entities = structure.get("entities").and_then(Value::as_array);
        let relations = structure.get("relations").and_then(Value::as_array);
        let entity_count = entities.map(|items| items.len()).unwrap_or(0);
        total_entities += entity_count;
        let stats = structure.get("stats").cloned().unwrap_or_else(|| json!({}));
        total_ptrs += stats.get("chunk_ptrs").and_then(Value::as_u64).unwrap_or(0) as usize;
        best_score = best_score.max(
            stats
                .get("top_score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
        );
        if let Some(paths) = structure.get("chunk_paths").and_then(Value::as_object) {
            for (cid, path) in paths {
                if !cid.is_empty() {
                    all_chunk_paths
                        .entry(cid.clone())
                        .or_insert_with(|| path.as_str().unwrap_or("").to_string());
                }
            }
        }
        let structure_doc = structure
            .get("doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        structure_docs.push(structure_doc.clone());
        let title = structure.get("title").and_then(Value::as_str).unwrap_or("");
        let relation_count = relations.map(|items| items.len()).unwrap_or(0);
        parts.push(format!(
            "  <doc rank=\"{}\" doc_id=\"{}\" doc_title=\"{}\" entities=\"{}\" relations=\"{}\">",
            index + 1,
            escape(&structure_doc),
            escape(title),
            entity_count,
            relation_count
        ));
        let outline = structure
            .get("outline")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !outline.is_empty() {
            parts.push(format!("    <structure>{}</structure>", escape(outline)));
        }
        parts.push("  </doc>".to_string());
    }
    parts.push("</structure_navigation>".to_string());
    NavResult {
        text: parts.join("\n"),
        doc_ids: structure_docs,
        entities: total_entities,
        chunk_ptrs: total_ptrs,
        top_score: (best_score * 10000.0).round() / 10000.0,
        chunk_paths: all_chunk_paths,
        empty_reason: if total_entities == 0 {
            "no_structure".to_string()
        } else {
            String::new()
        },
        ..NavResult::default()
    }
}

/// `_expand_related_via_structure`: other chunks related to the query behind
/// the beam-drilled entities (skipping `exclude`).
pub async fn expand_related_via_structure(
    host: &dyn NavStructureHost,
    query: &str,
    doc_ids: &[String],
    exclude: &mut std::collections::HashSet<String>,
    max_per_doc: usize,
) -> Vec<Value> {
    if doc_ids.is_empty() || query.trim().is_empty() {
        return Vec::new();
    }
    let qvec = host.embed_query(query).await;
    let vec_field = qvec
        .as_ref()
        .map(|vector| format!("q_{}_vec", vector.len()))
        .unwrap_or_default();
    let terms = query_terms(query);
    let kinds: Vec<String> = CATALOG_KINDS.iter().map(|k| (*k).to_string()).collect();
    let mut out: Vec<Value> = Vec::new();
    for doc_id in doc_ids.iter().take(NAV_TREE_MAX_DOCS) {
        let entities = if vec_field.is_empty() {
            Vec::new()
        } else {
            host.load_entities_with_vectors(doc_id, &kinds, &vec_field)
                .await
        };
        if entities.is_empty() {
            continue;
        }
        let structure = host.load_compiled_structure(doc_id, &kinds).await;
        let relations = structure.1;
        let (kept_nodes, _parents, _kept, _score) =
            drill_kept_nodes(&terms, qvec.as_ref(), &entities, &relations);
        if kept_nodes.is_empty() {
            continue;
        }
        let wanted: Vec<String> = collect_chunk_ids(&kept_nodes, 32)
            .into_iter()
            .filter(|id| !exclude.contains(id))
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let chunks = host.load_chunks_for_ids(&wanted).await;
        if chunks.is_empty() {
            continue;
        }
        let ranked = rank_chunks_by_terms(&chunks, &[query.to_string()]);
        for mut chunk in ranked.into_iter().take(max_per_doc) {
            let cid = crate::harness::chunk_utils::chunk_id(&chunk);
            if exclude.contains(&cid) {
                continue;
            }
            exclude.insert(cid);
            let text = crate::harness::chunk_utils::chunk_text(&chunk);
            if let Some(object) = chunk.as_object_mut() {
                object.insert(
                    "content_with_weight".to_string(),
                    json!(crate::harness::chunk_utils::snippet(
                        &text,
                        STRUCT_RELATED_SNIPPET_CHARS
                    )),
                );
                object.insert("related_via_structure".to_string(), json!(true));
            }
            out.push(chunk);
        }
    }
    out
}

#[cfg(test)]
mod structure_tests {
    use super::*;
    use async_trait::async_trait;

    struct MockChat {
        reply: String,
    }
    #[async_trait]
    impl HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen: &Value,
        ) -> Result<String, String> {
            Ok(self.reply.clone())
        }
        fn max_length(&self) -> usize {
            8192
        }
    }

    struct MockStructureHost {
        entities: Vec<Value>,
        relations: Vec<Value>,
        with_vectors: Vec<Value>,
        qvec: Option<Vec<f64>>,
        hits: Vec<(String, f64)>,
        chunks: Vec<Value>,
    }
    #[async_trait]
    impl NavStructureHost for MockStructureHost {
        async fn embed_query(&self, _query: &str) -> Option<Vec<f64>> {
            self.qvec.clone()
        }
        async fn load_entities_with_vectors(
            &self,
            _doc_id: &str,
            _kinds: &[String],
            _vec_field: &str,
        ) -> Vec<Value> {
            self.with_vectors.clone()
        }
        async fn load_compiled_structure(
            &self,
            _doc_id: &str,
            _kinds: &[String],
        ) -> (Vec<Value>, Vec<Value>) {
            (self.entities.clone(), self.relations.clone())
        }
        async fn recall_chunk_ids_in_doc(
            &self,
            _query: &str,
            _doc_id: &str,
            _top_n: usize,
        ) -> Vec<(String, f64)> {
            self.hits.clone()
        }
        async fn load_chunks_for_ids(&self, _chunk_ids: &[String]) -> Vec<Value> {
            self.chunks.clone()
        }
    }

    fn entity(name: &str, description: &str, chunks: &[&str]) -> Value {
        json!({
            "name": name,
            "type": "tree_node",
            "description": description,
            "source_chunk_ids": chunks,
        })
    }

    #[test]
    fn kinds_and_tree_maps() {
        assert_eq!(structure_kinds_for("mindmap"), vec!["mindmap", "mind_map"]);
        assert!(structure_kinds_for("graph").contains(&"entity".to_string()));
        assert_eq!(
            structure_kinds_for(""),
            vec!["tree", "timeline", "raptor", "page_index", "pageindex"]
        );

        let entities = vec![entity("Root", "r", &["c1"]), entity("Child", "c", &["c2"])];
        let relations = vec![json!({"from": "Root", "to": "Child", "type": "tree"})];
        let tree = build_toc_tree(&entities, &relations);
        assert_eq!(tree.roots, vec!["Root"]);
        assert_eq!(tree.parents.get("Child").map(String::as_str), Some("Root"));
        assert_eq!(
            tree.children.get("Root").cloned().unwrap_or_default(),
            vec!["Child"]
        );
    }

    #[test]
    fn drill_uses_vectors_then_keywords() {
        let entities = vec![
            entity("Alpha topic", "about alpha", &["c1"]),
            entity("Beta", "unrelated", &["c2"]),
        ];
        let qvec = vec![1.0, 0.0];
        let mut with_vectors = entities.clone();
        with_vectors[0]["_vec"] = json!([1.0, 0.0]);
        with_vectors[1]["_vec"] = json!([0.0, 1.0]);
        let (kept, _parents, order, best) = drill_kept_nodes(&[], Some(&qvec), &with_vectors, &[]);
        assert!(order.contains(&"Alpha topic".to_string()));
        assert!(!order.contains(&"Beta".to_string()));
        assert!((best - 1.0).abs() < 1e-9);
        assert_eq!(kept.len(), 1);

        // Keyword fallback without vectors.
        let terms = query_terms("alpha");
        let (_kept2, _p2, order2, best2) = drill_kept_nodes(&terms, None, &entities, &[]);
        assert!(order2.contains(&"Alpha topic".to_string()));
        assert!(best2 >= 1.0);
    }

    #[tokio::test]
    async fn drilldown_renders_paths_and_chunks() {
        let entities = vec![
            entity("Root", "root desc", &["c1"]),
            entity("Child", "child desc", &["c2"]),
        ];
        let relations = vec![json!({"from": "Root", "to": "Child", "type": "tree"})];
        let host = MockStructureHost {
            entities: entities.clone(),
            relations: relations.clone(),
            with_vectors: vec![],
            qvec: None,
            hits: vec![],
            chunks: vec![json!({"chunk_id": "c2", "content_with_weight": "child chunk text"})],
        };
        let selected = vec!["Child".to_string()];
        let (outline, stats, paths) = render_toc_drilldown(
            &host,
            "child",
            None,
            &entities,
            &relations,
            None,
            Some(&selected),
        )
        .await;
        assert!(outline.contains("- Child (tree_node): child desc"));
        assert!(outline.contains("  - Child") || outline.contains("- Child"));
        assert_eq!(stats["selector"], json!("llm_toc"));
        assert_eq!(paths.get("c2").map(String::as_str), Some("Root -> Child"));
        assert!(outline.contains("[chunk c2]"));
    }

    #[tokio::test]
    async fn navigate_impls_emit_xml_and_reasons() {
        struct Router;
        #[async_trait]
        impl NavRouterHost for Router {
            async fn list_nav_clusters(&self, _kb: &str, _tenant: &str) -> Vec<Value> {
                vec![]
            }
            async fn list_nav_children(&self, _kb: &str, _tenant: &str, _name: &str) -> Vec<Value> {
                vec![]
            }
            async fn content_recall(&self, _query: &str, _doc_scope: &[String]) -> Vec<Value> {
                vec![]
            }
            async fn search_layers(
                &self,
                _kb: &str,
                _tenant: &str,
                _query: &str,
                _doc_scope: &[String],
            ) -> Vec<Value> {
                vec![json!({"doc_id": "d1", "score": 0.5, "_nav": {"description": "doc summary"}})]
            }
        }
        let chat = MockChat {
            reply: "{}".to_string(),
        };
        let result = navigate_tree_impl(
            &Router,
            &[("kb1".to_string(), "t1".to_string())],
            "q",
            "",
            None,
        )
        .await;
        assert!(result.text.contains("<tree_navigation count=\"1\""));
        assert!(result.text.contains("<summary>doc summary</summary>"));
        assert_eq!(result.doc_ids, vec!["d1".to_string()]);

        let empty = navigate_tree_impl(
            &Router,
            &[("kb1".to_string(), "t1".to_string())],
            "  ",
            "",
            None,
        )
        .await;
        assert_eq!(empty.empty_reason, "bad_args");

        let host = MockStructureHost {
            entities: vec![entity("Root", "r", &["c1"])],
            relations: vec![],
            with_vectors: vec![],
            qvec: None,
            hits: vec![],
            chunks: vec![],
        };
        let result = navigate_structure_impl(
            &host,
            &Router,
            &chat,
            &[("kb1".to_string(), "t1".to_string())],
            "q",
            "",
            "catalog",
            "",
            None,
        )
        .await;
        assert!(result.text.contains("<structure_navigation count=\"1\""));
        assert_eq!(result.entities, 1);

        let empty_host = MockStructureHost {
            entities: vec![],
            relations: vec![],
            with_vectors: vec![],
            qvec: None,
            hits: vec![],
            chunks: vec![],
        };
        let result = navigate_structure_impl(
            &empty_host,
            &Router,
            &chat,
            &[("kb1".to_string(), "t1".to_string())],
            "q",
            "",
            "catalog",
            "",
            None,
        )
        .await;
        assert_eq!(result.empty_reason, "no_structure");
    }

    #[tokio::test]
    async fn related_expansion_skips_excluded() {
        let entities = vec![entity("Alpha", "alpha desc", &["c1", "c2"])];
        let host = MockStructureHost {
            entities: entities.clone(),
            relations: vec![],
            with_vectors: entities.clone(),
            qvec: Some(vec![1.0, 0.0]),
            hits: vec![],
            chunks: vec![
                json!({"chunk_id": "c1", "content_with_weight": "alpha one"}),
                json!({"chunk_id": "c2", "content_with_weight": "alpha two"}),
            ],
        };
        let mut exclude: std::collections::HashSet<String> =
            ["c1".to_string()].into_iter().collect();
        let out =
            expand_related_via_structure(&host, "alpha", &["d1".to_string()], &mut exclude, 4)
                .await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["chunk_id"], json!("c2"));
        assert_eq!(out[0]["related_via_structure"], json!(true));
    }
}

#[cfg(test)]
mod router_tests {
    use super::*;
    use async_trait::async_trait;

    struct MockChat {
        reply: String,
    }

    #[async_trait]
    impl HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen: &Value,
        ) -> Result<String, String> {
            Ok(self.reply.clone())
        }
        fn max_length(&self) -> usize {
            8192
        }
    }

    struct MockHost {
        clusters: Vec<Value>,
        children: std::collections::HashMap<String, Vec<Value>>,
        recall: Vec<Value>,
        layers: Vec<Value>,
    }

    #[async_trait]
    impl NavRouterHost for MockHost {
        async fn list_nav_clusters(&self, _kb: &str, _tenant: &str) -> Vec<Value> {
            self.clusters.clone()
        }
        async fn list_nav_children(&self, _kb: &str, _tenant: &str, name: &str) -> Vec<Value> {
            self.children.get(name).cloned().unwrap_or_default()
        }
        async fn content_recall(&self, _query: &str, _doc_scope: &[String]) -> Vec<Value> {
            self.recall.clone()
        }
        async fn search_layers(
            &self,
            _kb: &str,
            _tenant: &str,
            _query: &str,
            _doc_scope: &[String],
        ) -> Vec<Value> {
            self.layers.clone()
        }
    }

    #[tokio::test]
    async fn nav_select_parses_indices_and_renders_list() {
        let items = vec![
            json!({"name": "Cluster A", "description": "alpha stuff", "doc_count": 3}),
            json!({"name": "Cluster B", "description": "beta stuff"}),
        ];
        let chat = MockChat {
            reply: "{\"relevant\": [1, 0, 0, 9, \"x\"]}".to_string(),
        };
        let selected = ask_nav_select(&chat, "q", &items, "clusters", 500).await;
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0]["name"], json!("Cluster B"));
        assert_eq!(selected[1]["name"], json!("Cluster A"));

        let chat = MockChat {
            reply: "no json here".to_string(),
        };
        assert!(
            ask_nav_select(&chat, "q", &items, "clusters", 500)
                .await
                .is_empty()
        );
        assert!(
            ask_nav_select(&chat, "q", &[], "clusters", 500)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn leaves_bfs_respects_scope_and_caps() {
        let mut children = std::collections::HashMap::new();
        children.insert(
            "top".to_string(),
            vec![
                json!({"type": "doc", "doc_id": "d1"}),
                json!({"type": "cluster", "name": "sub"}),
                json!({"type": "doc", "doc_id": "d9"}),
            ],
        );
        children.insert(
            "sub".to_string(),
            vec![json!({"type": "doc", "doc_id": "d2"})],
        );
        let host = MockHost {
            clusters: vec![],
            children,
            recall: vec![],
            layers: vec![],
        };
        let scope = vec!["d1".to_string(), "d2".to_string()];
        let leaves = collect_nav_leaves(
            &host,
            &[json!({"name": "top", "kb_id": "kb1", "tenant_id": "t1"})],
            Some(&scope),
        )
        .await;
        let ids: Vec<String> = leaves
            .iter()
            .map(|leaf| leaf["doc_id"].as_str().unwrap_or("").to_string())
            .collect();
        assert_eq!(ids, vec!["d1".to_string(), "d2".to_string()]);
        assert_eq!(nav_cluster_names(&[json!({"name": "top"})]), "top");
        assert_eq!(nav_cluster_names(&[]), "none");
    }

    #[tokio::test]
    async fn tree_route_and_fallbacks() {
        let mut children = std::collections::HashMap::new();
        children.insert(
            "top".to_string(),
            vec![json!({"type": "doc", "doc_id": "d1"})],
        );
        let host = MockHost {
            clusters: vec![json!({"type": "cluster", "name": "top"})],
            children,
            recall: vec![json!({"doc_id": "d7"}), json!({"doc_id": "d1"})],
            layers: vec![],
        };
        let chat = MockChat {
            reply: "{\"relevant\": [0]}".to_string(),
        };
        let routed = dataset_navigation_by_tree(
            &host,
            &chat,
            &[("kb1".to_string(), "t1".to_string())],
            "q",
            "",
            None,
        )
        .await;
        assert_eq!(routed, vec!["d1".to_string(), "d7".to_string()]);

        // No clusters -> content-recall fallback.
        let host = MockHost {
            clusters: vec![],
            children: std::collections::HashMap::new(),
            recall: vec![json!({"doc_id": "d3"})],
            layers: vec![],
        };
        let routed = dataset_navigation_by_tree(
            &host,
            &chat,
            &[("kb1".to_string(), "t1".to_string())],
            "q",
            "",
            None,
        )
        .await;
        assert_eq!(routed, vec!["d3".to_string()]);
    }

    #[tokio::test]
    async fn nav_search_filters_scores_and_keeps_best() {
        let host = MockHost {
            clusters: vec![],
            children: std::collections::HashMap::new(),
            recall: vec![],
            layers: vec![
                json!({"doc_id": "d1", "score": 0.4, "_nav": {"description": "first summary"}}),
                json!({"doc_id": "d2", "score": 0.1, "_nav": {"name": "too low"}}),
                json!({"doc_id": "d1", "score": 0.7, "_nav": {"description": "better summary"}}),
            ],
        };
        let routed = nav_search_titled(
            &host,
            &[("kb1".to_string(), "t1".to_string())],
            "q",
            "",
            None,
        )
        .await;
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].0, "d1");
        assert_eq!(routed[0].1, "better summary");
        assert_eq!(
            dataset_navigation_search(
                &host,
                &[("kb1".to_string(), "t1".to_string())],
                "q",
                "",
                None
            )
            .await,
            vec!["d1".to_string()]
        );
    }
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
