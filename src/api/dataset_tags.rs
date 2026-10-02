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

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
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

/// Aggregate the dataset's tag counts from document metadata.
pub fn tag_counts(state: &AppState, dataset_id: &str) -> Vec<(String, usize)> {
    let flattened = state.document_metadata.flattened(&[dataset_id.to_string()]);
    let mut counts: Vec<(String, usize)> = flattened
        .get(TAGS_KEY)
        .map(|values| {
            values
                .iter()
                .map(|(tag, doc_ids)| (tag.clone(), doc_ids.len()))
                .collect()
        })
        .unwrap_or_default();
    // Upstream `TagWordCloud` sorts by count desc; ties keep a stable name order.
    counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts
}

fn tags_of(metadata: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    match metadata.get(TAGS_KEY) {
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(serde_json::Value::String(value)) => value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// Rewrite every document's tag list with `rewrite` applied. Returns the number
/// of documents whose tags changed.
pub fn rewrite_tags(
    state: &AppState,
    dataset_id: &str,
    rewrite: impl Fn(&[String]) -> Vec<String>,
) -> anyhow::Result<usize> {
    // Candidate documents come from the metadata store itself (a record may exist
    // without a document row), which is also what `flattened` aggregates.
    let flattened = state.document_metadata.flattened(&[dataset_id.to_string()]);
    let doc_ids: Vec<String> = flattened
        .get(TAGS_KEY)
        .map(|values| {
            values
                .values()
                .flatten()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap_or_default();
    let mut changed = 0;
    for doc_id in doc_ids {
        let Some(metadata) = state.document_metadata.get(&doc_id, dataset_id) else {
            continue;
        };
        let current = tags_of(&metadata);
        if current.is_empty() {
            continue;
        }
        let next = rewrite(&current);
        if next == current {
            continue;
        }
        // `batch_update` merges array values (upstream metadata upsert), so a tag
        // rewrite replaces the whole record instead.
        let mut rewritten = metadata.clone();
        rewritten.insert(
            TAGS_KEY.to_string(),
            serde_json::Value::Array(next.into_iter().map(serde_json::Value::String).collect()),
        );
        state
            .document_metadata
            .replace(&doc_id, dataset_id, rewritten)?;
        changed += 1;
    }
    Ok(changed)
}

/// GET `/api/v1/datasets/{id}/tags`
pub async fn list_tags(
    State(state): State<Arc<AppState>>,
    Path(dataset_id): Path<String>,
) -> Response {
    if state.kbs.get(&dataset_id).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"code": 102, "message": "Dataset not found"})),
        )
            .into_response();
    }
    let data: Vec<serde_json::Value> = tag_counts(&state, &dataset_id)
        .into_iter()
        .map(|(tag, count)| serde_json::json!([tag, count]))
        .collect();
    Json(serde_json::json!({"code": 0, "message": "ok", "data": data})).into_response()
}

/// DELETE `/api/v1/datasets/{id}/tags`
pub async fn delete_tags(
    State(state): State<Arc<AppState>>,
    Path(dataset_id): Path<String>,
    Json(body): Json<DeleteTagsRequest>,
) -> Response {
    let Some(tags) = body.tags else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"code": 102, "message": "Lack of tags in request body"})),
        )
            .into_response();
    };
    if !tags.iter().all(|tag| !tag.trim().is_empty()) || tags.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"code": 102, "message": "tags must be a list of strings"})),
        )
            .into_response();
    }
    let removed: std::collections::HashSet<&str> = tags.iter().map(|tag| tag.trim()).collect();
    match rewrite_tags(&state, &dataset_id, |current| {
        current
            .iter()
            .filter(|tag| !removed.contains(tag.trim()))
            .cloned()
            .collect()
    }) {
        Ok(changed) => {
            Json(serde_json::json!({"code": 0, "message": "ok", "data": changed})).into_response()
        }
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"code": 500, "message": error.to_string()})),
        )
            .into_response(),
    }
}

/// PUT `/api/v1/datasets/{id}/tags`
pub async fn rename_tag(
    State(state): State<Arc<AppState>>,
    Path(dataset_id): Path<String>,
    Json(body): Json<RenameTagRequest>,
) -> Response {
    let (Some(from_tag), Some(to_tag)) = (body.from_tag, body.to_tag) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::json!({"code": 102, "message": "Lack of from_tag or to_tag in request body"}),
            ),
        )
            .into_response();
    };
    let (from_tag, to_tag) = (from_tag.trim().to_string(), to_tag.trim().to_string());
    if from_tag.is_empty() || to_tag.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 102,
                "message": "from_tag and to_tag must not be empty"
            })),
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
        Ok(changed) => {
            Json(serde_json::json!({"code": 0, "message": "ok", "data": changed})).into_response()
        }
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
