//! Dataset navigation tree (RAGFlow `dataset_api.py` navigation surface).
//!
//! Rows are `compile_kwd = dataset_nav` documents in the tenant index: clusters carry
//! `type_kwd = nav_cluster` with a `doc_count_int` tally, document leaves carry
//! `type_kwd = nav_doc`, and every child points at its parent through `parent_kwd`. That is
//! exactly the shape [`crate::harness::knowlege_dataset_nav`] writes, so these handlers
//! read the engine's own rows rather than a second copy.
//!
//! Every store call runs inside `spawn_blocking` (see
//! [`crate::api::dataset_artifacts`] for why): the doc store backends are synchronous and
//! panic if touched from a tokio worker.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::doc_store::{DocStore, FilterCondition, SearchQuery};
use crate::harness::knowlege_dataset_nav::{COMPILE_KWD, NAV_SEARCH_FIELDS, index_name};
use crate::server::{AppState, AuthContext, kb_accessible};

/// Upstream's nav listing default page size.
const NAV_PAGE_SIZE: usize = 1000;

fn fields() -> Vec<String> {
    // `parent_kwd` is not part of upstream's `_NAV_SEARCH_FIELDS` (the store filtered on it
    // instead), but this module filters after the fetch, so it must be selected.
    let mut selected: Vec<String> = NAV_SEARCH_FIELDS.iter().map(|f| (*f).to_string()).collect();
    selected.push("parent_kwd".to_string());
    selected
}

/// Upstream `_scalar`: `*_kwd` fields may arrive as a list.
fn scalar(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .find_map(|item| item.as_str().filter(|text| !text.is_empty()))
            .unwrap_or_default()
            .to_string(),
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

fn int_of(row: &Value, key: &str) -> i64 {
    match row.get(key) {
        Some(Value::Number(number)) => number.as_i64().unwrap_or(0),
        Some(Value::Array(items)) => items
            .iter()
            .find_map(|item| item.as_i64().or_else(|| item.as_str()?.parse().ok()))
            .unwrap_or(0),
        Some(Value::String(text)) => text.parse().unwrap_or(0),
        _ => 0,
    }
}

/// Upstream `_nav_item`: one nav row shaped into the UI node the tree renders.
fn nav_item(row: &Value) -> Value {
    let payload: Value = row
        .get("content_with_weight")
        .and_then(Value::as_str)
        .and_then(|text| serde_json::from_str(text).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let row_type = scalar(row, "type_kwd");
    let is_cluster = row_type == "nav_cluster";
    serde_json::json!({
        "name": scalar(row, "name"),
        "description": payload.get("description").and_then(Value::as_str).unwrap_or_default(),
        "keywords": payload.get("keywords").cloned().unwrap_or_else(|| serde_json::json!([])),
        "entities": payload.get("entities").cloned().unwrap_or_else(|| serde_json::json!([])),
        "graph_content": payload.get("graph_content").and_then(Value::as_str).unwrap_or_default(),
        // A cluster counts the documents under it; a leaf is one document.
        "doc_count": if is_cluster { int_of(row, "doc_count_int") } else { 1 },
        "type": if is_cluster { "cluster" } else { "doc" },
        "doc_id": if is_cluster {
            Value::Null
        } else {
            let doc_id = scalar(row, "doc_id");
            if doc_id.is_empty() { serde_json::json!(scalar(row, "name")) } else { serde_json::json!(doc_id) }
        },
        "has_children": is_cluster,
    })
}

/// Run one nav query on a blocking thread.
async fn req_store<T, F>(state: &Arc<AppState>, work: F) -> Result<T, Response>
where
    F: FnOnce(&dyn DocStore) -> anyhow::Result<T> + Send + 'static,
    T: Send + 'static,
{
    let pool = Arc::clone(&state.doc_store);
    match tokio::task::spawn_blocking(move || {
        let store = pool.get_conn()?;
        work(store.as_ref())
    })
    .await
    {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => {
            tracing::error!(%error, "dataset navigation store operation failed");
            Err(store_error(&error))
        }
        Err(error) => Err(store_error(&anyhow::anyhow!("store task failed: {error}"))),
    }
}

fn store_error(error: &anyhow::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "code": 500,
            "message": format!("Navigation store unavailable: {error}")
        })),
    )
        .into_response()
}

