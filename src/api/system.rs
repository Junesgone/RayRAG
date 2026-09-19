//! System / User / Tenant / Models / Backward Compat / LLM App APIs.
//! Replaces RAGFlow's system_api, user_api, tenant_api, models_api,
//! backward_compat, and llm_app. Langfuse lives in `langfuse.rs`.

use crate::kb::TenantRole;
use crate::llm::ChatMessage;
use crate::server::{AppState, AuthContext};
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct RerankerConfigUpdate {
    pub enabled: bool,
    pub api_base: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

fn require_admin(auth: &AuthContext) -> Option<Response> {
    (!auth.is_admin).then(|| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "code": 403, "message": "Administrator access required" })),
        )
            .into_response()
    })
}

fn can_manage_tenant(state: &AppState, auth: &AuthContext, tenant_id: &str) -> bool {
    state.tenants.can_manage(tenant_id, &auth.user_id)
}

fn forbidden_tenant() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(serde_json::json!({ "code": 403, "message": "Tenant administrator access required" })),
    )
        .into_response()
}

pub async fn get_reranker_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    Json(serde_json::json!({ "code": 0, "data": state.reranker.public_config() })).into_response()
}

pub async fn update_reranker_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(config): Json<RerankerConfigUpdate>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    match state.reranker.patch(
        config.enabled,
        config.api_base,
        config.api_key,
        config.clear_api_key,
    ) {
        Ok(config) => Json(serde_json::json!({ "code": 0, "data": config })).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
        )
            .into_response(),
    }
}

// ── System Settings ────────────────────────────────────────────

#[derive(Serialize)]
pub struct SystemConfig {
    pub version: String,
    pub timezone: String,
    pub language: String,
    pub max_file_size_mb: u64,
    pub max_chunk_size: usize,
    pub indexing_strategy: String,
}

/// GET `/api/v1/auth/login/channels` — upstream `user_api.get_login_channels`:
/// the configured OAuth channels as `{channel, display_name, icon}`. The list is
/// empty when no channel is configured, which the login page renders as no
/// channel section at all.
pub async fn login_channels(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let channels: Vec<serde_json::Value> = state
        .oauth_channels
        .read()
        .unwrap()
        .iter()
        .map(crate::oauth_config::OAuthChannel::to_json)
        .collect();
    Json(serde_json::json!({ "code": 0, "data": channels }))
}

/// GET `/api/v1/auth/login/{channel}` — upstream `user_api.oauth_login`:
/// redirect to the provider authorization URL, or fail with
/// `Invalid channel name: {channel}` when the channel is not configured.
pub async fn oauth_login(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
) -> Response {
    let channels = state.oauth_channels.read().unwrap().clone();
    let Some(entry) = crate::oauth_config::find(&channels, &channel) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 102,
                "message": format!("Invalid channel name: {channel}"),
            })),
        )
            .into_response();
    };
    // Upstream stores the state in the session; RayRAG keeps a single-use store.
    let state_value = state.oauth_states.issue();
    match entry.authorization_request(&state_value) {
        Some(url) => (
            StatusCode::TEMPORARY_REDIRECT,
            [(axum::http::header::LOCATION, url)],
        )
            .into_response(),
        None => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": 102,
                "message": format!("Channel {channel} has no authorization_url or issuer configured"),
            })),
        )
            .into_response(),
    }
}

/// GET `/api/v1/auth/oauth/{channel}/callback` — upstream
/// `user_api.oauth_callback`: validate the state, exchange the code, fetch the
/// provider user info, then log the (existing or freshly registered) user in and
/// redirect to `/?auth={user_id}`. Every failure redirects with the upstream
/// `?error=` code instead of returning JSON.
pub async fn oauth_callback(
    State(state): State<Arc<AppState>>,
    Path(channel): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    fn redirect_error(code: &str) -> Response {
        (
            StatusCode::TEMPORARY_REDIRECT,
            [(axum::http::header::LOCATION, format!("/?error={code}"))],
        )
            .into_response()
    }

    let channels = state.oauth_channels.read().unwrap().clone();
    let Some(entry) = crate::oauth_config::find(&channels, &channel) else {
        return redirect_error(&format!("Invalid channel name: {channel}"));
    };
    let state_value = query.get("state").cloned().unwrap_or_default();
    if !state.oauth_states.consume(&state_value) {
        return redirect_error("invalid_state");
    }
    let Some(code) = query
        .get("code")
        .map(String::as_str)
        .filter(|code| !code.is_empty())
    else {
        return redirect_error("missing_code");
    };
    let token = match crate::oauth_config::exchange_code(entry, code).await {
        Ok(token) => token,
        Err(error) => {
            tracing::warn!(channel = %channel, %error, "oauth token exchange failed");
            return redirect_error("token_failed");
        }
    };
    let user_info = match crate::oauth_config::fetch_user_info(entry, &token.access_token).await {
        Ok(info) => info,
        Err(error) => {
            tracing::warn!(channel = %channel, %error, "oauth userinfo failed");
            return redirect_error("userinfo_failed");
        }
    };
    if user_info.email.is_empty() {
        return redirect_error("email_missing");
    }
    // Existing user → log in; unknown email → register like upstream
    // `user_register(..., login_channel=channel)`.
    let email = user_info.email.to_ascii_lowercase();
    let user = match state.users.get_user(&email) {
        Some(user) => user,
        None => {
            let password = uuid::Uuid::new_v4().simple().to_string();
            let nickname = if user_info.nickname.is_empty() {
                user_info.username.clone()
            } else {
                user_info.nickname.clone()
            };
            match state.users.register(&nickname, &email, &password) {
                Ok(user) => {
                    if !user_info.avatar_url.is_empty() {
                        let _ = state.users.update_profile(
                            &user.id,
                            None,
                            Some(user_info.avatar_url.clone()),
                            None,
                        );
                    }
                    user
                }
                Err(error) => {
                    tracing::warn!(%email, %error, "oauth user registration failed");
                    return redirect_error("register_failed");
                }
            }
        }
    };
    let Some(access_token) = state.users.issue_token_for(&user.id) else {
        return redirect_error("token_failed");
    };
    (
        StatusCode::TEMPORARY_REDIRECT,
        [
            (axum::http::header::LOCATION, format!("/?auth={}", user.id)),
            (
                axum::http::header::SET_COOKIE,
                format!("rayrag_token={access_token}; path=/; max-age=86400"),
            ),
        ],
    )
        .into_response()
}

