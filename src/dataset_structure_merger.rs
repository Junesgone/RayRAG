//! KB-wide structure-graph merge task (incremental, bucketed) — RAGFlow v0.27.2
//! `rag/svr/task_executor_refactor/dataset_structure_merger.py`.
//!
//! Triggered when the user POSTs to `/datasets/<id>/index` with a structure
//! index type. Scans `scope_kwd="doc"` entity rows grouped by `(name, type)`,
//! merges them into `scope_kwd="dataset"` rows with bucketing for bounded
//! memory.
//!
//! Key design (mirrored):
//! - 256 hash buckets — all rows accumulated in memory, merged once per bucket
//! - Incremental: only processes doc_graph rows changed since `last_build_time`
//! - Document deletions are tracked via [`record_doc_deletion`]; ghost entities
//!   are cleaned up incrementally at the start of the next build
//! - Template changes still require a full rebuild
//! - Resulting dataset_graph rows are searchable (`available_int=1`)

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::doc_store::{DocRow, DocStore, FilterCondition, OrderByExpr, SearchQuery};
use crate::structure_compile::{stable_row_id, tokenize_for_search};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const BUCKET_COUNT: usize = 256;
pub const BUCKET_FLUSH_THRESHOLD: usize = 500;
pub const PAGE_SIZE: usize = 1000;
pub const SCOPE_KWD_DOC: &str = "doc";
pub const SCOPE_KWD_DATASET: &str = "dataset";
pub const DELETION_META_KWD: &str = "doc_deleted";
pub const EMBED_BATCH_SIZE: usize = 32;
pub const META_ROW_KWD: &str = "kg_build_meta";

pub const STRUCTURE_MERGE_TASK_TYPES: [&str; 6] = [
    "structure_graph",
    "structure_mindmap",
    "timeline",
    "session_graph",
    "session_essence",
    "structure",
];

/// `is_structure_merge_task`.
pub fn is_structure_merge_task(task_type: &str) -> bool {
    STRUCTURE_MERGE_TASK_TYPES.contains(&task_type.to_lowercase().as_str())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `_meta_row_id`: xxh3-64 of the metadata seed (upstream xxh64; the id is
/// only an idempotent-upsert key, matching `structure_compile::stable_row_id`).
pub fn meta_row_id(kb_id: &str, compile_kwd: &str, template_id: Option<&str>) -> String {
    stable_row_id(&[
        META_ROW_KWD.to_string(),
        kb_id.to_string(),
        compile_kwd.to_string(),
        template_id.unwrap_or("").to_string(),
    ])
}

/// `_dataset_entity_row_id`.
pub fn dataset_entity_row_id(
    kb_id: &str,
    compile_kwd: &str,
    template_id: Option<&str>,
    name: &str,
) -> String {
    stable_row_id(&[
        name.to_string(),
        kb_id.to_string(),
        compile_kwd.to_string(),
        template_id.unwrap_or("").to_string(),
        SCOPE_KWD_DATASET.to_string(),
    ])
}

/// `_dataset_relation_row_id`.
pub fn dataset_relation_row_id(
    kb_id: &str,
    compile_kwd: &str,
    template_id: Option<&str>,
    src: &str,
    tgt: &str,
    rel_type: &str,
) -> String {
    let key = format!(
        "{} -> {} -> {}",
        src.to_lowercase(),
        rel_type.to_lowercase(),
        tgt.to_lowercase()
    );
    stable_row_id(&[
        key,
        kb_id.to_string(),
        compile_kwd.to_string(),
        template_id.unwrap_or("").to_string(),
        SCOPE_KWD_DATASET.to_string(),
    ])
}

/// `_bucket_id`: hash bucket for an entity name (or a src/type/tgt key).
pub fn bucket_id(name: &str, bucket_count: usize) -> usize {
    (xxhash_rust::xxh3::xxh3_64(name.as_bytes()) % bucket_count as u64) as usize
}

/// `hashable_key`: a canonical key for dedup sets. Python falls back to a
/// recursive canonicalization only for unhashable (malformed) values; Rust
/// values are always keyable, so malformed shapes serialize to canonical JSON.
pub fn hashable_key(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `_struct_entity_name`: the entity name from a payload (or from a row's
/// serialized `content_with_weight`).
pub fn struct_entity_name(payload: &Value) -> String {
    let value = match payload.get("name") {
        Some(Value::Null) | None => payload
            .get("content_with_weight")
            .and_then(Value::as_str)
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|parsed| parsed.get("name").cloned()),
        Some(other) => Some(other.clone()),
    };
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.trim().to_string(),
        Some(other) => other.to_string().trim().to_string(),
    }
}

fn parse_payload(row: &DocRow) -> Value {
    serde_json::from_str(
        row.get("content_with_weight")
            .and_then(Value::as_str)
            .unwrap_or(""),
    )
    .unwrap_or_else(|_| json!({}))
}

/// Python `str(x).strip() if x is not None else ""` over JSON values.
fn str_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.trim().to_string(),
        Some(other) => other.to_string().trim_matches('"').trim().to_string(),
    }
}