fn data_error(message: &str) -> Response {
    Json(serde_json::json!({ "code": 102, "message": message })).into_response()
}

/// What a navigation read selects, filtered here rather than in the store.
///
/// The store only sees `compile_kwd` (a plain string in every row): RAGFlow's `*_kwd`
/// fields are lists, and the backends disagree about whether a condition compares a list
/// member or the whole value — the in-memory store compares the value exactly, so pushing
/// `type_kwd`/`parent_kwd` down would silently return nothing there and everything on
/// another backend. Filtering after the fetch keeps every backend identical, and the trees
/// are small.
struct NavFilter {
    kind: Option<&'static str>,
    parent: Option<String>,
}

fn nav_condition() -> FilterCondition {
    let mut condition: FilterCondition = Default::default();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(COMPILE_KWD.to_string()),
    );
    condition
}

/// One page of nav rows matching `filter`, shaped as UI nodes.
fn query_rows(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    filter: NavFilter,
    page: usize,
    page_size: usize,
) -> anyhow::Result<(usize, Vec<Value>)> {
    let selected = fields();
    let mut offset = 0usize;
    let mut nodes: Vec<Value> = Vec::new();
    loop {
        let query = SearchQuery {
            select_fields: selected.clone(),
            condition: nav_condition(),
            limit: NAV_PAGE_SIZE,
            offset,
            index_names: vec![index_name(tenant_id)],
            dataset_ids: vec![kb_id.to_string()],
            ..Default::default()
        };
        let response = store.search(&query)?;
        let rows = store.get_fields(&response, &selected);
        if rows.is_empty() {
            break;
        }
        let count = rows.len();
        for (_, row) in rows {
            let row_value = Value::Object(row);
            if let Some(kind) = filter.kind
                && scalar(&row_value, "type_kwd") != kind
            {
                continue;
            }
            if let Some(parent) = filter.parent.as_ref()
                && &scalar(&row_value, "parent_kwd") != parent
            {
                continue;
            }
            nodes.push(nav_item(&row_value));
        }
        if count < NAV_PAGE_SIZE {
            break;
        }
        offset += NAV_PAGE_SIZE;
        if offset > 100_000 {
            tracing::warn!(%kb_id, "dataset navigation scan exceeded 100k rows; stopping");
            break;
        }
    }
    let total = nodes.len();
    let start = page.saturating_sub(1) * page_size;
    Ok((
        total,
        nodes.into_iter().skip(start).take(page_size).collect(),
    ))
}

/// `_NAV_ROOT_PARENT`: the sentinel parent of a top-level cluster.
const NAV_ROOT_PARENT: &str = "root";

