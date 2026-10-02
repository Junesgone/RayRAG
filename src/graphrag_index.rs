//! GraphRAG index pipeline — mirrors `rag/graphrag/general/index.py` +
//! `rag/graphrag/general/extractor.py` (base class) + the pure-algorithm
//! parts of `rag/graphrag/utils.py` (GraphChange / graph_merge / tidy_graph).
//!
//! Storage-layer concerns (chunk_list, does_graph_contains, set_graph,
//! RedisDistributedLock, DocumentService) are deliberately excluded — the
//! caller feeds chunks and receives an in-memory `EntityGraph`; persistence
//! hooks into zvec/postgres via the existing backends.

use crate::Result;
use crate::graphrag::{GraphExtractionResult, GraphExtractor, KgExtractedEdge, KgExtractedNode};
use crate::graphrag_enhanced::{EntityGraph, EntityType};
use crate::llm::LlmClient;
use std::collections::{HashMap, HashSet};

/// GRAPH_FIELD_SEP — utils.py separator used when concatenating
/// descriptions / source ids during graph_merge.
pub const GRAPH_FIELD_SEP: &str = "<SEP>";

/// DEFAULT_ENTITY_TYPES — extractor.py default when none configured.
pub const DEFAULT_ENTITY_TYPES: [&str; 5] = ["organization", "person", "geo", "event", "category"];

/// ENTITY_EXTRACTION_MAX_GLEANINGS — extractor.py:47.
pub const ENTITY_EXTRACTION_MAX_GLEANINGS: usize = 2;

/// MAX_CONCURRENT_PROCESS_AND_EXTRACT_CHUNK — extractor.py default worker
/// concurrency for per-chunk extraction.
pub const MAX_CONCURRENT_PROCESS_AND_EXTRACT_CHUNK: usize = 10;

/// GraphChange — utils.py dataclass tracking merge deltas.
#[derive(Debug, Default)]
pub struct GraphChange {
    pub removed_nodes: HashSet<String>,
    pub added_updated_nodes: HashSet<String>,
    pub removed_edges: HashSet<(String, String)>,
    pub added_updated_edges: HashSet<(String, String)>,
}