/// Python `int()` conversion for numeric/string values.
fn as_int_like(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

fn as_f64_like(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// `int(r.get("mention_count_int") or payload.get("mention_count", 1))`.
fn mention_count_of(row: &DocRow, payload: &Value) -> i64 {
    let from_row = row
        .get("mention_count_int")
        .and_then(as_int_like)
        .filter(|count| *count != 0);
    match from_row {
        Some(count) => count,
        None => payload
            .get("mention_count")
            .and_then(as_int_like)
            .unwrap_or(1),
    }
}

/// Python `max(items, key=len)` keeps the first longest element on ties.
fn longest_first(items: &[String]) -> Option<&String> {
    let mut best: Option<&String> = None;
    let mut best_len = 0usize;
    for item in items {
        let length = item.chars().count();
        if best.is_none() || length > best_len {
            best = Some(item);
            best_len = length;
        }
    }
    best
}

fn now_timestamp() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0)
}

fn now_iso() -> String {
    chrono::Local::now()
        .naive_local()
        .format("%Y-%m-%dT%H:%M:%S%.6f")
        .to_string()
}

fn with_condition(base: &FilterCondition, key: &str, value: Value) -> FilterCondition {
    let mut condition = base.clone();
    condition.insert(key.to_string(), value);
    condition
}

// ---------------------------------------------------------------------------
// Document-store I/O
// ---------------------------------------------------------------------------

async fn search_checked(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    condition: FilterCondition,
    fields: &[&str],
    limit: usize,
    offset: usize,
) -> Result<(Vec<DocRow>, usize), String> {
    let query = SearchQuery {
        select_fields: fields.iter().map(|field| (*field).to_string()).collect(),
        condition,
        order_by: OrderByExpr::default(),
        offset,
        limit: limit.max(1),
        index_names: vec![index_name.to_string()],
        dataset_ids: vec![kb_id.to_string()],
        ..SearchQuery::default()
    };
    let response = store.search(&query).map_err(|error| error.to_string())?;
    Ok((response.docs, response.total))
}

/// `_index_search`: store errors are logged upstream and yield no rows.
pub async fn index_search(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    condition: FilterCondition,
    fields: &[&str],
    limit: usize,
    offset: usize,
) -> Vec<DocRow> {
    match search_checked(store, index_name, kb_id, condition, fields, limit, offset).await {
        Ok((rows, _)) => rows,
        Err(_) => Vec::new(),
    }
}

/// `_index_delete`.
pub async fn index_delete(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    condition: FilterCondition,
) {
    if let Err(error) = store.delete(&condition, index_name, kb_id) {
        tracing::error!(%error, %index_name, %kb_id, "structure merge: index delete failed");
    }
}

/// `_index_insert` (bulk refresh is a per-backend concern in RayRAG).
pub async fn index_insert(store: &dyn DocStore, index_name: &str, kb_id: &str, rows: &[DocRow]) {
    if rows.is_empty() {
        return;
    }
    if let Err(error) = store.insert(rows, index_name, kb_id) {
        tracing::error!(
            %error,
            %index_name,
            %kb_id,
            rows = rows.len(),
            "structure merge: index insert failed"
        );
    }
}

/// `_refresh_index`: no-op — the RayRAG store backends make writes visible
/// immediately, so there is no per-index refresh to trigger.
pub async fn refresh_index(_store: &dyn DocStore, _index_name: &str) {}

/// Embedding hook for `_embed_rows` (`_encode(embd_mdl, texts)`).
pub type EmbedBatchFn<'a> = dyn Fn(&[String]) -> Result<Vec<Vec<f32>>, String> + Send + Sync + 'a;

/// `_embed_rows`: embed rows in bounded batches and attach vectors in order.
/// The batch size matches upstream; the caller's embed hook owns the
/// concurrency (upstream runs up to 4 batches in flight — the observable
/// result is identical).
pub fn embed_rows(rows: &mut [DocRow], texts: &[String], embed: Option<&EmbedBatchFn<'_>>) {
    let Some(embed) = embed else {
        return;
    };
    if rows.is_empty() {
        return;
    }
    let mut start = 0usize;
    while start < rows.len() {
        let end = (start + EMBED_BATCH_SIZE).min(rows.len());
        let end = end.min(texts.len());
        if start >= end {
            break;
        }
        match embed(&texts[start..end]) {
            Ok(vectors) => {
                for (offset, vector) in vectors.iter().enumerate() {
                    if !vector.is_empty() {
                        let index = start + offset;
                        if index < rows.len() {
                            rows[index].insert(format!("q_{}_vec", vector.len()), json!(vector));
                        }
                    }
                }
            }
            Err(error) => {
                // Rows written without vectors can never be found by vector search: the
                // compile still "succeeds", so this must be visible.
                tracing::error!(
                    %error,
                    start,
                    end,
                    "Embedding a structure batch failed; those rows get no vectors"
                );
            }
        }
        start = end;
    }
}

// ---------------------------------------------------------------------------
// Document deletion tracking
// ---------------------------------------------------------------------------