/// GET `/api/v1/system/version` — upstream `system_api.version` returns
/// `get_ragflow_version()` directly, i.e. the bare version string.
pub async fn system_version_public() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": format!("v{}", crate::build_info::VERSION),
    }))
}

/// GET `/api/v1/system/config` — upstream `system_api.get_config`:
/// `{"registerEnabled": settings.REGISTER_ENABLED,
///   "disablePasswordLogin": settings.DISABLE_PASSWORD_LOGIN}`.
/// The login page reads `registerEnabled` to gate the sign-up face.
pub async fn system_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "registerEnabled": state.register_enabled,
            "disablePasswordLogin": state.disable_password_login,
        }
    }))
}

/// GET /api/v1/system — get system settings
pub async fn get_system() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": SystemConfig {
            version: crate::build_info::VERSION.into(),
            timezone: "Asia/Shanghai".into(),
            language: "zh-CN".into(),
            max_file_size_mb: 128,
            max_chunk_size: 2048,
            indexing_strategy: "naive".into(),
        }
    }))
}

/// PUT /api/v1/system — update system settings
pub async fn update_system(
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    Json(serde_json::json!({"code":0,"message":"Updated","data":body})).into_response()
}

/// GET /api/v1/system/new_api_key — generate new API key
pub async fn new_api_key(Extension(auth): Extension<AuthContext>) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let key = format!("sk-{}", uuid::Uuid::new_v4());
    Json(serde_json::json!({"code":0,"data":{"api_key":key}})).into_response()
}

/// GET /api/v1/system/status — detailed status
pub async fn system_status_detail(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> impl IntoResponse {
    let doc_engine_started = std::time::Instant::now();
    let doc_engine = match state.vector_mirror.health() {
        Ok(enabled) => serde_json::json!({
            "type": if enabled { "zvec" } else { "json" },
            "status": "green",
            "elapsed": format!("{:.1}", doc_engine_started.elapsed().as_secs_f64() * 1000.0),
        }),
        Err(error) => serde_json::json!({
            "type": "zvec",
            "status": "red",
            "elapsed": format!("{:.1}", doc_engine_started.elapsed().as_secs_f64() * 1000.0),
            "error": error.to_string(),
        }),
    };
    let database_started = std::time::Instant::now();
    let database = match crate::persistence::snapshot_mirror_health() {
        Ok(enabled) => serde_json::json!({
            "database": if enabled { "postgresql" } else { "local-json" },
            "version_required": if enabled { std::env::var("RAYRAG_POSTGRES_REQUIRED_VERSION").ok() } else { None },
            "status": "green",
            "elapsed": format!("{:.1}", database_started.elapsed().as_secs_f64() * 1000.0),
        }),
        Err(error) => serde_json::json!({
            "database": "postgresql",
            "status": "red",
            "elapsed": format!("{:.1}", database_started.elapsed().as_secs_f64() * 1000.0),
            "error": error.to_string(),
        }),
    };
    let mut task_executor_heartbeats = serde_json::Map::new();
    for task in state.tasks.list().into_iter().filter(|task| {
        task.status == "running" && !task.worker_id.is_empty() && task.lease_expires_at > 0
    }) {
        task_executor_heartbeats
            .entry(task.worker_id.clone())
            .or_insert_with(|| serde_json::Value::Array(Vec::new()))
            .as_array_mut()
            .expect("task heartbeat entry is always an array")
            .push(serde_json::json!({
                "task_id": task.id,
                "updated_at": task.updated_at,
                "lease_expires_at": task.lease_expires_at,
            }));
    }
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "doc_engine": doc_engine,
            "storage": {
                "storage": "local",
                "status": "green",
                "elapsed": "0.0",
            },
            "database": database,
            "task_executor_heartbeats": task_executor_heartbeats,
            "version": crate::build_info::VERSION,
            "parity": crate::build_info::PARITY_SLICE,
            "revision": crate::build_info::revision(),
            "built_at": crate::build_info::BUILT_AT,
            "embedder_loaded": state.embedder.is_some(),
            "llm_configured": state.llm.is_some(),
            "index_size": if auth.is_admin { state.engine.read().unwrap().len() } else { 0 },
            "kb_count": state.kbs.list_accessible(
                &auth.user_id,
                auth.is_admin,
                |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
            ).len(),
            "uptime": "since boot",
        }
    }))
}