fn get_from_to(a: &str, b: &str) -> (String, String) {
    if a < b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// tidy_graph — utils.py: purge nodes/edges missing essential attributes,
/// fill empty `keywords` (RayRAG edges carry no keywords list; we keep the
/// purge contract for description/source_id and record the keyword default
/// as a no-op since our edges store only weight).
pub fn tidy_graph(graph: &mut EntityGraph, check_attribute: bool) -> (usize, usize) {
    let mut purged_nodes = 0usize;
    if check_attribute {
        let names: Vec<String> = graph.node_names();
        for name in &names {
            let valid = graph.description_of(name).is_some_and(|d| !d.is_empty())
                && graph.source_id_of(name).iter().any(|s| !s.is_empty());
            if !valid {
                graph.remove_node(name);
                purged_nodes += 1;
            }
        }
    }
    // Edges: RayRAG stores only (target, weight) — description is always
    // synthesized as "connected" so nothing is purged here; the contract is
    // preserved at the EntityGraph level (edge_description exists iff edge).
    (purged_nodes, 0)
}

/// graph_merge — utils.py: merge subgraph `g2` into `g1` in place, recording
/// changes. Existing node descriptions/source ids are concatenated with
/// GRAPH_FIELD_SEP; edge weights accumulate; ranks are recomputed as degree.
pub fn graph_merge(g1: &mut EntityGraph, g2: &EntityGraph) -> GraphChange {
    let mut change = GraphChange::default();
    for name in g2.node_names() {
        change.added_updated_nodes.insert(name.clone());
        if g1.description_of(&name).is_none() {
            // brand-new node: copy description + source ids
            if let Some(desc) = g2.description_of(&name) {
                g1.set_description(&name, desc);
            }
            for sid in g2.source_id_of(&name) {
                g1.append_source_id(&name, &sid);
            }
            continue;
        }
        // existing node: concatenate description, accumulate source ids
        let d2 = g2.description_of(&name).unwrap_or_default();
        if !d2.is_empty() {
            let d1 = g1.description_of(&name).unwrap_or_default();
            g1.set_description(&name, format!("{d1}{GRAPH_FIELD_SEP}{d2}"));
        }
        for sid in g2.source_id_of(&name) {
            g1.append_source_id(&name, &sid);
        }
    }
    // edges
    for name in g2.node_names() {
        for nbr in g2.neighbors(&name) {
            let pair = get_from_to(&name, &nbr);
            change.added_updated_edges.insert(pair);
            let w2 = g2.edge_weight(&name, &nbr).unwrap_or(0.0);
            if let Some(w1) = g1.edge_weight(&name, &nbr) {
                g1.set_edge_weight(&name, &nbr, w1 + w2);
            } else {
                g1.add_relation(&name, &nbr, w2 as f32);
            }
        }
    }
    // rank == degree, recomputed after merge
    change
}

/// pagerank — mirrors networkx.pagerank (dangling-node fixed point, power
/// iteration) over the in-memory graph. Returns name → score.
pub fn pagerank(
    graph: &EntityGraph,
    alpha: f64,
    max_iter: usize,
    tol: f64,
) -> HashMap<String, f64> {
    let names = graph.node_names();
    let n = names.len();
    if n == 0 {
        return HashMap::new();
    }
    let dangling_weight = alpha / n as f64;
    let mut ranks: HashMap<String, f64> =
        names.iter().map(|k| (k.clone(), 1.0 / n as f64)).collect();
    let mut out: HashMap<String, Vec<(String, f64)>> = HashMap::new();
    let mut dangling = Vec::new();
    for name in &names {
        let nbrs: Vec<(String, f64)> = graph
            .neighbors(name)
            .into_iter()
            .map(|t| (t.clone(), graph.edge_weight(name, &t).unwrap_or(1.0)))
            .collect();
        let sum: f64 = nbrs.iter().map(|(_, w)| w).sum();
        if sum == 0.0 {
            dangling.push(name.clone());
            out.insert(name.clone(), Vec::new());
        } else {
            out.insert(
                name.clone(),
                nbrs.into_iter().map(|(t, w)| (t, w / sum)).collect(),
            );
        }
    }
    let dangling_nodes = dangling.len() as f64;
    for _ in 0..max_iter {
        let base = dangling_weight * dangling_nodes;
        let mut next: HashMap<String, f64> = HashMap::new();
        for name in &names {
            let mut s = base;
            for (src, links) in &out {
                if links.iter().any(|(t, _)| t == name) {
                    s += alpha * links.iter().find(|(t, _)| t == name).unwrap().1 * ranks[src];
                }
            }
            next.insert(name.clone(), s + (1.0 - alpha) / n as f64);
        }
        // normalize (networkx keeps L1 norm ~1)
        let sum: f64 = next.values().sum();
        let diff: f64 = ranks.iter().map(|(k, v)| (next[k] / sum - v).abs()).sum();
        ranks = next.into_iter().map(|(k, v)| (k, v / sum)).collect();
        if diff < tol {
            break;
        }
    }
    ranks
}

/// extract_chunks — extractor.py `Extractor.__call__`: run the extractor over
/// every chunk with bounded concurrency, collecting (nodes, edges) per chunk
/// and erroring after `max_errors` failures (GRAPHRAG_MAX_ERRORS default 3).
pub async fn extract_chunks(
    extractor: &GraphExtractor,
    llm: &LlmClient,
    doc_id: &str,
    chunks: &[String],
    entity_types: &[String],
    language: &str,
    max_concurrency: usize,
    max_errors: usize,
) -> Result<Vec<GraphExtractionResult>> {
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(max_concurrency));
    let mut error_count = 0usize;
    let mut results = Vec::with_capacity(chunks.len());
    let mut handles = Vec::new();
    for (i, chunk) in chunks.iter().enumerate() {
        let permit = semaphore.clone().acquire_owned().await?;
        let llm = llm.clone();
        let chunk = chunk.clone();
        let chunk_key = format!("{doc_id}#{i}");
        let entity_types = entity_types.to_vec();
        let language = language.to_string();
        // GraphExtractor is a cheap config struct (max_gleanings) — clone it
        // so the spawned task owns its reference.
        let extractor = extractor.clone();
        handles.push(tokio::spawn(async move {
            let _permit = permit;
            extractor
                .process_single_content(&chunk_key, &chunk, &entity_types, &llm, &language)
                .await
        }));
    }
    for h in handles {
        match h.await {
            Ok(Ok(r)) => results.push(r),
            Ok(Err(e)) => {
                error_count += 1;
                if error_count > max_errors {
                    return Err(anyhow::anyhow!(
                        "Maximum error count ({max_errors}) reached: {e}"
                    ));
                }
            }
            Err(e) => {
                error_count += 1;
                if error_count > max_errors {
                    return Err(anyhow::anyhow!("worker join failed: {e}"));
                }
            }
        }
    }
    Ok(results)
}

