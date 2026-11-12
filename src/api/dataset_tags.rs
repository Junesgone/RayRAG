//! Dataset tag management (upstream `dataset_api.list_tags` / `delete_tags` /
//! `rename_tag` in `api/apps/restful_apis/dataset_api.py`).
//!
//! Tags live in the per-document metadata under the `tags` key (a list of
//! strings), so every operation aggregates across the dataset's documents:
//! - `GET    /api/v1/datasets/{id}/tags` → `[[tag, count], …]` (count = number of
//!   documents carrying the tag, sorted by count desc then name, like the React
//!   word cloud which sorts `b[1] - a[1]`).
//! - `DELETE /api/v1/datasets/{id}/tags` body `{"tags": [...]}`.
//! - `PUT    /api/v1/datasets/{id}/tags` body `{"from_tag", "to_tag"}`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::server::AuthContext;
use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::server::AppState;

/// Upstream metadata key holding a document's tag list.
pub const TAGS_KEY: &str = "tags";

#[derive(Debug, Deserialize)]
pub struct DeleteTagsRequest {
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct RenameTagRequest {
    #[serde(default)]
    pub from_tag: Option<String>,
    #[serde(default)]
    pub to_tag: Option<String>,
}

/// The `tag_kwd` values on one chunk: upstream stores a list, RayRAG keeps the
/// doc-store representation (a JSON array string or a comma-joined string).
fn chunk_tags(metadata: &std::collections::HashMap<String, String>) -> Vec<String> {
    let Some(raw) = metadata.get("tag_kwd") else {
        return Vec::new();
    };
    let trimmed = raw.trim();
    if trimmed.starts_with('[') {
        return serde_json::from_str::<Vec<String>>(trimmed).unwrap_or_default();
    }
    trimmed
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

fn set_chunk_tags(metadata: &mut std::collections::HashMap<String, String>, tags: &[String]) {
    let raw = metadata.get("tag_kwd").cloned().unwrap_or_default();
    if raw.trim().starts_with('[') {
        metadata.insert(
            "tag_kwd".into(),
            serde_json::to_string(tags).unwrap_or_else(|_| "[]".into()),
        );
    } else {
        metadata.insert("tag_kwd".into(), tags.join(","));
    }
}

/// Aggregate the dataset's chunk-level tag counts. Upstream `retriever.all_tags`
/// runs an aggregation over the chunks' `tag_kwd` field — document metadata tags
/// are a different concept and do not feed this endpoint.
pub fn tag_counts(state: &AppState, dataset_id: &str) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for chunk in state.engine.read().unwrap().chunks_for_kb(dataset_id) {
        for tag in chunk_tags(&chunk.metadata) {
            *counts.entry(tag).or_default() += 1;
        }
    }
    let mut counts: Vec<(String, usize)> = counts.into_iter().collect();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts
}

/// Rewrite the chunks' `tag_kwd` lists with `rewrite` applied. Returns the number
/// of chunks changed. The index snapshot is persisted so a restart keeps the tags.
pub fn rewrite_tags(
    state: &AppState,
    dataset_id: &str,
    rewrite: impl Fn(&[String]) -> Vec<String>,
) -> anyhow::Result<usize> {
    let mut engine = state.engine.write().unwrap();
    let mut chunks = engine.to_vec();
    let mut changed = 0;
    for chunk in &mut chunks {
        if chunk.metadata.get("kb_id").map(String::as_str) != Some(dataset_id) {
            continue;
        }
        let current = chunk_tags(&chunk.metadata);
        if current.is_empty() {
            continue;
        }
        let next = rewrite(&current);
        if next == current {
            continue;
        }
        set_chunk_tags(&mut chunk.metadata, &next);
        changed += 1;
    }
    if changed > 0 {
        *engine = crate::search::SearchEngine::from_chunks(chunks);
        engine.save(&state.index_path)?;
    }
    Ok(changed)
}

/// GET /api/v1/datasets/{id}/tags
pub async fn list_tags(
    State(state): State<Arc<AppState>>,
    Path(dataset_id): Path<String>,
) -> Response {
    if state.kbs.get(&dataset_id).is_none() {
        // Upstream list_tags answers the bare "no authorization" data error.
        return Json(serde_json::json!({"code": 102, "message": "no authorization"}))
            .into_response();
    }
    let data: Vec<serde_json::Value> = tag_counts(&state, &dataset_id)
        .into_iter()
        .map(|(tag, count)| serde_json::json!([tag, count]))
        .collect();
    // get_result(data=tags): success carries no message key.
    Json(serde_json::json!({"code": 0, "data": data})).into_response()
}

/// `GET /api/v1/datasets/tags/aggregation?dataset_ids=a,b` — upstream
/// `dataset_api.aggregate_tags`, the tag counter behind the dataset list's tag filter.
///
/// Upstream validates every dataset against the tenant and merges the per-dataset counts
/// into `[{"value": tag, "count": n}]`.
pub async fn aggregate_tags(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let dataset_ids: Vec<String> = query
        .get("dataset_ids")
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if dataset_ids.is_empty() {
        return Json(serde_json::json!({
            "code": 102,
            "message": "Lack of dataset_ids in query parameters"
        }))
        .into_response();
    }
    for dataset_id in &dataset_ids {
        let accessible = state
            .kbs
            .get(dataset_id)
            .is_some_and(|kb| kb.owner_id == auth.user_id || auth.is_admin);
        if !accessible {
            return Json(serde_json::json!({
                "code": 102,
                "message": format!("No authorization for dataset '{dataset_id}'")
            }))
            .into_response();
        }
    }
    let mut merged: BTreeMap<String, usize> = BTreeMap::new();
    for dataset_id in &dataset_ids {
        for (tag, count) in tag_counts(&state, dataset_id) {
            *merged.entry(tag).or_default() += count;
        }
    }
    let data: Vec<serde_json::Value> = merged
        .into_iter()
        .map(|(tag, count)| serde_json::json!({ "value": tag, "count": count }))
        .collect();
    Json(serde_json::json!({ "code": 0, "data": data })).into_response()
}

/// DELETE /api/v1/datasets/{id}/tags
pub async fn delete_tags(
    State(state): State<Arc<AppState>>,
    Path(dataset_id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if state.kbs.get(&dataset_id).is_none() {
        return Json(serde_json::json!({"code": 102, "message": "no authorization"}))
            .into_response();
    }
    let parsed = serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default();
    let Some(value) = parsed.get("tags") else {
        return Json(serde_json::json!({"code": 102, "message": "Lack of tags in request body"}))
            .into_response();
    };
    let Some(items) = value.as_array() else {
        return Json(serde_json::json!({"code": 101, "message": "tags must be a list of strings"}))
            .into_response();
    };
    let mut tags: Vec<String> = Vec::new();
    for item in items {
        let Some(tag) = item.as_str() else {
            return Json(
                serde_json::json!({"code": 101, "message": "tags must be a list of strings"}),
            )
            .into_response();
        };
        tags.push(tag.to_string());
    }
    if tags.is_empty() {
        return Json(serde_json::json!({"code": 0, "data": {}})).into_response();
    }
    let removed: std::collections::HashSet<&str> = tags.iter().map(|tag| tag.trim()).collect();
    match rewrite_tags(&state, &dataset_id, |current| {
        current
            .iter()
            .filter(|tag| !removed.contains(tag.trim()))
            .cloned()
            .collect()
    }) {
        Ok(_changed) => Json(serde_json::json!({"code": 0, "data": {}})).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"code": 500, "message": error.to_string()})),
        )
            .into_response(),
    }
}

