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

use crate::structure_compile::EmbeddingBackend;
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
    if let Some(title) = map.get("title").and_then(Value::as_str)
        && !title.trim().is_empty()
    {
        return title.trim().to_string();
    }
    for key in ["summary", "content_with_weight", "content"] {
        if let Some(text) = map.get(key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return text.trim().to_string();
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
        Some(Value::String(text)) if !text.is_empty() => vec![text.clone()],
        Some(Value::String(_)) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .filter(|item| value_truthy(item))
            .map(|item| match item {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect(),
        Some(_) => Vec::new(),
    }
}

fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
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

/// `_store_text_search`: BM25 leg over the nav rows' tokenized fields.
#[allow(clippy::too_many_arguments)]
pub fn store_text_search(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    query: &str,
    fields: &[String],
    limit: usize,
    compile_kwd: &str,
    type_kwd: &str,
    extra_filter: Option<&Value>,
) -> Vec<Value> {
    let tokenized_query = tokenize(query);
    let index = index_name(tenant_id);
    let mut filter_condition = serde_json::Map::new();
    filter_condition.insert("compile_kwd".to_string(), serde_json::json!([compile_kwd]));
    if !type_kwd.is_empty() {
        filter_condition.insert("type_kwd".to_string(), serde_json::json!([type_kwd]));
    }
    if let Some(extra) = extra_filter.and_then(Value::as_object) {
        for (key, value) in extra {
            filter_condition.insert(key.clone(), value.clone());
        }
    }
    let query_obj = crate::doc_store::SearchQuery {
        select_fields: fields.to_vec(),
        condition: filter_condition,
        match_expressions: vec![crate::doc_store::MatchExpr::text(
            &["content_ltks", "content_sm_ltks"],
            &tokenized_query,
            limit,
        )],
        offset: 0,
        limit,
        index_names: vec![index],
        dataset_ids: vec![kb_id.to_string()],
        ..Default::default()
    };
    let Ok(response) = store.search(&query_obj) else {
        return Vec::new();
    };
    store
        .get_fields(&response, fields)
        .into_values()
        .map(Value::Object)
        .collect()
}

/// `_store_knn`: dense KNN with the filter, plus the fallback scan when the
/// engine ignores filters (rows re-checked by condition/vector shape).
#[allow(clippy::too_many_arguments)]
pub fn store_knn(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    vec: &[f32],
    vec_dim: usize,
    filter_condition: &Value,
    top_k: usize,
) -> Vec<Value> {
    let index = index_name(tenant_id);
    let vf = vec_field(vec_dim);
    let fields: Vec<String> = [
        "content_with_weight",
        "name",
        "doc_id",
        "compile_kwd",
        "type_kwd",
        "parent_kwd",
        "depth_int",
        "doc_count_int",
        "doc_ids_kwd",
        vf.as_str(),
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let query_obj = crate::doc_store::SearchQuery {
        select_fields: fields.clone(),
        condition: filter_condition.as_object().cloned().unwrap_or_default(),
        match_expressions: vec![crate::doc_store::MatchExpr::dense(
            &vf,
            vec.to_vec(),
            "cosine",
            top_k,
        )],
        offset: 0,
        limit: top_k,
        index_names: vec![index],
        dataset_ids: vec![kb_id.to_string()],
        ..Default::default()
    };
    let mut rows: Vec<Value> = match store.search(&query_obj) {
        Ok(response) => store
            .get_fields(&response, &fields)
            .into_values()
            .map(Value::Object)
            .collect(),
        Err(_) => Vec::new(),
    };
    let condition_empty = filter_condition
        .as_object()
        .map(|map| map.is_empty())
        .unwrap_or(true);
    let has_bad_row = rows
        .iter()
        .any(|row| !matches_condition(row, filter_condition));
    if !condition_empty && has_bad_row {
        let scanned = store_search(store, tenant_id, kb_id, filter_condition, &fields, 10000);
        let mut filtered: Vec<Value> = scanned
            .into_iter()
            .filter(|row| {
                vector_len(row.get(&vf)) == vec_dim && matches_condition(row, filter_condition)
            })
            .collect();
        filtered.sort_by(|a, b| {
            let left = vector_of(a.get(&vf));
            let right = vector_of(b.get(&vf));
            cosine_sim(vec, &right)
                .partial_cmp(&cosine_sim(vec, &left))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        rows = filtered.into_iter().take(top_k).collect();
    }
    rows
}

fn vector_of(value: Option<&Value>) -> Vec<f32> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_f64().map(|number| number as f32))
                .collect()
        })
        .unwrap_or_default()
}

/// `_find_best_cluster`: layered KNN descent from the depth-0 root; returns
/// `(best cluster name, its parent, similarity)`.
pub fn find_best_cluster(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc_embedding: &[f32],
    vec_dim: usize,
) -> (Option<String>, Option<String>, f64) {
    let root_cond = serde_json::json!({
        "kb_id": [kb_id],
        "compile_kwd": [COMPILE_KWD],
        "type_kwd": ["nav_cluster"],
        "depth_int": [0],
    });
    let roots = store_knn(
        store,
        tenant_id,
        kb_id,
        doc_embedding,
        vec_dim,
        &root_cond,
        1,
    );
    let Some(best) = roots.into_iter().next() else {
        return (None, None, 0.0);
    };
    let mut best_name = best
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut best_parent = best
        .get("parent_kwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let stored = vector_of(best.get(vec_field(vec_dim)));
    let mut sim = cosine_sim(doc_embedding, &stored);
    let mut visited: Vec<String> = vec![best_name.clone()];
    let mut best = best;
    while sim >= RECURSE_THRESHOLD {
        let child_cond = serde_json::json!({
            "kb_id": [kb_id],
            "compile_kwd": [COMPILE_KWD],
            "type_kwd": ["nav_cluster"],
            "parent_kwd": [best_name],
        });
        let children = store_knn(
            store,
            tenant_id,
            kb_id,
            doc_embedding,
            vec_dim,
            &child_cond,
            1,
        );
        let Some(child) = children.into_iter().next() else {
            break;
        };
        let child_name = child
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if child_name.is_empty() || visited.iter().any(|name| name == &child_name) {
            break;
        }
        let stored = vector_of(child.get(vec_field(vec_dim)));
        let child_sim = cosine_sim(doc_embedding, &stored);
        if child_sim < RECURSE_THRESHOLD {
            break;
        }
        best_name = child_name;
        best_parent = child
            .get("parent_kwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or(best_parent);
        sim = child_sim;
        best = child;
        visited.push(best_name.clone());
    }
    let _ = best;
    (Some(best_name), Some(best_parent), sim)
}

/// Dataset-nav `gen_json`: one JSON call with the knowledge-compile controls.
async fn dnav_gen_json(chat: &dyn crate::harness::HarnessChat, prompt: &str) -> Option<Value> {
    let system = String::new();
    let gen_conf = crate::structure_compile::knowledge_compile_gen_conf(
        &chat_model_name(chat),
        Some(&serde_json::Map::from_iter([(
            "temperature".to_string(),
            serde_json::json!(0.1),
        )])),
    );
    let history = vec![serde_json::json!({"role": "user", "content": prompt})];
    let raw = chat
        .chat(&system, &history, &Value::Object(gen_conf))
        .await
        .ok()?;
    let think = regex::Regex::new(r"(?s)^.*</think>").expect("think regex");
    let stripped = think.replace(&raw, "").to_string();
    let fence = regex::Regex::new(r"```(?:json)?\s*|\s*```").expect("fence regex");
    let cleaned = fence.replace_all(&stripped, "").trim().to_string();
    serde_json::from_str::<Value>(&cleaned)
        .ok()
        .or_else(|| crate::structure_compile::parse_json_lenient(&cleaned))
}

/// The harness chat carries no model name; the host passes the model through a
/// wrapper when it needs model-specific controls. Kept empty here (the
/// default `reasoning_effort` branch applies).
fn chat_model_name(_chat: &dyn crate::harness::HarnessChat) -> String {
    String::new()
}

/// `_llm_merge`: fuse the cluster description with the new doc summary.
pub async fn llm_merge(
    chat: Option<&dyn crate::harness::HarnessChat>,
    cluster_desc: &str,
    doc_summary: &str,
) -> String {
    let Some(chat) = chat else {
        return cluster_desc.to_string();
    };
    let prompt = format!(
        "Merge the following two descriptions of the same topic into a single concise summary (1-3 sentences):\n\nExisting: {cluster_desc}\n\nNew: {doc_summary}\n\nReturn ONLY the merged text, no commentary."
    );
    if let Some(resp) = dnav_gen_json(chat, &prompt).await {
        if let Some(map) = resp.as_object() {
            if let Some(merged) = map.get("merged").and_then(Value::as_str) {
                return merged.to_string();
            }
            if let Some(result) = map.get("result").and_then(Value::as_str) {
                return result.to_string();
            }
        }
        if let Some(text) = resp.as_str() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    cluster_desc.to_string()
}

/// `_clean_title`: normalize an LLM title into a one-line capped display name.
pub fn clean_title(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(48)
        .collect::<String>()
        .trim()
        .to_string()
}

/// `_fallback_title`: a short readable title from a summary.
pub fn fallback_title(summary: &str) -> String {
    let words: Vec<&str> = summary.split_whitespace().collect();
    let joined = words.iter().take(6).cloned().collect::<Vec<_>>().join(" ");
    let trimmed = joined.trim().to_string();
    if trimmed.is_empty() {
        "Cluster".to_string()
    } else {
        trimmed
    }
}

/// `_readable_cluster_name`: `"<title> <8-hex>"` (hash of the seed).
pub fn readable_cluster_name(title: &str, seed: &str) -> String {
    let digest = xxhash_rust::xxh3::xxh3_64(seed.as_bytes());
    let suffix: String = format!("{digest:016x}").chars().take(8).collect();
    let cleaned = clean_title(title);
    let head = if cleaned.is_empty() {
        "Cluster".to_string()
    } else {
        cleaned
    };
    format!("{head} {suffix}")
}

/// `_llm_create_summary`: derive a cluster's readable `(name, summary)`.
pub async fn llm_create_summary(
    chat: Option<&dyn crate::harness::HarnessChat>,
    doc_summaries: &[String],
) -> (String, String) {
    let fallback_summary = doc_summaries.first().cloned().unwrap_or_default();
    let Some(chat) = chat else {
        return (fallback_title(&fallback_summary), fallback_summary);
    };
    let texts = doc_summaries.join("\n---\n");
    let prompt = format!(
        "Given the document excerpts below, produce a short human-readable topic name and a concise description of their common topic.\n\n{texts}\n\nReturn ONLY JSON: {{\"name\": \"<2-6 word topic title>\", \"summary\": \"<1-3 sentence description>\"}}"
    );
    if let Some(resp) = dnav_gen_json(chat, &prompt).await {
        if let Some(map) = resp.as_object() {
            let summary = map
                .get("summary")
                .and_then(Value::as_str)
                .or_else(|| map.get("result").and_then(Value::as_str))
                .unwrap_or("")
                .trim()
                .to_string();
            let summary = if summary.is_empty() {
                fallback_summary.clone()
            } else {
                summary
            };
            let name = {
                let cleaned = clean_title(map.get("name").and_then(Value::as_str).unwrap_or(""));
                if cleaned.is_empty() {
                    fallback_title(&summary)
                } else {
                    cleaned
                }
            };
            return (
                name,
                if summary.is_empty() {
                    fallback_summary
                } else {
                    summary
                },
            );
        }
        if let Some(text) = resp.as_str() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return (fallback_title(trimmed), trimmed.to_string());
            }
        }
    }
    (fallback_title(&fallback_summary), fallback_summary)
}

/// `build_nav_graph_text`: `(root title, full graph text)` from a graph node's
/// parsed content (entity descriptions only; child names = relation targets).
pub fn build_nav_graph_text(graph_json: &Value) -> (String, String) {
    let Some(map) = graph_json.as_object() else {
        return (String::new(), String::new());
    };
    let entities = map
        .get("entities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let relations = map
        .get("relations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut child_names: Vec<String> = Vec::new();
    for relation in &relations {
        if let Some(target) = relation.get("to").and_then(Value::as_str) {
            let target = target.trim().to_string();
            if !target.is_empty() && !child_names.iter().any(|name| name == &target) {
                child_names.push(target);
            }
        }
    }
    let mut name_desc: Vec<(String, String)> = Vec::new();
    for entity in &entities {
        let Some(name) = entity.get("name").and_then(Value::as_str) else {
            continue;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let desc = entity
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        name_desc.push((name, desc));
    }
    let mut root_name = String::new();
    let mut root_summary = String::new();
    for (name, desc) in &name_desc {
        if !child_names.iter().any(|child| child == name) {
            root_name = name.clone();
            root_summary = if desc.is_empty() {
                name.clone()
            } else {
                desc.split('\n').next().unwrap_or("").trim().to_string()
            };
            break;
        }
    }
    let mut graph_parts: Vec<String> = Vec::new();
    if !root_name.is_empty() {
        let root_desc = name_desc
            .iter()
            .find(|(name, _)| name == &root_name)
            .map(|(_, desc)| desc.clone())
            .unwrap_or_default();
        graph_parts.push(if root_desc.is_empty() {
            root_name.clone()
        } else {
            root_desc
        });
    }
    let mut emitted: Vec<String> = Vec::new();
    if !root_name.is_empty() {
        emitted.push(root_name.clone());
    }
    if name_desc.len() > emitted.len() {
        graph_parts.push(String::new());
        let mut sorted: Vec<&(String, String)> = name_desc.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, desc) in sorted {
            if emitted.iter().any(|existing| existing == name) {
                continue;
            }
            if !desc.is_empty() {
                graph_parts.push(desc.clone());
            }
            emitted.push(name.clone());
        }
    }
    (root_summary, graph_parts.join("\n"))
}

/// `RedisDistributedLock` seam for one KB's nav tree (`spin_acquire` +
/// `release`); the host owns the actual distributed lock.
pub trait NavKbLock: Send + Sync {
    /// `lock.spin_acquire()`.
    fn acquire(&self, kb_id: &str) -> bool;
    /// `lock.release()`.
    fn release(&self, kb_id: &str);
}

/// `_embed(embd_mdl, text)`: one text's embedding (`[]` when unavailable).
pub async fn nav_embed(embed: Option<&dyn EmbeddingBackend>, text: &str) -> Vec<f32> {
    let Some(embed) = embed else {
        return Vec::new();
    };
    match crate::structure_compile::encode(embed, &[text.to_string()]).await {
        Ok(vectors) => vectors.into_iter().next().unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

/// `upsert_dataset_nav_doc`: place a document into the nav clustering tree
/// (merge into the nearest cluster, create a sibling, or seed a root cluster).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_dataset_nav_doc(
    store: &dyn crate::doc_store::DocStore,
    embed: Option<&dyn EmbeddingBackend>,
    chat: Option<&dyn crate::harness::HarnessChat>,
    lock: &dyn NavKbLock,
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
    summary_or_tree: &Value,
) {
    if doc_id.is_empty() || kb_id.is_empty() {
        return;
    }
    let (summary, graph_content) = nav_summary_and_graph(summary_or_tree);
    if summary.is_empty() {
        return;
    }
    let embed_text = if graph_content.is_empty() {
        summary.clone()
    } else {
        graph_content.clone()
    };
    let doc_embedding = nav_embed(embed, &embed_text).await;
    let vec_dim = doc_embedding.len();
    if !lock.acquire(kb_id) {
        return;
    }
    let outcome = upsert_locked(
        store,
        embed,
        chat,
        tenant_id,
        kb_id,
        doc_id,
        &summary,
        &graph_content,
        &doc_embedding,
        vec_dim,
    )
    .await;
    let _ = outcome;
    lock.release(kb_id);
}

fn nav_summary_and_graph(summary_or_tree: &Value) -> (String, String) {
    match summary_or_tree {
        Value::String(text) => (text.clone(), String::new()),
        Value::Object(map) => {
            if map.contains_key("title") && map.contains_key("graph_text") {
                (
                    map.get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                    map.get("graph_text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                )
            } else {
                (
                    extract_root_summary_from_tree(summary_or_tree),
                    String::new(),
                )
            }
        }
        _ => (String::new(), String::new()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn upsert_locked(
    store: &dyn crate::doc_store::DocStore,
    embed: Option<&dyn EmbeddingBackend>,
    chat: Option<&dyn crate::harness::HarnessChat>,
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
    summary: &str,
    graph_content: &str,
    doc_embedding: &[f32],
    vec_dim: usize,
) -> std::result::Result<(), String> {
    // 3. Replace an existing nav_doc under the same lock (skip identical).
    let existing = store_get(store, tenant_id, kb_id, &nav_doc_id(doc_id));
    if let Some(existing) = &existing {
        let old_payload = existing
            .get("content_with_weight")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if old_payload.get("description").and_then(Value::as_str) == Some(summary) {
            return Ok(());
        }
        remove_dataset_nav_doc_locked(store, tenant_id, kb_id, doc_id);
    }

    // 4. Layered KNN placement.
    let (best_name, best_parent, sim) = if !doc_embedding.is_empty() {
        find_best_cluster(store, tenant_id, kb_id, doc_embedding, vec_dim)
    } else {
        (None, None, 0.0)
    };

    if let Some(best_name) = &best_name {
        if sim >= MERGE_THRESHOLD {
            // Merge into the best cluster.
            let cluster_id = nav_cluster_id(kb_id, best_name);
            let cluster_row = store_get(store, tenant_id, kb_id, &cluster_id);
            if let Some(cluster_row) = &cluster_row {
                let mut payload = cluster_row
                    .get("content_with_weight")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                let old_desc = payload
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let new_desc = llm_merge(chat, &old_desc, summary).await;
                if let Some(map) = payload.as_object_mut() {
                    map.insert("description".to_string(), Value::String(new_desc.clone()));
                }
                let mut row = cluster_row.clone();
                if let Some(map) = row.as_object_mut() {
                    map.insert(
                        "content_with_weight".to_string(),
                        Value::String(payload.to_string()),
                    );
                    let mut doc_ids = as_str_list(map.get("doc_ids_kwd"));
                    if !doc_ids.iter().any(|existing| existing == doc_id) {
                        doc_ids.push(doc_id.to_string());
                    }
                    map.insert("doc_ids_kwd".to_string(), serde_json::json!(doc_ids));
                    map.insert(
                        "doc_count_int".to_string(),
                        serde_json::json!(doc_ids.len()),
                    );
                }
                if embed.is_some() && new_desc != old_desc {
                    let new_emb = nav_embed(embed, &new_desc).await;
                    if !new_emb.is_empty() {
                        if let Some(map) = row.as_object_mut() {
                            map.insert(vec_field(new_emb.len()), serde_json::json!(new_emb));
                        }
                    }
                }
                store_upsert(store, tenant_id, kb_id, &row);
            }
            let depth = cluster_row
                .as_ref()
                .and_then(|row| row.get("depth_int"))
                .and_then(Value::as_i64)
                .map(|depth| depth + 1)
                .unwrap_or(2);
            let nav_doc_row = make_nav_doc_row(
                kb_id,
                doc_id,
                summary,
                best_name,
                depth,
                doc_embedding,
                graph_content,
            );
            store_upsert(store, tenant_id, kb_id, &nav_doc_row);
            maybe_split_cluster(store, embed, chat, tenant_id, kb_id, best_name).await;
            return Ok(());
        }
        if sim >= MIN_SIM {
            // Create a new cluster under the best cluster's parent.
            let parent_for_new = match &best_parent {
                Some(parent) if !parent.is_empty() => parent.clone(),
                _ => best_name.clone(),
            };
            let mut depth_of_parent = 1i64;
            let parent_row = store_get(
                store,
                tenant_id,
                kb_id,
                &nav_cluster_id(kb_id, &parent_for_new),
            );
            if let Some(row) = &parent_row {
                depth_of_parent = row.get("depth_int").and_then(Value::as_i64).unwrap_or(1);
            }
            let new_depth = depth_of_parent + 1;
            let (new_title, new_desc) = llm_create_summary(chat, &[summary.to_string()]).await;
            let new_name = readable_cluster_name(&new_title, summary);
            let new_cluster = make_nav_cluster_row(
                kb_id,
                &new_name,
                &new_desc,
                &parent_for_new,
                depth_of_parent,
                &[doc_id.to_string()],
                doc_embedding,
            );
            store_upsert(store, tenant_id, kb_id, &new_cluster);
            let nav_doc_row = make_nav_doc_row(
                kb_id,
                doc_id,
                summary,
                &new_name,
                new_depth,
                doc_embedding,
                graph_content,
            );
            store_upsert(store, tenant_id, kb_id, &nav_doc_row);
            return Ok(());
        }
    }

    // Root-level new cluster.
    let (root_title, new_desc) = llm_create_summary(chat, &[summary.to_string()]).await;
    let new_name = readable_cluster_name(&root_title, summary);
    let new_cluster = make_nav_cluster_row(
        kb_id,
        &new_name,
        &new_desc,
        "root",
        0,
        &[doc_id.to_string()],
        doc_embedding,
    );
    store_upsert(store, tenant_id, kb_id, &new_cluster);
    let nav_doc_row = make_nav_doc_row(
        kb_id,
        doc_id,
        summary,
        &new_name,
        1,
        doc_embedding,
        graph_content,
    );
    store_upsert(store, tenant_id, kb_id, &nav_doc_row);
    Ok(())
}

/// `remove_dataset_nav_doc`.
pub async fn remove_dataset_nav_doc(
    store: &dyn crate::doc_store::DocStore,
    lock: &dyn NavKbLock,
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
) {
    if doc_id.is_empty() || kb_id.is_empty() {
        return;
    }
    if !lock.acquire(kb_id) {
        return;
    }
    remove_dataset_nav_doc_locked(store, tenant_id, kb_id, doc_id);
    lock.release(kb_id);
}

/// `_remove_dataset_nav_doc_locked` (caller holds the KB nav lock).
fn remove_dataset_nav_doc_locked(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
) {
    let doc_row_id = nav_doc_id(doc_id);
    let Some(doc_row) = store_get(store, tenant_id, kb_id, &doc_row_id) else {
        return;
    };
    let parent_name = doc_row
        .get("parent_kwd")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    store_delete(store, tenant_id, kb_id, &doc_row_id);
    if !parent_name.is_empty() && parent_name != "root" {
        let cluster_id = nav_cluster_id(kb_id, &parent_name);
        if let Some(cluster_row) = store_get(store, tenant_id, kb_id, &cluster_id) {
            let mut row = cluster_row.clone();
            let mut doc_ids = as_str_list(row.get("doc_ids_kwd"));
            doc_ids.retain(|existing| existing != doc_id);
            if doc_ids.is_empty() {
                store_delete(store, tenant_id, kb_id, &cluster_id);
                let grandparent = cluster_row
                    .get("parent_kwd")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !grandparent.is_empty() && grandparent != "root" {
                    cleanup_empty_cluster(store, tenant_id, kb_id, &grandparent);
                }
            } else {
                if let Some(map) = row.as_object_mut() {
                    map.insert("doc_ids_kwd".to_string(), serde_json::json!(doc_ids));
                    map.insert(
                        "doc_count_int".to_string(),
                        serde_json::json!(doc_ids.len()),
                    );
                }
                store_upsert(store, tenant_id, kb_id, &row);
            }
        }
    }
}

/// `_cleanup_empty_cluster`: recursively drop empty clusters.
fn cleanup_empty_cluster(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    cluster_name: &str,
) {
    let cluster_id = nav_cluster_id(kb_id, cluster_name);
    let Some(cluster) = store_get(store, tenant_id, kb_id, &cluster_id) else {
        return;
    };
    let child_cond = serde_json::json!({
        "kb_id": [kb_id],
        "compile_kwd": [COMPILE_KWD],
        "parent_kwd": [cluster_name],
    });
    let children = store_search(
        store,
        tenant_id,
        kb_id,
        &child_cond,
        &["id".to_string()],
        100,
    );
    let doc_ids = as_str_list(cluster.get("doc_ids_kwd"));
    if children.is_empty() && doc_ids.is_empty() {
        store_delete(store, tenant_id, kb_id, &cluster_id);
        let grandparent = cluster
            .get("parent_kwd")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !grandparent.is_empty() && grandparent != "root" {
            cleanup_empty_cluster(store, tenant_id, kb_id, &grandparent);
        }
    }
}

/// `_maybe_split_cluster`: split a cluster that exceeds fanout/doc-count using
/// a lightweight 2-means pass over the children embeddings.
pub async fn maybe_split_cluster(
    store: &dyn crate::doc_store::DocStore,
    embed: Option<&dyn EmbeddingBackend>,
    chat: Option<&dyn crate::harness::HarnessChat>,
    tenant_id: &str,
    kb_id: &str,
    cluster_name: &str,
) {
    let child_cond = serde_json::json!({
        "kb_id": [kb_id],
        "compile_kwd": [COMPILE_KWD],
        "parent_kwd": [cluster_name],
    });
    let children = store_search(
        store,
        tenant_id,
        kb_id,
        &child_cond,
        &["id".to_string(), "name".to_string(), "type_kwd".to_string()],
        200,
    );
    if children.is_empty() {
        return;
    }
    let cluster_kids = children
        .iter()
        .filter(|child| child.get("type_kwd").and_then(Value::as_str) == Some("nav_cluster"))
        .count();
    let doc_kids = children
        .iter()
        .filter(|child| child.get("type_kwd").and_then(Value::as_str) == Some("nav_doc"))
        .count();
    let should_split = cluster_kids + doc_kids > MAX_FANOUT || doc_kids > MAX_DOCS_PER_CLUSTER;
    if !should_split {
        return;
    }
    // Discover the vector field from the first child that carries one.
    let child_details = store_search(
        store,
        tenant_id,
        kb_id,
        &child_cond,
        &[
            "id".to_string(),
            "name".to_string(),
            "type_kwd".to_string(),
            "content_with_weight".to_string(),
        ],
        200,
    );
    let mut vf = String::from("q_768_vec");
    'discover: for child in &child_details {
        if let Some(map) = child.as_object() {
            for key in map.keys() {
                if key.starts_with("q_") && key.ends_with("_vec") {
                    vf = key.clone();
                    break 'discover;
                }
            }
        }
    }
    let mut embeddings: Vec<Vec<f32>> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut name_to_type: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for child in &child_details {
        let stored = vector_of(child.get(&vf));
        let name = child
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !stored.is_empty() {
            embeddings.push(stored);
            names.push(name.clone());
        }
        if !name.is_empty() {
            name_to_type.insert(
                name,
                child
                    .get("type_kwd")
                    .and_then(Value::as_str)
                    .unwrap_or("nav_cluster")
                    .to_string(),
            );
        }
    }
    if embeddings.len() < 4 {
        return;
    }
    // 2-means with the first and middle embeddings as initial centroids.
    let mut centroids = vec![
        embeddings[0].clone(),
        embeddings[embeddings.len() / 2].clone(),
    ];
    let mut labels = vec![0usize; embeddings.len()];
    for _ in 0..10 {
        let mut groups: Vec<Vec<Vec<f32>>> = vec![Vec::new(), Vec::new()];
        for (index, emb) in embeddings.iter().enumerate() {
            let d0: f64 = emb
                .iter()
                .zip(centroids[0].iter())
                .map(|(a, b)| ((a - b) as f64).powi(2))
                .sum();
            let d1: f64 = emb
                .iter()
                .zip(centroids[1].iter())
                .map(|(a, b)| ((a - b) as f64).powi(2))
                .sum();
            let group = if d0 < d1 { 0 } else { 1 };
            labels[index] = group;
            groups[group].push(emb.clone());
        }
        for group in 0..2 {
            if groups[group].is_empty() {
                continue;
            }
            let mut avg = vec![0.0f32; centroids[group].len()];
            for emb in &groups[group] {
                for (slot, value) in avg.iter_mut().zip(emb.iter()) {
                    *slot += value / groups[group].len() as f32;
                }
            }
            centroids[group] = avg;
        }
    }
    let cluster_row = store_get(
        store,
        tenant_id,
        kb_id,
        &nav_cluster_id(kb_id, cluster_name),
    );
    let depth = cluster_row
        .as_ref()
        .and_then(|row| row.get("depth_int"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
        + 1;
    for group in 0..2 {
        let kid_names: Vec<String> = names
            .iter()
            .enumerate()
            .filter(|(index, _)| labels[*index] == group)
            .map(|(_, name)| name.clone())
            .collect();
        if kid_names.is_empty() {
            continue;
        }
        let mut doc_ids: Vec<String> = Vec::new();
        let mut descs: Vec<String> = Vec::new();
        for kid in &kid_names {
            let is_doc = name_to_type.get(kid).map(String::as_str) == Some("nav_doc");
            let cid = if is_doc {
                nav_doc_id(kid)
            } else {
                nav_cluster_id(kb_id, kid)
            };
            if let Some(row) = store_get(store, tenant_id, kb_id, &cid) {
                let payload = row
                    .get("content_with_weight")
                    .and_then(Value::as_str)
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                    .unwrap_or_else(|| serde_json::json!({}));
                descs.push(
                    payload
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
                for d in as_str_list(row.get("doc_ids_kwd")) {
                    if !doc_ids.iter().any(|existing| existing == &d) {
                        doc_ids.push(d);
                    }
                }
            }
        }
        let (group_title, group_desc) = if descs.is_empty() {
            let fallback = format!("Group {}", group + 1);
            (fallback.clone(), fallback)
        } else {
            llm_create_summary(chat, &descs).await
        };
        let group_name = readable_cluster_name(&group_title, &group_desc);
        let group_emb = nav_embed(embed, &group_desc).await;
        let new_cluster = make_nav_cluster_row(
            kb_id,
            &group_name,
            &group_desc,
            cluster_name,
            depth,
            &doc_ids,
            &group_emb,
        );
        store_upsert(store, tenant_id, kb_id, &new_cluster);
        for kid in &kid_names {
            let is_doc = name_to_type.get(kid).map(String::as_str) == Some("nav_doc");
            let cid = if is_doc {
                nav_doc_id(kid)
            } else {
                nav_cluster_id(kb_id, kid)
            };
            if let Some(mut row) = store_get(store, tenant_id, kb_id, &cid) {
                if let Some(map) = row.as_object_mut() {
                    map.insert("parent_kwd".to_string(), Value::String(group_name.clone()));
                    map.insert("depth_int".to_string(), serde_json::json!(depth + 1));
                }
                store_upsert(store, tenant_id, kb_id, &row);
            }
        }
    }
}

/// `_nav_text_score`: lexical coverage of the query terms over the node text.
pub fn nav_text_score(query: &str, row: &Value) -> f64 {
    let payload = row
        .get("content_with_weight")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let keywords = as_str_list(payload.get("keywords"));
    let entities = as_str_list(payload.get("entities"));
    let graph_content = payload
        .get("graph_content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut parts: Vec<String> = vec![
        row.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        payload
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        graph_content,
    ];
    parts.extend(keywords);
    parts.extend(entities);
    let haystack = parts.join(" ").to_lowercase();
    let token_re = regex::Regex::new(r"\w+").expect("word regex");
    let lowered = query.to_lowercase();
    let mut terms: Vec<String> = Vec::new();
    for hit in token_re.find_iter(&lowered) {
        let term = hit.as_str().to_string();
        if !terms.iter().any(|existing| existing == &term) {
            terms.push(term);
        }
    }
    if terms.is_empty() {
        return 0.0;
    }
    let hits = terms
        .iter()
        .filter(|term| haystack.contains(term.as_str()))
        .count();
    hits as f64 / terms.len() as f64
}

/// `_hybrid_fuse`: fuse KNN and text rows into a scored, deduplicated,
/// sorted list (adds `_score` / `_text_score` to the returned rows).
pub fn hybrid_fuse(
    vec: &[f32],
    vf: &str,
    query: &str,
    knn_rows: &[Value],
    text_rows: &[Value],
    dense_w: f64,
    top_k: usize,
) -> Vec<Value> {
    let text_w = 1.0 - dense_w;
    let mut fused: Vec<(String, Value, f64, f64)> = Vec::new(); // (key, row, score, text_score)
    for row in knn_rows {
        let key_name = row
            .get("name")
            .or_else(|| row.get("doc_id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if key_name.is_empty() {
            continue;
        }
        let score = cosine_sim(vec, &vector_of(row.get(vf))) * dense_w;
        fused.push((format!("knn:{key_name}"), row.clone(), score, 0.0));
    }
    for row in text_rows {
        let key_name = row
            .get("name")
            .or_else(|| row.get("doc_id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if key_name.is_empty() {
            continue;
        }
        let ts = nav_text_score(query, row);
        if ts <= 0.0 {
            continue;
        }
        if let Some(entry) = fused
            .iter_mut()
            .find(|(key, _, _, _)| key == &format!("knn:{key_name}"))
        {
            entry.2 += text_w * ts;
            entry.3 += ts;
        } else {
            fused.push((format!("text:{key_name}"), row.clone(), text_w * ts, ts));
        }
    }
    let mut rows_with_scores: Vec<(Value, f64, f64)> = fused
        .into_iter()
        .filter(|(_, _, score, _)| *score > 0.0)
        .map(|(_, row, score, text_score)| (row, score, text_score))
        .collect();
    rows_with_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    rows_with_scores.truncate(top_k);
    rows_with_scores
        .into_iter()
        .map(|(mut row, score, text_score)| {
            if let Some(map) = row.as_object_mut() {
                map.insert("_score".to_string(), serde_json::json!(score));
                map.insert("_text_score".to_string(), serde_json::json!(text_score));
            }
            row
        })
        .collect()
}

/// `_lexical_cluster_hits`: clusters whose summary carries a query term.
#[allow(clippy::too_many_arguments)]
pub fn lexical_cluster_hits(
    store: &dyn crate::doc_store::DocStore,
    tenant_id: &str,
    kb_id: &str,
    query: &str,
    vec: &[f32],
    vf: &str,
    dense_w: f64,
    fields: &[String],
    allowed_docs: &std::collections::HashSet<String>,
    limit: usize,
) -> Vec<Value> {
    let mut condition = serde_json::Map::new();
    condition.insert("type_kwd".to_string(), serde_json::json!(["nav_cluster"]));
    condition.insert("kb_id".to_string(), serde_json::json!([kb_id]));
    if !allowed_docs.is_empty() {
        let mut sorted: Vec<String> = allowed_docs.iter().cloned().collect();
        sorted.sort();
        condition.insert("doc_ids_kwd".to_string(), serde_json::json!(sorted));
    }
    let rows = store_text_search(
        store,
        tenant_id,
        kb_id,
        query,
        fields,
        limit,
        COMPILE_KWD,
        "",
        Some(&Value::Object(condition)),
    );
    let mut hits: Vec<Value> = Vec::new();
    for row in rows {
        let scope = if allowed_docs.is_empty() {
            None
        } else {
            Some(allowed_docs)
        };
        if !in_nav_scope(&row, scope) {
            continue;
        }
        let text_score = nav_text_score(query, &row);
        if text_score <= 0.0 {
            continue;
        }
        let score =
            dense_w * cosine_sim(vec, &vector_of(row.get(vf))) + (1.0 - dense_w) * text_score;
        if score < NAV_TREE_MIN_SCORE {
            continue;
        }
        if let Some(map) = row.clone().as_object_mut() {
            map.insert("_score".to_string(), serde_json::json!(score));
            map.insert("_text_score".to_string(), serde_json::json!(text_score));
        }
        let mut row = row;
        if let Some(map) = row.as_object_mut() {
            map.insert("_score".to_string(), serde_json::json!(score));
            map.insert("_text_score".to_string(), serde_json::json!(text_score));
        }
        hits.push(row);
    }
    hits.sort_by(|a, b| {
        let left = a.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
        let right = b.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
        right
            .partial_cmp(&left)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits
}

fn scope_set(doc_scope: Option<&[String]>) -> std::collections::HashSet<String> {
    doc_scope
        .unwrap_or(&[])
        .iter()
        .map(|doc| doc.trim().to_string())
        .filter(|doc| !doc.is_empty())
        .collect()
}

/// `search_dataset_nav`: flat hybrid search over all nav nodes for one KB.
#[allow(clippy::too_many_arguments)]
pub async fn search_dataset_nav(
    store: &dyn crate::doc_store::DocStore,
    embed: Option<&dyn EmbeddingBackend>,
    tenant_id: &str,
    kb_id: &str,
    query: &str,
    top_k: Option<usize>,
    type_kwd: &str,
    compile_kwd: &str,
    doc_scope: Option<&[String]>,
) -> Vec<Value> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let allowed_docs = scope_set(doc_scope);
    let mut condition = serde_json::Map::new();
    condition.insert("compile_kwd".to_string(), serde_json::json!([compile_kwd]));
    if !type_kwd.is_empty() {
        condition.insert("type_kwd".to_string(), serde_json::json!([type_kwd]));
    }
    if !allowed_docs.is_empty() {
        let mut sorted: Vec<String> = allowed_docs.iter().cloned().collect();
        sorted.sort();
        if type_kwd == "nav_doc" {
            condition.insert("doc_id".to_string(), serde_json::json!(sorted));
        } else if type_kwd == "nav_cluster" {
            condition.insert("doc_ids_kwd".to_string(), serde_json::json!(sorted));
        }
    }
    let dense_w = if embed.is_some() {
        NAV_HYBRID_DENSE_W
    } else {
        0.0
    };
    let mut fused: Vec<(String, Value, f64, f64)> = Vec::new(); // (key, row, score, text_score)
    if let Some(embed) = embed {
        let vec = nav_embed(Some(embed), query).await;
        if !vec.is_empty() {
            let rows = store_knn(
                store,
                tenant_id,
                kb_id,
                &vec,
                vec.len(),
                &Value::Object(condition.clone()),
                top_k.unwrap_or(10000),
            );
            let vf = vec_field(vec.len());
            for row in rows {
                let key_name = row
                    .get("name")
                    .or_else(|| row.get("doc_id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if key_name.is_empty() {
                    continue;
                }
                let score = dense_w * cosine_sim(&vec, &vector_of(row.get(&vf)));
                match fused.iter_mut().find(|(key, _, _, _)| key == &key_name) {
                    Some(entry) => entry.2 += score,
                    None => fused.push((key_name, row.clone(), score, 0.0)),
                }
            }
        }
    }
    let text_w = 1.0 - dense_w;
    let text_filter = if allowed_docs.is_empty() {
        None
    } else if type_kwd == "nav_doc" {
        let mut sorted: Vec<String> = allowed_docs.iter().cloned().collect();
        sorted.sort();
        Some(serde_json::json!({"doc_id": sorted}))
    } else if type_kwd == "nav_cluster" {
        let mut sorted: Vec<String> = allowed_docs.iter().cloned().collect();
        sorted.sort();
        Some(serde_json::json!({"doc_ids_kwd": sorted}))
    } else {
        None
    };
    let text_limit = if top_k.unwrap_or(0) > 0 {
        (top_k.unwrap_or(0) * 3).max(20)
    } else {
        10000
    };
    let fields: Vec<String> = NAV_SEARCH_FIELDS
        .iter()
        .map(|field| field.to_string())
        .collect();
    let text_rows = store_text_search(
        store,
        tenant_id,
        kb_id,
        query,
        &fields,
        text_limit,
        compile_kwd,
        type_kwd,
        text_filter.as_ref(),
    );
    for row in text_rows {
        let key_name = row
            .get("name")
            .or_else(|| row.get("doc_id"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if key_name.is_empty() {
            continue;
        }
        let ts = nav_text_score(query, &row);
        if ts <= 0.0 {
            continue;
        }
        match fused.iter_mut().find(|(key, _, _, _)| key == &key_name) {
            Some(entry) => {
                entry.2 += text_w * ts;
                entry.3 += ts;
            }
            None => fused.push((key_name, row.clone(), text_w * ts, ts)),
        }
    }
    let scope = if allowed_docs.is_empty() {
        None
    } else {
        Some(&allowed_docs)
    };
    let mut rows_with_scores: Vec<(Value, f64)> = fused
        .into_iter()
        .filter(|(_, row, score, text_score)| {
            *score > 0.0 && *text_score > 0.0 && in_nav_scope(row, scope)
        })
        .map(|(_, row, score, _)| (row, score))
        .collect();
    rows_with_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    if let Some(limit) = top_k {
        rows_with_scores.truncate(limit);
    }
    let mut out: Vec<Value> = Vec::new();
    for (row, score) in rows_with_scores {
        let payload = row
            .get("content_with_weight")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let typ = payload
            .get("type")
            .and_then(Value::as_str)
            .or_else(|| row.get("type_kwd").and_then(Value::as_str))
            .map(str::to_string)
            .unwrap_or_else(|| {
                if row.get("doc_ids_kwd").is_some() {
                    "nav_cluster".to_string()
                } else {
                    "nav_doc".to_string()
                }
            });
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let (doc_id, mut doc_ids, scoped_cluster) = if typ == "nav_cluster" {
            let mut doc_ids = as_str_list(row.get("doc_ids_kwd"));
            let scoped = !allowed_docs.is_empty();
            if scoped {
                doc_ids.retain(|doc| allowed_docs.contains(doc));
            }
            (None, doc_ids, scoped)
        } else {
            let doc_id = row
                .get("doc_id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| name.clone());
            let doc_ids = if doc_id.is_empty() {
                Vec::new()
            } else {
                vec![doc_id.clone()]
            };
            (Some(doc_id), doc_ids, false)
        };
        let doc_count = if typ == "nav_cluster" && scoped_cluster {
            doc_ids.len()
        } else {
            row.get("doc_count_int")
                .and_then(Value::as_i64)
                .map(|value| value.max(0) as usize)
                .unwrap_or(doc_ids.len())
        };
        doc_ids.shrink_to_fit();
        out.push(serde_json::json!({
            "type": typ,
            "doc_id": doc_id,
            "doc_ids": doc_ids,
            "name": name,
            "description": payload.get("description").and_then(Value::as_str).unwrap_or(""),
            "keywords": as_str_list(payload.get("keywords")),
            "entities": as_str_list(payload.get("entities")),
            "graph_content": payload.get("graph_content").and_then(Value::as_str).unwrap_or(""),
            "doc_title": payload.get("doc_title").and_then(Value::as_str).unwrap_or(""),
            "source_type": payload.get("source_type").and_then(Value::as_str).unwrap_or(""),
            "doc_count": doc_count,
            "parent_kwd": as_str_list(row.get("parent_kwd")),
            "score": score,
        }));
    }
    out
}

/// `search_nav_tree_descent`: tree-structured hybrid descent (BFS with beam
/// pruning), returning `{doc_id, score}` items.
pub async fn search_nav_tree_descent(
    store: &dyn crate::doc_store::DocStore,
    embed: Option<&dyn EmbeddingBackend>,
    tenant_id: &str,
    kb_id: &str,
    query: &str,
    top_k: Option<usize>,
    doc_scope: Option<&[String]>,
) -> Vec<Value> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    let allowed_docs = scope_set(doc_scope);
    let Some(embed) = embed else {
        let raw = search_dataset_nav(
            store,
            None,
            tenant_id,
            kb_id,
            query,
            top_k,
            "nav_doc",
            COMPILE_KWD,
            doc_scope,
        )
        .await;
        return raw
            .into_iter()
            .filter(|item| {
                item.get("doc_id")
                    .and_then(Value::as_str)
                    .map(|id| !id.is_empty())
                    .unwrap_or(false)
            })
            .map(|item| {
                serde_json::json!({
                    "doc_id": item.get("doc_id").and_then(Value::as_str).unwrap_or(""),
                    "score": item.get("score").and_then(Value::as_f64).unwrap_or(0.0),
                })
            })
            .collect();
    };
    let vec = nav_embed(Some(embed), query).await;
    let vec_dim = vec.len();
    if vec_dim == 0 {
        return Vec::new();
    }
    let beam_width = 5usize;
    let dense_w = NAV_HYBRID_DENSE_W;
    let vf = vec_field(vec_dim);
    let fields: Vec<String> = [
        "content_with_weight",
        "name",
        "doc_id",
        "compile_kwd",
        "type_kwd",
        "parent_kwd",
        "depth_int",
        "doc_count_int",
        "doc_ids_kwd",
        vf.as_str(),
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let mut collected: Vec<Value> = Vec::new();
    let mut seen_docs: Vec<String> = Vec::new();
    let mut seen_nodes: Vec<String> = Vec::new();
    let root_cond = {
        let mut condition = serde_json::Map::new();
        condition.insert("kb_id".to_string(), serde_json::json!([kb_id]));
        condition.insert("compile_kwd".to_string(), serde_json::json!([COMPILE_KWD]));
        condition.insert("type_kwd".to_string(), serde_json::json!(["nav_cluster"]));
        condition.insert("depth_int".to_string(), serde_json::json!([0]));
        if !allowed_docs.is_empty() {
            let mut sorted: Vec<String> = allowed_docs.iter().cloned().collect();
            sorted.sort();
            condition.insert("doc_ids_kwd".to_string(), serde_json::json!(sorted));
        }
        Value::Object(condition)
    };
    let root_scope = if allowed_docs.is_empty() {
        None
    } else {
        Some(&allowed_docs)
    };
    let roots_knn: Vec<Value> = store_knn(
        store,
        tenant_id,
        kb_id,
        &vec,
        vec_dim,
        &root_cond,
        beam_width * 3,
    )
    .into_iter()
    .filter(|row| in_nav_scope(row, root_scope))
    .collect();
    let mut current_level: Vec<Value>;
    if roots_knn.is_empty() {
        let all_cond = {
            let mut condition = serde_json::Map::new();
            condition.insert("kb_id".to_string(), serde_json::json!([kb_id]));
            condition.insert("compile_kwd".to_string(), serde_json::json!([COMPILE_KWD]));
            condition.insert("type_kwd".to_string(), serde_json::json!(["nav_cluster"]));
            if !allowed_docs.is_empty() {
                let mut sorted: Vec<String> = allowed_docs.iter().cloned().collect();
                sorted.sort();
                condition.insert("doc_ids_kwd".to_string(), serde_json::json!(sorted));
            }
            Value::Object(condition)
        };
        let all_clusters: Vec<Value> =
            store_search(store, tenant_id, kb_id, &all_cond, &fields, 10000)
                .into_iter()
                .filter(|row| in_nav_scope(row, root_scope))
                .collect();
        if all_clusters.is_empty() {
            return Vec::new();
        }
        let min_depth = all_clusters
            .iter()
            .filter_map(|row| row.get("depth_int").and_then(Value::as_i64))
            .min()
            .unwrap_or(0);
        let mut starters: Vec<Value> = all_clusters
            .iter()
            .filter(|row| row.get("depth_int").and_then(Value::as_i64) == Some(min_depth))
            .cloned()
            .collect();
        starters.sort_by(|a, b| {
            let left = cosine_sim(&vec, &vector_of(a.get(&vf)));
            let right = cosine_sim(&vec, &vector_of(b.get(&vf)));
            right
                .partial_cmp(&left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        current_level = starters.into_iter().take(beam_width).collect();
        for row in current_level.iter_mut() {
            let score = cosine_sim(&vec, &vector_of(row.get(&vf)));
            if let Some(map) = row.as_object_mut() {
                map.insert("_score".to_string(), serde_json::json!(score));
            }
        }
    } else {
        let mut roots = roots_knn;
        for row in roots.iter_mut() {
            let score = cosine_sim(&vec, &vector_of(row.get(&vf)));
            if let Some(map) = row.as_object_mut() {
                map.insert("_score".to_string(), serde_json::json!(score));
            }
        }
        roots.sort_by(|a, b| {
            let left = a.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
            let right = b.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
            right
                .partial_cmp(&left)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        current_level = roots.into_iter().take(beam_width).collect();
    }
    if current_level.is_empty() {
        return Vec::new();
    }
    let lexical_hits = lexical_cluster_hits(
        store,
        tenant_id,
        kb_id,
        query,
        &vec,
        &vf,
        dense_w,
        &fields,
        &allowed_docs,
        (top_k.unwrap_or(0) * 3).max(20),
    );
    for hit in lexical_hits.iter().take(beam_width) {
        let name = hit.get("name").and_then(Value::as_str).unwrap_or("");
        match current_level
            .iter_mut()
            .find(|row| row.get("name").and_then(Value::as_str) == Some(name))
        {
            Some(existing) => {
                let existing_text = existing
                    .get("_text_score")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let hit_text = hit
                    .get("_text_score")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let existing_score = existing
                    .get("_score")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                let hit_score = hit.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
                if let Some(map) = existing.as_object_mut() {
                    map.insert(
                        "_text_score".to_string(),
                        serde_json::json!(existing_text.max(hit_text)),
                    );
                    map.insert(
                        "_score".to_string(),
                        serde_json::json!(existing_score.max(hit_score)),
                    );
                }
            }
            None => current_level.push(hit.clone()),
        }
    }
    while !current_level.is_empty() && top_k.map(|limit| collected.len() < limit).unwrap_or(true) {
        let mut next_level: Vec<Value> = Vec::new();
        for node in &current_level {
            let node_name = node
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if seen_nodes.iter().any(|seen| seen == &node_name) {
                continue;
            }
            seen_nodes.push(node_name.clone());
            let parent_score = node.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
            let lexical_support = node
                .get("_lex")
                .and_then(Value::as_f64)
                .or_else(|| node.get("_text_score").and_then(Value::as_f64))
                .unwrap_or(0.0);
            let mut child_cond = serde_json::Map::new();
            child_cond.insert("kb_id".to_string(), serde_json::json!([kb_id]));
            child_cond.insert("compile_kwd".to_string(), serde_json::json!([COMPILE_KWD]));
            child_cond.insert("parent_kwd".to_string(), serde_json::json!([node_name]));
            let (children_knn, children_text) = if !allowed_docs.is_empty() {
                let mut sorted: Vec<String> = allowed_docs.iter().cloned().collect();
                sorted.sort();
                let mut doc_cond = child_cond.clone();
                doc_cond.insert("type_kwd".to_string(), serde_json::json!(["nav_doc"]));
                doc_cond.insert("doc_id".to_string(), serde_json::json!(sorted));
                let mut cluster_cond = child_cond.clone();
                cluster_cond.insert("type_kwd".to_string(), serde_json::json!(["nav_cluster"]));
                cluster_cond.insert("doc_ids_kwd".to_string(), serde_json::json!(sorted));
                let mut knn = store_knn(
                    store,
                    tenant_id,
                    kb_id,
                    &vec,
                    vec_dim,
                    &Value::Object(doc_cond.clone()),
                    beam_width * 3,
                );
                knn.extend(store_knn(
                    store,
                    tenant_id,
                    kb_id,
                    &vec,
                    vec_dim,
                    &Value::Object(cluster_cond.clone()),
                    beam_width * 3,
                ));
                let knn: Vec<Value> = knn
                    .into_iter()
                    .filter(|row| in_nav_scope(row, root_scope))
                    .collect();
                let mut text = store_text_search(
                    store,
                    tenant_id,
                    kb_id,
                    query,
                    &fields,
                    beam_width * 3,
                    COMPILE_KWD,
                    "",
                    Some(
                        &serde_json::json!({"parent_kwd": [node_name], "kb_id": [kb_id], "doc_id": sorted}),
                    ),
                );
                text.extend(store_text_search(store, tenant_id, kb_id, query, &fields, beam_width * 3, COMPILE_KWD, "", Some(&serde_json::json!({"parent_kwd": [node_name], "kb_id": [kb_id], "doc_ids_kwd": sorted}))));
                let text: Vec<Value> = text
                    .into_iter()
                    .filter(|row| in_nav_scope(row, root_scope))
                    .collect();
                (knn, text)
            } else {
                let knn = store_knn(
                    store,
                    tenant_id,
                    kb_id,
                    &vec,
                    vec_dim,
                    &Value::Object(child_cond.clone()),
                    beam_width * 3,
                );
                let text = store_text_search(
                    store,
                    tenant_id,
                    kb_id,
                    query,
                    &fields,
                    beam_width * 3,
                    COMPILE_KWD,
                    "",
                    Some(&serde_json::json!({"parent_kwd": [node_name], "kb_id": [kb_id]})),
                );
                (knn, text)
            };
            let candidates = hybrid_fuse(
                &vec,
                &vf,
                query,
                &children_knn,
                &children_text,
                dense_w,
                beam_width,
            );
            for candidate in candidates {
                if top_k.map(|limit| collected.len() >= limit).unwrap_or(false) {
                    break;
                }
                let lexical = candidate
                    .get("_text_score")
                    .and_then(Value::as_f64)
                    .filter(|value| *value > 0.0)
                    .unwrap_or(if lexical_support > 0.0 {
                        lexical_support
                    } else {
                        0.0
                    });
                if candidate.get("type_kwd").and_then(Value::as_str) == Some("nav_doc") {
                    let doc_id = candidate
                        .get("doc_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let score = candidate
                        .get("_score")
                        .and_then(Value::as_f64)
                        .unwrap_or(parent_score);
                    if doc_id.is_empty() || seen_docs.iter().any(|seen| seen == &doc_id) {
                        continue;
                    }
                    if !allowed_docs.is_empty() && !allowed_docs.contains(&doc_id) {
                        continue;
                    }
                    if lexical <= 0.0 || score < NAV_TREE_MIN_SCORE {
                        continue;
                    }
                    seen_docs.push(doc_id.clone());
                    let rounded = (score * 10000.0).round() / 10000.0;
                    collected.push(serde_json::json!({"doc_id": doc_id, "score": rounded}));
                } else {
                    let mut candidate = candidate;
                    if let Some(map) = candidate.as_object_mut() {
                        map.insert("_lex".to_string(), serde_json::json!(lexical));
                    }
                    next_level.push(candidate);
                }
            }
            if top_k.map(|limit| collected.len() >= limit).unwrap_or(false) {
                break;
            }
        }
        if top_k.map(|limit| collected.len() >= limit).unwrap_or(false) {
            break;
        }
        next_level.sort_by(|a, b| {
            let left_lex = a.get("_lex").and_then(Value::as_f64).unwrap_or(0.0) > 0.0;
            let right_lex = b.get("_lex").and_then(Value::as_f64).unwrap_or(0.0) > 0.0;
            let left_score = a.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
            let right_score = b.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
            right_lex.cmp(&left_lex).then(
                right_score
                    .partial_cmp(&left_score)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        current_level = next_level.into_iter().take(beam_width).collect();
    }
    if collected.is_empty() && !lexical_hits.is_empty() {
        'outer: for node in &lexical_hits {
            for did in as_str_list(node.get("doc_ids_kwd")) {
                if top_k.map(|limit| collected.len() >= limit).unwrap_or(false) {
                    break 'outer;
                }
                if seen_docs.iter().any(|seen| seen == &did) {
                    continue;
                }
                if !allowed_docs.is_empty() && !allowed_docs.contains(&did) {
                    continue;
                }
                seen_docs.push(did.clone());
                let score = node.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
                let rounded = (score * 10000.0).round() / 10000.0;
                collected.push(serde_json::json!({"doc_id": did, "score": rounded}));
            }
        }
    }
    collected
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
#[cfg(test)]
mod dataset_nav_part3_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    struct MockChat {
        reply: String,
    }

    #[async_trait]
    impl crate::harness::HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen_conf: &Value,
        ) -> std::result::Result<String, String> {
            Ok(self.reply.clone())
        }
        fn max_length(&self) -> usize {
            4096
        }
    }

    #[derive(Default)]
    struct MockStore {
        rows: Vec<Value>,
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
            _r: &[crate::doc_store::DocRow],
            _i: &str,
            _d: &str,
        ) -> crate::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn get(
            &self,
            _a: &str,
            _i: &str,
            _d: &[String],
        ) -> crate::Result<Option<crate::doc_store::DocRow>> {
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
            query: &crate::doc_store::SearchQuery,
        ) -> crate::Result<crate::doc_store::SearchResponse> {
            // Honour parent_kwd filters so the descent test can walk children.
            let parent = query
                .condition
                .get("parent_kwd")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(Value::as_str)
                .map(str::to_string);
            let docs: Vec<crate::doc_store::DocRow> = self
                .rows
                .iter()
                .filter(|row| match &parent {
                    None => true,
                    Some(parent) => {
                        row.get("parent_kwd").and_then(Value::as_str) == Some(parent.as_str())
                    }
                })
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
    fn title_helpers() {
        assert_eq!(clean_title("  a   b\n c  "), "a b c");
        let long = "x".repeat(60);
        assert_eq!(clean_title(&long).len(), 48);
        assert_eq!(
            fallback_title("one two three four five six seven"),
            "one two three four five six"
        );
        assert_eq!(fallback_title("   "), "Cluster");
        let name = readable_cluster_name("My Title", "seed");
        assert!(name.starts_with("My Title "), "{name}");
        assert_eq!(name.len(), "My Title ".len() + 8);
        assert_eq!(readable_cluster_name("", "seed"), {
            let digest = xxhash_rust::xxh3::xxh3_64(b"seed");
            format!("Cluster {:08x}", (digest & 0xffff_ffff) as u32).replace(
                &format!("{:08x}", (digest & 0xffff_ffff) as u32),
                &format!("{digest:016x}")[..8],
            )
        });
    }

    #[tokio::test]
    async fn llm_merge_and_summary_paths() {
        let chat = MockChat {
            reply: json!({"merged": "merged text"}).to_string(),
        };
        let merged = llm_merge(Some(&chat), "old", "new").await;
        assert_eq!(merged, "merged text");
        let no_chat = llm_merge(None, "old", "new").await;
        assert_eq!(no_chat, "old");

        let summary_chat = MockChat {
            reply: json!({"name": "Topic Name", "summary": "about things"}).to_string(),
        };
        let (name, summary) =
            llm_create_summary(Some(&summary_chat), &["doc one".to_string()]).await;
        assert_eq!(name, "Topic Name");
        assert_eq!(summary, "about things");
        let (fallback_name, fallback_summary) =
            llm_create_summary(None, &["doc one".to_string()]).await;
        assert_eq!(fallback_name, "doc one");
        assert_eq!(fallback_summary, "doc one");
    }

    #[test]
    fn find_best_cluster_descends_children() {
        let rows = vec![
            json!({"id": "r-root", "name": "root", "parent_kwd": "", "type_kwd": "nav_cluster", "depth_int": 0, "kb_id": "kb1", "compile_kwd": "dataset_nav", "q_2_vec": [1.0, 0.0]}),
            json!({"id": "r-childA", "name": "childA", "parent_kwd": "root", "type_kwd": "nav_cluster", "depth_int": 1, "kb_id": "kb1", "compile_kwd": "dataset_nav", "q_2_vec": [1.0, 0.0]}),
        ];
        let store = MockStore { rows };
        let (name, parent, sim) = find_best_cluster(&store, "t1", "kb1", &[1.0, 0.0], 2);
        assert_eq!(name.as_deref(), Some("childA"));
        assert_eq!(parent.as_deref(), Some("root"));
        assert!(sim >= RECURSE_THRESHOLD);

        let empty = MockStore::default();
        let (name, _, sim) = find_best_cluster(&empty, "t1", "kb1", &[1.0, 0.0], 2);
        assert!(name.is_none());
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn graph_text_uses_descriptions_without_redundant_prefix() {
        let graph = json!({
            "entities": [
                {"name": "Root", "description": "Root desc\nsecond line"},
                {"name": "Child", "description": "Child desc"},
            ],
            "relations": [{"from": "Root", "to": "Child"}],
        });
        let (title, text) = build_nav_graph_text(&graph);
        assert_eq!(title, "Root desc");
        assert!(text.starts_with("Root desc\nsecond line\n\n"));
        assert!(text.contains("Child desc"));
        assert!(
            !text.contains("Root: Root desc"),
            "no redundant name prefix"
        );
        let (_, empty) = build_nav_graph_text(&json!(null));
        assert!(empty.is_empty());
    }

    #[test]
    fn store_knn_falls_back_when_filters_ignored() {
        // The mock ignores conditions; the fallback scan re-checks and sorts.
        struct IgnoreFilterStore {
            rows: Mutex<Vec<Value>>,
        }
        impl crate::doc_store::DocStore for IgnoreFilterStore {
            fn db_type(&self) -> &'static str {
                "memory"
            }
            fn health(&self) -> crate::Result<crate::doc_store::HealthStatus> {
                Ok(crate::doc_store::HealthStatus::green("t"))
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
                _r: &[crate::doc_store::DocRow],
                _i: &str,
                _d: &str,
            ) -> crate::Result<Vec<String>> {
                Ok(Vec::new())
            }
            fn get(
                &self,
                _a: &str,
                _i: &str,
                _d: &[String],
            ) -> crate::Result<Option<crate::doc_store::DocRow>> {
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
                    .lock()
                    .unwrap()
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
        let store = IgnoreFilterStore {
            rows: Mutex::new(vec![
                json!({"id": "r-a", "type_kwd": "nav_cluster", "name": "A", "kb_id": "kb1", "compile_kwd": "dataset_nav", "q_2_vec": [1.0, 0.0]}),
                json!({"id": "r-b", "type_kwd": "nav_doc", "name": "B", "kb_id": "kb1", "compile_kwd": "dataset_nav", "q_2_vec": [0.0, 1.0]}),
            ]),
        };
        let rows = store_knn(
            &store,
            "t1",
            "kb1",
            &[1.0, 0.0],
            2,
            &json!({"type_kwd": ["nav_cluster"]}),
            1,
        );
        assert_eq!(rows.len(), 1, "fallback scan re-checked the filter");
        assert_eq!(rows[0]["name"], json!("A"));
    }
}
#[cfg(test)]
mod dataset_nav_part3b_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    struct OpenLock;

    impl NavKbLock for OpenLock {
        fn acquire(&self, _kb_id: &str) -> bool {
            true
        }
        fn release(&self, _kb_id: &str) {}
    }

    struct MockChat;

    #[async_trait]
    impl crate::harness::HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen_conf: &Value,
        ) -> std::result::Result<String, String> {
            Ok(json!({"name": "Topic", "summary": "Merged summary"}).to_string())
        }
        fn max_length(&self) -> usize {
            4096
        }
    }

    struct MockEmbed;

    #[async_trait]
    impl EmbeddingBackend for MockEmbed {
        async fn encode(&self, texts: &[String]) -> std::result::Result<Vec<Vec<f32>>, String> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[derive(Default)]
    struct MockStore {
        rows: Mutex<std::collections::HashMap<String, Value>>,
        writes: Mutex<Vec<Value>>,
        deletes: Mutex<Vec<String>>,
    }

    impl MockStore {
        fn seed(&self, row: Value) {
            let id = row
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.rows.lock().unwrap().insert(id, row);
        }
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
            rows: &[crate::doc_store::DocRow],
            _i: &str,
            _d: &str,
        ) -> crate::Result<Vec<String>> {
            for row in rows {
                let value = Value::Object(row.clone());
                if let Some(id) = row.get("id").and_then(Value::as_str) {
                    self.rows
                        .lock()
                        .unwrap()
                        .insert(id.to_string(), value.clone());
                }
                self.writes.lock().unwrap().push(value);
            }
            Ok(Vec::new())
        }
        fn get(
            &self,
            data_id: &str,
            _i: &str,
            _d: &[String],
        ) -> crate::Result<Option<crate::doc_store::DocRow>> {
            Ok(self
                .rows
                .lock()
                .unwrap()
                .get(data_id)
                .and_then(|row| row.as_object().cloned()))
        }
        fn update(
            &self,
            condition: &crate::doc_store::FilterCondition,
            new_value: &crate::doc_store::DocRow,
            _i: &str,
            _d: &str,
        ) -> crate::Result<bool> {
            let id = condition.get("id").and_then(Value::as_str).unwrap_or("");
            let mut rows = self.rows.lock().unwrap();
            if let Some(row) = rows.get_mut(id) {
                if let Some(map) = row.as_object_mut() {
                    for (key, value) in new_value {
                        map.insert(key.clone(), value.clone());
                    }
                }
                return Ok(true);
            }
            Ok(false)
        }
        fn delete(
            &self,
            condition: &crate::doc_store::FilterCondition,
            _i: &str,
            _d: &str,
        ) -> crate::Result<usize> {
            let ids = as_str_list(condition.get("id"));
            let mut removed = 0;
            for id in ids {
                if self.rows.lock().unwrap().remove(&id).is_some() {
                    removed += 1;
                }
                self.deletes.lock().unwrap().push(id);
            }
            Ok(removed)
        }
        fn search(
            &self,
            query: &crate::doc_store::SearchQuery,
        ) -> crate::Result<crate::doc_store::SearchResponse> {
            let parent = query
                .condition
                .get("parent_kwd")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(Value::as_str)
                .map(str::to_string);
            let docs: Vec<crate::doc_store::DocRow> = self
                .rows
                .lock()
                .unwrap()
                .values()
                .filter(|row| match &parent {
                    None => true,
                    Some(parent) => {
                        row.get("parent_kwd").and_then(Value::as_str) == Some(parent.as_str())
                    }
                })
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

    fn cluster_row(name: &str, parent: &str, depth: i64, doc_ids: &[&str]) -> Value {
        json!({
            "id": nav_cluster_id("kb1", name),
            "name": name,
            "parent_kwd": parent,
            "depth_int": depth,
            "type_kwd": "nav_cluster",
            "kb_id": "kb1",
            "compile_kwd": "dataset_nav",
            "doc_ids_kwd": doc_ids,
            "doc_count_int": doc_ids.len(),
            "q_2_vec": [1.0, 0.0],
            "content_with_weight": json!({"type": "nav_cluster", "description": "old desc"}).to_string(),
        })
    }

    #[tokio::test]
    async fn upsert_merges_into_nearest_cluster() {
        let store = MockStore::default();
        store.seed(cluster_row("Topic", "root", 0, &[]));
        upsert_dataset_nav_doc(
            &store,
            Some(&MockEmbed),
            Some(&MockChat),
            &OpenLock,
            "t1",
            "kb1",
            "d1",
            &json!("brand new summary"),
        )
        .await;
        let writes = store.writes.lock().unwrap();
        assert!(
            writes
                .iter()
                .any(|row| row.get("type_kwd").and_then(Value::as_str) == Some("nav_doc")),
            "nav_doc upserted"
        );
        assert!(
            !store
                .deletes
                .lock()
                .unwrap()
                .contains(&nav_cluster_id("kb1", "Topic")),
            "cluster kept"
        );
        let doc_row = writes
            .iter()
            .find(|row| row.get("type_kwd").and_then(Value::as_str) == Some("nav_doc"))
            .unwrap();
        assert_eq!(doc_row["parent_kwd"], json!("Topic"));
        assert_eq!(doc_row["depth_int"], json!(1));
    }

    #[tokio::test]
    async fn upsert_seeds_root_cluster_when_no_similar() {
        let store = MockStore::default();
        upsert_dataset_nav_doc(
            &store,
            Some(&MockEmbed),
            Some(&MockChat),
            &OpenLock,
            "t1",
            "kb1",
            "d9",
            &json!("fresh doc"),
        )
        .await;
        let writes = store.writes.lock().unwrap();
        let cluster = writes
            .iter()
            .find(|row| row.get("type_kwd").and_then(Value::as_str) == Some("nav_cluster"))
            .expect("root cluster created");
        assert_eq!(cluster["parent_kwd"], json!("root"));
        assert_eq!(cluster["depth_int"], json!(0));
        assert_eq!(cluster["doc_ids_kwd"], json!(["d9"]));
        let doc = writes
            .iter()
            .find(|row| row.get("type_kwd").and_then(Value::as_str) == Some("nav_doc"))
            .unwrap();
        assert_eq!(doc["depth_int"], json!(1));
    }

    #[tokio::test]
    async fn remove_updates_parent_and_cleans_empties() {
        let store = MockStore::default();
        store.seed(cluster_row("Parent", "root", 0, &["d1"]));
        store.seed(json!({
            "id": nav_doc_id("d1"),
            "name": "Doc One",
            "parent_kwd": "Parent",
            "type_kwd": "nav_doc",
            "kb_id": "kb1",
            "compile_kwd": "dataset_nav",
            "doc_id": "d1",
            "content_with_weight": json!({"type": "nav_doc", "description": "one"}).to_string(),
        }));
        remove_dataset_nav_doc(&store, &OpenLock, "t1", "kb1", "d1").await;
        let deletes = store.deletes.lock().unwrap();
        assert!(deletes.contains(&nav_doc_id("d1")), "nav_doc removed");
        assert!(
            deletes.contains(&nav_cluster_id("kb1", "Parent")),
            "parent became empty and was removed"
        );
    }
}
#[cfg(test)]
mod dataset_nav_part3c_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn text_score_counts_query_term_coverage() {
        let row = json!({
            "name": "Topic Node",
            "content_with_weight": json!({
                "description": "about paris",
                "keywords": ["france"],
                "entities": ["Paris"],
            }).to_string(),
        });
        let score = nav_text_score("paris france", &row);
        assert!((score - 1.0).abs() < 1e-9, "both terms hit");
        let half = nav_text_score("paris missingterm", &row);
        assert!((half - 0.5).abs() < 1e-9);
        assert_eq!(nav_text_score("   ", &row), 0.0);
    }

    #[test]
    fn hybrid_fuse_folds_legs_by_name() {
        let vec = vec![1.0f32, 0.0];
        let knn = vec![json!({"name": "A", "q_2_vec": [1.0, 0.0]})];
        let text = vec![json!({
            "name": "A",
            "content_with_weight": json!({"description": "alpha"}).to_string(),
        })];
        let fused = hybrid_fuse(&vec, "q_2_vec", "alpha", &knn, &text, 0.5, 5);
        assert_eq!(fused.len(), 1, "same name collapses into one row");
        assert!(
            fused[0]["_score"].as_f64().unwrap() > 0.5 + 0.25,
            "both legs contributed"
        );
        assert!(fused[0]["_text_score"].as_f64().unwrap() > 0.0);

        let knn_only = vec![json!({"name": "B", "q_2_vec": [1.0, 0.0]})];
        let fused = hybrid_fuse(&vec, "q_2_vec", "zzz", &knn_only, &[], 0.5, 5);
        assert_eq!(
            fused[0]["_text_score"],
            json!(0.0),
            "pure vector hit records zero text score"
        );
    }
}
