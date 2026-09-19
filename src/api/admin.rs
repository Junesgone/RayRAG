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

/// Upstream `common.ErrorWithCode`: HTTP 200 with `{code, message}` so the
/// console can render the failure inside the login card (a real 401 would hit
/// the global session interceptor and bounce to the app login page).
fn console_error(code: u16, message: &str) -> Response {
    Json(serde_json::json!({
        "code": code,
        "message": message,
        "data": serde_json::Value::Null,
    }))
    .into_response()
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

/// RFC 3339 timestamp for the `*_date` fields upstream returns alongside the
/// raw `*_time` epoch values.
fn format_ms(ms: u64) -> String {
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|value| value.to_rfc3339())
        .unwrap_or_default()
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
/// Body of `POST /api/v1/admin/login` (upstream `service.EmailLoginRequest`).
#[derive(Deserialize)]
pub struct AdminLoginRequest {
    #[serde(default)]
    pub email: String,
    /// Upstream accepts `email`; the admin service also posts `username`.
    #[serde(default)]
    pub username: String,
    pub password: String,
}

/// `POST /api/v1/admin/login` — upstream `internal/admin/handler.go::Login`.
///
/// The console is superuser-only: valid credentials on a non-admin account fail
/// with `CodeForbidden` and the upstream message, and the token minted while
/// verifying the password is revoked so no live credential leaks out. The
/// successful payload is `AdminService.LoginData` (`is_superuser`, `is_active`,
/// `status` as `"0"`/`"1"` strings, plus the token), and the token is mirrored
/// into the `Authorization` response header exactly like the Go handler.
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AdminLoginRequest>,
) -> Response {
    let email = if body.email.trim().is_empty() {
        body.username.trim().to_string()
    } else {
        body.email.trim().to_string()
    };
    // Upstream `common.ErrorWithCode` answers HTTP 200 with a business code in
    // the body, and `UserService.LoginByEmail` distinguishes an unknown account
    // (109 "is not registered!") from a wrong password (109 "do not match!").
    let Some(existing) = state.users.get_user(&email) else {
        return console_error(109, &format!("email: {email} is not registered!"));
    };
    let disabled = admin_registry()
        .activate_status
        .lock()
        .unwrap()
        .get(&existing.email)
        .copied()
        .map(|active| !active)
        .unwrap_or(false);
    if disabled {
        return console_error(
            403,
            "This account has been disabled, please contact the administrator!",
        );
    }
    let token = match state.users.login(&email, &body.password) {
        Ok(Some(token)) => token,
        _ => return console_error(109, "email and password do not match!"),
    };
    if existing.role != "admin" {
        state.users.logout(&token);
        return console_error(403, "Only superuser can login admin system");
    }
    let user = existing;
    let data = serde_json::json!({
        "access_token": token,
        "id": user.id,
        "email": user.email,
        "nickname": user.nickname,
        "avatar": if user.avatar.is_empty() { serde_json::Value::Null } else { serde_json::json!(user.avatar) },
        "is_superuser": true,
        "is_active": "1",
        "status": "1",
        "is_anonymous": "0",
        "is_authenticated": "1",
        "language": "en",
        "timezone": user.timezone,
        "create_time": user.created_at,
        "create_date": format_ms(user.created_at),
        "last_login_time": format_ms(now_ms()),
        "login_channel": serde_json::Value::Null,
        "color_schema": "Bright",
    });
    let mut response = Json(serde_json::json!({
        "code": 0,
        "message": "Welcome back!",
        "data": data,
    }))
    .into_response();
    if let Ok(value) = axum::http::HeaderValue::from_str(&token) {
        response.headers_mut().insert("Authorization", value);
    }
    response
}

/// `GET /api/v1/admin/logout` — upstream `Handler.Logout`: invalidate the
/// caller's token and report success.
pub async fn logout(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    state.users.logout(&auth.token);
    ok_with_message("Logout successfully", serde_json::json!(true))
}

