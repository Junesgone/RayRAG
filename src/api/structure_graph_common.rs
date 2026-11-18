//! Shared structure-graph subgraph sampling — RAGFlow v0.27.2
//! `api/apps/services/structure_graph_common.py`.
//! Graph row shapes live in `src/structure_compile.rs`; the two REST callers
//! (document structure graph / dataset artifacts) pass their own scope.
//!
//! Both the per-document (`/datasets/<id>/documents/<doc>/structure/graph`) and
//! the dataset-wide (`/datasets/<id>/artifacts/structure`) endpoints render
//! per-template structure graphs. For large graphs a representative subgraph is
//! fetched from the raw `knowledge_graph_kwd` rows so the response — and the
//! frontend render — stay bounded. The two callers differ only in *scope*:
//! the document endpoint filters raw rows by `doc_id`, the dataset endpoint
//! queries KB-wide. That difference lives entirely in the `scope` /
//! `base_entity_condition` maps the caller passes; everything else is shared.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::{Value, json};

use crate::doc_store::{DocRow, DocStore, FilterCondition, MatchExpr, OrderByExpr, SearchQuery};
use crate::structure_compile::{graph_entity, graph_relation, tokenize_for_search};

/// Below this combined (entities + relations) count for a bucket, return all rows.
pub const GRAPH_FULL_THRESHOLD: usize = 1024;
/// Size of the top-mention entity seed set (set A) for large buckets.
pub const GRAPH_TOP_ENTITIES: usize = 256;
/// Keyword search needs a small, relevant candidate set; the larger sampling
/// cap above is intended for rendering an entire large graph bucket.
pub const GRAPH_KEYWORD_CANDIDATES: usize = 16;
/// Semantic fallback is intentionally singular. KNN is only used after exact
/// name and BM25 lookup fail; returning several approximate tree nodes creates
/// unrelated sibling branches in an otherwise focused path response.
pub const GRAPH_KEYWORD_KNN_CANDIDATES: usize = 1;
/// Upper bound on the relation / neighbor-entity expansion so a hub node can't
/// blow up the response.
pub const GRAPH_EXPANSION_CAP: usize = 4096;

pub const GRAPH_ENTITY_FIELDS: [&str; 8] = [
    "id",
    "content_with_weight",
    "name_kwd",
    "mention_count_int",
    "source_chunk_ids",
    "doc_id",
    "doc_ids_kwd",
    "source_doc_ids",
];
pub const GRAPH_RELATION_FIELDS: [&str; 7] = [
    "id",
    "content_with_weight",
    "from_entity_kwd",
    "to_entity_kwd",
    "doc_id",
    "doc_ids_kwd",
    "source_doc_ids",
];
pub const GRAPH_ALL_FIELDS: [&str; 11] = [
    "id",
    "content_with_weight",
    "name_kwd",
    "mention_count_int",
    "source_chunk_ids",
    "from_entity_kwd",
    "to_entity_kwd",
    "knowledge_graph_kwd",
    "doc_id",
    "doc_ids_kwd",
    "source_doc_ids",
];

/// `graph_search`: one raw-row search. Returns `(rows, total)` where `total`
/// is the full match count (not the returned slice). Rows keep response order,
/// mirroring the upstream `field_map.values()` iteration order.
pub fn graph_search(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    select_fields: &[&str],
    condition: FilterCondition,
    order_by: OrderByExpr,
    limit: usize,
    match_expressions: Vec<MatchExpr>,
    offset: usize,
) -> Result<(Vec<DocRow>, usize), String> {
    let query = SearchQuery {
        select_fields: select_fields
            .iter()
            .map(|field| (*field).to_string())
            .collect(),
        condition,
        match_expressions,
        order_by,
        offset,
        limit: limit.max(1),
        index_names: vec![index_name.to_string()],
        dataset_ids: vec![kb_id.to_string()],
        ..SearchQuery::default()
    };
    let response = store.search(&query).map_err(|error| error.to_string())?;
    Ok((response.docs, response.total))
}

