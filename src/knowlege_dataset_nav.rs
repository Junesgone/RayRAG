//! Dataset-level navigation clustering — RAGFlow v0.27.2
//! `rag/advanced_rag/knowlege_compile/dataset_nav.py`.
//!
//! Incremental clustering for dataset-level navigation: each new document is
//! embedded and placed into the nearest `nav_cluster` via layered KNN search
//! plus threshold-based merge/create. Storage keeps one row per `nav_cluster`
//! or `nav_doc` node; the tree is encoded via `parent_kwd` (no tree blob).
//!
//! Port progress: part 1 (constants, ids, tree-summary helper, index/vector
//! helpers, doc-store get/search wrappers). Store I/O runs through the local
//! `DocStore`; row ids use the crate row-id helper over the upstream seed
//! (xxh3-64; upstream xxh64 — established divergence).

use serde_json::Value;

/// `_COMPILE_KWD`.
pub const COMPILE_KWD: &str = "dataset_nav";

/// `_MERGE_THRESHOLD`.
pub const MERGE_THRESHOLD: f64 = 0.80;
/// `_RECURSE_THRESHOLD`.
pub const RECURSE_THRESHOLD: f64 = 0.65;
/// `_MIN_SIM`.
pub const MIN_SIM: f64 = 0.50;
/// `_MAX_FANOUT`.
pub const MAX_FANOUT: usize = 64;
/// `_MAX_DOCS_PER_CLUSTER`.
pub const MAX_DOCS_PER_CLUSTER: usize = 50;
/// `_LOCK_TIMEOUT_S`.
pub const LOCK_TIMEOUT_S: u64 = 30;
/// `_LOCK_BLOCKING_TIMEOUT_S`.
pub const LOCK_BLOCKING_TIMEOUT_S: u64 = 5;
/// `_KNN_TOP_K`.
pub const KNN_TOP_K: usize = 5;
/// `_NAV_HYBRID_DENSE_W`.
pub const NAV_HYBRID_DENSE_W: f64 = 0.5;
/// `_NAV_TREE_MIN_SCORE`.
pub const NAV_TREE_MIN_SCORE: f64 = 0.1;

/// `_NAV_SEARCH_FIELDS`.
pub const NAV_SEARCH_FIELDS: [&str; 7] = [
    "id",
    "content_with_weight",
    "name",
    "doc_id",
    "type_kwd",
    "doc_ids_kwd",
    "doc_count_int",
];

/// `_NAV_STOP_WORDS`.
pub const NAV_STOP_WORDS: [&str; 30] = [
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "at", "is", "are", "was",
    "were", "be", "been", "being", "this", "that", "these", "those", "it", "its", "as", "by",
    "from", "about", "into",
];

/// `_nav_doc_id`: stable row id for a nav_doc (deterministic by doc_id).
pub fn nav_doc_id(doc_id: &str) -> String {
    crate::structure_compile::stable_row_id(&[format!("dataset_nav:doc:{doc_id}")])
}

/// `_nav_cluster_id`: stable row id for a nav_cluster (kb_id + name).
pub fn nav_cluster_id(kb_id: &str, name: &str) -> String {
    crate::structure_compile::stable_row_id(&[format!("dataset_nav:{kb_id}:cluster:{name}")])
}

/// `_nav_lock_key`: Redis lock key for a KB's nav tree.
pub fn nav_lock_key(kb_id: &str) -> String {
    format!("dataset_nav:{kb_id}")
}

/// `_extract_root_summary_from_tree`: the doc-level summary from a RAPTOR tree
/// (or a bare string value).
pub fn extract_root_summary_from_tree(tree: &Value) -> String {
    let Some(map) = tree.as_object() else {
        return String::new();
    };
    if let Some(title) = map.get("title").and_then(Value::as_str) {
        if !title.trim().is_empty() {
            return title.trim().to_string();
        }
    }
    for key in ["summary", "content_with_weight", "content"] {
        if let Some(text) = map.get(key).and_then(Value::as_str) {
            if !text.trim().is_empty() {
                return text.trim().to_string();
            }
        }
    }
    String::new()
}