/// Components reported by `/api/v1/admin/services`, mirroring the upstream
/// `service_type` vocabulary (`internal/server/config.go`): the application
/// server, the metadata store, the retrieval engine, the file store and the
/// task executor.
fn admin_services_payload(state: &Arc<AppState>) -> Vec<serde_json::Value> {
    let postgres = crate::persistence::snapshot_mirror_health();
    let zvec = state.vector_mirror.health();
    let postgres_url = std::env::var("RAYRAG_POSTGRES_URL").unwrap_or_default();
    let (pg_host, pg_port) = postgres_endpoint(&postgres_url);
    let storage = std::env::var("RAYRAG_STORAGE_ENDPOINT").unwrap_or_default();
    let host = std::env::var("RAYRAG_ADVERTISE_HOST")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let port: u16 = std::env::var("RAYRAG_ADVERTISE_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(9380);
    let alive = |health: &anyhow::Result<bool>| match health {
        Ok(true) => "alive",
        Ok(false) => "disabled",
        Err(_) => "fail",
    };
    vec![
        serde_json::json!({
            "name": "rayrag_server",
            "service_type": "ragflow_server",
            "host": host,
            "port": port,
            "status": "alive",
            "extra": {
                "version": crate::build_info::VERSION,
                "parity": crate::build_info::PARITY_SLICE,
                "revision": crate::build_info::GIT_REV,
            },
        }),
        serde_json::json!({
            "name": "postgres",
            "service_type": "meta_data",
            "host": pg_host,
            "port": pg_port,
            "status": alive(&postgres),
            "extra": { "required_version": std::env::var("RAYRAG_POSTGRES_REQUIRED_VERSION").unwrap_or_else(|_| "18.4".into()) },
        }),
        serde_json::json!({
            "name": "zvec",
            "service_type": "retrieval",
            "host": host,
            "port": 0,
            "status": alive(&zvec),
            "extra": { "dir": std::env::var("RAYRAG_ZVEC_DIR").unwrap_or_default() },
        }),
        serde_json::json!({
            "name": if storage.is_empty() { "local-filesystem" } else { "object-storage" },
            "service_type": "file_store",
            "host": if storage.is_empty() { "127.0.0.1".to_string() } else { storage.clone() },
            "port": 0,
            "status": "alive",
            "extra": {},
        }),
        serde_json::json!({
            "name": "task_executor",
            "service_type": "task_executor",
            "host": host,
            "port": 0,
            "status": "alive",
            "extra": { "pending": state.tasks.list().iter().filter(|task| task.status != "done").count() },
        }),
    ]
}

/// `host:port` of the configured metadata store, defaulting to the local
/// PostgreSQL baseline when only a password/port pair is configured.
fn postgres_endpoint(url: &str) -> (String, u16) {
    let rest = url.split("://").nth(1).unwrap_or("");
    let authority = rest.split('/').next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    match host_port.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => (host.to_string(), port.parse().unwrap_or(5432)),
        _ => (
            if host_port.is_empty() {
                "127.0.0.1".to_string()
            } else {
                host_port.to_string()
            },
            std::env::var("RAYRAG_POSTGRES_PORT")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(5432),
        ),
    }
}

/// `GET /api/v1/admin/services` — upstream `Service.ListServices()`: every
/// component with `{id,name,service_type,host,port,status,extra}` and the id
/// assigned by list position.
pub async fn list_services(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let services: Vec<serde_json::Value> = admin_services_payload(&state)
        .into_iter()
        .enumerate()
        .map(|(id, mut service)| {
            if let Some(map) = service.as_object_mut() {
                map.insert("id".to_string(), serde_json::json!(id));
            }
            service
        })
        .collect();
    ok_with_message("List services", serde_json::json!(services))
}

/// `GET /api/v1/admin/services/{service_id}` — upstream
/// `Service.GetServiceDetails`: `{service_name,status,message}`, where the
/// message carries the probe result for that component.
pub async fn service_details(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(service_id): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let services = admin_services_payload(&state);
    let index: usize = match service_id.parse() {
        Ok(value) => value,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "code": 400,
                    "message": "service_id must be an integer",
                })),
            )
                .into_response();
        }
    };
    let Some(service) = services.get(index) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 404,
                "message": "Service not found",
            })),
        )
            .into_response();
    };
    let name = service
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let status = service
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("timeout");
    let message = if name == "task_executor" {
        serde_json::json!({
            "rayrag": [{
                "name": "rayrag",
                "boot_at": crate::build_info::BUILT_AT,
                "now": format_ms(now_ms()),
                "ip_address": service.get("host").and_then(|v| v.as_str()).unwrap_or("127.0.0.1"),
                "current": {},
                "done": state.tasks.list().iter().filter(|task| task.status == "done").count(),
                "failed": state.tasks.list().iter().filter(|task| task.status == "failed").count(),
                "lag": 0,
                "pending": state.tasks.list().iter().filter(|task| task.status != "done").count(),
                "pid": std::process::id(),
            }]
        })
    } else {
        serde_json::json!(
            service
                .get("extra")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        )
    };
    ok_with_message(
        "Get service details",
        serde_json::json!({
            "service_name": name,
            "status": status,
            "message": message,
        }),
    )
}

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
    // Upstream `UserMgr.get_user_details` answers with a *list* of matching
    // accounts (one per e-mail) and this exact field set; the console reads
    // `data.data[0]`. RayRAG adds the ids the users table needs.
    ok(serde_json::json!([user_detail_row(&state, &user)]))
}