/// `record_doc_deletion`: write a deletion-marker row into the chunk table.
///
/// The row is consumed by [`cleanup_deleted_docs`] during the next incremental
/// merge build to strip the deleted doc's ID from candidate entity rows and
/// purge ghost entities (those whose *only* source document was deleted). The
/// deleted doc ID is stored in `deleted_doc_id` (not `doc_id`) so the row
/// survives the `doc_id`-based chunk sweep. Unlike the upstream logger, a
/// store failure is returned to the caller.
pub async fn record_doc_deletion(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    doc_id: &str,
) -> Result<(), String> {
    if !store.index_exist(index_name, kb_id).unwrap_or(false) {
        return Ok(());
    }
    let mut row = DocRow::new();
    row.insert(
        "id".to_string(),
        Value::String(format!("{DELETION_META_KWD}:{kb_id}:{doc_id}")),
    );
    row.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
    row.insert(
        "deleted_doc_id".to_string(),
        Value::String(doc_id.to_string()),
    );
    row.insert(
        "knowledge_graph_kwd".to_string(),
        Value::String(DELETION_META_KWD.to_string()),
    );
    row.insert("compile_kwd".to_string(), Value::String("*".to_string()));
    row.insert("create_timestamp_flt".to_string(), json!(now_timestamp()));
    store
        .insert(&[row], index_name, kb_id)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Merge logic
// ---------------------------------------------------------------------------

/// `_load_last_build_time`.
pub async fn load_last_build_time(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    compile_kwd: &str,
    template_id: Option<&str>,
) -> Option<f64> {
    let meta_id = meta_row_id(kb_id, compile_kwd, template_id);
    if !store.index_exist(index_name, kb_id).unwrap_or(false) {
        return None;
    }
    let row = store
        .get(&meta_id, index_name, &[kb_id.to_string()])
        .ok()
        .flatten()?;
    let timestamp = row.get("build_timestamp_flt").and_then(as_f64_like)?;
    if timestamp > 0.0 {
        Some(timestamp)
    } else {
        None
    }
}

/// `_save_build_time`.
pub async fn save_build_time(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    compile_kwd: &str,
    template_id: Option<&str>,
    timestamp: f64,
) {
    let meta_id = meta_row_id(kb_id, compile_kwd, template_id);
    let mut payload = DocRow::new();
    payload.insert("id".to_string(), Value::String(meta_id.clone()));
    payload.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
    payload.insert("doc_id".to_string(), Value::String(kb_id.to_string()));
    payload.insert(
        "compile_kwd".to_string(),
        Value::String(compile_kwd.to_string()),
    );
    payload.insert(
        "knowledge_graph_kwd".to_string(),
        Value::String(META_ROW_KWD.to_string()),
    );
    payload.insert("build_timestamp_flt".to_string(), json!(timestamp));
    payload.insert("create_time".to_string(), Value::String(now_iso()));
    let existing = store
        .get(&meta_id, index_name, &[kb_id.to_string()])
        .ok()
        .flatten();
    if existing.is_some() {
        let condition: FilterCondition =
            [("id".to_string(), json!([meta_id]))].into_iter().collect();
        if let Err(error) = store.update(&condition, &payload, index_name, kb_id) {
            tracing::error!(%error, %index_name, %kb_id, "Failed to update the structure meta row");
        }
    } else if let Err(error) = store.insert(&[payload], index_name, kb_id) {
        tracing::error!(%error, %index_name, %kb_id, "Failed to insert the structure meta row");
    }
}

/// `_merge_bucket`: flush one bucket — group by (name, type), merge, build
/// dataset rows. Returns the rows to insert.
pub fn merge_bucket(
    bucket_rows: &[DocRow],
    kb_id: &str,
    compile_kwd: &str,
    template_id: Option<&str>,
    structure_kind: Option<&str>,
    embed: Option<&EmbedBatchFn<'_>>,
) -> Vec<DocRow> {
    // Group by (name, type) in first-seen order.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut groups: HashMap<(String, String), Vec<&DocRow>> = HashMap::new();
    for row in bucket_rows {
        let payload = parse_payload(row);
        let name = struct_entity_name(&payload);
        let entity_type = match payload.get("type") {
            Some(Value::String(text)) => text.trim().to_string(),
            Some(other) if !other.is_null() => other.to_string().trim().to_string(),
            _ => "other".to_string(),
        };
        let key = (name.to_lowercase(), entity_type);
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(row);
    }

    let mut rows_out: Vec<DocRow> = Vec::new();
    let mut embedding_texts: Vec<String> = Vec::new();
    for (name_lower, entity_type) in order {
        let rows = &groups[&(name_lower.clone(), entity_type.clone())];
        let mut all_chunks: Vec<String> = Vec::new();
        let mut all_docs: Vec<String> = Vec::new();
        let mut all_descriptions: Vec<String> = Vec::new();
        let mut all_original_names: Vec<String> = Vec::new();
        let mut seen_chunks: HashSet<String> = HashSet::new();
        let mut seen_docs: HashSet<String> = HashSet::new();
        let mut seen_descriptions: HashSet<String> = HashSet::new();
        let mut seen_original_names: HashSet<String> = HashSet::new();
        let mut mention_count = 0i64;
        for row in rows {
            let payload = parse_payload(row);
            if let Some(chunks) = row.get("source_chunk_ids").and_then(Value::as_array) {
                for chunk in chunks {
                    let key = hashable_key(chunk);
                    if seen_chunks.insert(key) {
                        all_chunks.push(match chunk {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        });
                    }
                }
            }
            let doc = str_of(row.get("doc_id"));
            if !doc.is_empty() && seen_docs.insert(hashable_key(&Value::String(doc.clone()))) {
                all_docs.push(doc);
            }
            mention_count += mention_count_of(row, &payload);
            let description = payload
                .get("description")
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            if !description.is_empty()
                && seen_descriptions.insert(hashable_key(&Value::String(description.clone())))
            {
                all_descriptions.push(description);
            }
            let original = struct_entity_name(&payload);
            if !original.is_empty() && seen_original_names.insert(original.clone()) {
                all_original_names.push(original);
            }
        }

        let best_desc = longest_first(&all_descriptions)
            .cloned()
            .unwrap_or_else(|| name_lower.clone());
        let best_original = longest_first(&all_original_names)
            .cloned()
            .unwrap_or_else(|| name_lower.clone());
        let payload_out = json!({
            "name": best_original,
            "type": entity_type,
            "description": best_desc,
            "mention_count": mention_count,
        });
        let (ltks, sm_ltks) = tokenize_for_search(&best_desc);

        let row_id = dataset_entity_row_id(kb_id, compile_kwd, template_id, &name_lower);
        let mut row = DocRow::new();
        row.insert("id".to_string(), Value::String(row_id));
        row.insert(
            "content_with_weight".to_string(),
            Value::String(payload_out.to_string()),
        );
        row.insert(
            "compile_kwd".to_string(),
            Value::String(compile_kwd.to_string()),
        );
        row.insert(
            "knowledge_graph_kwd".to_string(),
            Value::String("entity".to_string()),
        );
        row.insert(
            "scope_kwd".to_string(),
            Value::String(SCOPE_KWD_DATASET.to_string()),
        );
        row.insert("doc_id".to_string(), Value::String(kb_id.to_string()));
        row.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
        row.insert("name_kwd".to_string(), Value::String(name_lower.clone()));
        row.insert("source_chunk_ids".to_string(), json!(all_chunks));
        row.insert("doc_ids_kwd".to_string(), json!(all_docs));
        row.insert("content_ltks".to_string(), json!(ltks));
        row.insert("content_sm_ltks".to_string(), json!(sm_ltks));
        row.insert("mention_count_int".to_string(), json!(mention_count));
        row.insert("available_int".to_string(), json!(1));
        if let Some(template) = template_id {
            row.insert("compilation_template_ids".to_string(), json!([template]));
        }
        if let Some(kind) = structure_kind {
            row.insert(
                "compilation_template_kind_kwd".to_string(),
                Value::String(kind.to_string()),
            );
        }
        rows_out.push(row);
        embedding_texts.push(best_desc);
    }

    embed_rows(&mut rows_out, &embedding_texts, embed);
    rows_out
}

/// `_merge_relations`: group doc-graph relation rows by (src, rel_type, tgt)
/// and merge them into one dataset-scoped relation row per group.
pub fn merge_relations(
    rel_bucket_rows: &[DocRow],
    kb_id: &str,
    compile_kwd: &str,
    template_id: Option<&str>,
    structure_kind: Option<&str>,
    embed: Option<&EmbedBatchFn<'_>>,
) -> Vec<DocRow> {
    let mut order: Vec<(String, String, String)> = Vec::new();
    let mut groups: HashMap<(String, String, String), Vec<&DocRow>> = HashMap::new();
    for row in rel_bucket_rows {
        let payload = parse_payload(row);
        let src = payload
            .get("from")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                row.get("from_entity_kwd")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let tgt = payload
            .get("to")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                row.get("to_entity_kwd")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let rel_type = payload
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("related")
            .trim()
            .to_lowercase();
        let key = (src, rel_type, tgt);
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(row);
    }

    let mut rows_out: Vec<DocRow> = Vec::new();
    let mut embedding_texts: Vec<String> = Vec::new();
    for (src, rel_type, tgt) in order {
        let rows = &groups[&(src.clone(), rel_type.clone(), tgt.clone())];
        let mut all_chunks: Vec<String> = Vec::new();
        let mut all_docs: Vec<String> = Vec::new();
        let mut all_desc: Vec<String> = Vec::new();
        let mut seen_chunks: HashSet<String> = HashSet::new();
        let mut seen_docs: HashSet<String> = HashSet::new();
        let mut seen_desc: HashSet<String> = HashSet::new();
        for row in rows {
            let payload = parse_payload(row);
            if let Some(chunks) = row.get("source_chunk_ids").and_then(Value::as_array) {
                for chunk in chunks {
                    let key = hashable_key(chunk);
                    if seen_chunks.insert(key) {
                        all_chunks.push(match chunk {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        });
                    }
                }
            }
            let doc = str_of(row.get("doc_id"));
            if !doc.is_empty() && seen_docs.insert(hashable_key(&Value::String(doc.clone()))) {
                all_docs.push(doc);
            }
            let description = payload
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|text| !text.is_empty())
                .unwrap_or_else(|| format!("{src} {rel_type} {tgt}"));
            let key = hashable_key(&Value::String(description.clone()));
            if seen_desc.insert(key) {
                all_desc.push(description);
            }
        }
        let best_desc = longest_first(&all_desc)
            .cloned()
            .unwrap_or_else(|| format!("{src} {rel_type} {tgt}"));
        let (ltks, sm_ltks) = tokenize_for_search(&best_desc);

        let row_id =
            dataset_relation_row_id(kb_id, compile_kwd, template_id, &src, &tgt, &rel_type);
        let mut row = DocRow::new();
        row.insert("id".to_string(), Value::String(row_id));
        row.insert(
            "content_with_weight".to_string(),
            Value::String(json!({"from": src, "to": tgt, "type": rel_type}).to_string()),
        );
        row.insert(
            "compile_kwd".to_string(),
            Value::String(compile_kwd.to_string()),
        );
        row.insert(
            "knowledge_graph_kwd".to_string(),
            Value::String("relation".to_string()),
        );
        row.insert(
            "scope_kwd".to_string(),
            Value::String(SCOPE_KWD_DATASET.to_string()),
        );
        row.insert("doc_id".to_string(), Value::String(kb_id.to_string()));
        row.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
        row.insert("from_entity_kwd".to_string(), Value::String(src.clone()));
        row.insert("to_entity_kwd".to_string(), Value::String(tgt.clone()));
        row.insert("source_chunk_ids".to_string(), json!(all_chunks));
        row.insert("doc_ids_kwd".to_string(), json!(all_docs));
        row.insert("content_ltks".to_string(), json!(ltks));
        row.insert("content_sm_ltks".to_string(), json!(sm_ltks));
        row.insert("available_int".to_string(), json!(1));
        if let Some(template) = template_id {
            row.insert("compilation_template_ids".to_string(), json!([template]));
        }
        if let Some(kind) = structure_kind {
            row.insert(
                "compilation_template_kind_kwd".to_string(),
                Value::String(kind.to_string()),
            );
        }
        rows_out.push(row);
        embedding_texts.push(best_desc);
    }

    embed_rows(&mut rows_out, &embedding_texts, embed);
    rows_out
}