/// `_index_name(tenant_id)`.
pub fn index_name(tenant_id: &str) -> String {
    crate::structure_compile::doc_store_index_name(tenant_id)
}

/// `_vec_field(dim)`.
pub fn vec_field(dim: usize) -> String {
    format!("q_{dim}_vec")
}

/// `_store_get`: one row by id.
pub fn store_get(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    row_id: &str,
) -> Option<Value> {
    let index = index_name(tenant_id);
    let row = store.get(row_id, &index, &[kb_id.to_string()]).ok()??;
    Some(Value::Object(row))
}

/// `_store_search`: rows matching a conjunctive condition.
pub fn store_search(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    condition: &Value,
    fields: &[String],
    limit: usize,
) -> Vec<Value> {
    let index = index_name(tenant_id);
    let mut query = crate::doc_store::SearchQuery {
        select_fields: fields.to_vec(),
        condition: condition.as_object().cloned().unwrap_or_default(),
        offset: 0,
        limit,
        index_names: vec![index],
        dataset_ids: vec![kb_id.to_string()],
        ..Default::default()
    };
    query.match_expressions = Vec::new();
    let Ok(response) = store.search(&query) else {
        return Vec::new();
    };
    store
        .get_fields(&response, fields)
        .into_values()
        .map(Value::Object)
        .collect()
}
/// `_store_upsert`: update by id when present, else insert.
pub fn store_upsert(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc: &Value,
) {
    let index = index_name(tenant_id);
    let row_id = doc.get("id").and_then(Value::as_str).unwrap_or("");
    let existing = store
        .get(row_id, &index, &[kb_id.to_string()])
        .ok()
        .flatten();
    if existing.is_some() {
        let mut upd = doc.as_object().cloned().unwrap_or_default();
        upd.remove("id");
        let condition: crate::doc_store::FilterCondition =
            [("id".to_string(), Value::String(row_id.to_string()))]
                .into_iter()
                .collect();
        let _ = store.update(&condition, &upd, &index, kb_id);
    } else {
        if let Some(map) = doc.as_object().cloned() {
            let _ = store.insert(&[map], &index, kb_id);
        }
    }
}

/// `_store_delete`: delete one row by id (best-effort).
pub fn store_delete(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    row_id: &str,
) {
    let index = index_name(tenant_id);
    let condition: crate::doc_store::FilterCondition =
        [("id".to_string(), serde_json::json!([row_id]))]
            .into_iter()
            .collect();
    let _ = store.delete(&condition, &index, kb_id);
}

/// `_vector_len`.
pub fn vector_len(vec: Option<&Value>) -> usize {
    vec.and_then(Value::as_array)
        .map(|items| items.len())
        .unwrap_or(0)
}