fn page_params(query: &HashMap<String, String>) -> (usize, usize) {
    let page = query
        .get("page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let page_size = query
        .get("page_size")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(NAV_PAGE_SIZE)
        .clamp(1, NAV_PAGE_SIZE);
    (page, page_size)
}

/// `GET /api/v1/datasets/{id}/navigation` — upstream `list_dataset_nav`: the top-level
/// clusters, optionally filtered by `keywords` and capped by `top_k`.
pub async fn list_nav(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return data_error(&format!("You don't own the dataset {kb_id}."));
    }
    let (page, page_size) = page_params(&query);
    let keywords = query
        .get("keywords")
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    let top_k = match query.get("top_k") {
        Some(raw) if !raw.trim().is_empty() => match raw.trim().parse::<usize>() {
            Ok(value) if value >= 1 => Some(value),
            _ => return data_error("top_k must be a positive integer"),
        },
        _ => None,
    };
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let rows = match req_store(&state, move |store| {
        query_rows(
            store,
            &tenant,
            &dataset,
            NavFilter {
                kind: Some("nav_cluster"),
                parent: Some(NAV_ROOT_PARENT.to_string()),
            },
            page,
            page_size,
        )
    })
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let (total, mut items) = rows;
    if !keywords.is_empty() {
        items.retain(|item| {
            let name = item["name"].as_str().unwrap_or_default().to_lowercase();
            let description = item["description"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase();
            name.contains(&keywords) || description.contains(&keywords)
        });
    }
    if let Some(top_k) = top_k {
        items.truncate(top_k);
    }
    Json(serde_json::json!({
        "code": 0,
        "data": { "total": total, "items": items }
    }))
    .into_response()
}

/// `GET /api/v1/datasets/{id}/navigation/{name}/children` — upstream `list_nav_children`:
/// the direct children of one node (sub-clusters and document leaves).
pub async fn list_nav_children(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path((kb_id, name)): Path<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return data_error(&format!("You don't own the dataset {kb_id}."));
    }
    if name.trim().is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "data": { "total": 0, "items": [] }
        }))
        .into_response();
    }
    let (page, page_size) = page_params(&query);
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let parent = name.trim().to_string();
    match req_store(&state, move |store| {
        query_rows(
            store,
            &tenant,
            &dataset,
            NavFilter {
                kind: None,
                parent: Some(parent),
            },
            page,
            page_size,
        )
    })
    .await
    {
        Ok((total, items)) => Json(serde_json::json!({
            "code": 0,
            "data": { "total": total, "items": items }
        }))
        .into_response(),
        Err(response) => response,
    }
}

/// `GET /api/v1/datasets/{id}/navigation/search` — upstream `search_dataset_nav`.
///
/// The lexical half of upstream's unified search, over the engine's own
/// [`nav_text_score`](crate::harness::knowlege_dataset_nav::nav_text_score): keyword and
/// entity overlap, no embedding call. The embedding-driven modes (`navigation_tree` beam
/// descent, dense `chunk` search) need the tenant embedder and are declared in the API
/// worklist rather than half-answered here; `mode` is echoed back so a caller can tell
/// which search it got.
pub async fn search_nav(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return data_error(&format!("You don't own the dataset {kb_id}."));
    }
    let q = query
        .get("q")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if q.is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "data": { "mode": query.get("mode").cloned().unwrap_or_else(|| "nav_doc".into()), "total": 0, "items": [] }
        }))
        .into_response();
    }
    let mode = query
        .get("mode")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "nav_doc".to_string());
    let top_k = query
        .get("top_k")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(20);
    let wanted_type = match mode.as_str() {
        "nav_cluster" => Some("nav_cluster".to_string()),
        "nav_doc" | "navigation_tree" | "all" => None,
        other => {
            return data_error(&format!(
                "mode '{other}' needs the tenant embedder and is not implemented yet"
            ));
        }
    };
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let query_text = q.clone();
    let rows = match req_store(&state, move |store| {
        let selected = fields();
        let search = SearchQuery {
            select_fields: selected.clone(),
            condition: nav_condition(),
            limit: NAVID_SCAN_LIMIT,
            index_names: vec![index_name(&tenant)],
            dataset_ids: vec![dataset.clone()],
            ..Default::default()
        };
        let response = store.search(&search)?;
        Ok(store.get_fields(&response, &selected))
    })
    .await
    {
        Ok(rows) => rows,
        Err(response) => return response,
    };
    let mut scored: Vec<(f64, Value)> = rows
        .into_iter()
        .filter_map(|(_, row)| {
            let row = Value::Object(row);
            if let Some(kind) = wanted_type.as_deref()
                && scalar(&row, "type_kwd") != kind
            {
                return None;
            }
            let score = crate::harness::knowlege_dataset_nav::nav_text_score(&query_text, &row);
            (score > 0.0).then(|| (score, nav_item(&row)))
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total = scored.len();
    let items: Vec<Value> = scored
        .into_iter()
        .take(top_k)
        .map(|(score, mut item)| {
            item["score"] = serde_json::json!(score);
            item
        })
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": { "mode": mode, "total": total, "items": items }
    }))
    .into_response()
}

