//! Dataset artifacts (RAGFlow `dataset_api.py` artifact surface over the compiled wiki).
//!
//! The wiki engines ([`crate::knowlege_wiki`], [`crate::knowlege_wiki_incremental`],
//! [`crate::knowlege_dataset_nav`]) were ported with their own tests long before there was
//! a place to put their rows: they read and write through
//! [`crate::doc_store::DocStore`], and nothing in the running server owned one. The store
//! now lives on `AppState` as a shared [`DocStorePool`](crate::doc_store::DocStorePool), so
//! these handlers read exactly the rows those engines write — no second copy of the data,
//! and no endpoint that can only ever answer "empty".
//!
//! Rows are `compile_kwd = wiki_page` documents in the tenant index, keyed by `slug_kwd`
//! (matching upstream's `_wiki_index_or_none`), carrying `title_kwd`, `page_type_kwd` and
//! the page body.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::harness::knowlege_dataset_nav::index_name;
use crate::harness::knowlege_wiki_incremental::{
    WIKI_PAGE_COMPILE_KWD, search_existing_pages, wiki_has_any_pages,
};
use crate::server::{AppState, AuthContext, kb_accessible};

/// Upstream `DEFAULT_PAGE_SIZE` for the artifact list.
const DEFAULT_PAGE_SIZE: usize = 200;
/// Upstream clamps the artifact list page size to 1000.
const MAX_PAGE_SIZE: usize = 1000;

/// The page fields the list needs (upstream `select_fields`).
fn list_fields() -> Vec<String> {
    ["slug_kwd", "title_kwd", "page_type_kwd", "outlinks_int"]
        .iter()
        .map(|field| (*field).to_string())
        .collect()
}

/// Upstream `_scalar`: a `*_kwd` field may arrive as a list, take its first non-empty value.
fn scalar(row: &Value, key: &str) -> String {
    match row.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .find_map(|item| item.as_str().filter(|text| !text.is_empty()))
            .unwrap_or_default()
            .to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// Run one compiled-artifact operation on a blocking thread.
///
/// The doc store backends are **synchronous**: the PostgreSQL driver runs its own
/// `block_on`, and both connecting *and querying* from a tokio worker panic with "Cannot
/// start a runtime from within a runtime". Every read and write therefore happens inside
/// `spawn_blocking`, on the same pool instance so the connection stays cached.
async fn with_store<T, F>(state: &Arc<AppState>, work: F) -> Result<T, Response>
where
    F: FnOnce(&dyn crate::doc_store::DocStore) -> anyhow::Result<T> + Send + 'static,
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
            tracing::error!(%error, "compiled-artifact store operation failed");
            Err(store_error(&error))
        }
        Err(error) => {
            tracing::error!(%error, "compiled-artifact store task failed");
            Err(store_error(&anyhow::anyhow!("store task failed: {error}")))
        }
    }
}

fn store_error(error: &anyhow::Error) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "code": 500,
            "message": format!("Artifact store unavailable: {error}")
        })),
    )
        .into_response()
}

fn invalid_ids(message: &str) -> Response {
    Json(serde_json::json!({ "code": 102, "message": message })).into_response()
}

/// `HEAD /api/v1/datasets/{id}/artifacts` — upstream `has_any_wiki`.
///
/// The dataset sidebar shows the Artifact tab only when this answers 200; a dataset with no
/// compiled pages answers 404, exactly like upstream.
pub async fn has_any_wiki(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    match with_store(&state, move |store| {
        Ok(wiki_has_any_pages(store, &tenant, &dataset))
    })
    .await
    {
        Ok(true) => StatusCode::OK.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(response) => response,
    }
}