// ---------------------------------------------------------------------------
// Deletion-driven ghost cleanup
// ---------------------------------------------------------------------------

/// `_cleanup_deleted_docs`: clean dataset-level rows whose *only* source doc
/// was deleted. Two passes; returns whether Pass 2 completed without store
/// errors (the caller deletes the processed meta rows only after **all**
/// (compile_kwd, template_id) pairs succeeded).
pub async fn cleanup_deleted_docs(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
    compile_kwd: &str,
    template_id: Option<&str>,
) -> bool {
    if !store.index_exist(index_name, kb_id).unwrap_or(false) {
        return false;
    }

    // ── Pass 1: collect pending deletions ────────────────────────────
    let mut cursor: Vec<(String, String)> = Vec::new();
    let mut offset = 0usize;
    loop {
        let marker_condition: FilterCondition = [
            ("kb_id".to_string(), json!([kb_id])),
            (
                "knowledge_graph_kwd".to_string(),
                json!([DELETION_META_KWD]),
            ),
        ]
        .into_iter()
        .collect();
        let rows = match search_checked(
            store,
            index_name,
            kb_id,
            marker_condition,
            &["id", "deleted_doc_id"],
            PAGE_SIZE,
            offset,
        )
        .await
        {
            Ok((rows, _)) => rows,
            Err(_) => break,
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let deleted = str_of(row.get("deleted_doc_id"));
            let id = str_of(row.get("id"));
            if !deleted.is_empty() {
                cursor.push((id, deleted));
            }
        }
        if rows.len() < PAGE_SIZE {
            break;
        }
        offset += PAGE_SIZE;
    }

    if cursor.is_empty() {
        return true;
    }
    let mut deleted_by_key: BTreeMap<String, String> = BTreeMap::new();
    for (_, deleted) in &cursor {
        let key = hashable_key(&Value::String(deleted.clone()));
        deleted_by_key.entry(key).or_insert_with(|| deleted.clone());
    }
    let deleted_keys: HashSet<String> = deleted_by_key.keys().cloned().collect();
    let deleted_values: Vec<String> = deleted_by_key.values().cloned().collect();

    // ── Pass 2: find affected dataset rows and remove ghosts ─────────
    let mut dataset_del_cond: FilterCondition = [
        ("scope_kwd".to_string(), json!([SCOPE_KWD_DATASET])),
        ("compile_kwd".to_string(), json!([compile_kwd])),
        ("kb_id".to_string(), json!([kb_id])),
        ("doc_ids_kwd".to_string(), json!(deleted_values)),
    ]
    .into_iter()
    .collect();
    if let Some(template) = template_id {
        dataset_del_cond.insert("compilation_template_ids".to_string(), json!([template]));
    }

    let mut ids_to_delete: Vec<String> = Vec::new();
    let mut ids_to_update: Vec<(String, Vec<String>)> = Vec::new();
    let mut pass2_ok = true;
    let mut offset = 0usize;
    loop {
        let rows = match search_checked(
            store,
            index_name,
            kb_id,
            dataset_del_cond.clone(),
            &["id", "doc_ids_kwd"],
            PAGE_SIZE,
            offset,
        )
        .await
        {
            Ok((rows, _)) => rows,
            Err(_) => {
                pass2_ok = false;
                break;
            }
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let current: Vec<String> = row
                .get("doc_ids_kwd")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let remaining: Vec<String> = current
                .iter()
                .filter(|doc| !deleted_keys.contains(&hashable_key(&Value::String((*doc).clone()))))
                .cloned()
                .collect();
            let id = str_of(row.get("id"));
            if remaining.is_empty() {
                ids_to_delete.push(id);
            } else if remaining.len() < current.len() {
                ids_to_update.push((id, remaining));
            }
        }
        if rows.len() < PAGE_SIZE {
            break;
        }
        offset += PAGE_SIZE;
    }

    if !pass2_ok {
        return false;
    }

    if !ids_to_delete.is_empty() {
        let condition: FilterCondition = [("id".to_string(), json!(ids_to_delete))]
            .into_iter()
            .collect();
        index_delete(store, index_name, kb_id, condition).await;
    }

    for (row_id, remaining) in &ids_to_update {
        let condition: FilterCondition =
            [("id".to_string(), json!([row_id]))].into_iter().collect();
        let mut new_value = DocRow::new();
        new_value.insert("doc_ids_kwd".to_string(), json!(remaining));
        if let Err(error) = store.update(&condition, &new_value, index_name, kb_id) {
            // A stale doc_ids_kwd attributes documents to a structure that no longer
            // contains them, and the compile still reports success.
            tracing::error!(%error, %row_id, %index_name, "Failed to shrink the merged structure doc list");
        }
    }

    // Markers are NOT deleted here — the caller deletes them once after all
    // (compile_kwd, template_id) pairs succeed.
    true
}

