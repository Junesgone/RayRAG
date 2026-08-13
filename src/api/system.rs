//! System / User / Tenant / Models / Langfuse / Backward Compat / LLM App APIs.
//! Replaces RAGFlow's system_api, user_api, tenant_api, models_api, langfuse_api,
//! backward_compat, and llm_app.

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

/// GET /api/v1/system — get system settings
pub async fn get_system() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": SystemConfig {
            version: env!("CARGO_PKG_VERSION").into(),
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
            "version": env!("CARGO_PKG_VERSION"),
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
    Json(serde_json::json!({ "code": 0, "data": tenants }))
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

// ── Langfuse Observability ─────────────────────────────────────

#[derive(Serialize, Deserialize)]
pub struct LangfuseConfig {
    pub enabled: bool,
    pub host: String,
    pub public_key: String,
}

/// GET /api/v1/langfuse — get langfuse config
pub async fn get_langfuse() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": LangfuseConfig {
            enabled: false,
            host: "https://cloud.langfuse.com".into(),
            public_key: "".into(),
        }
    }))
}

/// POST /api/v1/langfuse — update langfuse config
pub async fn update_langfuse(
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<LangfuseConfig>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    Json(serde_json::json!({"code":0,"message":"Updated","data":body})).into_response()
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
    // search_id is accepted for wire compatibility. RayRAG does not yet expose
    // RAGFlow SearchApp configuration, so model selection remains tenant-scoped.
    let _search_id = body.search_id.as_deref();
    let default_selector = state.tenant_models.default_chat_model(&auth.user_id);
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
    let generation = crate::generation_params::GenerationParamsPatch {
        temperature: Some(0.9),
        ..Default::default()
    };
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
            models,
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
        // The ocr-tagged factory rows surface their image2text side; an
        // ocr-only capability has no ModelConfig representation.
        assert!(
            models.iter().any(|model| {
                model.name == "qwen-vl-ocr-2025-11-20" && model.model_type == ModelCapability::ImageToText
            })
        );
        // Every emitted entry carries a representable capability.
        for model in &models {
            assert!(matches!(
                model.model_type,
                ModelCapability::Chat
                    | ModelCapability::Embedding
                    | ModelCapability::Rerank
                    | ModelCapability::ImageToText
                    | ModelCapability::SpeechToText
                    | ModelCapability::TextToSpeech
            ));
        }
    }
}