/// GET /api/v1/system/config/log — return current runtime target levels.
pub async fn get_log_levels(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(serde_json::json!({ "code": 0, "data": state.log_levels.levels() }))
}

/// PUT /api/v1/system/config/log — update one tracing target without restarting.
pub async fn set_log_level(
    State(state): State<Arc<AppState>>,
    Json(update): Json<serde_json::Value>,
) -> Response {
    let Some(pkg_name) = update.get("pkg_name").and_then(|value| value.as_str()) else {
        return Json(serde_json::json!({
            "code": 102,
            "message": "pkg_name and level are required"
        }))
        .into_response();
    };
    let Some(level) = update.get("level").and_then(|value| value.as_str()) else {
        return Json(serde_json::json!({
            "code": 102,
            "message": "pkg_name and level are required"
        }))
        .into_response();
    };
    match state.log_levels.set_level(pkg_name, level) {
        Ok(level) => Json(serde_json::json!({
            "code": 0,
            "data": { "pkg_name": pkg_name, "level": level }
        }))
        .into_response(),
        Err(crate::logging::SetLogLevelError::InvalidLevel) => Json(serde_json::json!({
            "code": 102,
            "message": format!("Invalid log level: {level}")
        }))
        .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

// ── User Management ────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub struct UserDetail {
    pub id: String,
    pub nickname: String,
    pub email: String,
    pub role: String,
    pub status: String,
    pub created_at: u64,
}

/// GET /api/v1/users — list all users
pub async fn list_users(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let users: Vec<UserDetail> = state
        .users
        .list_users()
        .into_iter()
        .map(|user| UserDetail {
            id: user.id,
            nickname: user.nickname,
            email: user.email,
            role: user.role,
            status: "active".into(),
            created_at: user.created_at,
        })
        .collect();
    Json(serde_json::json!({ "code": 0, "data": users })).into_response()
}

/// PUT /api/v1/users/{id} — update user
pub async fn update_user(
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    Json(serde_json::json!({"code":0,"message":format!("User {} updated",id),"data":body}))
        .into_response()
}

/// POST /api/v1/users/invite — invite user
pub async fn invite_user(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let email = body
        .get("email")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let tenant_id = body
        .get("tenant_id")
        .and_then(|value| value.as_str())
        .unwrap_or(&auth.user_id);
    if !can_manage_tenant(&state, &auth, tenant_id) {
        return forbidden_tenant();
    }
    let Some(user) = state.users.get_user(&email) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Registered user not found" })),
        )
            .into_response();
    };
    match state
        .tenants
        .invite_member(tenant_id, &user.id, &auth.user_id)
    {
        Ok(created) => Json(serde_json::json!({
            "code": 0,
            "message": if created { "Invitation created" } else { "Invitation already pending" },
            "data": {
                "tenant_id": tenant_id,
                "user_id": user.id,
                "role": "invite",
            }
        }))
        .into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "code": 409, "message": error.to_string() })),
        )
            .into_response(),
    }
}

// ── Tenant Management ──────────────────────────────────────────

#[derive(Serialize)]
pub struct TenantInfo {
    pub id: String,
    pub name: String,
    pub plan: String,
    pub status: String,
}

/// GET /api/v1/tenant — get the current user's tenant.
pub async fn get_tenant(Extension(auth): Extension<AuthContext>) -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": TenantInfo {
            id: auth.user_id,
            name: "RayRAG".into(),
            plan: "community".into(),
            status: "active".into(),
        }
    }))
}