fn with_condition(base: &FilterCondition, key: &str, value: Value) -> FilterCondition {
    let mut condition = base.clone();
    condition.insert(key.to_string(), value);
    condition
}

/// Python `int()` over the shapes the store returns: numbers truncate toward
/// zero, integer strings parse, everything else is not convertible.
fn as_int_like(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// `project_entity`: project a raw `knowledge_graph_kwd="entity"` row to the
/// graph-node shape the frontend consumes, surfacing `mention_count_int` as
/// `mention_count`.
pub fn project_entity(row: &DocRow) -> Option<Value> {
    let raw = row
        .get("content_with_weight")
        .and_then(Value::as_str)
        .unwrap_or("");
    let payload: Value = serde_json::from_str(if raw.is_empty() { "{}" } else { raw }).ok()?;
    if !payload.is_object() {
        return None;
    }
    let source_chunk_ids: Option<Vec<String>> = row
        .get("source_chunk_ids")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .collect()
        });
    let mut node = graph_entity(&payload, source_chunk_ids.as_deref())?;
    let mention = match row.get("mention_count_int") {
        // Infinity returns *_int scalars fine, but be defensive about lists.
        Some(Value::Array(items)) => items.first().and_then(as_int_like),
        Some(value) => as_int_like(value),
        None => None,
    };
    if let Some(count) = mention
        && let Some(object) = node.as_object_mut()
    {
        object.insert("mention_count".to_string(), json!(count));
    }
    Some(node)
}

/// `project_relation`: project a raw `knowledge_graph_kwd="relation"` row to
/// the edge shape. Prefers the payload (matching the blob projection); falls
/// back to the authoritative `*_entity_kwd` columns.
pub fn project_relation(row: &DocRow) -> Option<Value> {
    let raw = row
        .get("content_with_weight")
        .and_then(Value::as_str)
        .unwrap_or("");
    let payload: Value = serde_json::from_str(raw).unwrap_or_else(|_| json!({}));
    if let Some(node) = graph_relation(&payload) {
        return Some(node);
    }
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
    let relation_type = match payload.get("type") {
        Some(Value::String(text)) if !text.trim().is_empty() => text.trim().to_string(),
        Some(value) if !value.is_null() => value.to_string(),
        _ => "related".to_string(),
    };
    Some(json!({"from": source, "to": target, "type": relation_type}))
}

/// `dedup_entities`: order-preserving dedup by (lowercased name, type).
pub fn dedup_entities(entities: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for entity in entities {
        let name = entity
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        let entity_type = entity
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_lowercase();
        if name.is_empty() || !seen.insert((name, entity_type)) {
            continue;
        }
        out.push(entity.clone());
    }
    out
}

