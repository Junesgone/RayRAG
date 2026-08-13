//! Admin API — RAGFlow `admin/server/routes.py` port.
//!
//! System-administration endpoints under `/api/v1/admin`: ping, user
//! management (list / create / details / delete / password / activation /
//! admin grant-revoke / API keys), version, configs,
//! environments and log levels. Every handler requires an admin `AuthContext`
//! (`check_admin_auth` in RAGFlow).
//!
//! Activation flags and API keys live in a process-local registry here; durable
//! runtime variables are implemented by `api::system_settings`, and user
//! records themselves are the shared `UserStore` (`src/auth.rs`).

use crate::server::{AppState, AuthContext};
use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

fn require_admin(auth: &AuthContext) -> Option<Response> {
    (!auth.is_admin).then(|| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "code": 403, "message": "Administrator access required" })),
        )
            .into_response()
    })
}

fn error_response(status: StatusCode, code: i32, message: impl Into<String>) -> Response {
    (
        status,
        Json(serde_json::json!({ "code": code, "message": message.into() })),
    )
        .into_response()
}

fn ok(data: serde_json::Value) -> Response {
    Json(serde_json::json!({ "code": 0, "data": data })).into_response()
}

fn ok_with_message(message: &str, data: serde_json::Value) -> Response {
    Json(serde_json::json!({ "code": 0, "message": message, "data": data })).into_response()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

// ── Process-local admin registry ────────────────────────────────

/// In-process admin state mirroring RAGFlow's admin tables without adding
/// database schema (single-process server assumption).
#[derive(Default)]
struct AdminRegistry {
    activate_status: Mutex<HashMap<String, bool>>,
    api_keys: Mutex<HashMap<String, Vec<ApiKeyEntry>>>,
}

fn admin_registry() -> &'static AdminRegistry {
    static REGISTRY: OnceLock<AdminRegistry> = OnceLock::new();
    REGISTRY.get_or_init(AdminRegistry::default)
}

#[derive(Debug, Clone, Serialize)]
struct ApiKeyEntry {
    tenant_id: String,
    token: String,
    beta: String,
    create_time: u64,
    create_date: String,
}

/// Resolve a username (login name or user id) to a user record.
fn resolve_user(state: &AppState, username: &str) -> Option<crate::auth::User> {
    state
        .users
        .get_user(username)
        .or_else(|| state.users.get_user_by_id(username))
}

// ── Ping / auth ─────────────────────────────────────────────────

/// GET /api/v1/admin/ping
pub async fn ping() -> Response {
    Json(serde_json::json!({ "code": 0, "message": "pong" })).into_response()
}

// ── Users ───────────────────────────────────────────────────────

/// GET /api/v1/admin/users — list all users.
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let registry = admin_registry();
    let status = registry.activate_status.lock().unwrap();
    let users: Vec<serde_json::Value> = state
        .users
        .list_users()
        .into_iter()
        .map(|user| {
            let active = status.get(&user.email).copied().unwrap_or(true);
            serde_json::json!({
                "id": user.id,
                "username": user.email,
                "email": user.email,
                "nickname": user.nickname,
                "role": user.role,
                "status": if active { "active" } else { "inactive" },
                "created_at": user.created_at,
            })
        })
        .collect();
    ok_with_message("Get all users", serde_json::json!(users))
}

#[derive(Deserialize)]
pub struct CreateUserRequest {
    username: String,
    password: String,
    #[serde(default)]
    role: String,
}

/// POST /api/v1/admin/users — create a user with an explicit role.
pub async fn create_user(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateUserRequest>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    match state
        .users
        .create_user(&body.username, &body.password, &body.role)
    {
        Ok(user) => ok_with_message(
            "User created successfully",
            serde_json::json!({
                "id": user.id,
                "username": user.email,
                "email": user.email,
                "nickname": user.nickname,
                "role": user.role,
            }),
        ),
        Err(error) => error_response(StatusCode::BAD_REQUEST, 400, error.to_string()),
    }
}