/// `_cosine_sim`: cosine similarity (0.0 on shape/zero mismatch).
pub fn cosine_sim(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += (*x as f64) * (*y as f64);
        na += (*x as f64) * (*x as f64);
        nb += (*y as f64) * (*y as f64);
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// `_tokenize` (coarse): `rag_tokenizer.tokenize` via the crate's search
/// tokenizer (space-joined).
pub fn tokenize(text: &str) -> String {
    crate::structure_compile::tokenize_for_search(text)
        .0
        .join(" ")
}

/// `_fine_tokenize`: `rag_tokenizer.fine_grained_tokenize` over the coarse
/// tokens (the crate tokenizer derives both passes from one call).
pub fn fine_tokenize(coarse: &str) -> String {
    crate::structure_compile::tokenize_for_search(coarse)
        .1
        .join(" ")
}

fn nav_summary_hash(summary: &str) -> String {
    let digest = xxhash_rust::xxh3::xxh3_64(summary.as_bytes());
    let hex = format!("{digest:016x}");
    hex.chars().take(12).collect()
}

/// `_make_nav_doc_row`: one document leaf row.
pub fn make_nav_doc_row(
    kb_id: &str,
    doc_id: &str,
    summary: &str,
    parent_kwd: &str,
    depth_int: i64,
    embedding: &[f32],
    graph_content: &str,
) -> Value {
    let kw_text = if graph_content.is_empty() {
        summary
    } else {
        graph_content
    };
    let mut payload = serde_json::Map::new();
    payload.insert("type".to_string(), Value::String("nav_doc".to_string()));
    payload.insert(
        "description".to_string(),
        Value::String(summary.to_string()),
    );
    payload.insert(
        "keywords".to_string(),
        serde_json::json!(nav_keywords(kw_text)),
    );
    payload.insert(
        "entities".to_string(),
        serde_json::json!(nav_entities(kw_text)),
    );
    if !graph_content.is_empty() {
        payload.insert(
            "graph_content".to_string(),
            Value::String(graph_content.to_string()),
        );
    }
    let ltks = tokenize(kw_text);
    let mut row = serde_json::Map::new();
    row.insert("id".to_string(), Value::String(nav_doc_id(doc_id)));
    row.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
    row.insert("doc_id".to_string(), Value::String(doc_id.to_string()));
    row.insert(
        "compile_kwd".to_string(),
        Value::String(COMPILE_KWD.to_string()),
    );
    row.insert(
        "knowledge_graph_kwd".to_string(),
        Value::String("entity".to_string()),
    );
    row.insert("type_kwd".to_string(), Value::String("nav_doc".to_string()));
    row.insert(
        "name".to_string(),
        Value::String(format!("{parent_kwd}_{}", nav_summary_hash(summary))),
    );
    row.insert(
        "parent_kwd".to_string(),
        Value::String(parent_kwd.to_string()),
    );
    row.insert("depth_int".to_string(), serde_json::json!(depth_int));
    row.insert("available_int".to_string(), serde_json::json!(0));
    row.insert(
        "content_with_weight".to_string(),
        Value::String(Value::Object(payload).to_string()),
    );
    row.insert("content_ltks".to_string(), Value::String(ltks.clone()));
    row.insert(
        "content_sm_ltks".to_string(),
        Value::String(fine_tokenize(&ltks)),
    );
    if !embedding.is_empty() {
        row.insert(vec_field(embedding.len()), serde_json::json!(embedding));
    }
    Value::Object(row)
}

/// `_make_nav_cluster_row`: one internal tree node row.
pub fn make_nav_cluster_row(
    kb_id: &str,
    name: &str,
    description: &str,
    parent_kwd: &str,
    depth_int: i64,
    doc_ids: &[String],
    embedding: &[f32],
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("type".to_string(), Value::String("nav_cluster".to_string()));
    payload.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );
    payload.insert(
        "keywords".to_string(),
        serde_json::json!(nav_keywords(description)),
    );
    payload.insert(
        "entities".to_string(),
        serde_json::json!(nav_entities(description)),
    );
    let ltks = tokenize(description);
    let mut row = serde_json::Map::new();
    row.insert("id".to_string(), Value::String(nav_cluster_id(kb_id, name)));
    row.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
    row.insert("doc_id".to_string(), Value::String(kb_id.to_string()));
    row.insert(
        "compile_kwd".to_string(),
        Value::String(COMPILE_KWD.to_string()),
    );
    row.insert(
        "knowledge_graph_kwd".to_string(),
        Value::String("entity".to_string()),
    );
    row.insert(
        "type_kwd".to_string(),
        Value::String("nav_cluster".to_string()),
    );
    row.insert("name".to_string(), Value::String(name.to_string()));
    row.insert(
        "parent_kwd".to_string(),
        Value::String(parent_kwd.to_string()),
    );
    row.insert("depth_int".to_string(), serde_json::json!(depth_int));
    row.insert("doc_ids_kwd".to_string(), serde_json::json!(doc_ids));
    row.insert(
        "doc_count_int".to_string(),
        serde_json::json!(doc_ids.len()),
    );
    row.insert("available_int".to_string(), serde_json::json!(0));
    row.insert(
        "content_with_weight".to_string(),
        Value::String(Value::Object(payload).to_string()),
    );
    row.insert("content_ltks".to_string(), Value::String(ltks.clone()));
    row.insert(
        "content_sm_ltks".to_string(),
        Value::String(fine_tokenize(&ltks)),
    );
    if !embedding.is_empty() {
        row.insert(vec_field(embedding.len()), serde_json::json!(embedding));
    }
    Value::Object(row)
}