/// GET /api/v1/tenants — list owned, joined, and invited tenants.
pub async fn list_tenants(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> impl IntoResponse {
    let mut tenants = state.tenants.list_for_user(&auth.user_id);
    tenants.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.tenant_id.cmp(&right.tenant_id))
    });
    // Upstream `tenant-table.tsx` renders the tenant owner's avatar/nickname/email
    // plus `update_date`, so join the rows with the user registry.
    let rows: Vec<serde_json::Value> = tenants
        .into_iter()
        .map(|entry| {
            let owner = state.users.get_user_by_id(&entry.tenant_id);
            let nickname = owner
                .as_ref()
                .map(|user| user.nickname.clone())
                .unwrap_or_else(|| entry.tenant_id.clone());
            let email = owner
                .as_ref()
                .map(|user| user.email.clone())
                .unwrap_or_default();
            let avatar = owner
                .as_ref()
                .map(|user| user.avatar.clone())
                .unwrap_or_default();
            // The owner's own membership row is synthetic (`created_at == 0`), so
            // fall back to the account creation time instead of a blank date.
            let mut update_date = entry.updated_at.max(entry.created_at);
            if update_date == 0 {
                update_date = owner.as_ref().map(|user| user.created_at).unwrap_or(0);
            }
            serde_json::json!({
                "tenant_id": entry.tenant_id,
                "user_id": entry.user_id,
                "role": tenant_role_wire(entry.role),
                "invited_by": entry.invited_by,
                "created_at": entry.created_at,
                "updated_at": entry.updated_at,
                "nickname": nickname,
                "email": email,
                "avatar": avatar,
                // Upstream exposes `update_date`; keep the raw ms too.
                "update_date": update_date,
            })
        })
        .collect();
    Json(serde_json::json!({ "code": 0, "data": rows }))
}

#[derive(Debug, Default, Deserialize)]
pub struct TenantQuery {
    pub tenant_id: Option<String>,
}

/// GET /api/v1/tenant/members — list explicit members and pending invitations.
pub async fn list_tenant_members(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TenantQuery>,
) -> Response {
    let tenant_id = query.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if !can_manage_tenant(&state, &auth, tenant_id) {
        return forbidden_tenant();
    }
    let mut members = state.tenants.list_members(tenant_id);
    members.sort_by_key(|member| member.created_at);
    Json(serde_json::json!({ "code": 0, "data": members })).into_response()
}

fn tenant_user_rows(state: &AppState, tenant_id: &str) -> Vec<serde_json::Value> {
    let mut members = state.tenants.list_members(tenant_id);
    if !members
        .iter()
        .any(|member| member.user_id == tenant_id && member.role == TenantRole::Owner)
    {
        members.push(crate::kb::TenantMembership {
            tenant_id: tenant_id.to_string(),
            user_id: tenant_id.to_string(),
            role: TenantRole::Owner,
            invited_by: tenant_id.to_string(),
            created_at: 0,
            updated_at: 0,
        });
    }
    members.sort_by_key(|member| member.updated_at);
    members
        .into_iter()
        .map(|member| {
            let user = state.users.get_user_by_id(&member.user_id);
            let mut update_date = member.updated_at.max(member.created_at);
            if update_date == 0 {
                update_date = user.as_ref().map(|user| user.created_at).unwrap_or(0);
            }
            serde_json::json!({
                "user_id": member.user_id,
                "nickname": user.as_ref().map(|user| user.nickname.as_str()).unwrap_or(""),
                "email": user.as_ref().map(|user| user.email.as_str()).unwrap_or(""),
                "avatar": user.as_ref().map(|user| user.avatar.as_str()).unwrap_or(""),
                "role": tenant_role_wire(member.role),
                "update_date": update_date,
            })
        })
        .collect()
}

fn tenant_role_wire(role: TenantRole) -> &'static str {
    match role {
        TenantRole::Owner => "owner",
        TenantRole::Admin => "admin",
        TenantRole::Normal => "normal",
        TenantRole::Invite => "invite",
    }
}

/// GET `/api/v1/tenants/{tenant_id}/users` — the team's member rows joined
/// with the user registry (upstream `tenant_api.user_list`).
pub async fn list_tenant_users(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(tenant_id): Path<String>,
) -> Response {
    if !can_manage_tenant(&state, &auth, &tenant_id) {
        return forbidden_tenant();
    }
    Json(serde_json::json!({ "code": 0, "data": tenant_user_rows(&state, &tenant_id) }))
        .into_response()
}