/// Ceiling for one lexical nav scan.
const NAVID_SCAN_LIMIT: usize = 1000;

/// `DELETE /api/v1/datasets/{id}/navigation` — upstream `delete_nav`: drop the whole tree.
pub async fn delete_nav(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return data_error(&format!("You don't own the dataset {kb_id}."));
    }
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let index = index_name(&tenant);
    match req_store(&state, move |store| {
        store.delete(&nav_condition(), &index, &dataset)
    })
    .await
    {
        Ok(deleted) => Json(serde_json::json!({
            "code": 0,
            "data": { "deleted": deleted }
        }))
        .into_response(),
        Err(response) => response,
    }
}

/// `DELETE /api/v1/datasets/{id}/navigation/{name}` — upstream `delete_nav_node`:
/// one node **and its whole subtree**, because children only reference their parent by
/// name.
pub async fn delete_nav_node(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path((kb_id, name)): Path<(String, String)>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return data_error(&format!("You don't own the dataset {kb_id}."));
    }
    let name = name.trim().to_string();
    if name.is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "data": { "deleted": 0 }
        }))
        .into_response();
    }
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let index = index_name(&tenant);
    match req_store(&state, move |store| {
        // Walk the subtree top-down: a node's descendants are the rows whose parent chain
        // starts here, so collecting level by level is what makes the delete complete
        // without an unbounded recursion.
        let selected = fields();
        let mut frontier: Vec<String> = vec![name.clone()];
        let mut seen: HashSet<String> = HashSet::new();
        let mut deleted = 0usize;
        while !frontier.is_empty() {
            let mut next: Vec<String> = Vec::new();
            let mut condition = nav_condition();
            condition.insert(
                "parent_kwd".to_string(),
                Value::Array(frontier.iter().cloned().map(Value::String).collect()),
            );
            let query = SearchQuery {
                select_fields: selected.clone(),
                condition,
                limit: NAV_PAGE_SIZE,
                index_names: vec![index.clone()],
                dataset_ids: vec![dataset.clone()],
                ..Default::default()
            };
            let response = store.search(&query)?;
            for (_, row) in store.get_fields(&response, &selected) {
                let row_value = Value::Object(row);
                let child = scalar(&row_value, "name");
                if !child.is_empty() && seen.insert(child.clone()) {
                    next.push(child);
                }
            }
            frontier = next;
            if seen.len() > 10_000 {
                tracing::warn!(%dataset, "navigation subtree walk exceeded 10k nodes; stopping");
                break;
            }
        }
        // The node itself, then every descendant name collected above.
        let mut names: Vec<Value> = vec![Value::String(name.clone())];
        names.extend(seen.into_iter().map(Value::String));
        let mut condition = nav_condition();
        condition.insert("name".to_string(), Value::Array(names));
        deleted += store.delete(&condition, &index, &dataset)?;
        Ok(deleted)
    })
    .await
    {
        Ok(deleted) => Json(serde_json::json!({
            "code": 0,
            "data": { "deleted": deleted }
        }))
        .into_response(),
        Err(response) => response,
    }
}

/// Production [`NavKbLock`](crate::harness::knowlege_dataset_nav::NavKbLock): one in-process
/// lock per dataset, matching upstream `lock.spin_acquire()` / `release()`.
pub struct NavKbMutex {
    held: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl Default for NavKbMutex {
    fn default() -> Self {
        Self::new()
    }
}

impl NavKbMutex {
    pub fn new() -> Self {
        Self {
            held: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }
}

impl crate::harness::knowlege_dataset_nav::NavKbLock for NavKbMutex {
    fn acquire(&self, kb_id: &str) -> bool {
        self.held
            .lock()
            .map(|mut held| held.insert(kb_id.to_string()))
            .unwrap_or(false)
    }