/// `GET /api/v1/datasets/{id}/artifacts/{page_type}` — upstream's path-shaped variant of the
/// same listing (`structure`, `graph`, `alteration`, `wiki`, …).
///
/// It reuses [`list_artifacts`] verbatim with the path segment folded into the query, so the two
/// forms cannot drift apart. An unknown page type is reported with the types that do exist
/// rather than answering an empty list, which would look like "this dataset has none".
pub async fn list_artifacts_by_type(
    state: State<Arc<AppState>>,
    auth: axum::extract::Extension<AuthContext>,
    Path((kb_id, page_type)): Path<(String, String)>,
    Query(mut query): Query<HashMap<String, String>>,
) -> Response {
    let requested = page_type.trim().to_lowercase();
    if !matches!(
        requested.as_str(),
        "structure" | "graph" | "alteration" | "wiki" | "topic" | "summary"
    ) {
        return invalid_ids(&format!(
            "Unknown artifact page type '{page_type}'. Supported: structure, graph, alteration, wiki, topic, summary."
        ));
    }
    query.insert("page_type".to_string(), requested);
    list_artifacts(state, auth, Path(kb_id), Query(query)).await
}

/// Shared body of the literal `/artifacts/<type>` routes upstream declares one by one.
async fn list_literal_artifact_type(
    state: State<Arc<AppState>>,
    auth: axum::extract::Extension<AuthContext>,
    kb_id: String,
    page_type: &str,
    query: HashMap<String, String>,
) -> Response {
    let mut query = query;
    query.insert("page_type".to_string(), page_type.to_string());
    list_artifacts(state, auth, Path(kb_id), Query(query)).await
}

/// `GET /api/v1/datasets/{id}/artifacts/structure` — upstream literal route.
pub async fn list_structure_artifacts(
    state: State<Arc<AppState>>,
    auth: axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    list_literal_artifact_type(state, auth, kb_id, "structure", query).await
}

/// `DELETE /api/v1/datasets/{id}/artifacts/structure` — upstream clears **only** the structure
/// artifacts of a dataset.
///
/// RayRAG's `DELETE /datasets/{id}/artifacts` clears every artifact page; this one filters on the
/// row's `page_type_kwd`, the same field the listing filters on (and the same condition style
/// `clear_artifacts` already uses). It reports how many rows went away, and says so plainly when
/// there were none rather than returning a bare success.
pub async fn delete_structure_artifacts(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return invalid_ids(&format!("You don't own the dataset {kb_id}."));
    }
    let mut condition: crate::doc_store::FilterCondition = Default::default();
    condition.insert(
        "page_type_kwd".to_string(),
        Value::String("structure".to_string()),
    );
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let index = index_name(&tenant);
    match with_store(&state, move |store| {
        store.delete(&condition, &index, &dataset)
    })
    .await
    {
        Ok(removed) => Json(serde_json::json!({
            "code": 0,
            "data": {
                "dataset_id": kb_id,
                "page_type": "structure",
                "removed": removed,
                "message": if removed == 0 {
                    "No structure artifacts were stored for this dataset."
                } else {
                    "Structure artifacts removed."
                }
            }
        }))
        .into_response(),
        Err(response) => response,
    }
}

/// `GET /api/v1/datasets/{id}/artifacts/graph` — upstream literal route.
pub async fn list_graph_artifacts(
    state: State<Arc<AppState>>,
    auth: axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    list_literal_artifact_type(state, auth, kb_id, "graph", query).await
}

/// `GET /api/v1/datasets/{id}/artifacts/alteration` — upstream literal route.
pub async fn list_alteration_artifacts(
    state: State<Arc<AppState>>,
    auth: axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    list_literal_artifact_type(state, auth, kb_id, "alteration", query).await
}