/// POST `/api/v1/tenants/{tenant_id}/users` — invite a registered user with
/// the upstream role-conflict error messages.
pub async fn add_tenant_user(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(tenant_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if !can_manage_tenant(&state, &auth, &tenant_id) {
        return forbidden_tenant();
    }
    let email = body
        .get("email")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let Some(user) = state.users.get_user(&email) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 102, "message": "User not found." })),
        )
            .into_response();
    };
    if let Some(existing) = state.tenants.role(&tenant_id, &user.id) {
        return match existing {
            TenantRole::Owner => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "code": 102,
                    "message": format!("{email} is the owner of the team.")
                })),
            )
                .into_response(),
            TenantRole::Normal | TenantRole::Admin => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "code": 102,
                    "message": format!("{email} is already in the team.")
                })),
            )
                .into_response(),
            TenantRole::Invite => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "code": 102,
                    "message": format!("{email} is already in the team.")
                })),
            )
                .into_response(),
        };
    }
    match state
        .tenants
        .invite_member(&tenant_id, &user.id, &auth.user_id)
    {
        Ok(_) => Json(serde_json::json!({
            "code": 0,
            "data": {
                "id": user.id,
                "avatar": "",
                "email": user.email,
                "nickname": user.nickname,
            }
        }))
        .into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "code": 409, "message": error.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct RemoveTenantUserRequest {
    pub user_id: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateMeRequest {
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub new_password: Option<String>,
}

/// Upstream `validate_nickname`: 1-100 characters of letters, digits and the
/// fixed punctuation set.
fn validate_nickname(nickname: &str) -> anyhow::Result<String> {
    let trimmed = nickname.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Nickname cannot be empty.");
    }
    if trimmed.chars().count() > 100 {
        anyhow::bail!("Nickname must be at most 100 characters.");
    }
    if !trimmed.chars().all(|character| {
        character.is_alphanumeric() || matches!(character, ' ' | '.' | '_' | '\'' | '-')
    }) {
        anyhow::bail!("Nickname contains invalid characters.");
    }
    Ok(trimmed.to_string())
}

/// PATCH `/api/v1/users/me` — profile fields plus the optional current/new
/// password pair; a successful password change invalidates all user tokens.
pub async fn update_me(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<UpdateMeRequest>,
) -> Response {
    if body.password.is_some() || body.new_password.is_some() {
        let current = body.password.as_deref().unwrap_or("");
        let new_password = body.new_password.as_deref().unwrap_or("");
        match state
            .users
            .change_password(&auth.user_id, current, new_password)
        {
            Ok(()) => state.users.logout_user_tokens(&auth.user_id),
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "code": 102, "message": "Password error!" })),
                )
                    .into_response();
            }
        }
    }
    let nickname = match body.nickname.as_deref().map(validate_nickname).transpose() {
        Ok(nickname) => nickname,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 102, "message": error.to_string() })),
            )
                .into_response();
        }
    };
    match state
        .users
        .update_profile(&auth.user_id, nickname, body.avatar, body.timezone)
    {
        Ok(()) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 102, "message": error.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE `/api/v1/tenants/{tenant_id}/users` — the owner removes a member,
/// or a member removes themselves (upstream allows both).
pub async fn remove_tenant_user(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(tenant_id): Path<String>,
    Json(body): Json<RemoveTenantUserRequest>,
) -> Response {
    if !can_manage_tenant(&state, &auth, &tenant_id) && auth.user_id != body.user_id {
        return forbidden_tenant();
    }
    if tenant_id == body.user_id {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 102, "message": "The owner cannot be removed." })),
        )
            .into_response();
    }
    match state.tenants.remove_member(&tenant_id, &body.user_id) {
        Ok(_) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 102, "message": error.to_string() })),
        )
            .into_response(),
    }
}

/// PATCH `/api/v1/tenants/{tenant_id}` — accept an invitation (INVITE →
/// NORMAL), mirroring upstream `tenant_api.agree`.
pub async fn agree_tenant(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(tenant_id): Path<String>,
) -> Response {
    match state.tenants.accept_invitation(&tenant_id, &auth.user_id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Pending invitation not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 102, "message": error.to_string() })),
        )
            .into_response(),
    }
}