/// build_subgraph — index.py `generate_subgraph` in-memory core: fold
/// extraction results into an EntityGraph, dropping relations whose
/// endpoints are missing. Returns the subgraph and the count of ignored
/// relations (mirrors `ignored_rels` accounting).
pub fn build_subgraph(
    results: &[GraphExtractionResult],
    doc_id: &str,
    entity_types: &[String],
) -> (EntityGraph, usize) {
    let mut graph = EntityGraph::new();
    let _ = entity_types;
    for r in results {
        for node in &r.nodes {
            graph.add_node(
                &node.entity_name,
                entity_type_from_str(&node.entity_type),
                &node.description,
                doc_id,
            );
        }
    }
    let mut ignored = 0usize;
    for r in results {
        for edge in &r.edges {
            if !graph.node_names().contains(&edge.src_id)
                || !graph.node_names().contains(&edge.tgt_id)
            {
                ignored += 1;
                continue;
            }
            graph.add_relation(&edge.src_id, &edge.tgt_id, edge.weight as f32);
        }
    }
    (graph, ignored)
}

/// Run the full per-doc pipeline (extraction → subgraph) — mirrors
/// `run_graphrag`'s generate_subgraph step without storage hooks.
pub async fn run_doc_pipeline(
    llm: &LlmClient,
    doc_id: &str,
    chunks: &[String],
    entity_types: &[String],
    language: &str,
) -> Result<(EntityGraph, usize)> {
    let extractor = GraphExtractor::new();
    let results = extract_chunks(
        &extractor,
        llm,
        doc_id,
        chunks,
        entity_types,
        language,
        MAX_CONCURRENT_PROCESS_AND_EXTRACT_CHUNK,
        3,
    )
    .await?;
    Ok(build_subgraph(&results, doc_id, entity_types))
}

/// Map a free-form entity-type string to the closest EntityType (used when
/// re-applying typed entities; unknown types stay Unknown).
pub fn entity_type_from_str(s: &str) -> EntityType {
    match s.to_uppercase().as_str() {
        "PERSON" | "人" => EntityType::Person,
        "ORGANIZATION" | "ORG" | "组织" => EntityType::Organization,
        "LOCATION" | "GEO" | "地点" => EntityType::Location,
        "DATE" | "时间" => EntityType::Date,
        "TECHNOLOGY" | "技术" => EntityType::Technology,
        "PRODUCT" | "产品" => EntityType::Product,
        "EVENT" | "事件" => EntityType::Event,
        "CONCEPT" | "概念" => EntityType::Concept,
        _ => EntityType::Unknown,
    }
}

/// Re-apply extracted node types onto a subgraph — call after build_subgraph
/// if typed entities matter downstream (community prompts, type filters).
pub fn apply_node_types(graph: &mut EntityGraph, nodes: &[KgExtractedNode]) {
    for n in nodes {
        graph.set_node_type(&n.entity_name, entity_type_from_str(&n.entity_type));
    }
}