/// `GET /api/v1/datasets/{id}/artifacts` — upstream `list_wiki_pages`.
///
/// `{"total": n, "items": [{"slug", "title", "page_type"}]}`, ordered by page type then
/// title so pages of one kind stay grouped.
pub async fn list_artifacts(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return invalid_ids(&format!("You don't own the dataset {kb_id}."));
    }
    let page = query
        .get("page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let page_size = query
        .get("page_size")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .clamp(1, MAX_PAGE_SIZE);
    let page_type = query
        .get("page_type")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let keywords = query
        .get("keywords")
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    let topic = query
        .get("topic")
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty());

    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let pages = match with_store(&state, move |store| {
        Ok(search_existing_pages(
            store,
            &tenant,
            &dataset,
            &list_fields(),
        ))
    })
    .await
    {
        Ok(pages) => pages,
        Err(response) => return response,
    };
    let mut items: Vec<Value> = pages
        .into_values()
        .filter_map(|row| {
            let slug = scalar(&row, "slug_kwd");
            if slug.is_empty() {
                return None;
            }
            let title = {
                let title = scalar(&row, "title_kwd");
                if title.is_empty() {
                    slug.clone()
                } else {
                    title
                }
            };
            let page_type_of_row = {
                let kind = scalar(&row, "page_type_kwd");
                if kind.is_empty() {
                    "concept".to_string()
                } else {
                    kind
                }
            };
            if let Some(wanted) = page_type.as_ref()
                && &page_type_of_row != wanted
            {
                return None;
            }
            if !keywords.is_empty()
                && !slug.to_lowercase().contains(&keywords)
                && !title.to_lowercase().contains(&keywords)
            {
                return None;
            }
            if let Some(topic) = topic.as_ref()
                && !page_type_of_row.eq_ignore_ascii_case("topic")
                && !slug.to_lowercase().contains(topic)
                && !title.to_lowercase().contains(topic)
            {
                return None;
            }
            Some(serde_json::json!({
                "slug": slug,
                "title": title,
                "page_type": page_type_of_row,
            }))
        })
        .collect();
    items.sort_by(|left, right| {
        let left_type = left["page_type"].as_str().unwrap_or_default();
        let right_type = right["page_type"].as_str().unwrap_or_default();
        left_type.cmp(right_type).then_with(|| {
            left["title"]
                .as_str()
                .unwrap_or_default()
                .cmp(right["title"].as_str().unwrap_or_default())
        })
    });
    let total = items.len();
    let offset = (page - 1) * page_size;
    let items: Vec<Value> = items.into_iter().skip(offset).take(page_size).collect();
    Json(serde_json::json!({
        "code": 0,
        "data": { "total": total, "items": items }
    }))
    .into_response()
}

/// `GET /api/v1/datasets/{id}/artifacts/{page_type}/{slug}` — one artifact page.
pub async fn get_artifact_page(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path((kb_id, page_type, slug)): Path<(String, String, String)>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return invalid_ids(&format!("You don't own the dataset {kb_id}."));
    }
    let fields: Vec<String> = [
        "slug_kwd",
        "title_kwd",
        "page_type_kwd",
        "md_with_weight",
        "summary_with_weight",
        "entity_names_kwd",
        "source_doc_ids",
        "source_chunk_ids",
    ]
    .iter()
    .map(|field| (*field).to_string())
    .collect();
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let pages = match with_store(&state, move |store| {
        Ok(search_existing_pages(store, &tenant, &dataset, &fields))
    })
    .await
    {
        Ok(pages) => pages,
        Err(response) => return response,
    };
    // Upstream routes the page as `<page_type>/<path:slug>` and reassembles the full slug
    // the same way (`f"{page_type}/{slug}"`), so `/artifacts/topic/rust` finds
    // `topic/rust` rather than a page literally called `rust`.
    let full_slug = if slug.contains('/') {
        slug.clone()
    } else {
        format!("{page_type}/{slug}")
    };
    let Some(row) = pages.get(&full_slug) else {
        return (
            StatusCode::NOT_FOUND,
            Json(
                serde_json::json!({ "code": 102, "message": format!("Page not found: {full_slug}") }),
            ),
        )
            .into_response();
    };
    let stored_type = scalar(row, "page_type_kwd");
    if !stored_type.is_empty() && stored_type != page_type {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 102,
                "message": format!("Page {slug} is a {stored_type}, not a {page_type}")
            })),
        )
            .into_response();
    }
    let title = {
        let title = scalar(row, "title_kwd");
        if title.is_empty() {
            slug.clone()
        } else {
            title
        }
    };
    let page_type = if stored_type.is_empty() {
        page_type
    } else {
        stored_type
    };
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "slug": full_slug,
            "title": title,
            "page_type": page_type,
            "content": scalar(row, "md_with_weight"),
            "summary": scalar(row, "summary_with_weight"),
            "entity_names": row.get("entity_names_kwd").cloned().unwrap_or(Value::Null),
            "source_doc_ids": row.get("source_doc_ids").cloned().unwrap_or(Value::Null),
            "source_chunk_ids": row.get("source_chunk_ids").cloned().unwrap_or(Value::Null),
        }
    }))
    .into_response()
}

