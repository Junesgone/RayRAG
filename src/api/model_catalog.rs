//! `/api/v1/model-catalog/*` — the public model catalogue behind the provider dialogs.
//!
//! Filling in a custom provider means answering questions the operator often cannot:
//! which model types does this model serve, is it vision-capable, does it take tool
//! calls, how large is its context, what does it cost per million tokens? The
//! catalogue published at [`crate::model_catalog::CATALOG_URL`] knows, so these
//! endpoints expose it to the browser:
//!
//! * `GET  /api/v1/model-catalog/status`    — what is cached and how old it is.
//! * `GET  /api/v1/model-catalog/providers` — the provider directory (and, with
//!   `?provider=`, that provider's models) so a dialog can suggest endpoints.
//! * `GET  /api/v1/model-catalog/lookup`    — look a model name up, optionally
//!   preferring the provider that matches the base URL the operator is typing.
//! * `POST /api/v1/model-catalog/refresh`   — admin-only, fetch a fresh copy now.
//!
//! Lookups read a cached copy and refresh it at most once a day, so the dialog never
//! pays for a 4.5 MB download per keystroke. Everything is read-only for
//! non-admins: the catalogue informs what the operator types, it never fills in a
//! credential.

use crate::api::common::ok_json;
use crate::model_catalog::{
    CatalogLookupEntry, CatalogProvider, CatalogStatus, ModelCatalog, cache_path, catalog, refresh,
    status,
};
use crate::server::{AppState, AuthContext};
use axum::{
    Json,
    extract::{Extension, Query, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Models returned for one provider when a dialog asks for a suggestion list.
const MAX_PROVIDER_MODELS: usize = 400;
/// Models returned by a free-text search.
const MAX_SEARCH_HITS: usize = 20;

/// One provider in the directory listing.
#[derive(Debug, Clone, Serialize)]
pub struct DirectoryEntry {
    pub id: String,
    pub name: String,
    pub website: String,
    pub api_base_url: String,
    pub models: usize,
}

impl DirectoryEntry {
    fn from_provider(provider: &CatalogProvider) -> Self {
        Self {
            id: provider.id.clone(),
            name: provider.name.clone(),
            website: provider.website.clone(),
            api_base_url: provider.api_base_url.clone(),
            models: provider.models.len(),
        }
    }
}

/// `GET /api/v1/model-catalog/status`
pub async fn catalog_status(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    let _ = auth;
    ok_json(status(&state.static_dir).await).into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct DirectoryQuery {
    /// Restrict the answer to one provider and list its models.
    #[serde(default)]
    pub provider: Option<String>,
    /// Instead of an id, find the provider by the endpoint being configured.
    #[serde(default)]
    pub base_url: Option<String>,
}

/// `GET /api/v1/model-catalog/providers`
pub async fn catalog_providers(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<DirectoryQuery>,
) -> Response {
    let _ = auth;
    let base_url = query
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Ok(catalog) = catalog(&state.static_dir).await else {
        // The directory is an aid, not a dependency: an unreachable catalogue answers
        // an empty list plus the reason instead of failing the dialog.
        let status = status(&state.static_dir).await;
        return Json(serde_json::json!({
            "code": 0,
            "data": { "catalog": status, "providers": Vec::<DirectoryEntry>::new(), "models": Vec::<serde_json::Value>::new() }
        }))
        .into_response();
    };
    let wanted = query
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        // A base URL names a provider too: the dialog only has the endpoint.
        .or_else(|| {
            base_url
                .and_then(|url| catalog.provider_for_base_url(url))
                .map(|provider| provider.id.clone())
        });
    let providers: Vec<DirectoryEntry> = match wanted.as_deref() {
        Some(id) => catalog
            .providers
            .iter()
            .filter(|provider| provider.id.eq_ignore_ascii_case(id))
            .map(DirectoryEntry::from_provider)
            .collect(),
        None => catalog
            .providers
            .iter()
            .map(DirectoryEntry::from_provider)
            .collect(),
    };
    let models: Vec<CatalogLookupEntry> = wanted
        .as_deref()
        .and_then(|id| {
            catalog
                .providers
                .iter()
                .find(|provider| provider.id.eq_ignore_ascii_case(id))
        })
        .map(|provider| provider_entries(provider, MAX_PROVIDER_MODELS))
        .unwrap_or_default();
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "catalog": catalog_summary(&catalog, &state.static_dir).await,
            "providers": providers,
            "models": models,
        }
    }))
    .into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct LookupQuery {
    /// Model name the operator typed.
    #[serde(default)]
    pub model: Option<String>,
    /// `q` is accepted as a synonym so a search box can call this endpoint directly.
    #[serde(default)]
    pub q: Option<String>,
    /// Base URL being configured; a match on its host ranks first.
    #[serde(default)]
    pub base_url: Option<String>,
}