/// GET /api/v1/admin/users/{username} — user details.
pub async fn get_user_details(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(username): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(user) = resolve_user(&state, &username) else {
        return error_response(StatusCode::NOT_FOUND, 404, "User not found");
    };
    let registry = admin_registry();
    let status = registry.activate_status.lock().unwrap();
    let active = status.get(&user.email).copied().unwrap_or(true);
    let key_count = registry
        .api_keys
        .lock()
        .unwrap()
        .get(&user.email)
        .map(Vec::len)
        .unwrap_or(0);
    let datasets: Vec<serde_json::Value> = state
        .kbs
        .list_accessible(&user.id, false, |tenant_id, user_id| {
            state.tenants.is_member(tenant_id, user_id)
        })
        .into_iter()
        .map(|kb| {
            serde_json::json!({
                "id": kb.id,
                "name": kb.name,
                "doc_count": kb.doc_count,
                "chunk_count": kb.chunk_count,
            })
        })
        .collect();
    ok(serde_json::json!({
        "id": user.id,
        "username": user.email,
        "email": user.email,
        "nickname": user.nickname,
        "role": user.role,
        "status": if active { "active" } else { "inactive" },
        "created_at": user.created_at,
        "api_key_count": key_count,
        "datasets": datasets,
        "agents": [],
    }))
}

/// DELETE /api/v1/admin/users/{username}
pub async fn delete_user(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(username): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(user) = resolve_user(&state, &username) else {
        return error_response(StatusCode::NOT_FOUND, 404, "User not found");
    };
    match state.users.delete_user(&user.id, &auth.user_id) {
        Ok(true) => ok_with_message("User deleted successfully", serde_json::json!(true)),
        Ok(false) => error_response(StatusCode::NOT_FOUND, 404, "User not found"),
        Err(error) => error_response(StatusCode::BAD_REQUEST, 400, error.to_string()),
    }
}

#[derive(Deserialize)]
pub struct PasswordUpdate {
    new_password: String,
}

/// PUT /api/v1/admin/users/{username}/password
pub async fn change_password(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(username): Path<String>,
    Json(body): Json<PasswordUpdate>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(user) = resolve_user(&state, &username) else {
        return error_response(StatusCode::NOT_FOUND, 404, "User not found");
    };
    match state.users.reset_password(&user.id, &body.new_password) {
        Ok(true) => ok_with_message("Password updated successfully", serde_json::json!(true)),
        Ok(false) => error_response(StatusCode::NOT_FOUND, 404, "User not found"),
        Err(error) => error_response(StatusCode::BAD_REQUEST, 400, error.to_string()),
    }
}

#[derive(Deserialize)]
pub struct ActivateUpdate {
    activate_status: bool,
}

/// PUT /api/v1/admin/users/{username}/activate
pub async fn alter_user_activate_status(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(username): Path<String>,
    Json(body): Json<ActivateUpdate>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(user) = resolve_user(&state, &username) else {
        return error_response(StatusCode::NOT_FOUND, 404, "User not found");
    };
    admin_registry()
        .activate_status
        .lock()
        .unwrap()
        .insert(user.email.clone(), body.activate_status);
    ok_with_message(
        "Activation status updated successfully",
        serde_json::json!({ "username": user.email, "activate_status": body.activate_status }),
    )
}

fn grant_or_revoke(state: &AppState, auth: &AuthContext, username: &str, role: &str) -> Response {
    let Some(user) = resolve_user(state, username) else {
        return error_response(StatusCode::NOT_FOUND, 404, "User not found");
    };
    if auth.user_id == user.id {
        return error_response(
            StatusCode::CONFLICT,
            409,
            format!("can't grant current user: {username}"),
        );
    }
    match state.users.set_role(&user.id, role) {
        Ok(true) => ok_with_message(
            "User role updated successfully",
            serde_json::json!({ "username": user.email, "role": role }),
        ),
        Ok(false) => error_response(StatusCode::NOT_FOUND, 404, "User not found"),
        Err(error) => error_response(StatusCode::BAD_REQUEST, 400, error.to_string()),
    }
}

/// PUT /api/v1/admin/users/{username}/admin — grant admin role.
pub async fn grant_admin(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(username): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    grant_or_revoke(&state, &auth, &username, "admin")
}

/// DELETE /api/v1/admin/users/{username}/admin — revoke admin role.
pub async fn revoke_admin(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(username): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    grant_or_revoke(&state, &auth, &username, "normal")
}

// ── API keys ────────────────────────────────────────────────────