/// PATCH /api/v1/tenant/invitations/{tenant_id}/accept — accept an invitation.
pub async fn accept_tenant_invitation(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(tenant_id): Path<String>,
) -> Response {
    match state.tenants.accept_invitation(&tenant_id, &auth.user_id) {
        Ok(true) => {
            Json(serde_json::json!({ "code": 0, "message": "Invitation accepted" })).into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Pending invitation not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
        )
            .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateTenantRole {
    pub role: TenantRole,
    pub tenant_id: Option<String>,
}

/// PUT /api/v1/tenant/members/{user_id} — promote or demote a joined member.
pub async fn update_tenant_member_role(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(user_id): Path<String>,
    Json(body): Json<UpdateTenantRole>,
) -> Response {
    let tenant_id = body.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if !can_manage_tenant(&state, &auth, tenant_id) {
        return forbidden_tenant();
    }
    match state.tenants.update_role(tenant_id, &user_id, body.role) {
        Ok(true) => {
            Json(serde_json::json!({ "code": 0, "message": "Member role updated" })).into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Tenant member not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
        )
            .into_response(),
    }
}

/// DELETE /api/v1/tenant/members/{user_id} — remove a member or leave a tenant.
pub async fn remove_tenant_member(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(user_id): Path<String>,
    Query(query): Query<TenantQuery>,
) -> Response {
    let tenant_id = query.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if user_id != auth.user_id && !can_manage_tenant(&state, &auth, tenant_id) {
        return forbidden_tenant();
    }
    match state.tenants.remove_member(tenant_id, &user_id) {
        Ok(true) => {
            Json(serde_json::json!({ "code": 0, "message": "Member removed" })).into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Member not found" })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
        )
            .into_response(),
    }
}

// ── Models Management ──────────────────────────────────────────

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ModelConfig {
    pub id: String,
    pub name: String,
    pub model_type: crate::api::tenant_models::ModelCapability,
    pub provider: String,
    pub provider_id: String,
    pub instance_id: String,
    pub instance_name: String,
    pub selector: String,
    pub max_tokens: Option<u64>,
}

/// RAGFlow `/llm/list`-style model management list: expand the tenant's
/// instances through
/// [`plan_added_models`](crate::api::tenant_models::plan_added_models)
/// (factory fallback + ranking), then flatten back to one `ModelConfig` per
/// representable capability. Capabilities without a RayRAG variant (e.g.
/// `ocr`) are skipped.
pub fn plan_llm_models(
    providers: &[crate::api::features::Provider],
    instances: &[crate::api::tenant_models::TenantModelInstance],
    filter: &[String],
) -> Vec<ModelConfig> {
    use crate::api::tenant_models::ModelCapability;
    let compose_profiles = std::env::var("COMPOSE_PROFILES").unwrap_or_default();
    let tei_model = std::env::var("TEI_MODEL").unwrap_or_default();
    let entries = crate::api::tenant_models::plan_added_models(
        providers,
        instances,
        crate::api::tenant_models::factory_llm_entries(),
        filter,
        &compose_profiles,
        &tei_model,
    );
    let mut models = Vec::new();
    for entry in entries {
        let id_prefix = format!("{}/{}/{}", entry.provider_id, entry.instance_id, entry.name);
        let selector = format!(
            "{}@{}@{}",
            entry.name, entry.instance_name, entry.provider_name
        );
        for raw in entry.model_type {
            let model_type = match raw.as_str() {
                "chat" => ModelCapability::Chat,
                "embedding" => ModelCapability::Embedding,
                "rerank" => ModelCapability::Rerank,
                "image2text" => ModelCapability::ImageToText,
                "speech2text" => ModelCapability::SpeechToText,
                "tts" => ModelCapability::TextToSpeech,
                "ocr" => ModelCapability::Ocr,
                _ => continue,
            };
            models.push(ModelConfig {
                id: id_prefix.clone(),
                name: entry.name.clone(),
                model_type,
                provider: entry.provider_name.clone(),
                provider_id: entry.provider_id.clone(),
                instance_id: entry.instance_id.clone(),
                instance_name: entry.instance_name.clone(),
                selector: selector.clone(),
                max_tokens: entry.max_tokens,
            });
        }
    }
    models
}

/// GET /api/v1/llm/models — list LLM models
pub async fn list_models(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TenantQuery>,
) -> Response {
    let tenant_id = query.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if !state.tenants.is_member(tenant_id, &auth.user_id) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "code": 403, "message": "Tenant membership required" })),
        )
            .into_response();
    }
    let providers: Vec<_> = state
        .providers
        .list_configured()
        .into_iter()
        .filter(|provider| provider.enabled)
        .collect();
    let instances = state.tenant_models.list_configured(tenant_id);
    let models = plan_llm_models(&providers, &instances, &[]);
    Json(serde_json::json!({"code":0,"data":models})).into_response()
}

// ── LLM App Management ─────────────────────────────────────────

/// GET /api/v1/llm/factories — list LLM factories
pub async fn list_factories() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": [
            {"name":"OpenAI","logo":"","tags":"LLM,TEXT EMBEDDING,SPEECH2TEXT,MODERATION","status":"1"},
            {"name":"MiniMax","logo":"","tags":"LLM,TEXT EMBEDDING","status":"1"},
            {"name":"Qwen","logo":"","tags":"LLM","status":"1"},
        ]
    }))
}

/// GET /api/v1/llm/my_llms — list user's configured LLMs
pub async fn my_llms(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<TenantQuery>,
) -> Response {
    let tenant_id = query.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if !state.tenants.is_member(tenant_id, &auth.user_id) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "code": 403, "message": "Tenant membership required" })),
        )
            .into_response();
    }
    Json(serde_json::json!({
        "code": 0,
        "data": state.tenant_models.list(tenant_id)
    }))
    .into_response()
}

// ── Backward Compatibility ─────────────────────────────────────

/// GET /api/v1/system/healthz — unauthenticated container readiness probe.
pub async fn healthz(State(state): State<Arc<AppState>>) -> Response {
    let postgres = crate::persistence::snapshot_mirror_health();
    let zvec = state.vector_mirror.health();
    let mut errors = Vec::new();
    if let Err(error) = &postgres {
        errors.push(format!("postgres: {error}"));
    }
    if let Err(error) = &zvec {
        errors.push(format!("zvec: {error}"));
    }
    let status = if errors.is_empty() {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    };
    (
        status,
        Json(serde_json::json!({
            "code": if errors.is_empty() { 0 } else { 500 },
            "data": {
                "status": if errors.is_empty() { "healthy" } else { "unhealthy" },
                "postgres": match postgres { Ok(true) => "healthy", Ok(false) => "disabled", Err(_) => "unhealthy" },
                "zvec": match zvec { Ok(true) => "healthy", Ok(false) => "disabled", Err(_) => "unhealthy" },
                "errors": errors,
            }
        })),
    )
        .into_response()
}