/// `_nav_keywords`: routing tag-words from a summary (zero-LLM).
pub fn nav_keywords(summary: &str) -> Vec<String> {
    nav_keywords_limited(summary, 6)
}

/// `_nav_keywords(summary, max_kwds)`.
pub fn nav_keywords_limited(summary: &str, max_kwds: usize) -> Vec<String> {
    let tokens: Vec<String> = tokenize(summary)
        .split_whitespace()
        .map(|token| token.to_string())
        .collect();
    let mut seen: Vec<String> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for token in tokens {
        let token = token.trim().to_string();
        let lower = token.to_lowercase();
        if token.chars().count() < 2
            || token.chars().all(|c| c.is_ascii_digit())
            || NAV_STOP_WORDS.contains(&lower.as_str())
            || seen.iter().any(|existing| existing == &lower)
        {
            continue;
        }
        seen.push(lower);
        out.push(token);
        if out.len() >= max_kwds {
            break;
        }
    }
    out
}

/// `_nav_entities`: likely named entities from a summary (zero-LLM heuristic:
/// English capitalized sequences first, then tokenizer-based for CJK).
pub fn nav_entities(summary: &str) -> Vec<String> {
    nav_entities_limited(summary, 6)
}

/// `_nav_entities(summary, max_entities)`.
pub fn nav_entities_limited(summary: &str, max_entities: usize) -> Vec<String> {
    let text = summary;
    let mut entities: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let capitalize =
        regex::Regex::new(r"\b([A-Z][a-z]+(?:\s+[A-Z][a-z]+)+)\b").expect("entity regex");
    for hit in capitalize.captures_iter(text) {
        let ent = hit[1].trim().to_string();
        let key = ent.to_lowercase();
        if !NAV_STOP_WORDS.contains(&key.as_str()) && !seen.iter().any(|existing| existing == &key)
        {
            seen.push(key);
            entities.push(ent);
            if entities.len() >= max_entities {
                return entities;
            }
        }
    }
    for token in tokenize(text).split_whitespace() {
        let token = token.trim().to_string();
        if token.chars().count() < 3 || token.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if token.is_ascii()
            && token
                .chars()
                .next()
                .map(|c| c.is_ascii_lowercase())
                .unwrap_or(false)
        {
            continue;
        }
        let key = token.to_lowercase();
        if NAV_STOP_WORDS.contains(&key.as_str()) || seen.iter().any(|existing| existing == &key) {
            continue;
        }
        seen.push(key);
        entities.push(token);
        if entities.len() >= max_entities {
            break;
        }
    }
    entities
}

/// `_as_str_list`.
pub fn as_str_list(value: Option<&Value>) -> Vec<String> {
    match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        Some(other) => vec![other.to_string()],
    }
}

/// `_matches_condition`: simple equality filters used by dataset navigation.
pub fn matches_condition(row: &Value, condition: &Value) -> bool {
    let Some(condition_map) = condition.as_object() else {
        return true;
    };
    for (field, expected) in condition_map {
        let expected_values = as_str_list(Some(expected));
        if expected_values.is_empty() || field == "kb_id" {
            continue;
        }
        let actual_values = as_str_list(row.get(field));
        let matched = actual_values
            .iter()
            .any(|actual| expected_values.iter().any(|item| actual == item));
        if !matched {
            return false;
        }
    }
    true
}