/// `_consume_deletion_markers`: remove all pending `doc_deleted` meta rows.
pub async fn consume_deletion_markers(store: &dyn DocStore, index_name: &str, kb_id: &str) {
    if !store.index_exist(index_name, kb_id).unwrap_or(false) {
        return;
    }
    let mut marker_ids: Vec<String> = Vec::new();
    let mut offset = 0usize;
    loop {
        let condition: FilterCondition = [
            ("kb_id".to_string(), json!([kb_id])),
            (
                "knowledge_graph_kwd".to_string(),
                json!([DELETION_META_KWD]),
            ),
        ]
        .into_iter()
        .collect();
        let rows = match search_checked(
            store,
            index_name,
            kb_id,
            condition,
            &["id"],
            PAGE_SIZE,
            offset,
        )
        .await
        {
            Ok((rows, _)) => rows,
            Err(_) => break,
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let id = str_of(row.get("id"));
            if !id.is_empty() {
                marker_ids.push(id);
            }
        }
        if rows.len() < PAGE_SIZE {
            break;
        }
        offset += PAGE_SIZE;
    }
    if !marker_ids.is_empty() {
        let condition: FilterCondition = [("id".to_string(), json!(marker_ids))]
            .into_iter()
            .collect();
        index_delete(store, index_name, kb_id, condition).await;
    }
}