/// `GET /api/v1/model-catalog/lookup`
pub async fn catalog_lookup(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<LookupQuery>,
) -> Response {
    let _ = auth;
    let needle = query
        .model
        .as_deref()
        .or(query.q.as_deref())
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    let base_url = query
        .base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if needle.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 400,
                "message": "A model name (model=) or search term (q=) is required"
            })),
        )
            .into_response();
    }
    let Ok(catalog) = catalog(&state.static_dir).await else {
        let status = status(&state.static_dir).await;
        return Json(serde_json::json!({
            "code": 0,
            "data": {
                "query": needle,
                "provider": serde_json::Value::Null,
                "matches": Vec::<CatalogLookupEntry>::new(),
                "catalog": status,
            }
        }))
        .into_response();
    };
    let provider = base_url
        .and_then(|url| catalog.provider_for_base_url(url))
        .map(DirectoryEntry::from_provider);
    let matches: Vec<CatalogLookupEntry> = catalog
        .lookup(base_url, &needle)
        .iter()
        .map(CatalogLookupEntry::from_match)
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "query": needle,
            "provider": provider,
            "matches": matches,
            "catalog": catalog_summary(&catalog, &state.static_dir).await,
        }
    }))
    .into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

/// `GET /api/v1/model-catalog/search` — free-text search across every provider.
pub async fn catalog_search(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let _ = auth;
    let needle = query
        .q
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if needle.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": "A search term (q=) is required" })),
        )
            .into_response();
    }
    let Ok(catalog) = catalog(&state.static_dir).await else {
        let status = status(&state.static_dir).await;
        return Json(serde_json::json!({
            "code": 0,
            "data": { "query": needle, "matches": Vec::<CatalogLookupEntry>::new(), "catalog": status }
        }))
        .into_response();
    };
    let preferred = query
        .base_url
        .as_deref()
        .and_then(|url| catalog.provider_for_base_url(url))
        .map(|provider| provider.id.clone());
    let mut hits: Vec<(u8, CatalogLookupEntry)> = Vec::new();
    for provider in &catalog.providers {
        let on_preferred = preferred.as_deref() == Some(provider.id.as_str());
        for model in &provider.models {
            let name = model.name.to_ascii_lowercase();
            let id = model.id.to_ascii_lowercase();
            let provider_name = provider.name.to_ascii_lowercase();
            let rank = if name == needle || id == needle {
                0
            } else if name.starts_with(&needle) || id.starts_with(&needle) {
                1
            } else if name.contains(&needle) || id.contains(&needle) {
                2
            } else if provider_name.contains(&needle) {
                3
            } else {
                continue;
            };
            let rank = if on_preferred { rank } else { rank + 4 };
            hits.push((
                rank,
                CatalogLookupEntry {
                    provider_id: provider.id.clone(),
                    provider_name: provider.name.clone(),
                    provider_website: provider.website.clone(),
                    provider_api_base_url: provider.api_base_url.clone(),
                    model: model.clone(),
                    summary: model.summary(),
                    suggested_model_types: model.suggested_model_types(),
                },
            ));
        }
    }
    hits.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.model.name.len().cmp(&right.1.model.name.len()))
            .then_with(|| left.1.provider_name.cmp(&right.1.provider_name))
    });
    hits.truncate(MAX_SEARCH_HITS);
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "query": needle,
            "matches": hits.into_iter().map(|(_, entry)| entry).collect::<Vec<_>>(),
            "catalog": catalog_summary(&catalog, &state.static_dir).await,
        }
    }))
    .into_response()
}

/// `POST /api/v1/model-catalog/refresh` — admin only.
pub async fn catalog_refresh(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if !auth.is_admin {
        return (
            axum::http::StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "code": 403, "message": "Administrator access required" })),
        )
            .into_response();
    }
    match refresh(&state.static_dir).await {
        Ok(catalog) => Json(serde_json::json!({
            "code": 0,
            "data": {
                "providers": catalog.provider_count(),
                "models": catalog.model_count(),
                "fetched_at": catalog.fetched_at,
                "source": catalog.source,
                "cache_path": cache_path(&state.static_dir).display().to_string(),
            }
        }))
        .into_response(),
        Err(error) => (
            axum::http::StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "code": 502,
                "message": format!("Could not refresh the model catalogue: {error}")
            })),
        )
            .into_response(),
    }
}

fn provider_entries(provider: &CatalogProvider, limit: usize) -> Vec<CatalogLookupEntry> {
    provider
        .models
        .iter()
        .take(limit)
        .map(|model| CatalogLookupEntry {
            provider_id: provider.id.clone(),
            provider_name: provider.name.clone(),
            provider_website: provider.website.clone(),
            provider_api_base_url: provider.api_base_url.clone(),
            model: model.clone(),
            summary: model.summary(),
            suggested_model_types: model.suggested_model_types(),
        })
        .collect()
}

/// The catalogue's own numbers, so every response can tell the operator how complete
/// the answer is.
pub(crate) async fn catalog_summary(catalog: &ModelCatalog, static_dir: &str) -> CatalogStatus {
    let status = status(static_dir).await;
    CatalogStatus {
        providers: catalog.provider_count(),
        models: catalog.model_count(),
        fetched_at: catalog.fetched_at,
        age_ms: status.age_ms,
        fresh: status.fresh,
        source: if catalog.source.is_empty() {
            status.source
        } else {
            catalog.source.clone()
        },
        ..status
    }
}