/// `PUT /api/v1/datasets/{id}/artifacts/{page_type}/{slug}` — upstream `update_wiki_page`:
/// save an edited page. Body `{"content_md": "...", "title"?, "comments"?}`.
///
/// The row is addressed by `slug_kwd` and updated **by row id**, so a partial update
/// cannot accidentally rewrite a sibling page. Two upstream side effects are deliberately
/// not reproduced and are stated here rather than silently skipped: the `[[slug]]` link
/// rendering (RayRAG stores the markdown as written) and the `file_commit` edit record
/// (that service is not ported — see the API worklist).
pub async fn update_artifact_page(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path((kb_id, page_type, slug)): Path<(String, String, String)>,
    Json(body): Json<Value>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return invalid_ids(&format!("You don't own the dataset {kb_id}."));
    }
    let Some(body) = body.as_object() else {
        return invalid_ids("Body must be a JSON object.");
    };
    let Some(content_md) = body.get("content_md").and_then(|value| value.as_str()) else {
        return invalid_ids("'content_md' must be a string.");
    };
    if body.get("title").is_some_and(|value| !value.is_string()) {
        return invalid_ids("'title' must be a string.");
    }
    if body.get("comments").is_some_and(|value| !value.is_string()) {
        return invalid_ids("'comments' must be a string.");
    }
    let full_slug = if slug.contains('/') {
        slug.clone()
    } else {
        format!("{page_type}/{slug}")
    };
    let fields: Vec<String> = ["slug_kwd", "title_kwd"]
        .iter()
        .map(|f| (*f).to_string())
        .collect();
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let pages = match with_store(&state, move |store| {
        Ok(search_existing_pages(store, &tenant, &dataset, &fields))
    })
    .await
    {
        Ok(pages) => pages,
        Err(response) => return response,
    };
    let Some(row) = pages.get(&full_slug) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 102,
                "message": format!("Page not found: {full_slug}")
            })),
        )
            .into_response();
    };
    let row_id = scalar(row, "id");
    if row_id.is_empty() {
        return invalid_ids("Page row has no id; refusing to update by condition.");
    }
    let mut payload: crate::doc_store::DocRow = Default::default();
    payload.insert(
        "md_with_weight".to_string(),
        Value::String(content_md.to_string()),
    );
    if let Some(title) = body.get("title").and_then(|value| value.as_str()) {
        payload.insert("title_kwd".to_string(), Value::String(title.to_string()));
    }
    if let Some(comments) = body.get("comments").and_then(|value| value.as_str()) {
        payload.insert(
            "comments_with_weight".to_string(),
            Value::String(comments.to_string()),
        );
    }
    let condition: crate::doc_store::FilterCondition =
        [("id".to_string(), Value::String(row_id.clone()))]
            .into_iter()
            .collect();
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let index = index_name(&tenant);
    let saved = with_store(&state, move |store| {
        store
            .update(&condition, &payload, &index, &dataset)
            .map(|_| ())
    })
    .await;
    match saved {
        Ok(()) => Json(serde_json::json!({
            "code": 0,
            "data": { "slug": full_slug, "page_type": page_type, "saved": true }
        }))
        .into_response(),
        Err(response) => response,
    }
}