// ---------------------------------------------------------------------------
// Build driver
// ---------------------------------------------------------------------------

/// `_do_build`: read doc_graph rows, bucket, merge, write dataset rows.
/// Returns `true` when deletion markers were processed successfully (or none
/// existed), `false` when a store error prevented cleanup.
#[allow(clippy::too_many_arguments)]
pub async fn do_build(
    store: &dyn DocStore,
    index_name: &str,
    tenant_id: &str,
    kb_id: &str,
    compile_kwd: &str,
    template_id: Option<&str>,
    structure_kind: Option<&str>,
    embed: Option<&EmbedBatchFn<'_>>,
    incremental: bool,
    disabled_doc_ids: &HashSet<String>,
) -> bool {
    let _ = tenant_id;
    let mut cleanup_ok = true;

    // ── Determine the time window ─────────────────────────────────────
    let last_build_time = if incremental {
        load_last_build_time(store, index_name, kb_id, compile_kwd, template_id).await
    } else {
        None
    };

    // ── Full rebuild: delete existing dataset rows ────────────────────
    if !incremental || last_build_time.is_none() {
        let mut del_cond: FilterCondition = [
            ("scope_kwd".to_string(), json!([SCOPE_KWD_DATASET])),
            ("compile_kwd".to_string(), json!([compile_kwd])),
            ("kb_id".to_string(), json!([kb_id])),
        ]
        .into_iter()
        .collect();
        if let Some(template) = template_id {
            del_cond.insert("compilation_template_ids".to_string(), json!([template]));
        }
        index_delete(store, index_name, kb_id, del_cond).await;
        let meta_condition: FilterCondition = [(
            "id".to_string(),
            json!([meta_row_id(kb_id, compile_kwd, template_id)]),
        )]
        .into_iter()
        .collect();
        if let Err(error) = store.delete(&meta_condition, index_name, kb_id) {
            tracing::error!(%error, %index_name, %kb_id, "Failed to delete the consumed structure meta row");
        }
    }

    // ── Incremental: clean up ghosts from deleted docs ───────────────
    if incremental && last_build_time.is_some() {
        cleanup_ok = cleanup_deleted_docs(store, index_name, kb_id, compile_kwd, template_id).await;
    }

    // ── Scan document-level graph rows ───────────────────────────────
    let mut base_cond: FilterCondition = [
        ("compile_kwd".to_string(), json!([compile_kwd])),
        (
            "knowledge_graph_kwd".to_string(),
            json!(["entity", "relation"]),
        ),
    ]
    .into_iter()
    .collect();
    if let Some(template) = template_id {
        base_cond.insert("compilation_template_ids".to_string(), json!([template]));
    }
    // A full rebuild must only scan document-level rows: dataset-level rows
    // are this task's output and must never become input on the next run.
    base_cond.insert("scope_kwd".to_string(), json!([SCOPE_KWD_DOC]));
    let scan_all = !incremental || last_build_time.is_none();
    let timestamp_floor = if scan_all { None } else { last_build_time };

    let mut buckets: Vec<Vec<DocRow>> = vec![Vec::new(); BUCKET_COUNT];
    let mut rel_buckets: Vec<Vec<DocRow>> = vec![Vec::new(); BUCKET_COUNT];
    let mut offset = 0usize;
    loop {
        let rows = index_search(
            store,
            index_name,
            kb_id,
            base_cond.clone(),
            &[
                "id",
                "content_with_weight",
                "source_chunk_ids",
                "doc_id",
                "name_kwd",
                "mention_count_int",
                "compilation_template_ids",
                "create_timestamp_flt",
                "from_entity_kwd",
                "to_entity_kwd",
            ],
            PAGE_SIZE,
            offset,
        )
        .await;
        if rows.is_empty() {
            break;
        }
        let row_count = rows.len();
        for row in rows {
            let doc_id = str_of(row.get("doc_id"));
            if disabled_doc_ids.contains(&doc_id) {
                continue;
            }
            if let Some(floor) = timestamp_floor {
                let timestamp = row
                    .get("create_timestamp_flt")
                    .and_then(as_f64_like)
                    .unwrap_or(0.0);
                if timestamp < floor {
                    continue;
                }
            }
            let is_relation = row.get("knowledge_graph_kwd").and_then(Value::as_str)
                == Some("relation")
                || row.contains_key("from_entity_kwd");
            if is_relation {
                let payload = parse_payload(&row);
                let rsrc = payload
                    .get("from")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        row.get("from_entity_kwd")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();
                let rtgt = payload
                    .get("to")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        row.get("to_entity_kwd")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();
                let rtype = payload
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("related")
                    .trim()
                    .to_lowercase();
                let bucket = bucket_id(&format!("{rsrc}:{rtype}:{rtgt}"), BUCKET_COUNT);
                if bucket < BUCKET_COUNT {
                    rel_buckets[bucket].push(row);
                }
            } else {
                let name = str_of(row.get("name_kwd"));
                if name.is_empty() {
                    continue;
                }
                let bucket = bucket_id(&name, BUCKET_COUNT);
                if bucket < BUCKET_COUNT {
                    buckets[bucket].push(row);
                }
            }
        }
        if row_count < PAGE_SIZE {
            break;
        }
        offset += PAGE_SIZE;
    }

    for bucket in buckets.iter().filter(|rows| !rows.is_empty()) {
        let rows = merge_bucket(
            bucket,
            kb_id,
            compile_kwd,
            template_id,
            structure_kind,
            embed,
        );
        for batch in rows.chunks(BUCKET_FLUSH_THRESHOLD) {
            index_insert(store, index_name, kb_id, batch).await;
        }
    }
    for bucket in rel_buckets.iter().filter(|rows| !rows.is_empty()) {
        let rows = merge_relations(
            bucket,
            kb_id,
            compile_kwd,
            template_id,
            structure_kind,
            embed,
        );
        for batch in rows.chunks(BUCKET_FLUSH_THRESHOLD) {
            index_insert(store, index_name, kb_id, batch).await;
        }
    }

    // ── Save build timestamp ─────────────────────────────────────────
    save_build_time(
        store,
        index_name,
        kb_id,
        compile_kwd,
        template_id,
        now_timestamp(),
    )
    .await;
    cleanup_ok
}

