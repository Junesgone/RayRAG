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
        best_parent = best
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