/// `DELETE /api/v1/datasets/{id}/artifacts` — upstream `clear_wiki`: drop every compiled
/// page of this dataset (the wiki can be regenerated; a partially deleted index cannot be
/// trusted).
pub async fn clear_artifacts(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return invalid_ids(&format!("You don't own the dataset {kb_id}."));
    }
    let mut condition: crate::doc_store::FilterCondition = Default::default();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let index = index_name(&tenant);
    match with_store(&state, move |store| {
        store.delete(&condition, &index, &dataset)
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

/// `GET /api/v1/datasets/{id}/artifacts/topics` — upstream `list_wiki_topics`:
/// the topic pages, which the Artifact tab renders as the right-hand tree.
pub async fn list_artifact_topics(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return invalid_ids(&format!("You don't own the dataset {kb_id}."));
    }
    let keywords = query
        .get("keywords")
        .map(|value| value.trim().to_lowercase())
        .unwrap_or_default();
    let tenant = auth.user_id.clone();
    let dataset = kb_id.clone();
    let pages = match with_store(&state, move |store| {
        Ok(search_existing_pages(
            store,
            &tenant,
            &dataset,
            &list_fields(),
        ))
    })
    .await
    {
        Ok(pages) => pages,
        Err(response) => return response,
    };
    let mut items: Vec<Value> = pages
        .into_values()
        .filter_map(|row| {
            let slug = scalar(&row, "slug_kwd");
            let page_type = scalar(&row, "page_type_kwd");
            if slug.is_empty() || !page_type.eq_ignore_ascii_case("topic") {
                return None;
            }
            let title = {
                let title = scalar(&row, "title_kwd");
                if title.is_empty() {
                    slug.clone()
                } else {
                    title
                }
            };
            if !keywords.is_empty()
                && !slug.to_lowercase().contains(&keywords)
                && !title.to_lowercase().contains(&keywords)
            {
                return None;
            }
            let topic = slug.split('/').next_back().unwrap_or(&slug).to_string();
            Some(serde_json::json!({ "topic": topic, "title": title, "slug": slug }))
        })
        .collect();
    items.sort_by(|left, right| {
        left["title"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["title"].as_str().unwrap_or_default())
    });
    Json(serde_json::json!({
        "code": 0,
        "data": { "total": items.len(), "items": items }
    }))
    .into_response()
}

/// `POST /api/v1/datasets/{id}/artifacts/compile` — run the wiki/artifact compilation.
///
/// Upstream's artifact generation is **two phases**, and skipping either one produces a silent
/// no-op: the MAP phase calls the model per document and **stores** the extracts, and the
/// COMPILE phase reads those stored extracts back
/// (`knowlege_wiki::load_map_extracts_for_state`) to synthesise pages. So this handler maps
/// first, then compiles, and reports both steps plus every document it could not map.
pub async fn compile_artifacts(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return invalid_ids(&format!("You don't own the dataset {kb_id}."));
    }
    let Some(chat_client) = state.llm.clone() else {
        return invalid_ids(
            "Artifact generation needs a chat model. Configure one in Models and try again.",
        );
    };
    let language = body
        .get("language")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| "en".to_string());
    // Group this dataset's chunks per document; the MAP phase consumes chunk text.
    let mut per_doc: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
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
            if doc_id.is_empty() || chunk.content.trim().is_empty() {
                continue;
            }
            per_doc
                .entry(doc_id)
                .or_default()
                .push(serde_json::json!({ "id": chunk.id, "text": chunk.content }));
        }
    }
    if per_doc.is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "data": {
                "mapped": 0,
                "skipped": [],
                "message": "No parsed chunks in this dataset; there is nothing to compile."
            }
        }))
        .into_response();
    }
    let store = crate::doc_store::OffThreadDocStore::new(Arc::clone(&state.doc_store));
    let chat = crate::llm::LlmHarnessChat::new(chat_client);
    // Phase 1 — MAP: model per document, extracts stored for phase 2.
    let mut mapped = 0usize;
    let mut extracts = 0usize;
    let mut failed_docs = 0usize;
    let mut skipped: Vec<Value> = Vec::new();
    let mut all_chunk_ids: BTreeSet<String> = BTreeSet::new();
    let total = per_doc.len();
    for (doc_id, chunks) in &per_doc {
        for chunk in chunks {
            if let Some(id) = chunk.get("id").and_then(Value::as_str) {
                all_chunk_ids.insert(id.to_string());
            }
        }
        let mapped_doc = crate::harness::knowlege_wiki::wiki_map_from_chunks(
            &store,
            &chat,
            chunks,
            doc_id,
            &auth.user_id,
            &kb_id,
            &language,
            1,
            120,
            &serde_json::json!({}),
            None,
            None,
            None,
            None,
        )
        .await;
        // Report what MAP actually found. Without this, a compile that produces no pages is
        // ambiguous: "the model extracted nothing" and "COMPILE never saw the extracts" both
        // come back as `pages_created: 0`.
        let per_doc_extracts: usize = crate::harness::knowlege_wiki::EXTRACT_LIST_KEYS
            .iter()
            .map(|key| {
                mapped_doc
                    .get(*key)
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0)
            })
            .sum::<usize>()
            + mapped_doc
                .get("topics")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0);
        if per_doc_extracts == 0 {
            // `_meta.requested > 0` with nothing extracted means the extraction did not
            // produce anything: that is a failure, not "this document has no entities". The
            // earlier wording called it a data result, which was wrong - a document full of
            // named entities came back empty because the model call itself had failed.
            let requested = mapped_doc
                .get("_meta")
                .and_then(|meta| meta.get("requested"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let (reason, failed) = if requested > 0 {
                (
                    "extraction produced nothing: the model call or its JSON answer failed (see the server log)",
                    true,
                )
            } else {
                ("the document had no chunk text to extract from", false)
            };
            if failed {
                failed_docs += 1;
                tracing::warn!(%doc_id, %kb_id, "artifact MAP extraction produced nothing");
            }
            skipped.push(serde_json::json!({ "doc_id": doc_id, "reason": reason }));
        }
        extracts += per_doc_extracts;
        mapped += 1;
    }
    // Phase 2 — COMPILE: reads the extracts phase 1 just stored.
    let doc_ids: BTreeSet<String> = per_doc.keys().cloned().collect();
    let current = match crate::harness::knowlege_wiki::scan_current_chunk_state(
        &store,
        &auth.user_id,
        &kb_id,
        &doc_ids,
    ) {
        Ok(state) => state,
        Err(error) => return store_error(&error),
    };
    // `load_map_versions` returns the map directly (no Result) and takes the requested
    // versions as an optional map; `None` means "every stored version".
    let previous = crate::harness::knowlege_wiki::load_map_versions(
        &store,
        &auth.user_id,
        &kb_id,
        &doc_ids,
        None,
    );
    let mut delta: serde_json::Map<String, Value> = serde_json::Map::new();
    delta.insert(
        "new_chunk_ids".to_string(),
        Value::Array(all_chunk_ids.into_iter().map(Value::String).collect()),
    );
    delta.insert("changed_chunk_ids".to_string(), Value::Array(Vec::new()));
    delta.insert("deleted_chunk_ids".to_string(), Value::Array(Vec::new()));
    let outcome = crate::harness::knowlege_wiki_incremental::wiki_compile_incremental(
        &store,
        &chat,
        state.embedder.as_deref(),
        &auth.user_id,
        &kb_id,
        "generate",
        &delta,
        &previous,
        &current,
        false,
        None,
        &BTreeSet::new(),
        None,
    )
    .await;
    Json(serde_json::json!({
        "code": 0,
        "data": { "mapped": mapped, "total": total, "extracts": extracts, "failed": failed_docs, "skipped": skipped, "compile": outcome }
    }))
    .into_response()
}