/// `_collect_structure_pairs`: distinct (compile_kwd, template_id) pairs from
/// doc_graph entity rows.
pub async fn collect_structure_pairs(
    store: &dyn DocStore,
    index_name: &str,
    kb_id: &str,
) -> std::collections::BTreeSet<(String, String)> {
    if !store.index_exist(index_name, kb_id).unwrap_or(false) {
        return std::collections::BTreeSet::new();
    }
    let mut pairs: std::collections::BTreeSet<(String, String)> = std::collections::BTreeSet::new();
    let mut offset = 0usize;
    loop {
        let condition: FilterCondition = [("knowledge_graph_kwd".to_string(), json!(["entity"]))]
            .into_iter()
            .collect();
        let rows = match search_checked(
            store,
            index_name,
            kb_id,
            condition,
            &["id", "compile_kwd", "compilation_template_ids"],
            PAGE_SIZE,
            offset,
        )
        .await
        {
            Ok((rows, _)) => rows,
            Err(_) => break,
        };
        if rows.is_empty() {
            break;
        }
        for row in &rows {
            let compile_kwd = str_of(row.get("compile_kwd"));
            if compile_kwd.is_empty() {
                continue;
            }
            match row.get("compilation_template_ids") {
                Some(Value::Array(items)) => {
                    for item in items {
                        if let Some(template) = item.as_str().filter(|text| !text.is_empty()) {
                            pairs.insert((compile_kwd.clone(), template.to_string()));
                        }
                    }
                }
                Some(Value::String(template)) if !template.is_empty() => {
                    pairs.insert((compile_kwd.clone(), template.clone()));
                }
                _ => {}
            }
        }
        if rows.len() < PAGE_SIZE {
            break;
        }
        offset += PAGE_SIZE;
    }
    pairs
}