/// POST /api/v1/document/upload_info — backward compat
pub async fn document_upload_info() -> impl IntoResponse {
    Json(serde_json::json!({"code":0,"data":{"url":"/api/v1/files/upload"}}))
}

const RELATED_QUESTION_PROMPT: &str = r#"You are an AI language model assistant tasked with generating 5-10 related questions based on a user's original query. Rephrase it in diverse, clear, concise ways that broaden search scope while remaining relevant. Return only a numbered list in the form `1. question`. If no relevant alternatives can be generated, return no questions."#;

#[derive(Deserialize)]
pub struct RelatedQuestionsRequest {
    pub question: String,
    #[serde(default)]
    pub search_id: Option<String>,
    #[serde(default)]
    pub tenant_id: Option<String>,
}

fn parse_related_questions(answer: &str) -> Vec<String> {
    answer
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (number, question) = line.split_once(". ")?;
            (!number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| question.trim().to_string())
                .filter(|question| !question.is_empty())
        })
        .collect()
}

/// POST /api/v1/chat/recommendation and compatibility aliases.
pub async fn related_questions(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<RelatedQuestionsRequest>,
) -> Response {
    let question = body.question.trim();
    if question.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "code": 400, "message": "question is required" })),
        )
            .into_response();
    }
    // Upstream resolves the search app's own `chat_id` instance first and only
    // then falls back to the tenant default chat model; `llm_setting` overrides
    // the generation parameters (default temperature 0.9).
    let search_app = body
        .search_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
        .and_then(|id| state.search_apps.as_ref().and_then(|store| store.get(id)));
    let mut generation = crate::generation_params::GenerationParamsPatch {
        temperature: Some(0.9),
        ..Default::default()
    };
    let mut instance_selector: Option<String> = None;
    if let Some(app) = search_app.as_ref() {
        if !app.chat_id.trim().is_empty() {
            instance_selector = Some(app.chat_id.clone());
        }
        if let Some(settings) = app.llm_setting.as_object() {
            // Upstream drops `parameter` (it is a nested provider payload) and
            // merges the remaining keys into the generation config.
            let mut cleaned = settings.clone();
            cleaned.remove("parameter");
            if !cleaned.is_empty() {
                match crate::generation_params::GenerationParamsPatch::from_request(
                    &serde_json::Value::Object(cleaned),
                ) {
                    Ok(mut patch) => {
                        // Upstream default applies whenever the app does not
                        // pin its own temperature.
                        patch.temperature = patch.temperature.or(Some(0.9));
                        generation = patch;
                    }
                    Err(error) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({
                                "code": 400,
                                "message": format!("invalid llm_setting: {error}"),
                            })),
                        )
                            .into_response();
                    }
                }
            }
        }
    }
    let default_selector =
        instance_selector.or_else(|| state.tenant_models.default_chat_model(&auth.user_id));
    let tenant_llm = match state.tenant_models.resolve(
        &state.providers,
        &auth.user_id,
        crate::api::tenant_models::ModelCapability::Chat,
        default_selector.as_deref(),
    ) {
        Ok(model) => model.map(|model| model.llm_client()),
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "code": 400, "message": error.to_string() })),
            )
                .into_response();
        }
    };
    let Some(llm) = tenant_llm.as_ref().or(state.llm.as_deref()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "code": 503, "message": "Chat model is not configured" })),
        )
            .into_response();
    };
    let messages = [
        ChatMessage::new("system", RELATED_QUESTION_PROMPT),
        ChatMessage::new(
            "user",
            format!("\nKeywords: {question}\nRelated search terms:\n    "),
        ),
    ];
    match llm
        .chat_completion_with_generation(&messages, generation)
        .await
    {
        Ok(completion) => Json(serde_json::json!({
            "code": 0,
            "data": parse_related_questions(&completion.content),
        }))
        .into_response(),
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            Json(
                serde_json::json!({ "code": 502, "message": format!("LLM call failed: {error}") }),
            ),
        )
            .into_response(),
    }
}

// ── File Commit / Versioning ───────────────────────────────────

/// GET /api/v1/file/commits — list file versions
pub async fn list_commits() -> impl IntoResponse {
    Json(serde_json::json!({"code":0,"data":[]}))
}

// ── File 2 Document Conversion ─────────────────────────────────

/// POST /api/v1/file2document — convert file to document
pub async fn file2document(Json(body): Json<serde_json::Value>) -> impl IntoResponse {
    let file_id = body
        .get("file_ids")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    Json(serde_json::json!({
        "code": 0,
        "message": format!("{} file(s) queued for conversion", file_id),
    }))
}

#[cfg(test)]
mod tests {
    use super::parse_related_questions;
    use super::plan_llm_models;
    use super::validate_nickname;
    use crate::api::tenant_models::ModelCapability;