/// POST /api/v1/admin/users/{username}/keys — generate a tenant API key.
pub async fn generate_user_api_key(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(username): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(user) = resolve_user(&state, &username) else {
        return error_response(StatusCode::NOT_FOUND, 404, "User not found!");
    };
    let token = format!("sk-{}", uuid::Uuid::new_v4().simple());
    let beta: String = uuid::Uuid::new_v4().simple().to_string()[..32].to_string();
    let entry = ApiKeyEntry {
        tenant_id: user.id.clone(),
        token,
        beta,
        create_time: now_ms(),
        create_date: chrono::Utc::now().to_rfc3339(),
    };
    admin_registry()
        .api_keys
        .lock()
        .unwrap()
        .entry(user.email.clone())
        .or_default()
        .push(entry.clone());
    ok_with_message(
        "API key generated successfully",
        serde_json::to_value(entry).unwrap_or_default(),
    )
}

/// GET /api/v1/admin/users/{username}/keys
pub async fn get_user_api_keys(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(username): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(user) = resolve_user(&state, &username) else {
        return error_response(StatusCode::NOT_FOUND, 404, "User not found!");
    };
    let keys = admin_registry()
        .api_keys
        .lock()
        .unwrap()
        .get(&user.email)
        .cloned()
        .unwrap_or_default();
    ok_with_message("Get user API keys", serde_json::json!(keys))
}

/// DELETE /api/v1/admin/users/{username}/keys/{key}
pub async fn delete_user_api_key(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((username, key)): Path<(String, String)>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(user) = resolve_user(&state, &username) else {
        return error_response(StatusCode::NOT_FOUND, 404, "User not found!");
    };
    let mut keys = admin_registry().api_keys.lock().unwrap();
    let Some(entries) = keys.get_mut(&user.email) else {
        return error_response(
            StatusCode::NOT_FOUND,
            404,
            "API key not found or could not be deleted",
        );
    };
    let before = entries.len();
    entries.retain(|entry| entry.token != key);
    if entries.len() == before {
        return error_response(
            StatusCode::NOT_FOUND,
            404,
            "API key not found or could not be deleted",
        );
    }
    ok_with_message("API key deleted successfully", serde_json::json!(true))
}

// ── Version / configs / environments ────────────────────────────

/// GET /api/v1/admin/version
pub async fn show_version() -> Response {
    ok(serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }))
}

/// GET /api/v1/admin/configs — static system configuration snapshot.
pub async fn get_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    ok(serde_json::json!([
        { "name": "version", "value": env!("CARGO_PKG_VERSION") },
        { "name": "timezone", "value": "Asia/Shanghai" },
        { "name": "language", "value": "zh-CN" },
        { "name": "max_file_size_mb", "value": 128 },
        { "name": "max_chunk_size", "value": 2048 },
        { "name": "indexing_strategy", "value": "naive" },
        { "name": "log_levels", "value": state.log_levels.levels() },
    ]))
}

/// GET /api/v1/admin/environments — RAYRAG_* environment snapshot with
/// secret-looking values redacted.
pub async fn get_environments() -> Response {
    let mut list = Vec::new();
    for (key, value) in std::env::vars() {
        if !key.starts_with("RAYRAG_") {
            continue;
        }
        let masked = if key.contains("PASSWORD")
            || key.contains("SECRET")
            || key.contains("TOKEN")
            || key.contains("KEY")
        {
            "<redacted>"
        } else {
            &value
        };
        list.push(serde_json::json!({ "name": key, "value": masked }));
    }
    list.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    ok(serde_json::json!(list))
}

// ── Log levels ──────────────────────────────────────────────────

/// GET /api/v1/admin/log_levels — current tracing target levels.
pub async fn get_log_levels(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({ "code": 0, "data": state.log_levels.levels() }))
}

/// PUT /api/v1/admin/log_levels — set one tracing target level.
pub async fn set_log_level(
    State(state): State<Arc<AppState>>,
    Json(update): Json<serde_json::Value>,
) -> Response {
    let Some(pkg_name) = update.get("pkg_name").and_then(serde_json::Value::as_str) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            102,
            "pkg_name and level are required",
        );
    };
    let Some(level) = update.get("level").and_then(serde_json::Value::as_str) else {
        return error_response(
            StatusCode::BAD_REQUEST,
            102,
            "pkg_name and level are required",
        );
    };
    match state.log_levels.set_level(pkg_name, level) {
        Ok(level) => ok_with_message(
            "Log level updated successfully",
            serde_json::json!({ "pkg_name": pkg_name, "level": level }),
        ),
        Err(crate::logging::SetLogLevelError::InvalidLevel) => error_response(
            StatusCode::BAD_REQUEST,
            102,
            format!("Invalid log level: {level}"),
        ),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, 500, error.to_string()),
    }
}