/// `_in_nav_scope`: whether a nav row belongs to the given document scope.
pub fn in_nav_scope(row: &Value, allowed_docs: Option<&std::collections::HashSet<String>>) -> bool {
    let Some(allowed) = allowed_docs else {
        return true;
    };
    if allowed.is_empty() {
        return true;
    }
    let doc_ids = as_str_list(row.get("doc_ids_kwd"));
    if !doc_ids.is_empty() {
        return doc_ids.iter().any(|doc| allowed.contains(doc));
    }
    let doc_id = row
        .get("doc_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    allowed.contains(&doc_id)
}

#[cfg(test)]
mod dataset_nav_tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    #[test]
    fn ids_are_stable_and_scoped() {
        assert_eq!(nav_doc_id("d1"), nav_doc_id("d1"));
        assert_ne!(nav_doc_id("d1"), nav_doc_id("d2"));
        assert_ne!(nav_cluster_id("kb1", "A"), nav_cluster_id("kb1", "B"));
        assert_ne!(nav_cluster_id("kb1", "A"), nav_cluster_id("kb2", "A"));
        assert_eq!(nav_lock_key("kb1"), "dataset_nav:kb1");
        assert_eq!(index_name("t1"), "ragflow_t1");
        assert_eq!(vec_field(1024), "q_1024_vec");
        assert_eq!(COMPILE_KWD, "dataset_nav");
    }

    #[test]
    fn root_summary_variants() {
        assert_eq!(
            extract_root_summary_from_tree(&json!({"title": " Root "})),
            "Root"
        );
        assert_eq!(
            extract_root_summary_from_tree(&json!({"summary": "S"})),
            "S"
        );
        assert_eq!(
            extract_root_summary_from_tree(&json!({"content_with_weight": "C"})),
            "C"
        );
        assert_eq!(extract_root_summary_from_tree(&json!({"title": "  "})), "");
        assert_eq!(extract_root_summary_from_tree(&json!(null)), "");
        assert_eq!(extract_root_summary_from_tree(&json!("bare")), "");
    }

    #[derive(Default)]
    struct MockStore {
        rows: Vec<Value>,
        gets: Mutex<usize>,
    }

    impl crate::doc_store::DocStore for MockStore {
        fn db_type(&self) -> &'static str {
            "memory"
        }
        fn health(&self) -> crate::Result<crate::doc_store::HealthStatus> {
            Ok(crate::doc_store::HealthStatus::green("test"))
        }
        fn create_idx(&self, _i: &str, _d: &str, _v: usize) -> crate::Result<()> {
            Ok(())
        }
        fn delete_idx(&self, _i: &str, _d: &str) -> crate::Result<()> {
            Ok(())
        }
        fn index_exist(&self, _i: &str, _d: &str) -> crate::Result<bool> {
            Ok(true)
        }
        fn insert(
            &self,
            _rows: &[crate::doc_store::DocRow],
            _i: &str,
            _d: &str,
        ) -> crate::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn get(
            &self,
            _data_id: &str,
            _i: &str,
            _d: &[String],
        ) -> crate::Result<Option<crate::doc_store::DocRow>> {
            *self.gets.lock().unwrap() += 1;
            Ok(None)
        }
        fn update(
            &self,
            _c: &crate::doc_store::FilterCondition,
            _n: &crate::doc_store::DocRow,
            _i: &str,
            _d: &str,
        ) -> crate::Result<bool> {
            Ok(false)
        }
        fn delete(
            &self,
            _c: &crate::doc_store::FilterCondition,
            _i: &str,
            _d: &str,
        ) -> crate::Result<usize> {
            Ok(0)
        }
        fn search(
            &self,
            _q: &crate::doc_store::SearchQuery,
        ) -> crate::Result<crate::doc_store::SearchResponse> {
            let docs: Vec<crate::doc_store::DocRow> = self
                .rows
                .iter()
                .filter_map(|row| row.as_object().cloned())
                .collect();
            Ok(crate::doc_store::SearchResponse {
                total: docs.len(),
                docs,
                ..Default::default()
            })
        }
        fn sql(&self, _s: &str, _f: usize) -> crate::Result<Vec<Value>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn store_wrappers_delegate() {
        let store = MockStore {
            rows: vec![json!({"id": "r1", "name": "A"})],
            ..MockStore::default()
        };
        let fields = vec!["id".to_string(), "name".to_string()];
        let rows = store_search(
            &store,
            "t1",
            "kb1",
            &json!({"compile_kwd": ["dataset_nav"]}),
            &fields,
            100,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], json!("A"));
        assert!(store_get(&store, "t1", "kb1", "r1").is_none());
        assert_eq!(*store.gets.lock().unwrap(), 1, "get was attempted");
    }
}
#[cfg(test)]
mod dataset_nav_part2_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn vector_and_cosine_math() {
        assert_eq!(vector_len(Some(&json!([1.0, 2.0]))), 2);
        assert_eq!(vector_len(None), 0);
        assert_eq!(vector_len(Some(&json!(5))), 0);
        let sim = cosine_sim(&[1.0, 0.0], &[1.0, 0.0]);
        assert!((sim - 1.0).abs() < 1e-9);
        assert_eq!(cosine_sim(&[1.0], &[1.0, 0.0]), 0.0, "shape mismatch");
        assert_eq!(cosine_sim(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn nav_rows_have_expected_shape() {
        let row = make_nav_doc_row("kb1", "d1", "Alpha doc", "root", 1, &[0.1, 0.2], "");
        assert_eq!(row["type_kwd"], json!("nav_doc"));
        assert_eq!(row["compile_kwd"], json!("dataset_nav"));
        assert_eq!(row["available_int"], json!(0));
        assert!(row["name"].as_str().unwrap().starts_with("root_"));
        assert!(row.get("q_2_vec").is_some());
        let payload: Value =
            serde_json::from_str(row["content_with_weight"].as_str().unwrap()).unwrap();
        assert_eq!(payload["type"], json!("nav_doc"));
        assert!(payload.get("graph_content").is_none());

        let cluster = make_nav_cluster_row(
            "kb1",
            "Topic",
            "About Topic",
            "root",
            0,
            &["d1".to_string(), "d2".to_string()],
            &[],
        );
        assert_eq!(cluster["type_kwd"], json!("nav_cluster"));
        assert_eq!(cluster["doc_count_int"], json!(2));
        assert_eq!(cluster["doc_ids_kwd"], json!(["d1", "d2"]));
        assert!(
            cluster.get("q_2_vec").is_none(),
            "no embedding -> no vec field"
        );
    }

    #[test]
    fn graph_content_replaces_keyword_source() {
        let row = make_nav_doc_row("kb1", "d1", "summary", "root", 1, &[], "Graph Alpha Beta");
        let payload: Value =
            serde_json::from_str(row["content_with_weight"].as_str().unwrap()).unwrap();
        assert_eq!(payload["graph_content"], json!("Graph Alpha Beta"));
    }

    #[test]
    fn keywords_and_entities_are_deduped_and_capped() {
        let keywords = nav_keywords_limited("Alpha Beta Gamma Delta Epsilon Zeta Eta", 3);
        assert_eq!(keywords.len(), 3);
        let entities = nav_entities_limited("New York City and Machine Learning work", 2);
        assert_eq!(entities.len(), 2);
        assert!(entities.contains(&"New York City".to_string()));
    }

    #[test]
    fn condition_and_scope_matrix() {
        let row = json!({"doc_id": "d1", "type_kwd": "nav_doc", "kb_id": "kb1"});
        assert!(matches_condition(
            &row,
            &json!({"type_kwd": "nav_doc", "kb_id": "kb9"})
        ));
        assert!(!matches_condition(
            &row,
            &json!({"type_kwd": "nav_cluster"})
        ));
        assert!(
            matches_condition(&row, &json!({"type_kwd": []})),
            "empty filter passes"
        );

        let cluster = json!({"doc_id": "kb1", "doc_ids_kwd": ["d1", "d2"]});
        let mut allowed = std::collections::HashSet::new();
        allowed.insert("d1".to_string());
        assert!(in_nav_scope(&cluster, Some(&allowed)));
        let mut allowed_other = std::collections::HashSet::new();
        allowed_other.insert("d9".to_string());
        assert!(!in_nav_scope(&cluster, Some(&allowed_other)));
        assert!(in_nav_scope(&row, Some(&allowed)));
        assert!(!in_nav_scope(&row, Some(&allowed_other)));
        assert!(in_nav_scope(&row, None), "no scope allows everything");
    }
}