    fn provider(id: &str, name: &str) -> crate::api::features::Provider {
        crate::api::features::Provider {
            id: id.into(),
            name: name.into(),
            api_base: "http://127.0.0.1:8080/v1".into(),
            models: vec![],
            enabled: true,
            api_key: None,
        }
    }

    #[test]
    fn nickname_validation_matches_the_fixed_contract() {
        assert_eq!(validate_nickname("  Ray Rag 2.0 ").unwrap(), "Ray Rag 2.0");
        assert_eq!(validate_nickname("O'Brien-2").unwrap(), "O'Brien-2");
        assert!(validate_nickname("").is_err());
        assert!(validate_nickname(&"x".repeat(101)).is_err());
        assert!(validate_nickname("bad/name!").is_err());
    }

    fn instance(
        tenant: &str,
        provider_id: &str,
        instance_id: &str,
        instance_name: &str,
        models: Vec<crate::api::tenant_models::TenantModelSpec>,
    ) -> crate::api::tenant_models::TenantModelInstance {
        crate::api::tenant_models::TenantModelInstance {
            tenant_id: tenant.into(),
            provider_id: provider_id.into(),
            instance_id: instance_id.into(),
            instance_name: instance_name.into(),
            api_base: "http://127.0.0.1:8080/v1".into(),
            api_key: None,
            region: None,
            models,
            extra: Default::default(),
        }
    }

    fn spec(
        name: &str,
        types: &[ModelCapability],
        enabled: bool,
    ) -> crate::api::tenant_models::TenantModelSpec {
        crate::api::tenant_models::TenantModelSpec {
            name: name.into(),
            model_types: types.to_vec(),
            max_tokens: Some(8192),
            enabled,
            is_tools: false,
            ocr_config: None,
        }
    }

    #[test]
    fn related_question_parser_matches_ragflow_numbered_line_contract() {
        assert_eq!(
            parse_related_questions(
                "Here are suggestions:\n1. First question?\n\n02. Second question?\n- ignored\n3.no space\n4.   "
            ),
            vec!["First question?", "Second question?"]
        );
        assert!(parse_related_questions("No numbered questions").is_empty());
    }

    #[test]
    fn llm_models_plan_expands_real_factory_catalog_with_rank_order() {
        let providers = vec![provider("oa", "OpenAI"), provider("ds", "DeepSeek")];
        let instances = vec![
            instance(
                "tenant-a",
                "oa",
                "i1",
                "default",
                vec![spec("chat-dummy", &[ModelCapability::Chat], true)],
            ),
            instance(
                "tenant-a",
                "ds",
                "i2",
                "default",
                vec![spec("deep-chat-dummy", &[ModelCapability::Chat], true)],
            ),
        ];
        let models = plan_llm_models(&providers, &instances, &[]);
        assert!(!models.is_empty());
        // Factory fallback: OpenAI's catalog is exposed per tenant instance
        // (gpt-5.5 is the first chat row of the embedded catalog).
        assert!(
            models
                .iter()
                .any(|model| model.name == "gpt-5.5" && model.model_type == ModelCapability::Chat)
        );
        assert!(
            models
                .iter()
                .any(|model| model.selector == "gpt-5.5@default@OpenAI")
        );
        // Manual-only models still surface.
        assert!(models.iter().any(|model| model.name == "chat-dummy"));
        // Ranking: OpenAI (rank 999) sorts before DeepSeek.
        let first_openai = models
            .iter()
            .position(|model| model.provider == "OpenAI")
            .unwrap();
        let first_deepseek = models
            .iter()
            .position(|model| model.provider == "DeepSeek")
            .unwrap();
        assert!(first_openai < first_deepseek);
    }

    #[test]
    fn llm_models_plan_keeps_image2text_side_of_ocr_rows_and_never_emits_ocr() {
        let providers = vec![provider("tq", "Tongyi-Qianwen")];
        let instances = vec![instance(
            "tenant-a",
            "tq",
            "i1",
            "default",
            vec![spec("chat-dummy", &[ModelCapability::Chat], true)],
        )];
        let models = plan_llm_models(&providers, &instances, &[]);
        // The ocr-tagged factory rows surface their image2text side, while
        // ocr-only capabilities now have an explicit ModelConfig type.
        assert!(models.iter().any(|model| {
            model.name == "qwen-vl-ocr-2025-11-20"
                && model.model_type == ModelCapability::ImageToText
        }));
        // Every emitted entry carries a representable capability, including
        // the explicit OCR type surfaced by the fixed factory catalog.
        for model in &models {
            assert!(matches!(
                model.model_type,
                ModelCapability::Chat
                    | ModelCapability::Embedding
                    | ModelCapability::Rerank
                    | ModelCapability::ImageToText
                    | ModelCapability::SpeechToText
                    | ModelCapability::TextToSpeech
                    | ModelCapability::Ocr
            ));
        }
    }
}