    fn release(&self, kb_id: &str) {
        if let Ok(mut held) = self.held.lock() {
            held.remove(kb_id);
        }
    }
}

/// Cap on the text summarised for one document: the generator needs a summary, and reading a
/// whole book into memory to summarise it would defeat the on-demand pipeline the rest of the
/// ingestion path follows.
const NAV_SUMMARY_CHAR_LIMIT: usize = 4000;

/// `POST /api/v1/datasets/{id}/navigation` — upstream `dataset_api.generate_dataset_nav`.
///
/// Builds (or incrementally updates) the dataset navigation tree from the documents' parsed
/// chunks. Every prerequisite is reported rather than skipped: a missing embedding or chat
/// model is a `code 102` naming which one, and a document without parsed chunks lands in
/// `skipped` with a reason, so a partial run never looks like a complete one.
pub async fn generate_navigation(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return data_error(&format!("You don't own the dataset {kb_id}."));
    }
    let Some(embedder) = state.embedder.clone() else {
        return data_error(
            "Navigation generation needs an embedding model. Configure one in Models and try again.",
        );
    };
    let Some(llm) = state.llm.clone() else {
        return data_error(
            "Navigation generation needs a chat model. Configure one in Models and try again.",
        );
    };
    let requested: Option<std::collections::HashSet<String>> = body
        .get("documents")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        });
    // Group the dataset's chunks per document, keeping only what the summariser needs.
    let mut texts: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    {
        let engine = state.engine.read().unwrap();
        for chunk in engine.to_vec() {
            if chunk.metadata.get("kb_id").map(String::as_str) != Some(kb_id.as_str()) {
                continue;
            }
            let doc_id = ["doc_id", "document_id", "doc_name"]
                .iter()
                .find_map(|key| chunk.metadata.get(*key).cloned())
                .unwrap_or_default();
            if doc_id.is_empty() {
                continue;
            }
            if let Some(scope) = requested.as_ref()
                && !scope.contains(&doc_id)
            {
                continue;
            }
            let entry = texts.entry(doc_id).or_default();
            if entry.len() < NAV_SUMMARY_CHAR_LIMIT {
                if !entry.is_empty() {
                    entry.push('\n');
                }
                let remaining = NAV_SUMMARY_CHAR_LIMIT.saturating_sub(entry.len());
                entry.extend(chunk.content.chars().take(remaining));
            }
        }
    }
    let mut skipped: Vec<Value> = Vec::new();
    if let Some(scope) = requested.as_ref() {
        for doc_id in scope {
            if !texts.contains_key(doc_id) {
                skipped.push(serde_json::json!({
                    "doc_id": doc_id,
                    "reason": "no parsed chunks in this dataset"
                }));
            }
        }
    }
    if texts.is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "data": {
                "generated": 0,
                "skipped": skipped,
                "total": 0,
                "message": "No parsed chunks in this dataset; nothing to build a navigation tree from."
            }
        }))
        .into_response();
    }
    // The engine calls the store from inside an async function, where this handler cannot
    // wrap those calls: `OffThreadDocStore` runs each one on its own OS thread instead, so the
    // synchronous backend never runs on a tokio worker (see the panic this replaced).
    let store = crate::doc_store::OffThreadDocStore::new(std::sync::Arc::clone(&state.doc_store));
    let chat = crate::llm::LlmHarnessChat::new(llm);
    let bridge = crate::embed::EmbedderBridge::new(embedder);
    let lock = NavKbMutex::new();
    let total = texts.len();
    let mut generated = 0usize;
    for (doc_id, text) in texts {
        crate::harness::knowlege_dataset_nav::upsert_dataset_nav_doc(
            &store,
            Some(&bridge as &dyn crate::structure_compile::EmbeddingBackend),
            Some(&chat),
            &lock,
            &auth.user_id,
            &kb_id,
            &doc_id,
            &Value::String(text),
        )
        .await;
        generated += 1;
    }
    Json(serde_json::json!({
        "code": 0,
        "data": { "generated": generated, "skipped": skipped, "total": total }
    }))
    .into_response()
}