/// Upstream `UserMgr.get_user_details` row.
fn user_detail_row(state: &Arc<AppState>, user: &crate::auth::User) -> serde_json::Value {
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
    serde_json::json!({
        "id": user.id,
        "username": user.email,
        "nickname": user.nickname,
        "role": user.role,
        "avatar": if user.avatar.is_empty() { serde_json::Value::Null } else { serde_json::json!(user.avatar) },
        "email": user.email,
        "language": "en",
        "last_login_time": "",
        "is_active": if active { "1" } else { "0" },
        "is_anonymous": "0",
        "is_authenticated": "1",
        "login_channel": "password",
        "status": if active { "active" } else { "inactive" },
        "is_superuser": user.role == "admin",
        "create_date": format_ms(user.created_at),
        "update_date": format_ms(user.created_at),
        "timezone": user.timezone,
        "api_key_count": key_count,
        "datasets": user_datasets(state, user),
    })
}

/// Upstream `UserServiceMgr.get_user_datasets`: every knowledge base in the
/// tenants the account belongs to that the account may use.
fn user_datasets(state: &Arc<AppState>, user: &crate::auth::User) -> Vec<serde_json::Value> {
    state
        .kbs
        .list_accessible(&user.id, false, |tenant_id, user_id| {
            state.tenants.is_member(tenant_id, user_id)
        })
        .into_iter()
        .map(|kb| {
            serde_json::json!({
                "id": kb.id,
                "name": kb.name,
                "avatar": if kb.avatar.is_empty() { serde_json::Value::Null } else { serde_json::json!(kb.avatar) },
                "doc_num": kb.doc_count,
                "chunk_num": kb.chunk_count,
                "token_num": 0,
                "language": kb.language,
                "permission": kb.permission,
                "create_date": format_ms(kb.created_at),
                "update_date": format_ms(kb.updated_at),
            })
        })
        .collect()
}

/// `GET /api/v1/admin/users/{username}/datasets`.
pub async fn list_user_datasets(
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
    ok(serde_json::json!(user_datasets(&state, &user)))
}

/// `GET /api/v1/admin/users/{username}/agents` — upstream
/// `UserServiceMgr.get_user_agents` returns `title` / `permission` /
/// `canvas_category` (the first `_`-separated segment) / `avatar`.
pub async fn list_user_agents(
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
    let query = crate::api::features::AgentListQuery {
        owner_ids: None,
        ..Default::default()
    };
    let agents = state
        .agents
        .list_accessible(
            &user.id,
            false,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
            &query,
        )
        .map(|(agents, _)| agents)
        .unwrap_or_default()
        .into_iter()
        .map(|agent| {
            serde_json::json!({
                "title": agent.name,
                "permission": agent.permission,
                "canvas_category": agent.canvas_category.split('_').next().unwrap_or_default(),
                "avatar": if agent.avatar.is_empty() { serde_json::Value::Null } else { serde_json::json!(agent.avatar) },
            })
        })
        .collect::<Vec<_>>();
    ok(serde_json::json!(agents))
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
    ok(serde_json::json!({
        "version": crate::build_info::VERSION,
        "parity": crate::build_info::PARITY_SLICE,
        "revision": crate::build_info::revision(),
        "built_at": crate::build_info::BUILT_AT,
    }))
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
        { "name": "version", "value": crate::build_info::VERSION },
        { "name": "parity", "value": crate::build_info::PARITY_SLICE },
        { "name": "revision", "value": crate::build_info::revision() },
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