fn entity_response_id(entity: &Value) -> String {
    for field in ["id", "name", "slug"] {
        if let Some(value) = entity.get(field).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

fn endpoint_terms(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let mut terms: BTreeSet<String> = BTreeSet::new();
    terms.insert(trimmed.to_string());
    terms.insert(trimmed.to_lowercase());
    terms.into_iter().collect()
}

/// `normalize_relation_endpoints`: align relation endpoints to the returned
/// entity ids/names.
pub fn normalize_relation_endpoints(entities: &[Value], relations: &[Value]) -> Vec<Value> {
    if entities.is_empty() || relations.is_empty() {
        return relations.to_vec();
    }
    let mut lookup: HashMap<String, String> = HashMap::new();
    let mut ambiguous: HashSet<String> = HashSet::new();
    for entity in entities {
        let response_id = entity_response_id(entity);
        if response_id.is_empty() {
            continue;
        }
        for field in ["id", "name", "slug"] {
            let Some(value) = entity.get(field).and_then(Value::as_str) else {
                continue;
            };
            let trimmed = value.trim();
            if trimmed.is_empty() {
                continue;
            }
            let key = trimmed.to_lowercase();
            if let Some(existing) = lookup.get(&key) {
                if existing != &response_id {
                    ambiguous.insert(key);
                }
                continue;
            }
            lookup.insert(key, response_id.clone());
        }
    }
    for key in &ambiguous {
        lookup.remove(key);
    }
    relations
        .iter()
        .map(|relation| {
            let mut item = relation.clone();
            if let Some(object) = item.as_object_mut() {
                for field in ["from", "to"] {
                    if let Some(current) = object.get(field).and_then(Value::as_str)
                        && let Some(mapped) = lookup.get(&current.trim().to_lowercase())
                    {
                        object.insert(field.to_string(), Value::String(mapped.clone()));
                    }
                }
            }
            item
        })
        .collect()
}

/// `filter_entities_with_relations`: keep only entities that are referenced by
/// at least one relation.
pub fn filter_entities_with_relations(entities: &[Value], relations: &[Value]) -> Vec<Value> {
    if entities.is_empty() || relations.is_empty() {
        return Vec::new();
    }
    // Match case-insensitively: the dataset-scoped merge lowercases relation
    // endpoints while entity names keep their original case, so exact matching
    // would drop connected nodes from graph-like views.
    let mut connected: HashSet<String> = HashSet::new();
    for relation in relations {
        for field in ["from", "to"] {
            if let Some(endpoint) = relation.get(field).and_then(Value::as_str) {
                let trimmed = endpoint.trim().to_lowercase();
                if !trimmed.is_empty() {
                    connected.insert(trimmed);
                }
            }
        }
    }
    if connected.is_empty() {
        return Vec::new();
    }
    entities
        .iter()
        .filter(|entity| {
            // Structure-graph nodes are name-keyed and their relations reference
            // names; artifact-graph nodes are slug-keyed and their relations
            // reference slugs. Check all three identity fields so the same
            // filter serves both callers.
            let mut keys: HashSet<String> = HashSet::new();
            for field in ["id", "name", "slug"] {
                if let Some(value) = entity.get(field).and_then(Value::as_str) {
                    let trimmed = value.trim().to_lowercase();
                    if !trimmed.is_empty() {
                        keys.insert(trimmed);
                    }
                }
            }
            !keys.is_disjoint(&connected)
        })
        .cloned()
        .collect()
}

fn flatten_ids(value: &Value) -> HashSet<String> {
    match value {
        Value::Null => HashSet::new(),
        Value::String(text) => {
            let raw = text.trim();
            if raw.is_empty() {
                return HashSet::new();
            }
            match serde_json::from_str::<Value>(raw) {
                Ok(parsed) => flatten_ids(&parsed),
                Err(_) => HashSet::from([raw.to_string()]),
            }
        }
        Value::Array(items) => {
            let mut result: HashSet<String> = HashSet::new();
            for item in items {
                result.extend(flatten_ids(item));
            }
            result
        }
        other => HashSet::from([other.to_string()]),
    }
}

/// `_row_has_enabled_source`: rows whose every source document is disabled are
/// dropped from the structure views.
pub fn row_has_enabled_source(row: &DocRow, excluded_doc_ids: &HashSet<String>) -> bool {
    if excluded_doc_ids.is_empty() {
        return true;
    }
    let mut source_ids: HashSet<String> = HashSet::new();
    for field in ["doc_ids_kwd", "source_doc_ids"] {
        if let Some(value) = row.get(field) {
            source_ids.extend(flatten_ids(value));
        }
    }
    if !source_ids.is_empty() {
        return !source_ids.is_subset(excluded_doc_ids);
    }
    let doc_ids = row.get("doc_id").map(flatten_ids).unwrap_or_default();
    doc_ids.is_empty() || !doc_ids.is_subset(excluded_doc_ids)
}

/// `build_bucket`: build one bucket's `(entities, relations)` from raw rows.
///
/// `scope` is the filter WITHOUT `knowledge_graph_kwd` — e.g. `{"doc_id":[id],
/// "compilation_template_ids":[tid]}` (document scope) or
/// `{"compilation_template_ids":[tid]}` (dataset scope). Small buckets are
/// returned whole; large ones are sampled: top-`GRAPH_TOP_ENTITIES` entities by
/// `mention_count_int`, the relations sourced from them, and those relations'
/// target entities.
pub fn build_bucket(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    scope: &FilterCondition,
    excluded_doc_ids: &HashSet<String>,
) -> Result<(Vec<Value>, Vec<Value>), String> {
    let both_cond = with_condition(scope, "knowledge_graph_kwd", json!(["entity", "relation"]));
    let (_, total) = graph_search(
        store,
        index_name,
        kb_id,
        &["id"],
        both_cond.clone(),
        OrderByExpr::default(),
        1,
        Vec::new(),
        0,
    )?;

    if total < GRAPH_FULL_THRESHOLD {
        let (rows, _) = graph_search(
            store,
            index_name,
            kb_id,
            &GRAPH_ALL_FIELDS,
            both_cond,
            OrderByExpr::default(),
            total.max(1),
            Vec::new(),
            0,
        )?;
        let mut entities: Vec<Value> = Vec::new();
        let mut relations: Vec<Value> = Vec::new();
        for row in &rows {
            if !row_has_enabled_source(row, excluded_doc_ids) {
                continue;
            }
            if row.get("knowledge_graph_kwd") == Some(&Value::String("relation".to_string())) {
                if let Some(edge) = project_relation(row) {
                    relations.push(edge);
                }
            } else if let Some(node) = project_entity(row) {
                entities.push(node);
            }
        }
        entities = dedup_entities(&entities);
        return Ok((
            entities.clone(),
            normalize_relation_endpoints(&entities, &relations),
        ));
    }

    // Large bucket: sample. A = top entities by mention_count_int desc.
    let order_by = OrderByExpr::default().desc("mention_count_int");
    let mut set_a: Vec<Value> = Vec::new();
    let mut entity_offset = 0usize;
    let mut entity_total: Option<usize> = None;
    while set_a.len() < GRAPH_TOP_ENTITIES
        && entity_total
            .map(|total| entity_offset < total)
            .unwrap_or(true)
    {
        let (rows, total) = graph_search(
            store,
            index_name,
            kb_id,
            &GRAPH_ENTITY_FIELDS,
            with_condition(scope, "knowledge_graph_kwd", json!(["entity"])),
            order_by.clone(),
            GRAPH_TOP_ENTITIES,
            Vec::new(),
            entity_offset,
        )?;
        entity_total = Some(total);
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            if row_has_enabled_source(row, excluded_doc_ids)
                && let Some(node) = project_entity(row)
            {
                set_a.push(node);
            }
        }
        entity_offset += rows.len();
    }
    set_a.truncate(GRAPH_TOP_ENTITIES);
    let mut a_names: Vec<String> = set_a
        .iter()
        .filter_map(|entity| {
            entity
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .collect();
    a_names.sort();
    a_names.dedup();
    let mut a_name_terms: Vec<String> = Vec::new();
    for name in &a_names {
        a_name_terms.extend(endpoint_terms(name));
    }
    a_name_terms.sort();
    a_name_terms.dedup();

    // Relations whose source is one of A.
    let mut relations: Vec<Value> = Vec::new();
    let mut target_names_lower: BTreeSet<String> = BTreeSet::new();
    if !a_name_terms.is_empty() {
        let condition = with_condition(
            &with_condition(scope, "knowledge_graph_kwd", json!(["relation"])),
            "from_entity_kwd",
            json!(a_name_terms),
        );
        let (rows, _) = graph_search(
            store,
            index_name,
            kb_id,
            &GRAPH_RELATION_FIELDS,
            condition,
            OrderByExpr::default(),
            GRAPH_EXPANSION_CAP,
            Vec::new(),
            0,
        )?;
        for row in &rows {
            if !row_has_enabled_source(row, excluded_doc_ids) {
                continue;
            }
            if let Some(edge) = project_relation(row) {
                let target = edge
                    .get("to")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                if !target.is_empty() {
                    target_names_lower.insert(target);
                }
                relations.push(edge);
            }
        }
    }

    // Target entities of those relations (case-insensitive via name_kwd).
    let mut set_t: Vec<Value> = Vec::new();
    if !target_names_lower.is_empty() {
        let condition = with_condition(
            &with_condition(scope, "knowledge_graph_kwd", json!(["entity"])),
            "name_kwd",
            json!(target_names_lower.iter().cloned().collect::<Vec<_>>()),
        );
        let (rows, _) = graph_search(
            store,
            index_name,
            kb_id,
            &GRAPH_ENTITY_FIELDS,
            condition,
            OrderByExpr::default(),
            GRAPH_EXPANSION_CAP,
            Vec::new(),
            0,
        )?;
        set_t = rows
            .iter()
            .filter(|row| row_has_enabled_source(row, excluded_doc_ids))
            .filter_map(project_entity)
            .collect();
    }

    let mut combined = set_a;
    combined.extend(set_t);
    let entities = dedup_entities(&combined);
    Ok((
        entities.clone(),
        normalize_relation_endpoints(&entities, &relations),
    ))
}

/// `_valid_top_nodes`: rows that survive the excluded-document filter and
/// project to an entity with a non-blank name.
fn valid_top_nodes(rows: &[DocRow], excluded_doc_ids: &HashSet<String>) -> Vec<(DocRow, Value)> {
    let mut valid: Vec<(DocRow, Value)> = Vec::new();
    for row in rows {
        if !row_has_enabled_source(row, excluded_doc_ids) {
            continue;
        }
        if let Some(node) = project_entity(row)
            && !node
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .is_empty()
        {
            valid.push((row.clone(), node));
        }
    }
    valid
}

fn name_matches_query(node: &Value, query: &str) -> bool {
    let name = node
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let query = query.to_lowercase();
    if name.is_empty() || query.is_empty() {
        return false;
    }
    if name.contains(&query) {
        return true;
    }
    let terms: Vec<&str> = query.split_whitespace().collect();
    !terms.is_empty() && terms.iter().all(|term| name.contains(term))
}

/// `Compiler`-style keyword sanitizer (`re.sub` of the punctuation class with a
/// single space, then strip).
fn sanitize_text_query(keywords: &str) -> String {
    const PUNCTUATION: &str = " :|\r\n\t,，。？?/`!！&^%()[]{}<>*~'\"\\=";
    let mut out = String::new();
    let mut pending_space = false;
    for ch in keywords.chars() {
        if PUNCTUATION.contains(ch) {
            pending_space = true;
            continue;
        }
        if pending_space && !out.is_empty() {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }
    out.trim().to_string()
}

fn relation_key(edge: &Value) -> (String, String, String) {
    (
        edge.get("from")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        edge.get("to")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        edge.get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    )
}

/// Embedding hook for the KNN fallback (`embd_mdl.encode_queries`).
pub type EmbedQueryFn<'a> = dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync + 'a;

/// `keyword_subgraph`: find matching entity rows and return their focused
/// subgraph. BM25 provides lexical candidates filtered by entity-name
/// containment; KNN is the semantic fallback. `tree` / `page_index` buckets
/// additionally include the full ancestor path to the root.
#[allow(clippy::too_many_arguments)]
pub async fn keyword_subgraph(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    embed_query: Option<&EmbedQueryFn<'_>>,
    base_entity_condition: &FilterCondition,
    keywords: &str,
    scope_for_template: &(dyn Fn(&DocRow) -> (Value, FilterCondition) + Send + Sync),
    excluded_doc_ids: &HashSet<String>,
) -> Result<(Option<Value>, Vec<Value>, Vec<Value>), String> {
    let mut top_fields: Vec<&str> = GRAPH_ENTITY_FIELDS.to_vec();
    top_fields.extend([
        "compilation_template_ids",
        "compile_kwd",
        "compilation_template_kind_kwd",
    ]);

    // Entity names are identifiers in the graph. Check the exact normalized
    // name first: BM25 searches the entity description and may miss a stored
    // name even when the query equals it, which would otherwise trigger KNN
    // and return an approximate result. Partial-name queries continue through
    // BM25 so they can intentionally return multiple nodes.
    let text_query = sanitize_text_query(keywords);
    let mut candidates: Vec<(DocRow, Value)> = Vec::new();
    let exact_name = keywords.trim().to_lowercase();
    if !exact_name.is_empty() {
        let condition = with_condition(base_entity_condition, "name_kwd", json!([exact_name]));
        let (rows, _) = graph_search(
            store,
            index_name,
            kb_id,
            &top_fields,
            condition,
            OrderByExpr::default(),
            GRAPH_KEYWORD_CANDIDATES,
            Vec::new(),
            0,
        )?;
        candidates = valid_top_nodes(&rows, excluded_doc_ids);
    }
    if !text_query.is_empty() && candidates.is_empty() {
        let (coarse_query, fine_query) = tokenize_for_search(&text_query);
        let joined = format!("{} {}", coarse_query.join(" "), fine_query.join(" "));
        let mut unique_tokens: Vec<String> = Vec::new();
        let mut seen_tokens: HashSet<&str> = HashSet::new();
        for token in joined.split_whitespace() {
            if seen_tokens.insert(token) {
                unique_tokens.push(token.to_string());
            }
        }
        let tokenized_query = if unique_tokens.is_empty() {
            text_query.clone()
        } else {
            unique_tokens.join(" ")
        };
        let text_expr = MatchExpr::text(
            &["content_ltks^10", "content_sm_ltks"],
            &tokenized_query,
            GRAPH_KEYWORD_CANDIDATES,
        );
        let (rows, _) = graph_search(
            store,
            index_name,
            kb_id,
            &top_fields,
            base_entity_condition.clone(),
            OrderByExpr::default(),
            GRAPH_KEYWORD_CANDIDATES,
            vec![text_expr],
            0,
        )?;
        candidates = valid_top_nodes(&rows, excluded_doc_ids)
            .into_iter()
            .filter(|(_, node)| name_matches_query(node, &text_query))
            .collect();
    }
    // In a hierarchical index, a title containing the keyword is usually an
    // ancestor context, not the requested detail. Prefer matching detail
    // entities so the title is added only through the path-to-root walk.
    let detail_candidates: Vec<(DocRow, Value)> = candidates
        .iter()
        .filter(|(_, node)| {
            node.get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_lowercase()
                != "title"
        })
        .cloned()
        .collect();
    if !detail_candidates.is_empty() {
        candidates = detail_candidates;
    }

    // Fall back to semantic matching for aliases, paraphrases, and cases where
    // the query does not occur in the stored entity name.
    if candidates.is_empty() {
        let Some(embed) = embed_query else {
            return Ok((None, Vec::new(), Vec::new()));
        };
        let vector = match embed(keywords) {
            Ok(vector) => vector,
            Err(error) => {
                return Err(format!(
                    "structure graph: keyword embedding failed ({error})"
                ));
            }
        };
        if vector.is_empty() {
            return Ok((None, Vec::new(), Vec::new()));
        }
        let match_expr = MatchExpr::dense(
            &format!("q_{}_vec", vector.len()),
            vector,
            "cosine",
            GRAPH_KEYWORD_KNN_CANDIDATES,
        );
        let (rows, _) = graph_search(
            store,
            index_name,
            kb_id,
            &top_fields,
            base_entity_condition.clone(),
            OrderByExpr::default(),
            GRAPH_KEYWORD_KNN_CANDIDATES,
            vec![match_expr],
            0,
        )?;
        candidates = valid_top_nodes(&rows, excluded_doc_ids);
    }

    if candidates.is_empty() {
        return Ok((None, Vec::new(), Vec::new()));
    }
    let (first_row, _) = &candidates[0];
    let (bucket_meta, scope) = scope_for_template(first_row);

    // A response represents one template bucket. Keep all matching entities
    // from that bucket instead of silently discarding every candidate after
    // the first one.
    let bucket_id = bucket_meta.get("template_id").cloned();
    let mut matched_nodes: Vec<Value> = Vec::new();
    for (row, node) in &candidates {
        let (candidate_meta, _) = scope_for_template(row);
        if candidate_meta.get("template_id").cloned() == bucket_id {
            matched_nodes.push(node.clone());
        }
    }
    if matched_nodes.is_empty() {
        return Ok((None, Vec::new(), Vec::new()));
    }

    let structure_kind = bucket_meta
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_lowercase()
        .replace('-', "_");
    let tree_like = matches!(structure_kind.as_str(), "tree" | "page_index" | "pageindex");

    // Relations where a matched entity is source OR target (two term queries).
    let mut relations: Vec<Value> = Vec::new();
    let mut seen_rel: HashSet<(String, String, String)> = HashSet::new();
    let mut neighbor_names_lower: HashSet<String> = HashSet::new();
    let matched_names: HashSet<String> = matched_nodes
        .iter()
        .map(|node| {
            node.get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_lowercase()
        })
        .collect();
    if !tree_like {
        'outer: for matched_node in &matched_nodes {
            let matched_name = matched_node
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            let matched_name_terms = endpoint_terms(&matched_name);
            for field in ["from_entity_kwd", "to_entity_kwd"] {
                let condition = with_condition(
                    &with_condition(&scope, "knowledge_graph_kwd", json!(["relation"])),
                    field,
                    json!(matched_name_terms),
                );
                let (rows, _) = graph_search(
                    store,
                    index_name,
                    kb_id,
                    &GRAPH_RELATION_FIELDS,
                    condition,
                    OrderByExpr::default(),
                    GRAPH_EXPANSION_CAP,
                    Vec::new(),
                    0,
                )?;
                for row in &rows {
                    if !row_has_enabled_source(row, excluded_doc_ids) {
                        continue;
                    }
                    let Some(edge) = project_relation(row) else {
                        continue;
                    };
                    let key = relation_key(&edge);
                    if !seen_rel.insert(key) {
                        continue;
                    }
                    for endpoint in ["from", "to"] {
                        let endpoint = edge
                            .get(endpoint)
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !endpoint.is_empty() && !matched_names.contains(&endpoint.to_lowercase())
                        {
                            neighbor_names_lower.insert(endpoint.to_lowercase());
                        }
                    }
                    relations.push(edge);
                    if relations.len() >= GRAPH_EXPANSION_CAP {
                        break 'outer;
                    }
                }
            }
        }
    }

    // Tree-like structures encode hierarchy as parent -> child. A keyword may
    // hit a leaf, but the UI needs the complete path back to the root in order
    // to render that leaf in context. Resolve the path in this template one
    // endpoint at a time.
    let mut tree_entities: Vec<Value> = Vec::new();
    if tree_like && relations.len() < GRAPH_EXPANSION_CAP {
        let mut ancestor_frontier: Vec<(String, String)> = matched_nodes
            .iter()
            .filter_map(|node| {
                let name = node
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if name.is_empty() {
                    None
                } else {
                    Some((name.to_lowercase(), name))
                }
            })
            .collect();
        let mut seen_ancestors = matched_names.clone();
        let mut ancestor_order: Vec<String> = Vec::new();
        while !ancestor_frontier.is_empty() && relations.len() < GRAPH_EXPANSION_CAP {
            let mut next_frontier: Vec<(String, String)> = Vec::new();
            for (_, child_name) in &ancestor_frontier {
                for child_term in endpoint_terms(child_name) {
                    let condition = with_condition(
                        &with_condition(&scope, "knowledge_graph_kwd", json!(["relation"])),
                        "to_entity_kwd",
                        json!([child_term]),
                    );
                    let (rows, _) = graph_search(
                        store,
                        index_name,
                        kb_id,
                        &GRAPH_RELATION_FIELDS,
                        condition,
                        OrderByExpr::default(),
                        GRAPH_EXPANSION_CAP - relations.len(),
                        Vec::new(),
                        0,
                    )?;
                    for row in &rows {
                        if !row_has_enabled_source(row, excluded_doc_ids) {
                            continue;
                        }
                        let Some(edge) = project_relation(row) else {
                            continue;
                        };
                        let key = relation_key(&edge);
                        if !seen_rel.insert(key) {
                            continue;
                        }
                        relations.push(edge.clone());
                        let parent_name = edge
                            .get("from")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let parent = parent_name.to_lowercase();
                        if !parent.is_empty() && !seen_ancestors.contains(&parent) {
                            seen_ancestors.insert(parent.clone());
                            next_frontier.push((parent.clone(), parent_name));
                            ancestor_order.push(parent);
                        }
                        if relations.len() >= GRAPH_EXPANSION_CAP {
                            break;
                        }
                    }
                    if relations.len() >= GRAPH_EXPANSION_CAP {
                        break;
                    }
                }
                if relations.len() >= GRAPH_EXPANSION_CAP {
                    break;
                }
            }
            ancestor_frontier = next_frontier;
        }
        for name in seen_ancestors.difference(&matched_names) {
            neighbor_names_lower.insert(name.clone());
        }
        for name in &ancestor_order {
            let condition = with_condition(
                &with_condition(&scope, "knowledge_graph_kwd", json!(["entity"])),
                "name_kwd",
                json!([name]),
            );
            let (rows, _) = graph_search(
                store,
                index_name,
                kb_id,
                &GRAPH_ENTITY_FIELDS,
                condition,
                OrderByExpr::default(),
                GRAPH_EXPANSION_CAP,
                Vec::new(),
                0,
            )?;
            for row in &rows {
                if row_has_enabled_source(row, excluded_doc_ids)
                    && let Some(node) = project_entity(row)
                {
                    tree_entities.push(node);
                }
            }
        }
    }

    let mut entities: Vec<Value> = matched_nodes.clone();
    entities.extend(tree_entities);
    if !neighbor_names_lower.is_empty() && !tree_like {
        let condition = with_condition(
            &with_condition(&scope, "knowledge_graph_kwd", json!(["entity"])),
            "name_kwd",
            json!(
                neighbor_names_lower
                    .iter()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            ),
        );
        let (rows, _) = graph_search(
            store,
            index_name,
            kb_id,
            &GRAPH_ENTITY_FIELDS,
            condition,
            OrderByExpr::default(),
            GRAPH_EXPANSION_CAP,
            Vec::new(),
            0,
        )?;
        for row in &rows {
            if row_has_enabled_source(row, excluded_doc_ids)
                && let Some(node) = project_entity(row)
            {
                entities.push(node);
            }
        }
    }

    let entities = dedup_entities(&entities);
    let relations = if tree_like {
        let entity_names: HashSet<String> = entities
            .iter()
            .map(|entity| {
                entity
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_lowercase()
            })
            .collect();
        relations
            .into_iter()
            .filter(|relation| {
                let from = relation
                    .get("from")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                let to = relation
                    .get("to")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                entity_names.contains(&from) && entity_names.contains(&to)
            })
            .collect()
    } else {
        relations
    };
    Ok((
        Some(bucket_meta),
        entities.clone(),
        normalize_relation_endpoints(&entities, &relations),
    ))
}