// ---------------------------------------------------------------------------
// Task entry point
// ---------------------------------------------------------------------------

/// Context fields the merge task reads from the upstream `TaskContext`.
#[derive(Debug, Clone, Default)]
pub struct StructureMergeContext {
    pub tenant_id: String,
    pub kb_id: String,
    pub language: String,
    pub task_type: String,
    pub id: String,
}

/// Progress / cancellation hooks (upstream `ctx.progress_cb` +
/// `ctx.has_canceled_func`).
pub trait MergeHooks {
    fn progress(&self, progress: f32, message: &str);
    fn has_canceled(&self) -> bool;
}

/// `run_structure_merge`: entry point — called when
/// [`is_structure_merge_task`] matches.
#[allow(clippy::too_many_arguments)]
pub async fn run_structure_merge(
    ctx: &StructureMergeContext,
    store: &dyn DocStore,
    index_name: &str,
    embed: Option<&EmbedBatchFn<'_>>,
    template_kind: &(dyn Fn(&str) -> Option<String> + Send + Sync),
    disabled_doc_ids: &HashSet<String>,
    hooks: &dyn MergeHooks,
) {
    let task_type = ctx.task_type.to_lowercase();
    let target_kind = match task_type.as_str() {
        "structure_graph" => Some("knowledge_graph"),
        "structure_mindmap" => Some("mind_map"),
        "timeline" => Some("timeline"),
        "session_graph" => Some("session_graph"),
        "session_essence" => Some("session_essence"),
        _ => None,
    };
    let merge_all = task_type == "structure";

    let Some(embed) = embed else {
        hooks.progress(1.0, "Failed to bind embedding model — aborting.");
        return;
    };

    hooks.progress(0.0, "Scanning doc_graph rows...");
    let pairs = collect_structure_pairs(store, index_name, &ctx.kb_id).await;
    if pairs.is_empty() {
        hooks.progress(1.0, "No doc_graph rows found.");
        return;
    }

    let template_meta: HashMap<String, (bool, Option<String>)> = HashMap::new();
    let resolve_template = |template_id: &str| -> (bool, Option<String>) {
        if let Some(cached) = template_meta.get(template_id) {
            return cached.clone();
        }
        let structure_kind = template_kind(template_id)
            .map(|kind| kind.trim().to_string())
            .filter(|kind| !kind.is_empty());
        let keep = match &structure_kind {
            Some(kind) => merge_all || target_kind == Some(kind.as_str()),
            None => false,
        };
        (keep, structure_kind)
    };

    let mut eligible: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    for (compile_kwd, template_id) in &pairs {
        let (keep, structure_kind) = resolve_template(template_id);
        if keep {
            eligible.push((
                compile_kwd.clone(),
                Some(template_id.clone()),
                structure_kind,
            ));
        }
    }

    if eligible.is_empty() {
        let kind_label = if merge_all {
            "any kind".to_string()
        } else {
            target_kind.unwrap_or(task_type.as_str()).to_string()
        };
        hooks.progress(1.0, &format!("No eligible templates for {kind_label}."));
        return;
    }

    // A disabled document does not trigger an immediate rebuild. On the next
    // explicit merge, rebuild from all active source rows so shared entities
    // and relations are recalculated without the disabled document's data.
    let rebuild_all = !disabled_doc_ids.is_empty();
    let total = eligible.len();
    let mut rebuilt = 0usize;
    let mut all_cleanup_ok = true;
    for (index, (compile_kwd, template_id, structure_kind)) in eligible.iter().enumerate() {
        if hooks.has_canceled() {
            hooks.progress(
                1.0,
                &format!("Cancelled after {rebuilt}/{total} dataset graph(s)."),
            );
            return;
        }
        hooks.progress(
            0.05 + 0.9 * (index as f32 / total as f32),
            &format!("Building dataset graph {}/{total} ...", index + 1),
        );
        let cleanup_ok = do_build(
            store,
            index_name,
            &ctx.tenant_id,
            &ctx.kb_id,
            compile_kwd,
            template_id.as_deref(),
            structure_kind.as_deref(),
            Some(embed),
            !rebuild_all,
            disabled_doc_ids,
        )
        .await;
        if !cleanup_ok {
            all_cleanup_ok = false;
        }
        rebuilt += 1;
    }

    refresh_index(store, index_name).await;

    if all_cleanup_ok {
        consume_deletion_markers(store, index_name, &ctx.kb_id).await;
    }

    hooks.progress(1.0, &format!("Built {rebuilt}/{total} dataset graph(s)."));
}