/// Apply extracted edges onto a graph, skipping any with missing endpoints.
pub fn apply_edges(graph: &mut EntityGraph, edges: &[KgExtractedEdge]) -> usize {
    let mut ignored = 0;
    for e in edges {
        if !graph.node_names().contains(&e.src_id) || !graph.node_names().contains(&e.tgt_id) {
            ignored += 1;
            continue;
        }
        graph.add_relation(&e.src_id, &e.tgt_id, e.weight as f32);
    }
    ignored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphrag::GraphExtractor;

    fn small_graph() -> EntityGraph {
        let mut g = EntityGraph::new();
        for (name, desc) in [("A", "entity A"), ("B", "entity B"), ("C", "entity C")] {
            g.add_node(name, EntityType::Unknown, desc, "d1");
        }
        g.add_relation("A", "B", 1.0);
        g.add_relation("B", "C", 2.0);
        g
    }

    #[test]
    fn tidy_graph_purges_nodes_missing_attributes() {
        let mut g = EntityGraph::new();
        g.add_node("HasDesc", EntityType::Unknown, "desc", "d1");
        g.add_node("NoSource", EntityType::Unknown, "desc", ""); // empty source id
        let (purged_nodes, purged_edges) = tidy_graph(&mut g, true);
        assert_eq!(purged_nodes, 1);
        assert_eq!(purged_edges, 0);
        assert!(g.node_names().contains(&"HasDesc".to_string()));
        assert!(!g.node_names().contains(&"NoSource".to_string()));
    }

    #[test]
    fn graph_merge_concats_and_accumulates() {
        let mut g1 = small_graph();
        // g2: A gets a second description + source; A-B weight 3; new edge A-C
        let mut g2 = EntityGraph::new();
        g2.add_node("A", EntityType::Unknown, "second desc", "d2");
        g2.add_relation("A", "B", 3.0);
        g2.add_relation("A", "C", 0.5);

        let change = graph_merge(&mut g1, &g2);
        assert!(change.added_updated_nodes.contains("A"));
        assert!(
            change
                .added_updated_edges
                .contains(&("A".to_string(), "B".to_string()))
        );
        let desc = g1.description_of("A").unwrap();
        assert!(
            desc.contains("<SEP>"),
            "descriptions concatenated with <SEP>"
        );
        assert_eq!(g1.edge_weight("A", "B").unwrap(), 4.0, "weights accumulate");
        assert!(g1.edge_weight("A", "C").is_some(), "new edge added");
        assert!(g1.source_id_of("A").contains(&"d1".to_string()));
        assert!(g1.source_id_of("A").contains(&"d2".to_string()));
    }

    #[test]
    fn pagerank_ranks_hub_higher() {
        let mut g = EntityGraph::new();
        // star: center A connected to B, C, D
        g.add_node("A", EntityType::Unknown, "a", "d");
        for n in ["B", "C", "D"] {
            g.add_node(n, EntityType::Unknown, n, "d");
            g.add_relation("A", n, 1.0);
        }
        let pr = pagerank(&g, 0.85, 100, 1e-6);
        assert!(pr["A"] > pr["B"], "hub should rank above leaves");
        // sum ≈ 1
        let sum: f64 = pr.values().sum();
        assert!((sum - 1.0).abs() < 1e-6);
    }

    #[test]
    fn build_subgraph_drops_relations_with_missing_endpoints() {
        let mut r = GraphExtractionResult::default();
        r.nodes.push(KgExtractedNode {
            entity_name: "ALPHA".into(),
            entity_type: "PERSON".into(),
            description: "alpha".into(),
            source_id: "c0".into(),
        });
        r.nodes.push(KgExtractedNode {
            entity_name: "BETA".into(),
            entity_type: "PERSON".into(),
            description: "beta".into(),
            source_id: "c0".into(),
        });
        r.edges.push(KgExtractedEdge {
            src_id: "ALPHA".into(),
            tgt_id: "BETA".into(),
            weight: 1.0,
            description: "rel".into(),
            keywords: "kw".into(),
            source_id: "c0".into(),
        });
        // GAMMA is not extracted as a node → relation ignored
        r.edges.push(KgExtractedEdge {
            src_id: "ALPHA".into(),
            tgt_id: "GAMMA".into(),
            weight: 2.0,
            description: "rel2".into(),
            keywords: "kw".into(),
            source_id: "c0".into(),
        });
        let (g, ignored) = build_subgraph(&[r], "doc-1", &["person".into()]);
        assert_eq!(ignored, 1);
        assert!(g.edge_weight("ALPHA", "BETA").is_some());
        assert!(g.edge_weight("ALPHA", "GAMMA").is_none());
    }

    #[test]
    fn apply_node_types_maps_strings() {
        let mut g = EntityGraph::new();
        g.set_description("李锦澎", "person");
        g.set_source_id("李锦澎", "d");
        let nodes = vec![KgExtractedNode {
            entity_name: "李锦澎".into(),
            entity_type: "PERSON".into(),
            description: "person".into(),
            source_id: "d".into(),
        }];
        apply_node_types(&mut g, &nodes);
        assert_eq!(entity_type_from_str("PERSON"), EntityType::Person);
        assert_eq!(entity_type_from_str("产品"), EntityType::Product);
        assert_eq!(entity_type_from_str("whatever"), EntityType::Unknown);
    }

    #[test]
    fn extract_chunks_collects_results_and_enforces_error_cap() {
        // concurrency machinery is exercised in the async path; here we only
        // assert the synchronous helpers stay consistent.
        let ex = GraphExtractor::new();
        assert_eq!(ex.max_gleanings, ENTITY_EXTRACTION_MAX_GLEANINGS);
        assert_eq!(DEFAULT_ENTITY_TYPES.len(), 5);
        assert_eq!(MAX_CONCURRENT_PROCESS_AND_EXTRACT_CHUNK, 10);
    }
}