/// PUT /api/v1/datasets/{id}/tags
pub async fn rename_tag(
    State(state): State<Arc<AppState>>,
    Path(dataset_id): Path<String>,
    body: axum::body::Bytes,
) -> Response {
    if state.kbs.get(&dataset_id).is_none() {
        return Json(serde_json::json!({"code": 102, "message": "no authorization"}))
            .into_response();
    }
    let parsed = serde_json::from_slice::<serde_json::Value>(&body).unwrap_or_default();
    let (Some(from_value), Some(to_value)) = (parsed.get("from_tag"), parsed.get("to_tag")) else {
        return Json(
            serde_json::json!({"code": 102, "message": "Lack of from_tag or to_tag in request body"}),
        )
        .into_response();
    };
    let (Some(from_tag), Some(to_tag)) = (from_value.as_str(), to_value.as_str()) else {
        return Json(
            serde_json::json!({"code": 101, "message": "from_tag and to_tag must be strings"}),
        )
        .into_response();
    };
    let (from_tag, to_tag) = (from_tag.trim().to_string(), to_tag.trim().to_string());
    if from_tag.is_empty() || to_tag.is_empty() {
        return Json(
            serde_json::json!({"code": 101, "message": "from_tag and to_tag must not be empty"}),
        )
        .into_response();
    }
    match rewrite_tags(&state, &dataset_id, |current| {
        let mut next: Vec<String> = Vec::with_capacity(current.len());
        for tag in current {
            let value = if tag.trim() == from_tag {
                to_tag.clone()
            } else {
                tag.clone()
            };
            if !next.contains(&value) {
                next.push(value);
            }
        }
        next
    }) {
        // Upstream rename_tag returns {"from": from_tag, "to": to_tag}.
        Ok(_changed) => Json(serde_json::json!({
            "code": 0,
            "data": { "from": from_tag, "to": to_tag }
        }))
        .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"code": 500, "message": error.to_string()})),
        )
            .into_response(),
    }
}

/// Convenience wrapper used by tests and the dataset summary endpoint.
pub fn tag_summary(state: &AppState, dataset_id: &str) -> BTreeMap<String, usize> {
    tag_counts(state, dataset_id).into_iter().collect()
}
