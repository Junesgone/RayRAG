//! `pages/admin/whitelist.tsx` — the registration whitelist surface.
//!
//! Reads and writes the store in [`crate::registration_whitelist`], which `POST /api/v1/users`
//! consults before it creates an account.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use crate::server::{AppState, AuthContext, api_error_code, code};

fn require_admin(auth: &AuthContext) -> Option<Response> {
    if auth.is_admin {
        return None;
    }
    Some(crate::server::no_permission(
        "Only an administrator can manage the registration whitelist",
    ))
}

/// `GET /api/v1/admin/whitelist`
pub async fn get_whitelist(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let snapshot = state.whitelist.snapshot();
    Json(serde_json::json!({
        "code": 0,
        "data": { "enabled": snapshot.enabled, "entries": snapshot.entries },
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct AddEntryRequest {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub entry: Option<String>,
}

/// `POST /api/v1/admin/whitelist`
pub async fn add_entry(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<AddEntryRequest>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(entry) = body.email.or(body.entry) else {
        return crate::server::invalid_argument("email is required");
    };
    match state.whitelist.add(&entry) {
        Ok(true) => Json(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": { "enabled": state.whitelist.snapshot().enabled, "entries": state.whitelist.snapshot().entries },
        }))
        .into_response(),
        // Already present is not an error, but it is not a new entry either - say so.
        Ok(false) => Json(serde_json::json!({
            "code": 0,
            "message": "already listed",
            "data": { "enabled": state.whitelist.snapshot().enabled, "entries": state.whitelist.snapshot().entries },
        }))
        .into_response(),
        Err(error) => crate::server::invalid_argument(&error.to_string()),
    }
}

#[derive(Deserialize)]
pub struct RemoveEntriesRequest {
    /// The endpoints accept `ids` (upstream spelling) or `entries`.
    #[serde(default)]
    pub ids: Option<Vec<String>>,
    #[serde(default)]
    pub entries: Option<Vec<String>>,
}

/// `DELETE /api/v1/admin/whitelist`
pub async fn remove_entries(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<RemoveEntriesRequest>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let entries = body.ids.or(body.entries).unwrap_or_default();
    if entries.is_empty() {
        return crate::server::invalid_argument("ids is required");
    }
    match state.whitelist.remove(&entries) {
        Ok(removed) => Json(serde_json::json!({
            "code": 0,
            "message": format!("removed {removed}"),
            "data": { "removed": removed, "entries": state.whitelist.snapshot().entries },
        }))
        .into_response(),
        Err(error) => api_error_code(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        ),
    }
}

#[derive(Deserialize)]
pub struct ToggleRequest {
    pub enabled: bool,
}

/// `PUT /api/v1/admin/whitelist`
pub async fn set_enabled(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<ToggleRequest>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    match state.whitelist.set_enabled(body.enabled) {
        Ok(()) => Json(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": { "enabled": body.enabled, "entries": state.whitelist.snapshot().entries },
        }))
        .into_response(),
        Err(error) => api_error_code(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_admin_is_refused() {
        let auth = AuthContext {
            user_id: "u".into(),
            is_admin: false,
            token: String::new(),
        };
        let response = require_admin(&auth).expect("refused");
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
        let admin = AuthContext {
            user_id: "a".into(),
            is_admin: true,
            token: String::new(),
        };
        assert!(require_admin(&admin).is_none());
    }
}
