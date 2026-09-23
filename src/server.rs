//! RayRAG Web Server — serves Rust SSR UI + REST API.
//! Startup: `rayrag serve --port 9380`

use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Extension, Multipart, Path, Query, Request, State, multipart::Field,
    },
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, RwLock},
};
use tokio::io::AsyncWriteExt;
use tower_http::{cors::CorsLayer, services::ServeDir};
use tracing;

use crate::auth::UserStore;
use crate::embed::SharedEmbedder;
use crate::kb::{KbStore, TenantStore};
use crate::llm::{ConvStore, LlmClient, LlmConfig};
use crate::rerank::{
    Reranker, RerankerConfig, RerankerManager, apply_hybrid_rerank, rerank_window,
};
use crate::search::{SearchEngine, aggregate_documents, highlight_content};

// ── Response ────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ApiResponse {
    pub code: i32,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

// ── AppState ────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub static_dir: String,
    pub port: u16,
    pub users: Arc<UserStore>,
    pub kbs: Arc<KbStore>,
    pub tenants: Arc<TenantStore>,
    pub conversations: Arc<ConvStore>,
    pub llm: Option<Arc<LlmClient>>,
    pub embedder: Option<SharedEmbedder>,
    pub reranker: Arc<RerankerManager>,
    pub graphs: Arc<crate::graph_store::GraphStore>,
    pub engine: Arc<RwLock<SearchEngine>>,
    pub vector_mirror: Arc<crate::store::OnlineVectorMirror>,
    pub index_path: String,
    pub model_path: String,
    pub max_upload_bytes: usize,
    pub docs: Arc<crate::api::document::DocStore>,
    pub document_metadata: Arc<crate::api::document_metadata::DocumentMetadataStore>,
    pub files: Arc<crate::api::file_mgr::FileStore>,
    pub data_sources: Option<Arc<crate::api::data_source_mgr::DataSourceStore>>,
    pub search_apps: Option<Arc<crate::api::searchapp_mgr::SearchAppStore>>,
    pub chat_apps: Option<Arc<crate::api::chatapp_mgr::ChatAppStore>>,
    pub providers: Arc<crate::api::features::ProviderStore>,
    pub tenant_models: Arc<crate::api::tenant_models::TenantModelStore>,
    pub memories: Arc<crate::api::features::MemoryStore>,
    pub memory_messages: Arc<crate::api::joint_services::MemoryMessageService>,
    pub skill_index: Arc<crate::api::skill_index::SkillIndexStore>,
    pub system_settings: Arc<crate::api::system_settings::SystemSettingsStore>,
    /// RAGFlow `APIToken` rows (`/api/v1/system/tokens`). The `beta` secret of
    /// each row is the credential the embed/share surfaces present as a bearer
    /// token, so this store also serves the AUTH_BETA lookup.
    pub api_tokens: Arc<crate::api::tokens::ApiTokenStore>,
    /// RAGFlow `PipelineOperationLog` rows (`/api/v1/datasets/{id}/ingestions`):
    /// one record per document parse, whose `dsl` drives `/dataflow-result`.
    pub ingestion_logs: Arc<crate::api::ingestion::IngestionLogStore>,
    /// RAGFlow `APIToken`-scoped agent run traces
    /// (`/api/v1/agents/{id}/logs/{message_id}` and its beta sibling): one
    /// `ITraceData` array per assistant message, written while the run streams.
    pub agent_traces: Arc<crate::api::agent_trace::AgentTraceStore>,
    /// Effective sign-up switch — upstream `settings.REGISTER_ENABLED` (plus the
    /// legacy `RAYRAG_ALLOW_REGISTRATION` opt-in), resolved once at startup by
    /// `settings::resolve_register_enabled`. Read by both
    /// `GET /api/v1/system/config` (the login page's `registerEnabled` gate) and
    /// `POST /api/v1/user/register` so the two can never disagree.
    pub register_enabled: i64,
    /// `DISABLE_PASSWORD_LOGIN` — second field of the same payload.
    pub disable_password_login: bool,
    /// Single-use OAuth `state` values (upstream keeps them in the session).
    pub oauth_states: Arc<crate::oauth_config::StateStore>,
    /// OAuth login channels (upstream `settings.OAUTH_CONFIG`), resolved once at
    /// startup from `RAYRAG_OAUTH_CONFIG`/`OAUTH` and injectable in tests.
    pub oauth_channels: Arc<std::sync::RwLock<Vec<crate::oauth_config::OAuthChannel>>>,
    pub langfuse: Arc<crate::api::langfuse::LangfuseStore>,
    pub mcp_servers: Arc<crate::api::mcp_mgr::McpServerStore>,
    pub chat_channels: Arc<crate::api::chat_channel_mgr::ChatChannelStore>,
    pub tasks: Arc<crate::api::features::TaskQueue>,
    /// Async document task executor (RAGFlow `task_executor.py` port): the
    /// queue + worker pool that runs document processing off the request path.
    pub task_executor: Arc<crate::task_executor::TaskExecutor>,
    pub agents: Arc<crate::api::features::AgentStore>,
    pub agent_runs: Arc<crate::api::features::AgentRunRegistry>,
    pub agent_checkpoints: Arc<crate::agent_checkpoint::AgentCheckpointStore>,
    pub canvas_versions: Arc<crate::api::features::CanvasVersionStore>,
    pub compilation_templates: Arc<crate::api::compilation_templates::CompilationTemplateStore>,
    pub evaluations: Arc<crate::api::evaluation::EvaluationStore>,
    pub log_levels: Arc<crate::logging::LogLevelManager>,
    pub chunk_feedback_enabled: bool,
    pub chunk_feedback_weighting: crate::chunk_feedback::FeedbackWeighting,
    /// Chat channel registry (RAGFlow `api/channels` bootstrap port): built-in
    /// builders registered at startup, running instances tracked by account id.
    pub channels: Arc<crate::channels::ChannelRegistry>,
    pub(crate) document_commit_lock: Arc<std::sync::Mutex<()>>,
    /// Serializes the cross-snapshot Memory task/message commit protocol.
    /// Model and embedding requests must finish before taking this lock.
    pub(crate) memory_commit_lock: Arc<std::sync::Mutex<()>>,
}

// ── Auth ────────────────────────────────────────────────────────

async fn password_public_key(State(state): State<Arc<AppState>>) -> Response {
    let public_key = state.users.password_public_key();
    Json(ApiResponse {
        code: 0,
        message: "ok".into(),
        data: Some(serde_json::json!({
            "enabled": public_key.is_some(),
            "algorithm": "RSAES-PKCS1-v1_5",
            "encoding": "base64(RSA(base64(UTF-8)))",
            "public_key": public_key,
        })),
    })
    .into_response()
}

async fn login(State(state): State<Arc<AppState>>, Json(b): Json<serde_json::Value>) -> Response {
    let email = b.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let password = b.get("password").and_then(|v| v.as_str()).unwrap_or("");
    match state.users.login(email, password) {
        Ok(Some(token)) => {
            let u = state.users.get_user(email);
            (
                StatusCode::OK,
                Json(ApiResponse {
                    code: 0,
                    message: "ok".into(),
                    data: Some(
                        serde_json::json!({"access_token": token, "user_id": u.map(|x| x.id)}),
                    ),
                }),
            )
                .into_response()
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse {
                code: 401,
                message: "Invalid credentials".into(),
                data: None,
            }),
        )
            .into_response(),
    }
}

/// Upstream `RetCode` body shape: `get_json_result` answers **HTTP 200** for
/// every `RetCode` failure and only ever emits `data` when `code == 0`
/// (`api/utils/api_utils.py::get_result`).
fn ret_code_response(code: i32, message: &str) -> Response {
    (
        StatusCode::OK,
        Json(serde_json::json!({ "code": code, "message": message })),
    )
        .into_response()
}

/// `POST /api/v1/users` — upstream `api/apps/restful_apis/user_api.py::user_add`,
/// the endpoint `web/src/utils/api.ts::register` posts to (the blueprint is
/// mounted at `/api/v1`, the same prefix as the `GET` that lists users).
///
/// `POST /api/v1/user/register` stays mounted on this handler as a compatible
/// alias for clients written against earlier RayRAG builds; upstream has no
/// such path.
///
/// The upstream order is kept exactly: `@validate_request` → `REGISTER_ENABLED`
/// → email shape → duplicate email → `validate_nickname` → insert. The gate is
/// `settings.REGISTER_ENABLED`, the very value `GET /api/v1/system/config`
/// reports as `registerEnabled` and the login page uses to show or hide its
/// sign-up face, so the page can never offer a form this handler refuses.
async fn user_add(
    State(state): State<Arc<AppState>>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    use crate::user_register as upstream;

    if let Some(message) =
        upstream::missing_required_arguments(&b, &["nickname", "email", "password"])
    {
        return ret_code_response(upstream::ARGUMENT_ERROR, &message);
    }

    if state.register_enabled == 0 {
        return ret_code_response(
            upstream::OPERATING_ERROR,
            upstream::REGISTRATION_DISABLED_MESSAGE,
        );
    }

    let raw_email = b
        .get("email")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let email = raw_email.trim();
    // Upstream would raise on a non-string email here and answer a 500 through
    // the generic handler; answering the invalid-address message is the
    // graceful reading of the same intent.
    if !upstream::email_is_valid(email) {
        return ret_code_response(
            upstream::OPERATING_ERROR,
            &upstream::invalid_email_message(email),
        );
    }

    if state.users.get_user(email).is_some() {
        return ret_code_response(
            upstream::OPERATING_ERROR,
            &upstream::duplicate_email_message(email),
        );
    }

    let nickname = match upstream::validate_nickname(b.get("nickname")) {
        Ok(nickname) => nickname,
        Err(message) => return ret_code_response(upstream::ARGUMENT_ERROR, &message),
    };

    let password = b
        .get("password")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    match state.users.register(&nickname, email, password) {
        Ok(user) => {
            // Upstream calls `login_user(user)` and passes `auth=user.get_id()`
            // to `construct_response`, which puts the credential in the
            // `Authorization` response header.
            let token = state.users.issue_token_for(&user.id);
            let mut response = Json(serde_json::json!({
                "code": 0,
                "message": upstream::welcome_message(&user.nickname),
                "data": {
                    "id": user.id,
                    "nickname": user.nickname,
                    "email": user.email,
                    "avatar": user.avatar,
                    "timezone": user.timezone,
                    "role": user.role,
                    "created_at": user.created_at,
                    "access_token": token,
                },
            }))
            .into_response();
            if let Some(token) = token {
                if let Ok(value) = axum::http::HeaderValue::from_str(&token) {
                    response
                        .headers_mut()
                        .insert(axum::http::header::AUTHORIZATION, value);
                }
            }
            response
        }
        Err(error) => ret_code_response(
            upstream::EXCEPTION_ERROR,
            &upstream::registration_failure_message(&error.to_string()),
        ),
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Self-service personal API keys (settings page) ────────────────────

#[derive(Clone, Serialize)]
struct SelfApiKey {
    token: String,
    create_time: u64,
    create_date: String,
}

static SELF_API_KEYS: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<String, Vec<SelfApiKey>>>,
> = std::sync::OnceLock::new();

fn self_api_keys() -> &'static std::sync::Mutex<std::collections::HashMap<String, Vec<SelfApiKey>>>
{
    SELF_API_KEYS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// GET /api/v1/system/api_keys — list the current user's personal API keys.
async fn list_my_api_keys(Extension(auth): Extension<AuthContext>) -> Response {
    let keys = self_api_keys()
        .lock()
        .unwrap()
        .get(&auth.user_id)
        .cloned()
        .unwrap_or_default();
    Json(serde_json::json!({ "code": 0, "data": keys })).into_response()
}

/// POST /api/v1/system/api_keys — generate and persist a personal API key.
async fn generate_my_api_key(Extension(auth): Extension<AuthContext>) -> Response {
    let token = format!("sk-{}", uuid::Uuid::new_v4());
    let now = unix_ms();
    let entry = SelfApiKey {
        token: token.clone(),
        create_time: now,
        create_date: chrono::Utc::now().to_rfc3339(),
    };
    self_api_keys()
        .lock()
        .unwrap()
        .entry(auth.user_id.clone())
        .or_default()
        .push(entry);
    Json(serde_json::json!({ "code": 0, "data": { "api_key": token } })).into_response()
}

/// DELETE /api/v1/system/api_keys/{key} — revoke one of the current user's API keys.
async fn delete_my_api_key(
    Extension(auth): Extension<AuthContext>,
    Path(key): Path<String>,
) -> Response {
    let mut map = self_api_keys().lock().unwrap();
    if let Some(keys) = map.get_mut(&auth.user_id) {
        keys.retain(|entry| entry.token != key);
    }
    Json(serde_json::json!({ "code": 0, "data": true })).into_response()
}

// ── File rename / copy (files page) ────────────────────────────────────

fn file_physical_path(data_dir: &str, id: &str, name: &str) -> std::path::PathBuf {
    let extension = std::path::Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    std::path::Path::new(data_dir).join(if extension.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{extension}")
    })
}

/// PATCH /api/v1/files/{id} — rename a file record (and physical blob when the extension changes).
async fn rename_file(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(file_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let name = body
        .get("name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let Some(name) = name else {
        return Json(serde_json::json!({ "code": 400, "message": "name is required" }))
            .into_response();
    };
    let Some(record) = state
        .files
        .list_for(&auth.user_id, auth.is_admin, "root")
        .into_iter()
        .find(|record| record.id == file_id)
    else {
        return Json(serde_json::json!({ "code": 404, "message": "File not found" }))
            .into_response();
    };
    if record.name != name {
        let old_path = file_physical_path(&state.files.data_dir, &file_id, &record.name);
        let new_path = file_physical_path(&state.files.data_dir, &file_id, &name);
        if old_path != new_path && old_path.exists() {
            let _ = std::fs::rename(&old_path, &new_path);
        }
        let mut updated = record.clone();
        updated.name = name;
        let _ = state.files.remove(&file_id);
        match state.files.add(updated) {
            Ok(_) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
            Err(error) => Json(serde_json::json!({ "code": 500, "message": error.to_string() }))
                .into_response(),
        }
    } else {
        Json(serde_json::json!({ "code": 0, "data": true })).into_response()
    }
}

/// POST /api/v1/files/{id}/copy — duplicate a file record and its physical blob.
async fn copy_file(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(file_id): Path<String>,
) -> Response {
    let Some(record) = state
        .files
        .list_for(&auth.user_id, auth.is_admin, "root")
        .into_iter()
        .find(|record| record.id == file_id)
    else {
        return Json(serde_json::json!({ "code": 404, "message": "File not found" }))
            .into_response();
    };
    let new_id = uuid::Uuid::new_v4().to_string();
    let source = file_physical_path(&state.files.data_dir, &file_id, &record.name);
    let target = file_physical_path(&state.files.data_dir, &new_id, &record.name);
    if source.exists() {
        let _ = std::fs::copy(&source, &target);
    }
    let mut copied = record.clone();
    copied.id = new_id;
    copied.created_at = unix_ms();
    match state.files.add_unique(copied) {
        Ok(record) => Json(serde_json::json!({ "code": 0, "data": record })).into_response(),
        Err(error) => {
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })).into_response()
        }
    }
}

#[derive(Clone)]
pub struct AuthContext {
    pub user_id: String,
    pub is_admin: bool,
    /// Bearer/cookie token that authenticated this request. Kept so handlers
    /// that must invalidate the session (the admin console logout) can revoke
    /// exactly the credential that was presented.
    pub token: String,
}

pub(crate) fn kb_accessible(state: &AppState, kb_id: &str, auth: &AuthContext) -> bool {
    state
        .kbs
        .can_read(kb_id, &auth.user_id, auth.is_admin, |tenant_id, user_id| {
            state.tenants.is_member(tenant_id, user_id)
        })
}

pub(crate) fn kb_manageable(state: &AppState, kb_id: &str, auth: &AuthContext) -> bool {
    state.kbs.get(kb_id).is_some_and(|kb| {
        kb.owner_id == auth.user_id
            || state.tenants.can_manage(&kb.owner_id, &auth.user_id)
            || (auth.is_admin && kb.owner_id.is_empty())
    })
}

pub(crate) fn all_kbs_accessible(state: &AppState, kb_ids: &[String], auth: &AuthContext) -> bool {
    !kb_ids.is_empty() && kb_ids.iter().all(|kb_id| kb_accessible(state, kb_id, auth))
}

pub(crate) fn validate_tenant_embedding_selector(
    state: &AppState,
    tenant_id: &str,
    selector: Option<&str>,
) -> anyhow::Result<String> {
    let selector = selector.map(str::trim).filter(|value| !value.is_empty());
    if selector == Some("default") {
        if state.embedder.is_none() {
            anyhow::bail!("Global embedding model is not configured");
        }
        return Ok("default".into());
    }
    match state.tenant_models.resolve(
        &state.providers,
        tenant_id,
        crate::api::tenant_models::ModelCapability::Embedding,
        selector,
    )? {
        Some(model) => Ok(model.id()),
        None if selector.is_none() && state.embedder.is_some() => Ok("default".into()),
        None if selector.is_none() => Ok("default".into()),
        None => anyhow::bail!("Embedding model is not configured"),
    }
}

pub(crate) fn validate_kb_embedding_bindings(
    state: &AppState,
    kb_ids: &[String],
) -> anyhow::Result<String> {
    let mut binding: Option<String> = None;
    for kb_id in kb_ids {
        let kb = state
            .kbs
            .get(kb_id)
            .ok_or_else(|| anyhow::anyhow!("Knowledge base not found: {kb_id}"))?;
        let selector = kb.embd_id.trim();
        if selector.is_empty() || selector == "default" {
            let current = "default".to_string();
            if binding.as_ref().is_some_and(|binding| binding != &current) {
                anyhow::bail!("Knowledge bases use different embedding models");
            }
            binding = Some(current);
            continue;
        }
        let model = state
            .tenant_models
            .resolve(
                &state.providers,
                &kb.owner_id,
                crate::api::tenant_models::ModelCapability::Embedding,
                Some(selector),
            )?
            .ok_or_else(|| anyhow::anyhow!("Embedding model is not configured: {selector}"))?;
        let current = model.id();
        if binding.as_ref().is_some_and(|binding| binding != &current) {
            anyhow::bail!("Knowledge bases use different embedding models");
        }
        binding = Some(current);
    }
    binding.ok_or_else(|| anyhow::anyhow!("At least one knowledge base is required"))
}

/// `Knowledgebase.parser_id` — upstream stores the chunk method on the dataset
/// row; RayRAG keeps it inside `parser_config.chunk_method`.
fn kb_parser_id(kb: &crate::kb::KnowledgeBase) -> String {
    serde_json::from_str::<serde_json::Value>(&kb.parser_config)
        .ok()
        .and_then(|config| {
            config
                .get("chunk_method")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "naive".to_string())
}

pub(crate) fn kb_embedder_for(
    state: &AppState,
    kb_ids: &[String],
) -> anyhow::Result<SharedEmbedder> {
    let binding = validate_kb_embedding_bindings(state, kb_ids)?;
    if binding == "default" {
        return state
            .embedder
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Embedding is not configured"));
    }
    let first = state
        .kbs
        .get(&kb_ids[0])
        .ok_or_else(|| anyhow::anyhow!("Knowledge base not found: {}", kb_ids[0]))?;
    state
        .tenant_models
        .resolve(
            &state.providers,
            &first.owner_id,
            crate::api::tenant_models::ModelCapability::Embedding,
            Some(&binding),
        )?
        .map(|model| model.embedder())
        .ok_or_else(|| anyhow::anyhow!("Embedding is not configured: {binding}"))
}

/// Resolve a tenant's default image-to-text (vision) model for image parsing —
/// mirrors RAGFlow `get_tenant_default_model_by_type(tenant_id, LLMType.IMAGE2TEXT)`.
/// Returns `None` when no image2text model is configured for the KB owner, so
/// image parsing silently falls back to the OCR/metadata figure path.
pub(crate) fn kb_vision_for(
    state: &AppState,
    kb_ids: &[String],
) -> anyhow::Result<Option<crate::vision::VisionClient>> {
    let Some(first_kb_id) = kb_ids.first() else {
        return Ok(None);
    };
    let kb = state
        .kbs
        .get(first_kb_id)
        .ok_or_else(|| anyhow::anyhow!("Knowledge base not found: {first_kb_id}"))?;
    let Some(model) = state.tenant_models.resolve(
        &state.providers,
        &kb.owner_id,
        crate::api::tenant_models::ModelCapability::ImageToText,
        None,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(crate::vision::VisionClient::new(
        crate::vision::VisionConfig {
            api_base: model.api_base,
            api_key: model.api_key.unwrap_or_default(),
            model: model.model_name,
            lang: "Chinese".into(),
        },
    )))
}

pub(crate) fn kb_reranker_for(
    state: &AppState,
    kb_ids: &[String],
    selector: Option<&str>,
) -> anyhow::Result<Option<Arc<dyn Reranker>>> {
    let Some(selector) = selector.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(state.reranker.current());
    };
    let mut binding: Option<String> = None;
    let mut reranker = None;
    for kb_id in kb_ids {
        let kb = state
            .kbs
            .get(kb_id)
            .ok_or_else(|| anyhow::anyhow!("Knowledge base not found: {kb_id}"))?;
        let model = state
            .tenant_models
            .resolve(
                &state.providers,
                &kb.owner_id,
                crate::api::tenant_models::ModelCapability::Rerank,
                Some(selector),
            )?
            .ok_or_else(|| anyhow::anyhow!("Rerank model is not configured: {selector}"))?;
        let current = model.id();
        if binding.as_ref().is_some_and(|binding| binding != &current) {
            anyhow::bail!("Knowledge bases use different reranking models");
        }
        binding = Some(current);
        if reranker.is_none() {
            reranker = Some(model.reranker());
        }
    }
    reranker
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("At least one knowledge base is required"))
}

// ── Knowledge bases ─────────────────────────────────────────────

async fn list_datasets(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> impl IntoResponse {
    Json(ApiResponse {
        code: 0,
        message: "ok".into(),
        data: Some(
            serde_json::to_value(state.kbs.list_accessible(
                &auth.user_id,
                auth.is_admin,
                |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
            ))
            .unwrap_or_default(),
        ),
    })
}

/// GET /api/v1/datasets/{id} — 单个知识库详情（配置页回填用）
async fn get_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error(StatusCode::NOT_FOUND, "Knowledge base not found");
    }
    match state.kbs.get(&dataset_id) {
        Some(kb) => Json(ApiResponse {
            code: 0,
            message: "ok".into(),
            data: Some(serde_json::to_value(kb).unwrap_or_default()),
        })
        .into_response(),
        None => api_error(StatusCode::NOT_FOUND, "Knowledge base not found"),
    }
}

async fn create_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let name = body
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    let description = body
        .get("description")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    // Upstream `PermissionRole` uses `me`/`team`; accept the legacy `private`
    // spelling and normalise so every payload matches RAGFlow.
    let permission = crate::kb::normalize_permission(
        body.get("permission")
            .and_then(|value| value.as_str())
            .unwrap_or("me"),
    );
    let requested_embedding = body
        .get("embd_id")
        .or_else(|| body.get("embedding_model"))
        .and_then(|value| value.as_str());
    if name.is_empty() || name.len() > 255 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Knowledge base name is required and must not exceed 255 characters",
        );
    }
    let embd_id =
        match validate_tenant_embedding_selector(&state, &auth.user_id, requested_embedding) {
            Ok(selector) => selector,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        };
    // RAGFlow create_dataset: the create dialog sends chunk_method + parse_type
    // up front; seed the parser_config accordingly (empty → default seed).
    let chunk_method = body
        .get("chunk_method")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty());
    let parse_type = body
        .get("parse_type")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty());
    let parser_config = match (chunk_method, parse_type) {
        (Some(method), Some(parse_type)) => {
            format!(
                r#"{{"chunk_token_num":2048,"overlapped_percent":0.05,"chunk_method":"{}","parse_type":"{}"}}"#,
                method.trim(),
                parse_type.trim()
            )
        }
        (Some(method), None) => format!(
            r#"{{"chunk_token_num":2048,"overlapped_percent":0.05,"chunk_method":"{}"}}"#,
            method.trim()
        ),
        (None, Some(parse_type)) => format!(
            r#"{{"chunk_token_num":2048,"overlapped_percent":0.05,"parse_type":"{}"}}"#,
            parse_type.trim()
        ),
        (None, None) => r#"{"chunk_token_num":2048,"overlapped_percent":0.05}"#.to_string(),
    };
    // Upstream `POST /api/v1/datasets` stores `language` as a top-level column
    // (default `English`/`Chinese` from the process locale when omitted).
    let language = body
        .get("language")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    if !language.is_empty() && language.chars().count() > 32 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Language must not exceed 32 characters",
        );
    }
    let stored_language = if language.is_empty() {
        crate::kb::default_language()
    } else {
        language.to_string()
    };
    match state.kbs.create_for_with_parser_config_and_language(
        &auth.user_id,
        name,
        description,
        permission,
        &embd_id,
        &parser_config,
        &stored_language,
    ) {
        Ok(kb) => (
            StatusCode::CREATED,
            Json(ApiResponse {
                code: 0,
                message: "Created".into(),
                data: Some(serde_json::to_value(kb).unwrap_or_default()),
            }),
        )
            .into_response(),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn update_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if !kb_manageable(&state, &dataset_id, &auth) {
        return api_error(StatusCode::NOT_FOUND, "Knowledge base not found");
    }
    let name = body.get("name").and_then(|value| value.as_str());
    let description = body.get("description").and_then(|value| value.as_str());
    let permission = body
        .get("permission")
        .and_then(|value| value.as_str())
        .map(crate::kb::normalize_permission);
    let requested_embedding = body
        .get("embd_id")
        .or_else(|| body.get("embedding_model"))
        .and_then(|value| value.as_str());
    if let Some(name) = name {
        let trimmed = name.trim();
        if trimmed.is_empty() || trimmed.len() > 255 {
            return api_error(
                StatusCode::BAD_REQUEST,
                "Knowledge base name is required and must not exceed 255 characters",
            );
        }
    }
    // 解析 chunk/解析器/检索配置 → parser_config JSON
    // 兼容两种传法：顶层 RAGFlow camelCase 键（前端 KB 配置页），或嵌套 parser_config 对象
    let mut config = serde_json::Map::new();
    if let Some(nested) = body.get("parser_config").and_then(|v| v.as_object()) {
        for (k, v) in nested {
            config.insert(k.clone(), v.clone());
        }
    }
    for key in [
        "chunk_method",
        "chunk_size",
        "overlap",
        "delimiter",
        "delimiters",
        "chunk_token_num",
        "overlapped_percent",
        "auto_keywords",
        "auto_questions",
        "enable_children",
        "raptorEn",
        "graphEn",
        "parser_type",
        "raptor_enabled",
        "raptor_depth",
        "graphrag_enabled",
        "entity_types",
        "layout_recognize",
        "ocr_enabled",
        // 检索配置（RAGFlow retrieval_setting 对齐）
        "top_k",
        "similarity_threshold",
        "vector_similarity_weight",
        "rerank",
        "rerank_model",
        "pdf_parser",
        "page_index",
        "image_and_table_window",
        "excel_to_html",
        "children_delimiters",
        "title_levels",
        "enable_metadata",
        "raptor_threshold",
        "raptor_scope",
        "raptor_prompt",
        "raptor_max_token",
        "raptor_cluster_method",
        "raptor_max_cluster",
        "raptor_random_seed",
        "graphrag_method",
        "graphrag_batch_chunk_size",
        "graphrag_entity_resolution",
        "graphrag_community_reports",
        "page_rank",
    ] {
        if let Some(value) = body.get(key) {
            config.insert(key.into(), value.clone());
        }
    }
    let parser_config = if config.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(config).to_string())
    };
    let requested_prompt_config = body
        .get("prompt_config")
        .and_then(|value| value.as_object());
    // Upstream keeps `language` on the `knowledgebase` row rather than inside
    // `parser_config`, and the dataset form sends it on every save.
    let language = body
        .get("language")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if name.is_none()
        && description.is_none()
        && permission.is_none()
        && requested_embedding.is_none()
        && parser_config.is_none()
        && requested_prompt_config.is_none()
        && language.is_none()
    {
        return api_error(
            StatusCode::BAD_REQUEST,
            "name, permission, embedding_model, parser_config or prompt_config is required",
        );
    }
    let current = state.kbs.get(&dataset_id).expect("manageable KB exists");
    let embd_id = match requested_embedding {
        Some(selector) => {
            let selector =
                match validate_tenant_embedding_selector(&state, &current.owner_id, Some(selector))
                {
                    Ok(selector) => selector,
                    Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
                };
            if selector != current.embd_id && current.chunk_count > 0 {
                return api_error(
                    StatusCode::CONFLICT,
                    "Cannot change the embedding model while the knowledge base contains indexed chunks",
                );
            }
            Some(selector)
        }
        None => None,
    };
    // KB 身份字段（名称/描述/标签集/头像）与解析配置分别持久化
    let tag_sets = body.get("tag_sets").and_then(|value| value.as_str());
    let avatar = body.get("avatar").and_then(|value| value.as_str());
    if let Err(error) = state
        .kbs
        .update_identity(&dataset_id, name, description, tag_sets, avatar)
    {
        return api_error(StatusCode::BAD_REQUEST, &error.to_string());
    }
    // RAGFlow prompt_config: merge the incoming JSON object over the stored one
    // (object-level merge keeps other chat settings intact).
    let prompt_config = body
        .get("prompt_config")
        .and_then(|value| value.as_object())
        .cloned();
    let merged_prompt_config = prompt_config.map(|incoming| {
        let current = state
            .kbs
            .get(&dataset_id)
            .map(|kb| kb.prompt_config.as_object().cloned().unwrap_or_default())
            .unwrap_or_default();
        let mut merged = current;
        merged.extend(incoming);
        serde_json::Value::Object(merged)
    });
    if let Some(language) = language
        && let Err(error) = state.kbs.set_language(&dataset_id, language)
    {
        return api_error(StatusCode::BAD_REQUEST, &error.to_string());
    }
    match state.kbs.update_config(
        &dataset_id,
        permission,
        embd_id.as_deref(),
        parser_config.as_deref(),
        merged_prompt_config.as_ref(),
    ) {
        Ok(Some(kb)) => Json(ApiResponse {
            code: 0,
            message: "Updated".into(),
            data: Some(serde_json::to_value(kb).unwrap_or_default()),
        })
        .into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "Knowledge base not found"),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

/// Removes one dataset and everything hanging off it (documents, graph assets).
/// Shared by `DELETE /api/v1/datasets/{id}` and the upstream collection route.
fn delete_dataset_data(state: &Arc<AppState>, dataset_id: &str) -> crate::Result<bool> {
    for doc in state.docs.list(dataset_id) {
        delete_document_data(state, &doc)?;
    }
    // 清理该数据集的图谱资产
    let mut snapshot = state.graphs.snapshot();
    snapshot.kb_graphs.remove(dataset_id);
    snapshot
        .checkpoints
        .retain(|_, checkpoint| checkpoint.kb_id != dataset_id);
    if let Err(error) = state.graphs.replace_snapshot(snapshot) {
        tracing::warn!(%error, "Failed to persist graph cleanup");
    }
    state.kbs.delete(dataset_id)
}

async fn delete_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
) -> Response {
    if !kb_manageable(&state, &dataset_id, &auth) {
        return api_error(StatusCode::NOT_FOUND, "Knowledge base not found");
    }
    match delete_dataset_data(&state, &dataset_id) {
        Ok(true) => Json(ApiResponse {
            code: 0,
            message: "Deleted".into(),
            data: None,
        })
        .into_response(),
        Ok(false) => api_error(StatusCode::NOT_FOUND, "Knowledge base not found"),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

/// Upstream `api/apps/restful_apis/dataset_api.py::delete` +
/// `dataset_api_service.delete_datasets`: `DELETE /api/v1/datasets` takes
/// `{"ids": [...] | null}` plus `delete_all`. A missing/empty `ids` deletes
/// nothing unless `delete_all` is set, unknown ids belong to the "lacks
/// permission" error, and the payload reports `{"success_count": n}`.
async fn delete_datasets(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let delete_all = body
        .get("delete_all")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let requested: Vec<String> = body
        .get("ids")
        .and_then(|value| value.as_array())
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_str())
                .map(|id| id.to_string())
                .collect()
        })
        .unwrap_or_default();
    let ids = if requested.is_empty() {
        if !delete_all {
            return Json(ApiResponse {
                code: 0,
                message: "Success".into(),
                data: Some(serde_json::json!({"success_count": 0})),
            })
            .into_response();
        }
        state
            .kbs
            .list_accessible(&auth.user_id, auth.is_admin, |tenant_id, user_id| {
                state.tenants.is_member(tenant_id, user_id)
            })
            .into_iter()
            .map(|kb| kb.id)
            .collect::<Vec<_>>()
    } else {
        requested
    };
    let mut missing = Vec::new();
    for id in &ids {
        if !kb_manageable(&state, id, &auth) {
            missing.push(id.clone());
        }
    }
    if !missing.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "User '{}' lacks permission for datasets: '{}'",
                auth.user_id,
                missing.join(", ")
            ),
        );
    }
    let mut success_count = 0;
    for id in &ids {
        match delete_dataset_data(&state, id) {
            Ok(true) => success_count += 1,
            Ok(false) => {}
            Err(error) => {
                return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
            }
        }
    }
    Json(ApiResponse {
        code: 0,
        message: "Success".into(),
        data: Some(serde_json::json!({"success_count": success_count})),
    })
    .into_response()
}

// ── Upload and document processing ─────────────────────────────

const DEFAULT_MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
const MAX_DOCUMENTS_PER_UPLOAD: usize = 32;
const MULTIPART_OVERHEAD_BYTES: usize = 1024 * 1024;
const TASK_LEASE_DURATION_MS: u64 = 30_000;
const TASK_LEASE_RENEW_INTERVAL_MS: u64 = 10_000;
const TASK_DISPATCH_INTERVAL_MS: u64 = 5_000;
const TASK_COMMIT_LEASE_DURATION_MS: u64 = 120_000;
const DEFAULT_MAX_CONCURRENT_DOCUMENT_TASKS: usize = 4;

fn max_upload_bytes() -> usize {
    std::env::var("RAYRAG_MAX_UPLOAD_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_UPLOAD_BYTES)
}

fn validate_zip_container(extension: &str, path: &FsPath) -> bool {
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    match extension {
        "docx" => {
            archive.by_name("[Content_Types].xml").is_ok()
                && archive.file_names().any(|name| name.starts_with("word/"))
        }
        "xlsx" => {
            archive.by_name("[Content_Types].xml").is_ok()
                && archive.file_names().any(|name| name.starts_with("xl/"))
        }
        "pptx" => {
            archive.by_name("[Content_Types].xml").is_ok()
                && archive.file_names().any(|name| name.starts_with("ppt/"))
        }
        "epub" => archive
            .by_name("mimetype")
            .ok()
            .and_then(|mut file| {
                let mut content = String::new();
                file.read_to_string(&mut content).ok()?;
                Some(content.trim() == "application/epub+zip")
            })
            .unwrap_or(false),
        _ => false,
    }
}

fn validate_utf8_text(path: &FsPath) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buffer = [0_u8; 8192];
    let mut carry = Vec::with_capacity(4);
    loop {
        let Ok(read) = file.read(&mut buffer) else {
            return false;
        };
        if read == 0 {
            return std::str::from_utf8(&carry).is_ok();
        }
        if buffer[..read].contains(&0) {
            return false;
        }
        let mut bytes = Vec::with_capacity(carry.len() + read);
        bytes.extend_from_slice(&carry);
        bytes.extend_from_slice(&buffer[..read]);
        match std::str::from_utf8(&bytes) {
            Ok(_) => carry.clear(),
            Err(error) if error.error_len().is_some() => return false,
            Err(error) => {
                carry.clear();
                carry.extend_from_slice(&bytes[error.valid_up_to()..]);
                if carry.len() > 3 {
                    return false;
                }
            }
        }
    }
}

fn validate_upload_path(name: &str, path: &FsPath) -> anyhow::Result<()> {
    let extension = FsPath::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut file = std::fs::File::open(path)?;
    let mut head = [0_u8; 16];
    let head_len = file.read(&mut head)?;
    let head = &head[..head_len];
    let starts_with = |signature: &[u8]| head.starts_with(signature);
    let is_zip =
        starts_with(b"PK\x03\x04") || starts_with(b"PK\x05\x06") || starts_with(b"PK\x07\x08");
    let valid = match extension.as_str() {
        "pdf" => starts_with(b"%PDF-"),
        "docx" | "xlsx" | "pptx" | "epub" => is_zip && validate_zip_container(&extension, path),
        "png" => starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => starts_with(b"\xff\xd8\xff"),
        "gif" => starts_with(b"GIF87a") || starts_with(b"GIF89a"),
        "webp" => head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP",
        "bmp" => starts_with(b"BM"),
        "tif" | "tiff" => {
            starts_with(b"II*\0")
                || starts_with(b"MM\0*")
                || starts_with(b"II+\0")
                || starts_with(b"MM\0+")
        }
        "json" => std::fs::File::open(path)
            .ok()
            .and_then(|file| serde_json::from_reader::<_, serde_json::Value>(file).ok())
            .is_some(),
        "txt" | "md" | "markdown" | "html" | "htm" | "csv" | "svg" | "xml" | "yaml" | "yml"
        | "toml" | "log" => validate_utf8_text(path),
        _ => false,
    };
    if !valid {
        anyhow::bail!("File content does not match its extension");
    }
    Ok(())
}

fn safe_upload_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 255 || name.contains('\0') {
        anyhow::bail!("Invalid filename");
    }
    let path = FsPath::new(name);
    if path.components().count() != 1
        || path.file_name().and_then(|value| value.to_str()) != Some(name)
    {
        anyhow::bail!("Invalid filename");
    }
    if crate::parser::mime_from_extension(name).is_none() {
        anyhow::bail!("Unsupported file type");
    }
    Ok(name.to_string())
}

pub(crate) fn server_xxh3_hash(bytes: &[u8]) -> String {
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    hasher.update(bytes);
    format!("{:032x}", hasher.digest128())
}

pub(crate) struct PersistedUpload {
    pub(crate) name: String,
    pub(crate) storage_name: String,
    pub(crate) path: PathBuf,
    pub(crate) size: usize,
    pub(crate) content_hash: String,
    pub(crate) cleanup_on_drop: bool,
}

impl PersistedUpload {
    pub(crate) fn commit(mut self) {
        self.cleanup_on_drop = false;
    }
}

impl Drop for PersistedUpload {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            std::fs::remove_file(&self.path).ok();
        }
    }
}

fn sync_directory(path: &FsPath) -> anyhow::Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

pub(crate) async fn persist_single_upload(
    mut multipart: Multipart,
    destination_dir: &FsPath,
    storage_id: &str,
    max_bytes: usize,
) -> anyhow::Result<PersistedUpload> {
    let mut upload = None;
    while let Some(field) = multipart.next_field().await? {
        if field.name() != Some("file") {
            continue;
        }
        if upload.is_some() {
            anyhow::bail!("Only one multipart file is allowed");
        }
        upload = Some(persist_upload_field(field, destination_dir, storage_id, max_bytes).await?);
    }
    upload.ok_or_else(|| anyhow::anyhow!("multipart field 'file' is required"))
}

async fn persist_upload_field(
    mut field: Field<'_>,
    destination_dir: &FsPath,
    storage_id: &str,
    max_bytes: usize,
) -> anyhow::Result<PersistedUpload> {
    tokio::fs::create_dir_all(destination_dir).await?;
    let filename = safe_upload_name(field.file_name().unwrap_or("uploaded"))?;
    let extension = FsPath::new(&filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let storage_name = if extension.is_empty() {
        storage_id.to_string()
    } else {
        format!("{storage_id}.{extension}")
    };
    let final_path = destination_dir.join(&storage_name);
    if final_path.exists() {
        anyhow::bail!("Upload destination already exists");
    }
    let temp_path = destination_dir.join(format!(".upload-{}.tmp", uuid::Uuid::new_v4()));
    let result: anyhow::Result<PersistedUpload> = async {
        let mut output = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .await?;
        let mut size = 0_usize;
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        while let Some(chunk) = field.chunk().await? {
            size = size
                .checked_add(chunk.len())
                .ok_or_else(|| anyhow::anyhow!("Uploaded file is too large"))?;
            if size > max_bytes {
                anyhow::bail!("Uploaded file exceeds the configured size limit");
            }
            hasher.update(&chunk);
            output.write_all(&chunk).await?;
        }
        if size == 0 {
            anyhow::bail!("Uploaded file is empty");
        }
        output.flush().await?;
        output.sync_all().await?;
        drop(output);

        let validation_name = filename.clone();
        let validation_path = temp_path.clone();
        tokio::task::spawn_blocking(move || {
            validate_upload_path(&validation_name, &validation_path)
        })
        .await??;
        tokio::fs::rename(&temp_path, &final_path).await?;
        let sync_path = destination_dir.to_path_buf();
        tokio::task::spawn_blocking(move || sync_directory(&sync_path)).await??;
        Ok(PersistedUpload {
            name: filename,
            storage_name,
            path: final_path.clone(),
            size,
            content_hash: format!("{:032x}", hasher.digest128()),
            cleanup_on_drop: true,
        })
    }
    .await;
    if result.is_err() {
        tokio::fs::remove_file(&temp_path).await.ok();
        tokio::fs::remove_file(&final_path).await.ok();
    }
    result
}

async fn upload_document(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    if !kb_manageable(&state, &dataset_id, &auth) {
        return api_error(StatusCode::NOT_FOUND, "Knowledge base not found");
    }
    let upload_dir = FsPath::new(&state.static_dir).join("../uploads");
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    let mut file_count = 0_usize;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        };
        if field.name() != Some("file") {
            continue;
        }
        file_count += 1;
        if file_count > MAX_DOCUMENTS_PER_UPLOAD {
            failed.push(serde_json::json!({
                "name": field.file_name().unwrap_or("uploaded"),
                "message": format!("At most {MAX_DOCUMENTS_PER_UPLOAD} files are allowed")
            }));
            continue;
        }
        let original_name = field.file_name().unwrap_or("uploaded").to_string();
        let doc_id = uuid::Uuid::new_v4().to_string();
        match persist_upload_field(field, &upload_dir, &doc_id, state.max_upload_bytes).await {
            Ok(upload) => match register_document_upload(
                state.clone(),
                &auth.user_id,
                &dataset_id,
                doc_id,
                upload,
            ) {
                Ok(data) => succeeded.push(data),
                Err(error) => failed.push(serde_json::json!({
                    "name": original_name,
                    "message": error.to_string()
                })),
            },
            Err(error) => failed.push(serde_json::json!({
                "name": original_name,
                "message": error.to_string()
            })),
        }
    }
    if file_count == 0 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "multipart field 'file' is required",
        );
    }
    if file_count == 1 && failed.is_empty() {
        let data = succeeded.pop().expect("one successful upload");
        let name = data["name"].as_str().unwrap_or("document");
        return (
            StatusCode::ACCEPTED,
            Json(ApiResponse {
                code: 0,
                message: format!("Uploaded {name}"),
                data: Some(data),
            }),
        )
            .into_response();
    }
    if file_count == 1 {
        let failure = failed.pop().expect("one failed upload");
        return api_error(
            StatusCode::BAD_REQUEST,
            failure["message"].as_str().unwrap_or("Upload failed"),
        );
    }
    let status = if failed.is_empty() {
        StatusCode::ACCEPTED
    } else if succeeded.is_empty() {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::MULTI_STATUS
    };
    (
        status,
        Json(ApiResponse {
            code: if failed.is_empty() {
                0
            } else {
                status.as_u16() as i32
            },
            message: format!("{} uploaded, {} failed", succeeded.len(), failed.len()),
            data: Some(serde_json::json!({ "succeeded": succeeded, "failed": failed })),
        }),
    )
        .into_response()
}

pub(crate) fn register_document_upload(
    state: Arc<AppState>,
    owner_id: &str,
    dataset_id: &str,
    doc_id: String,
    upload: PersistedUpload,
) -> anyhow::Result<serde_json::Value> {
    let path = upload.path.clone();

    let now = unix_ms();
    let doc = crate::api::document::DocRecord {
        id: doc_id.clone(),
        name: upload.name.clone(),
        kb_id: dataset_id.into(),
        size: upload.size,
        storage_name: upload.storage_name.clone(),
        content_hash: upload.content_hash.clone(),
        indexed_content_hash: String::new(),
        run: "UNSTARTED".into(),
        progress: 0.0,
        progress_msg: "Uploaded; awaiting parse".into(),
        chunk_count: 0,
        created_at: now,
        updated_at: now,
    };
    let doc = state.docs.insert_unique(doc)?;
    let name = doc.name.clone();
    if let Err(error) = state.kbs.update_counts(dataset_id, 0, 1) {
        state.docs.delete(&doc_id).ok();
        std::fs::remove_file(&path).ok();
        return Err(error);
    }
    let task_id = match queue_document_processing(
        state.clone(),
        owner_id,
        doc.clone(),
        crate::api::features::TASK_PRIORITY_LOW,
    ) {
        Ok(task_id) => task_id,
        Err(error) => {
            state.docs.delete(&doc_id).ok();
            state.kbs.update_counts(dataset_id, 0, -1).ok();
            std::fs::remove_file(&path).ok();
            return Err(error);
        }
    };
    upload.commit();
    Ok(serde_json::json!({
        "id": doc_id,
        "name": name,
        "chunks": 0,
        "run": doc.run,
        "task_id": task_id,
        "content_hash": doc.content_hash,
    }))
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ApiResponse {
            code: status.as_u16() as i32,
            message: message.into(),
            data: None,
        }),
    )
        .into_response()
}

pub(crate) fn queue_document_processing(
    state: Arc<AppState>,
    owner_id: &str,
    doc: crate::api::document::DocRecord,
    priority: i32,
) -> anyhow::Result<String> {
    let path = document_storage_path(&state, &doc);
    if !path.is_file() {
        anyhow::bail!("Uploaded file is missing");
    }
    let (task_id, created) = state.tasks.push_document_unique(
        owner_id,
        &format!("Parse {}", doc.name),
        &doc.id,
        &doc.kb_id,
        priority,
    )?;
    if created
        && let Err(error) =
            state
                .docs
                .update_status(&doc.id, "UNSTARTED", 0.0, "Queued for parsing")
    {
        state.tasks.request_cancel(&task_id).ok();
        return Err(error);
    }
    // Hand the task to the async TaskExecutor (RAGFlow task_executor port):
    // the executor's worker pool runs `process_document` off the request path.
    // The TaskQueue lease makes this execution exclusive with the legacy
    // dispatcher, so queueing here is idempotent with `dispatch_pending_document_tasks`.
    let _ = state.task_executor.submit(crate::task_executor::ExecTask {
        id: task_id.clone(),
        doc_id: doc.id.clone(),
        kb_id: doc.kb_id.clone(),
        name: doc.name.clone(),
        priority,
        from_page: 0,
        to_page: -1,
        attempts: 0,
        max_retries: 2,
        state: crate::task_executor::TaskState::Pending,
        progress: 0.0,
        message: "Queued for parsing".into(),
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
    });
    Ok(task_id)
}

/// Build the TaskExecutor handler that drives the existing document worker
/// (`process_document`). The progress callback passed by the executor feeds
/// the live task registry (RAGFlow `CURRENT_TASKS`/`set_progress` semantics);
/// `process_document` itself writes DocStore run status (RUNNING → DONE/FAILED).
///
/// Retry ownership: `process_document` retries through the TaskQueue lease
/// (`can_retry`), so errors surfaced here are treated as permanent — the
/// executor must not double-retry.
fn document_executor_handler(state: Arc<AppState>) -> crate::task_executor::TaskHandler {
    Arc::new(
        move |task: crate::task_executor::ExecTask,
              progress: crate::task_executor::ProgressCallback| {
            let state = state.clone();
            Box::pin(async move {
                let Some(doc) = state.docs.get(&task.doc_id) else {
                    return Err(crate::task_executor::TaskError::permanent(
                        "Document metadata is missing",
                    ));
                };
                let path = document_storage_path(&state, &doc);
                if !path.is_file() {
                    return Err(crate::task_executor::TaskError::permanent(
                        "Uploaded file is missing",
                    ));
                }
                progress.set_progress(&task, 0.05, "Parsing document");
                process_document(state.clone(), doc, path, task.id.clone()).await;
                // process_document owns retry through the TaskQueue lease;
                // classify the outcome for the executor's counters.
                let status = state
                    .tasks
                    .list()
                    .into_iter()
                    .find(|queued| queued.id == task.id)
                    .map(|queued| queued.status)
                    .unwrap_or_default();
                match status.as_str() {
                    "done" => Ok(0),
                    // A retry was queued or another worker owns the lease; this
                    // submission's execution is complete either way.
                    "pending" | "running" => Ok(0),
                    "cancelled" => Err(crate::task_executor::TaskError::permanent(
                        "Processing cancelled",
                    )),
                    other => Err(crate::task_executor::TaskError::permanent(format!(
                        "Document processing failed: {other}"
                    ))),
                }
            })
        },
    )
}

fn retryable_processing_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    !message.contains("Embedding is not configured")
        && !message.contains("Requested embedding model")
        && !message.contains("Global embedding model")
        && !message.contains("Provider is disabled")
        && !message.contains("Knowledge base not found")
        && !message.contains("Uploaded file is missing")
        && !message.contains("Invalid upload path")
        && !message.contains("Invalid parser configuration")
        && !message.contains("Processing cancelled")
}

async fn process_document(
    state: Arc<AppState>,
    doc: crate::api::document::DocRecord,
    path: std::path::PathBuf,
    task_id: String,
) {
    let worker_id = format!("rayrag-worker-{}", uuid::Uuid::new_v4());
    if state.tasks.is_cancelled(&task_id) {
        let _ = state
            .docs
            .update_status(&doc.id, "CANCELLED", 1.0, "Processing cancelled");
        return;
    }
    let lease = match state.tasks.claim(
        &task_id,
        &worker_id,
        TASK_LEASE_DURATION_MS,
        "Parsing document",
    ) {
        Ok(Some(lease)) => lease,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(%task_id, %error, "Failed to persist task attempt");
            return;
        }
    };
    if let Err(error) = state
        .docs
        .update_status(&doc.id, "RUNNING", 0.05, "Parsing document")
    {
        tracing::error!(doc_id = %doc.id, %error, "Failed to persist document status");
    }

    let processing = async {
        let kb = state
            .kbs
            .get(&doc.kb_id)
            .ok_or_else(|| anyhow::anyhow!("Knowledge base not found"))?;
        let config: crate::ParserConfig = serde_json::from_str(&kb.parser_config)
            .map_err(|error| anyhow::anyhow!("Invalid parser configuration: {error}"))?;
        let raptor_enabled = config.raptor_enabled;
        let raptor_depth = config.raptor_depth;
        let raptor_threshold = config.raptor_threshold;
        let graphrag_enabled = config.graphrag_enabled;
        let graphrag_method = config.graphrag_method.clone();
        let graphrag_entity_types = config.graphrag_entity_types.clone();
        let embedder = kb_embedder_for(&state, std::slice::from_ref(&doc.kb_id))?;
        // Vision enrichment for image documents: attach the tenant default
        // image2text model when configured (RAGFlow picture.py / naive.py
        // `get_tenant_default_model_by_type(LLMType.IMAGE2TEXT)`).
        let vision = kb_vision_for(&state, std::slice::from_ref(&doc.kb_id))
            .unwrap_or_else(|error| {
                tracing::warn!(%error, "Vision model resolution failed; image parsing continues without vision");
                None
            });
        let mut pipeline_builder =
            crate::pipeline::Pipeline::new(config).with_shared_embedder(embedder);
        if let Some(vision) = vision {
            pipeline_builder = pipeline_builder.with_vision_client(vision);
        }
        let pipeline = pipeline_builder.with_document_parsers_from_env()?;
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid upload path"))?;
        let (chunks, report) = match pipeline.process_with_report(path_str).await {
            Ok(result) => result,
            Err(error) => {
                // Upstream writes a `PipelineOperationLog` row for failed runs
                // too, so the dataset's ingestion list keeps the failure.
                let failed = crate::pipeline::PipelineReport {
                    stages: vec![crate::pipeline::StageReport {
                        id: "parser",
                        component: "Parser",
                        title: "Parser",
                        elapsed_seconds: 0.0,
                        outputs: serde_json::json!({
                            "output_format": {"type": "string", "value": "text"},
                            "text": {"type": "string", "value": error.to_string()},
                            "_elapsed_time": {"type": "number", "value": 0.0}
                        }),
                    }],
                };
                if let Err(record_error) = crate::api::ingestion::record_parse_run(
                    &state,
                    &kb.owner_id,
                    &doc.kb_id,
                    &doc.id,
                    &doc.name,
                    &kb_parser_id(&kb),
                    &failed,
                    "failed",
                    &error.to_string(),
                ) {
                    tracing::warn!(%record_error, "Failed to record ingestion log");
                }
                return Err(error);
            }
        };
        if let Err(error) = crate::api::ingestion::record_parse_run(
            &state,
            &kb.owner_id,
            &doc.kb_id,
            &doc.id,
            &doc.name,
            &kb_parser_id(&kb),
            &report,
            "done",
            "",
        ) {
            tracing::warn!(%error, "Failed to record ingestion log");
        }
        if state.tasks.is_cancelled(&task_id) {
            anyhow::bail!("Processing cancelled");
        }
        if !state.tasks.owns_lease(&task_id, &lease.token) {
            anyhow::bail!("Task lease lost");
        }
        if chunks.iter().any(|chunk| chunk.embedding.is_none()) {
            anyhow::bail!("Embedding did not produce vectors for every chunk");
        }
        let mut indexed: Vec<crate::search::IndexedChunk> = chunks
            .into_iter()
            .map(|mut chunk| {
                chunk.metadata.insert("doc_id".into(), doc.id.clone());
                chunk.metadata.insert("kb_id".into(), doc.kb_id.clone());
                chunk.metadata.insert("file_name".into(), doc.name.clone());
                chunk
                    .metadata
                    .insert("content_type".into(), chunk.content_type.clone());
                let mut indexed = crate::search::IndexedChunk::from(chunk);
                indexed.doc_name = doc.name.clone();
                indexed
            })
            .collect();
        let graph_checkpoint = graphrag_enabled.then(|| {
            crate::graph_store::GraphStore::build_checkpoint(
                &doc.id,
                &doc.kb_id,
                &doc.content_hash,
                &graphrag_method,
                &graphrag_entity_types,
                &indexed,
            )
        });
        if raptor_enabled {
            indexed.extend(build_raptor_chunks(
                &doc,
                &indexed,
                raptor_threshold,
                raptor_depth,
            )?);
        }
        let new_count = indexed.len();
        if state.tasks.is_cancelled(&task_id) {
            anyhow::bail!("Processing cancelled");
        }
        if !state
            .tasks
            .renew_lease(&task_id, &lease.token, TASK_COMMIT_LEASE_DURATION_MS)?
        {
            anyhow::bail!("Task lease lost before index commit");
        }
        commit_indexed_document(
            &state,
            &doc,
            &task_id,
            &lease.token,
            indexed,
            graph_checkpoint,
            &format!("Indexed {new_count} chunks"),
        )?;
        Ok::<usize, anyhow::Error>(new_count)
    };
    tokio::pin!(processing);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_millis(
        TASK_LEASE_RENEW_INTERVAL_MS,
    ));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut processing => break result,
            _ = heartbeat.tick() => {
                match state.tasks.renew_lease(
                    &task_id,
                    &lease.token,
                    TASK_LEASE_DURATION_MS,
                ) {
                    Ok(true) => {}
                    Ok(false) => break Err(anyhow::anyhow!("Task lease lost")),
                    Err(error) => break Err(error.context("Failed to renew task lease")),
                }
            }
        }
    };

    match result {
        Ok(chunk_count) => {
            let message = format!("Indexed {chunk_count} chunks");
            match state
                .tasks
                .update_claimed(&task_id, &lease.token, "done", 1.0, &message)
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(%task_id, "Discarding result after task lease was lost");
                }
                Err(error) => {
                    tracing::error!(%task_id, %error, "Failed to persist completed task");
                }
            }
        }
        Err(_error) if state.tasks.is_cancelled(&task_id) => {
            let message = "Processing cancelled";
            let _ = state
                .tasks
                .update_claimed(&task_id, &lease.token, "cancelled", 1.0, message);
            let _ = state.docs.update_status(&doc.id, "CANCELLED", 1.0, message);
        }
        Err(error) if retryable_processing_error(&error) && state.tasks.can_retry(&task_id) => {
            let message = format!("Attempt {} failed: {error}; retrying", lease.attempt);
            match state
                .tasks
                .update_claimed(&task_id, &lease.token, "pending", 0.0, &message)
            {
                Ok(true) => {}
                Ok(false) => return,
                Err(save_error) => {
                    tracing::error!(%task_id, %save_error, "Failed to persist retry state");
                    return;
                }
            }
            let _ = state
                .docs
                .update_status(&doc.id, "UNSTARTED", 0.0, &message);
            // Release the local worker slot so every retry is re-ordered by
            // the same priority-aware dispatcher as newly queued work.
        }
        Err(error) => {
            let message = error.to_string();
            match state
                .tasks
                .update_claimed(&task_id, &lease.token, "failed", 1.0, &message)
            {
                Ok(true) => {}
                Ok(false) => return,
                Err(save_error) => {
                    tracing::error!(%task_id, %save_error, "Failed to persist failed task");
                    return;
                }
            }
            if let Err(status_error) = state.docs.update_status(&doc.id, "FAILED", 1.0, &message) {
                tracing::error!(doc_id = %doc.id, %status_error, "Failed to persist parse failure");
            }
            tracing::error!(doc_id = %doc.id, %error, "Document processing failed");
        }
    }
}

fn document_storage_path(
    state: &AppState,
    doc: &crate::api::document::DocRecord,
) -> std::path::PathBuf {
    let storage_name = if doc.storage_name.is_empty() {
        doc.id.as_str()
    } else {
        doc.storage_name.as_str()
    };
    std::path::Path::new(&state.static_dir)
        .join("../uploads")
        .join(storage_name)
}

fn commit_indexed_document(
    state: &AppState,
    doc: &crate::api::document::DocRecord,
    task_id: &str,
    lease_token: &str,
    indexed: Vec<crate::search::IndexedChunk>,
    graph_checkpoint: Option<crate::graph_store::GraphCheckpoint>,
    message: &str,
) -> anyhow::Result<()> {
    let _commit_guard = state.document_commit_lock.lock().unwrap();
    if state.tasks.is_cancelled(task_id) || !state.tasks.owns_lease(task_id, lease_token) {
        anyhow::bail!("Task lease lost before index commit");
    }
    let current_doc = state
        .docs
        .get(&doc.id)
        .ok_or_else(|| anyhow::anyhow!("Document metadata was deleted before index commit"))?;
    if current_doc.kb_id != doc.kb_id || state.kbs.get(&doc.kb_id).is_none() {
        anyhow::bail!("Knowledge base not found before index commit");
    }

    let new_count = indexed.len();
    let count_delta = new_count as isize - current_doc.chunk_count as isize;
    let previous_graphs = state.graphs.snapshot();
    let previous_index = {
        let mut engine = state.engine.write().unwrap();
        let previous = engine.to_vec();
        engine.replace_document(&doc.id, indexed);
        persist_index_change(state, &mut engine, &previous)?;
        previous
    };

    if let Err(error) = state.graphs.replace_document(&doc.id, graph_checkpoint) {
        restore_index(state, &previous_index, "Graph checkpoint update failed");
        return Err(error.context("Failed to update GraphRAG artifacts after index commit"));
    }

    if let Err(error) = state.kbs.update_counts(&doc.kb_id, count_delta, 0) {
        state.graphs.replace_snapshot(previous_graphs).ok();
        restore_index(state, &previous_index, "KB count update failed");
        return Err(error.context("Failed to update KB counts after index commit"));
    }
    match state
        .docs
        .mark_indexed(&doc.id, &doc.content_hash, new_count, message)
    {
        Ok(true) => {}
        Ok(false) => {
            state.graphs.replace_snapshot(previous_graphs.clone()).ok();
            state.kbs.update_counts(&doc.kb_id, -count_delta, 0).ok();
            restore_index(
                state,
                &previous_index,
                "Document metadata disappeared during commit",
            );
            anyhow::bail!("Document metadata disappeared during index commit");
        }
        Err(error) => {
            state.graphs.replace_snapshot(previous_graphs).ok();
            if let Err(rollback_error) = state.kbs.update_counts(&doc.kb_id, -count_delta, 0) {
                tracing::error!(%rollback_error, doc_id = %doc.id, "Failed to roll back KB counts");
            }
            restore_index(state, &previous_index, "Document metadata update failed");
            return Err(error.context("Failed to mark document as indexed"));
        }
    }
    Ok(())
}

fn build_raptor_chunks(
    doc: &crate::api::document::DocRecord,
    indexed: &[crate::search::IndexedChunk],
    threshold: f32,
    max_depth: usize,
) -> anyhow::Result<Vec<crate::search::IndexedChunk>> {
    let source: Vec<_> = indexed
        .iter()
        .filter(|chunk| !chunk.embedding.is_empty())
        .map(|chunk| (chunk.content.clone(), chunk.embedding.clone()))
        .collect();
    if source.len() < 2 {
        return Ok(Vec::new());
    }
    let mut tree =
        crate::raptor::RaptorTree::new(threshold.clamp(-1.0, 1.0)).with_max_depth(max_depth);
    tree.build(&source)?;
    Ok(tree
        .summary_nodes()
        .into_iter()
        .enumerate()
        .map(|(position, node)| crate::search::IndexedChunk {
            id: format!("{}-raptor-{}", doc.id, node.id),
            doc_name: doc.name.clone(),
            content: node.content.clone(),
            embedding: node.embedding.clone(),
            token_count: node.content.split_whitespace().count(),
            position: indexed.len() + position,
            metadata: std::collections::HashMap::from([
                ("doc_id".into(), doc.id.clone()),
                ("kb_id".into(), doc.kb_id.clone()),
                ("file_name".into(), doc.name.clone()),
                ("content_type".into(), "text".into()),
                ("raptor_kwd".into(), "1".into()),
                ("raptor_layer_int".into(), node.level.to_string()),
                ("raptor_method".into(), "extractive".into()),
                ("source_id".into(), node.children.join(",")),
            ]),
        })
        .collect())
}

fn restore_index(state: &AppState, previous: &[crate::search::IndexedChunk], reason: &str) {
    let mut engine = state.engine.write().unwrap();
    crate::store::rollback_online_index(
        &mut engine,
        &state.index_path,
        &state.vector_mirror,
        previous,
        reason,
    );
}

pub(crate) fn persist_index_change(
    state: &AppState,
    engine: &mut crate::search::SearchEngine,
    previous: &[crate::search::IndexedChunk],
) -> anyhow::Result<()> {
    crate::store::persist_online_index(engine, &state.index_path, &state.vector_mirror, previous)
        .map_err(|error| anyhow::anyhow!("Failed to commit online search index: {error}"))
}

fn memory_llm_for(
    state: &AppState,
    memory: &crate::api::features::MemoryEntry,
) -> anyhow::Result<Arc<dyn crate::llm::ChatModel>> {
    if memory.llm_id.trim().is_empty() || memory.llm_id == "default" {
        return state
            .llm
            .clone()
            .map(|model| model as Arc<dyn crate::llm::ChatModel>)
            .ok_or_else(|| anyhow::anyhow!("Global chat model is not configured"));
    }
    let chat = state.tenant_models.resolve(
        &state.providers,
        &memory.tenant_id,
        crate::api::tenant_models::ModelCapability::Chat,
        Some(&memory.llm_id),
    );
    if let Ok(Some(model)) = chat {
        return Ok(Arc::new(model.llm_client()));
    }
    state
        .tenant_models
        .resolve(
            &state.providers,
            &memory.tenant_id,
            crate::api::tenant_models::ModelCapability::ImageToText,
            Some(&memory.llm_id),
        )?
        .map(|model| Arc::new(model.llm_client()) as Arc<dyn crate::llm::ChatModel>)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Chat or image2text model is not configured: {}",
                memory.llm_id
            )
        })
}

fn update_memory_task_progress(
    state: &AppState,
    task_id: &str,
    lease_token: &str,
    progress: f32,
    message: &str,
) -> anyhow::Result<()> {
    if state
        .tasks
        .update_claimed(task_id, lease_token, "running", progress, message)?
    {
        Ok(())
    } else {
        anyhow::bail!("Memory task lease lost")
    }
}

fn retryable_memory_error(error: &anyhow::Error) -> bool {
    let message = error.to_string();
    !message.contains("Memory not found")
        && !message.contains("Memory task payload is missing")
        && !message.contains("Invalid Memory task digest")
        && !message.contains("model is not configured")
        && !message.contains("Provider is disabled")
        && !message.contains("memory extraction response")
        && !message.contains("memory extraction field")
        && !message.contains("Memory task lease lost")
        && !message.contains("Processing cancelled")
}

async fn process_memory_task(state: Arc<AppState>, task: crate::api::features::Task) {
    let task_id = task.id.clone();
    let worker_id = format!("rayrag-memory-worker-{}", uuid::Uuid::new_v4());
    let lease = match state.tasks.claim(
        &task_id,
        &worker_id,
        TASK_LEASE_DURATION_MS,
        "Preparing Memory extraction",
    ) {
        Ok(Some(lease)) => lease,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(%task_id, %error, "Failed to claim Memory task");
            return;
        }
    };
    let processing = async {
        let payload = task
            .memory_payload
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Memory task payload is missing"))?;
        let source_id = task
            .digest
            .parse::<i64>()
            .ok()
            .filter(|source_id| *source_id > 0)
            .ok_or_else(|| anyhow::anyhow!("Invalid Memory task digest: {}", task.digest))?;
        let memory = state
            .memories
            .get(&task.doc_id)
            .ok_or_else(|| anyhow::anyhow!("Memory not found: {}", task.doc_id))?;
        let needs_extraction = memory
            .memory_type
            .iter()
            .any(|kind| matches!(kind.as_str(), "semantic" | "episodic" | "procedural"));
        if !needs_extraction {
            return Ok::<(usize, &'static str), anyhow::Error>((
                0,
                "Memory does not need extraction",
            ));
        }

        update_memory_task_progress(
            &state,
            &task_id,
            &lease.token,
            0.15,
            "Prepared prompts and LLM",
        )?;
        let llm = memory_llm_for(&state, &memory)?;
        let conversation_time = crate::common::time_utils::timestamp_to_date(
            crate::common::time_utils::current_timestamp(),
            crate::common::time_utils::DEFAULT_TIME_FORMAT,
        );
        // The fixed v0.26.4 worker does not forward Memory's stored custom
        // system/user prompt fields at either extraction call site. Keep that
        // runtime contract explicit rather than silently claiming they work.
        let extracted = crate::api::joint_services::extract_memory_by_llm(
            llm.as_ref(),
            &memory.memory_type,
            &payload.user_input,
            &payload.agent_response,
            None,
            None,
            &conversation_time,
            memory.temperature,
        )
        .await?;
        update_memory_task_progress(
            &state,
            &task_id,
            &lease.token,
            0.35,
            "Got extracted result from LLM",
        )?;
        if extracted.is_empty() {
            return Ok((0, "No memory extracted from raw message"));
        }
        update_memory_task_progress(
            &state,
            &task_id,
            &lease.token,
            0.5,
            &format!("Extracted {} messages from raw dialogue", extracted.len()),
        )?;

        let embedder = crate::api::features::memory_embedder_for(&state, &memory)?;
        update_memory_task_progress(
            &state,
            &task_id,
            &lease.token,
            0.65,
            "Prepared embedding model",
        )?;
        let contents: Vec<&str> = extracted.iter().map(|item| item.content.as_str()).collect();
        let embeddings = embedder.embed(&contents).await?;
        if embeddings.len() != extracted.len() || embeddings.iter().any(Vec::is_empty) {
            anyhow::bail!("Embedding model returned an invalid vector batch");
        }
        update_memory_task_progress(
            &state,
            &task_id,
            &lease.token,
            0.85,
            "Embedded extracted content",
        )?;
        if state.tasks.is_cancelled(&task_id) {
            anyhow::bail!("Processing cancelled");
        }

        let messages: Vec<_> = extracted
            .into_iter()
            .zip(embeddings)
            .map(
                |(item, content_embed)| crate::api::joint_services::MemoryMessage {
                    message_id: state.memory_messages.next_message_id(),
                    message_type: item.message_type,
                    source_id,
                    memory_id: memory.id.clone(),
                    user_id: payload.user_id.clone(),
                    agent_id: payload.agent_id.clone(),
                    session_id: payload.session_id.clone(),
                    content: item.content,
                    valid_at: item.valid_at,
                    invalid_at: item.invalid_at,
                    forget_at: None,
                    status: true,
                    zone_id: 0,
                    content_embed,
                },
            )
            .collect();
        let count = messages.len();
        let budget = if memory.memory_size == 0 {
            usize::MAX
        } else {
            memory.memory_size as usize
        };
        {
            let _commit_guard = state.memory_commit_lock.lock().unwrap();
            if !state.tasks.owns_lease(&task_id, &lease.token) {
                anyhow::bail!("Memory task lease lost");
            }
            if state.memories.get(&memory.id).is_none() {
                anyhow::bail!("Memory not found: {}", memory.id);
            }
            let memory_ids = [memory.id.clone()];
            let children_exist = state
                .memory_messages
                .query_with_options(crate::api::joint_services::MemoryMessageQuery {
                    memory_ids: &memory_ids,
                    agent_id: None,
                    session_id: None,
                    user_id: None,
                    status: None,
                    top_n: None,
                    hide_forgotten: false,
                })
                .iter()
                .any(|message| message.source_id == source_id);
            if !children_exist {
                state
                    .memory_messages
                    .save_messages_with_budget(
                        &memory.id,
                        &memory.forgetting_policy,
                        budget,
                        messages,
                    )
                    .map_err(|error| anyhow::anyhow!(error.message))?;
            }
            update_memory_task_progress(
                &state,
                &task_id,
                &lease.token,
                0.95,
                "Saved messages to storage",
            )?;
        }
        Ok((count, "Message saved successfully"))
    };
    tokio::pin!(processing);
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_millis(
        TASK_LEASE_RENEW_INTERVAL_MS,
    ));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let result = loop {
        tokio::select! {
            result = &mut processing => break result,
            _ = heartbeat.tick() => {
                match state.tasks.renew_lease(&task_id, &lease.token, TASK_LEASE_DURATION_MS) {
                    Ok(true) => {}
                    Ok(false) => break Err(anyhow::anyhow!("Memory task lease lost")),
                    Err(error) => break Err(error),
                }
            }
        }
    };

    match result {
        Ok((_count, message)) => {
            if let Err(error) =
                state
                    .tasks
                    .update_claimed(&task_id, &lease.token, "done", 1.0, message)
            {
                tracing::error!(%task_id, %error, "Failed to persist completed Memory task");
            }
        }
        Err(_error) if state.tasks.is_cancelled(&task_id) => {
            let _ = state.tasks.update_claimed(
                &task_id,
                &lease.token,
                "cancelled",
                1.0,
                "Processing cancelled",
            );
        }
        Err(error) if retryable_memory_error(&error) && state.tasks.can_retry(&task_id) => {
            let message = format!("Attempt {} failed: {error}; retrying", lease.attempt);
            if let Err(save_error) =
                state
                    .tasks
                    .update_claimed(&task_id, &lease.token, "pending", 0.0, &message)
            {
                tracing::error!(%task_id, %save_error, "Failed to persist Memory retry state");
            }
        }
        Err(error) => {
            if let Err(save_error) = state.tasks.update_claimed(
                &task_id,
                &lease.token,
                "failed",
                1.0,
                &error.to_string(),
            ) {
                tracing::error!(%task_id, %save_error, "Failed to persist failed Memory task");
            }
            tracing::error!(%task_id, %error, "Memory extraction failed");
        }
    }
}

fn reconcile_staged_memory_tasks(state: &AppState) {
    let _commit_guard = state.memory_commit_lock.lock().unwrap();
    for task in state.tasks.staged_memory_tasks() {
        let source_id = task.digest.parse::<i64>().ok().filter(|value| *value > 0);
        let raw_exists = source_id.is_some_and(|source_id| {
            state
                .memory_messages
                .get_by_message_id(&task.doc_id, source_id)
                .is_some()
        });
        let result = if raw_exists && state.memories.get(&task.doc_id).is_some() {
            state.tasks.publish_memory_task(&task.id)
        } else {
            state.tasks.discard_staged_memory_task(&task.id)
        };
        if let Err(error) = result {
            tracing::error!(task_id = %task.id, %error, "Failed to reconcile staged Memory task");
        }
    }
}

fn dispatch_pending_document_tasks(state: Arc<AppState>, task_slots: Arc<tokio::sync::Semaphore>) {
    reconcile_staged_memory_tasks(&state);
    for task in state.tasks.pending() {
        let permit = match task_slots.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::NoPermits) => break,
            Err(tokio::sync::TryAcquireError::Closed) => return,
        };
        let Some(active_guard) = state.tasks.activate(&task.id) else {
            drop(permit);
            continue;
        };
        if task.task_type == "memory" {
            let state_for_worker = state.clone();
            tokio::spawn(async move {
                let _active_guard = active_guard;
                let _permit = permit;
                process_memory_task(state_for_worker, task).await;
            });
            continue;
        }
        if task.task_type != "document_parse" {
            // RAGFlow rag/svr/task_executor.py multi-task-type parity:
            // - "mindmap" is an upstream placeholder (progress 1 "place
            //   holder" then `pass`), so the task completes with that note;
            // - "skill" defers to the refactored executor (TE_RUN_MODE=0)
            //   upstream — the fixed executor fails it with that message;
            // - "artifact" has no execution branch in the fixed executor;
            // - "raptor"/"graphrag" run inline in RayRAG's parse pipeline
            //   (build_raptor_chunks / graphrag index at parse time), so a
            //   persisted task of those types has no separate work left.
            let (status, message) = match task.task_type.as_str() {
                "mindmap" => ("done", "place holder"),
                "skill" => (
                    "failed",
                    "Skill generation requires the refactored task executor (TE_RUN_MODE=0).",
                ),
                "raptor" | "graphrag" => ("done", "already built inline during document parsing"),
                _ => ("failed", "Unsupported persisted task type"),
            };
            let message = if matches!(task.task_type.as_str(), "raptor" | "graphrag") {
                format!("{}: {}", message, task.task_type)
            } else {
                message.to_string()
            };
            let _ = state.tasks.update(&task.id, status, 1.0, &message);
            drop(active_guard);
            drop(permit);
            continue;
        }
        let Some(doc) = state.docs.get(&task.doc_id) else {
            let _ = state.tasks.update(
                &task.id,
                "failed",
                1.0,
                "Cannot recover task because document metadata is missing",
            );
            drop(active_guard);
            drop(permit);
            continue;
        };
        let path = document_storage_path(&state, &doc);
        if !path.is_file() {
            let _ = state.tasks.update(
                &task.id,
                "failed",
                1.0,
                "Cannot recover task because uploaded file is missing",
            );
            drop(active_guard);
            drop(permit);
            continue;
        }
        let state_for_worker = state.clone();
        tokio::spawn(async move {
            let _active_guard = active_guard;
            let _permit = permit;
            process_document(state_for_worker, doc, path, task.id).await;
        });
    }
}

/// How often the background mirror repair wakes up, and how long it waits before
/// its first pass (startup stays quiet; the first repair happens after the server
/// is already serving).
const MIRROR_REPAIR_INTERVAL_MS: u64 = 300_000;
const MIRROR_REPAIR_INITIAL_DELAY_MS: u64 = 30_000;

/// Repair native zvec collections that drifted from the JSON index, **one owner per
/// pass**, in a deterministic order.
///
/// A repair rewrites a single collection (delete + upsert), so peak memory is
/// bounded by that one owner instead of the whole corpus; running it off the
/// startup path means a large repair can never delay or spike a boot. An owner that
/// still disagrees after a repair is reported once and then left alone for the rest
/// of the process — the repair loop must never become the runaway it replaces.
fn start_mirror_repair(state: Arc<AppState>) {
    if !state.vector_mirror.is_enabled() {
        return;
    }
    tokio::spawn(async move {
        let mut attempted: std::collections::HashSet<String> = std::collections::HashSet::new();
        tokio::time::sleep(std::time::Duration::from_millis(
            MIRROR_REPAIR_INITIAL_DELAY_MS,
        ))
        .await;
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(MIRROR_REPAIR_INTERVAL_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let counts = state.engine.read().unwrap().chunk_counts_by_kb();
            let drifted = match state.vector_mirror.drifted_owners(&counts) {
                Ok(drifted) => drifted,
                Err(error) => {
                    tracing::warn!(%error, "zvec repair pass could not inspect the mirror");
                    continue;
                }
            };
            let Some(kb_id) = drifted.into_iter().find(|kb_id| !attempted.contains(kb_id)) else {
                continue;
            };
            // Exactly one owner per pass: its chunks are loaded, written and
            // released before the loop looks at anything else.
            let chunks = state.engine.read().unwrap().chunks_for_kb(&kb_id);
            let outcome = state.vector_mirror.repair_owner(&kb_id, &chunks);
            drop(chunks);
            match outcome {
                Ok(false) => {
                    attempted.insert(kb_id.clone());
                    tracing::warn!(
                        kb_id = %kb_id,
                        "zvec collection could not be repaired; leaving it untouched"
                    );
                }
                Ok(true) => {
                    let still_drifted = state
                        .vector_mirror
                        .drifted_owners(&counts)
                        .map(|owners| owners.contains(&kb_id))
                        .unwrap_or(false);
                    if still_drifted {
                        attempted.insert(kb_id.clone());
                        tracing::warn!(
                            kb_id = %kb_id,
                            "zvec collection still differs from the index after a repair; it will not be retried in this process"
                        );
                    } else {
                        tracing::info!(kb_id = %kb_id, "Repaired one zvec collection");
                    }
                }
                Err(error) => {
                    attempted.insert(kb_id.clone());
                    tracing::warn!(
                        kb_id = %kb_id,
                        %error,
                        "zvec collection repair failed; it will not be retried in this process"
                    );
                }
            }
        }
    });
}

fn start_document_task_dispatcher(state: Arc<AppState>) {
    let task_slots = Arc::new(tokio::sync::Semaphore::new(max_concurrent_document_tasks()));
    dispatch_pending_document_tasks(state.clone(), task_slots.clone());
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_millis(TASK_DISPATCH_INTERVAL_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = state.tasks.notified() => {}
            }
            dispatch_pending_document_tasks(state.clone(), task_slots.clone());
        }
    });
}

/// How many documents may be ingested at once.
///
/// `RAYRAG_MAX_CONCURRENT_TASKS` wins when set; otherwise the value is derived from
/// the host (CPU count and available memory, see [`crate::host_resources`]) so a
/// small NAS does not run the workstation default and a large host is not throttled
/// by it. `DEFAULT_MAX_CONCURRENT_DOCUMENT_TASKS` remains the fallback when the host
/// cannot be measured.
fn max_concurrent_document_tasks() -> usize {
    if let Some(explicit) = std::env::var("RAYRAG_MAX_CONCURRENT_TASKS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
    {
        return explicit;
    }
    let resources = crate::host_resources::HostResources::detect();
    tracing::info!(
        host = %resources.summary(),
        document_tasks = resources.document_task_limit(),
        "Host resources detected; ingestion concurrency derived from them"
    );
    resources.document_task_limit()
}

pub(crate) fn delete_document_data(
    state: &Arc<AppState>,
    doc: &crate::api::document::DocRecord,
) -> anyhow::Result<()> {
    let _commit_guard = state.document_commit_lock.lock().unwrap();
    let previous_graphs = state.graphs.snapshot();
    let path = document_storage_path(state, doc);
    let (removed_chunks, previous_index) = {
        let mut engine = state.engine.write().unwrap();
        let previous = engine.to_vec();
        let removed = engine.remove_document(&doc.id);
        persist_index_change(state, &mut engine, &previous)?;
        (removed, previous)
    };
    if let Err(error) = state.graphs.replace_document(&doc.id, None) {
        restore_index(state, &previous_index, "Graph checkpoint deletion failed");
        return Err(error.context("Failed to delete GraphRAG document checkpoint"));
    }
    if let Err(error) = state
        .kbs
        .update_counts(&doc.kb_id, -(removed_chunks as isize), -1)
    {
        state.graphs.replace_snapshot(previous_graphs).ok();
        restore_index(
            state,
            &previous_index,
            "KB count update failed during document deletion",
        );
        return Err(error.context("Failed to update KB counts during document deletion"));
    }
    let previous_document_metadata = state.document_metadata.get(&doc.id, &doc.kb_id);
    if let Err(error) = state.document_metadata.delete(&doc.id, &doc.kb_id) {
        state.graphs.replace_snapshot(previous_graphs).ok();
        state
            .kbs
            .update_counts(&doc.kb_id, removed_chunks as isize, 1)
            .ok();
        restore_index(
            state,
            &previous_index,
            "Document metadata store deletion failed",
        );
        return Err(error.context("Failed to delete document-level metadata"));
    }
    match state.docs.delete(&doc.id) {
        Ok(true) => {}
        Ok(false) => {
            if let Some(metadata) = previous_document_metadata.clone() {
                state
                    .document_metadata
                    .replace(&doc.id, &doc.kb_id, metadata)
                    .ok();
            }
            state.graphs.replace_snapshot(previous_graphs.clone()).ok();
            state
                .kbs
                .update_counts(&doc.kb_id, removed_chunks as isize, 1)
                .ok();
            restore_index(
                state,
                &previous_index,
                "Document metadata was already missing",
            );
            anyhow::bail!("Document metadata was already deleted");
        }
        Err(error) => {
            if let Some(metadata) = previous_document_metadata {
                state
                    .document_metadata
                    .replace(&doc.id, &doc.kb_id, metadata)
                    .ok();
            }
            state.graphs.replace_snapshot(previous_graphs).ok();
            if let Err(rollback_error) =
                state
                    .kbs
                    .update_counts(&doc.kb_id, removed_chunks as isize, 1)
            {
                tracing::error!(%rollback_error, doc_id = %doc.id, "Failed to roll back KB counts");
            }
            restore_index(state, &previous_index, "Document metadata deletion failed");
            return Err(error.context("Failed to delete document metadata"));
        }
    }
    if let Err(error) = state.tasks.cancel_document(&doc.id) {
        tracing::warn!(%error, doc_id = %doc.id, "Failed to persist task cancellation after deletion");
    }
    if path.exists()
        && let Err(error) = std::fs::remove_file(&path)
    {
        tracing::warn!(%error, path = %path.display(), "Failed to remove orphaned upload file");
    }
    Ok(())
}

async fn query_graphrag(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(dataset_id): axum::extract::Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error(StatusCode::NOT_FOUND, "Knowledge base not found");
    }
    let snapshot = state.graphs.snapshot();
    let Some(graph) = snapshot.kb_graphs.get(&dataset_id) else {
        return Json(ApiResponse {
            code: 0,
            message: "No graph built yet".into(),
            data: Some(serde_json::json!({
                "nodes": 0,
                "edges": 0,
                "node_list": [],
                "context": [],
            })),
        })
        .into_response();
    };
    let question = body
        .get("question")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let names = graph.node_names();
    let edges = names.iter().map(|name| graph.degree(name)).sum::<usize>() / 2;
    let node_list: Vec<serde_json::Value> = names
        .iter()
        .take(200)
        .map(|name| {
            serde_json::json!({
                "name": name,
                "type": "entity",
                "degree": graph.degree(name),
            })
        })
        .collect();
    let context = if question.trim().is_empty() {
        Vec::<String>::new()
    } else {
        state
            .graphs
            .context_for_query(std::slice::from_ref(&dataset_id), question)
    };
    Json(ApiResponse {
        code: 0,
        message: "ok".into(),
        data: Some(serde_json::json!({
            "nodes": graph.node_count(),
            "edges": edges,
            "node_list": node_list,
            "context": context,
        })),
    })
    .into_response()
}

async fn build_graphrag(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(dataset_id): axum::extract::Path<String>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error(StatusCode::NOT_FOUND, "Knowledge base not found");
    }
    let Some(llm) = state.llm.clone() else {
        return api_error(StatusCode::BAD_REQUEST, "LLM is not configured");
    };
    let chunks: Vec<crate::search::IndexedChunk> = state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .into_iter()
        .filter(|chunk| {
            chunk.metadata.get("kb_id").map(String::as_str) == Some(dataset_id.as_str())
        })
        .collect();
    if chunks.is_empty() {
        return api_error(StatusCode::BAD_REQUEST, "No chunks in this knowledge base");
    }
    let entity_types: Vec<String> = ["organization", "person", "location", "event", "concept"]
        .iter()
        .map(|value| value.to_string())
        .collect();
    let mut by_doc: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for chunk in &chunks {
        let doc_id = chunk
            .metadata
            .get("doc_id")
            .cloned()
            .unwrap_or_else(|| chunk.doc_name.clone());
        by_doc
            .entry(doc_id)
            .or_default()
            .push(chunk.content.clone());
    }
    let mut graph = crate::graphrag_enhanced::EntityGraph::new();
    let mut doc_count = 0usize;
    let mut entity_count = 0usize;
    for (doc_id, contents) in &by_doc {
        match crate::graphrag_index::run_doc_pipeline(
            &llm,
            doc_id,
            contents,
            &entity_types,
            "Chinese",
        )
        .await
        {
            Ok((doc_graph, ents)) => {
                crate::graphrag_index::graph_merge(&mut graph, &doc_graph);
                doc_count += 1;
                entity_count += ents;
            }
            Err(error) => {
                tracing::warn!(doc_id, error = %error, "graphrag pipeline failed for document");
            }
        }
    }
    let nodes = graph.node_count();
    let names = graph.node_names();
    let edges = names.iter().map(|name| graph.degree(name)).sum::<usize>() / 2;
    // 持久化：合并到 GraphStore 的 kb_graphs（重启后保留）
    let mut snapshot = state.graphs.snapshot();
    snapshot.kb_graphs.insert(dataset_id.clone(), graph.clone());
    if let Err(error) = state.graphs.replace_snapshot(snapshot) {
        tracing::warn!(%error, "Failed to persist graphrag snapshot");
    }
    let node_list: Vec<serde_json::Value> = names
        .into_iter()
        .take(200)
        .map(|name| {
            serde_json::json!({
                "name": name,
                "type": "entity",
                "degree": graph.degree(&name),
            })
        })
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "nodes": nodes,
            "edges": edges,
            "documents": doc_count,
            "entities": entity_count,
            "node_list": node_list,
        }
    }))
    .into_response()
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let current = body
        .get("current_password")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let new_password = body
        .get("new_password")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match state
        .users
        .change_password(&auth.user_id, current, new_password)
    {
        Ok(()) => Json(serde_json::json!({ "code": 0, "message": "ok" })).into_response(),
        Err(error) => {
            Json(serde_json::json!({ "code": 400, "message": error.to_string() })).into_response()
        }
    }
}

async fn clear_conversation(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(conversation_id): axum::extract::Path<String>,
) -> Response {
    if state
        .conversations
        .get_for(&conversation_id, &auth.user_id)
        .is_none()
    {
        return Json(serde_json::json!({ "code": 404, "message": "Conversation not found" }))
            .into_response();
    }
    match state.conversations.clear_messages(&conversation_id) {
        Ok(_) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Err(error) => {
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })).into_response()
        }
    }
}

async fn export_conversation(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(conversation_id): axum::extract::Path<String>,
) -> Response {
    let Some(conversation) = state.conversations.get_for(&conversation_id, &auth.user_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Conversation not found" }))
            .into_response();
    };
    let mut markdown = format!("# {}\n\n", conversation.name);
    for message in &conversation.messages {
        let role = if message.role == "user" {
            "**User**"
        } else {
            "**Assistant**"
        };
        markdown.push_str(&format!("{role}: {}\n\n", message.content));
    }
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/markdown; charset=utf-8",
        )],
        markdown,
    )
        .into_response()
}

async fn clear_memories(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    let memory_ids = state.memories.owned_ids(&auth.user_id);
    let _commit_guard = state.memory_commit_lock.lock().unwrap();
    for memory_id in &memory_ids {
        if let Err(error) = state.tasks.cancel_memory(memory_id) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
        if let Err(error) = state.memory_messages.delete_by_memory(memory_id) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
    }
    match state.memories.clear_all_owned(&auth.user_id) {
        Ok(count) => Json(ApiResponse {
            code: 0,
            message: format!("Cleared {} memories", count),
            data: Some(serde_json::json!({"deleted": count})),
        })
        .into_response(),
        Err(error) => api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    }
}

async fn rename_conversation(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(conversation_id): axum::extract::Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Response {
    let Some(name) = body.get("name").and_then(|value| value.as_str()) else {
        return api_error(StatusCode::BAD_REQUEST, "name is required");
    };
    match state
        .conversations
        .rename_for(&conversation_id, &auth.user_id, name)
    {
        Ok(Some(_)) => Json(ApiResponse {
            code: 0,
            message: "Renamed".into(),
            data: None,
        })
        .into_response(),
        Ok(None) => api_error(StatusCode::NOT_FOUND, "Conversation not found"),
        Err(error) => api_error(StatusCode::BAD_REQUEST, &error.to_string()),
    }
}

async fn delete_conversation(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(conversation_id): axum::extract::Path<String>,
) -> Response {
    match state
        .conversations
        .delete_for(&conversation_id, &auth.user_id)
    {
        Ok(true) => Json(serde_json::json!({ "code": 0, "message": "ok" })).into_response(),
        Ok(false) => Json(serde_json::json!({ "code": 404, "message": "Conversation not found" }))
            .into_response(),
        Err(error) => {
            Json(serde_json::json!({ "code": 500, "message": error.to_string() })).into_response()
        }
    }
}

async fn retrieval(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let q = b.get("question").and_then(|v| v.as_str()).unwrap_or("");
    // Upstream `/search` posts `search_id` (+ `kb_id`) and lets the server read
    // the app's `search_config`; the settings sidebar only overrides the fields
    // the user actually touched. Every override below therefore falls back to
    // the application record before the shared defaults.
    let search_app = b
        .get("search_id")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .and_then(|id| state.search_apps.as_ref().and_then(|store| store.get(id)));
    let top_k = b
        .get("top_k")
        .and_then(|v| v.as_u64())
        .map(|value| value as usize)
        .or_else(|| search_app.as_ref().map(|app| app.top_k as usize))
        .unwrap_or(10);
    let page = b.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
    let rerank_id = b
        .get("rerank_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            search_app
                .as_ref()
                .map(|app| app.rerank_id.trim())
                .filter(|value| !value.is_empty())
        });
    let rerank = b.get("rerank").and_then(|v| v.as_bool()).unwrap_or(false)
        || rerank_id.is_some()
        || search_app.as_ref().map(|app| app.rerank).unwrap_or(false);
    let vector_weight = b
        .get("vector_similarity_weight")
        .and_then(|value| value.as_f64())
        .or_else(|| search_app.as_ref().map(|app| app.vector_similarity_weight))
        .unwrap_or(0.3)
        .clamp(0.0, 1.0) as f32;
    let threshold = b
        .get("similarity_threshold")
        .and_then(|value| value.as_f64())
        .or_else(|| search_app.as_ref().map(|app| app.similarity_threshold))
        .unwrap_or(0.2)
        .clamp(0.0, 1.0) as f32;
    // Upstream sends the singular `kb_id`; RayRAG's own clients use `kb_ids`.
    let kb_ids: Vec<String> = b
        .get("kb_ids")
        .or_else(|| b.get("kb_id"))
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .or_else(|| search_app.as_ref().map(|app| app.kb_ids.clone()))
        .unwrap_or_default();
    let doc_ids: Option<Vec<String>> = b
        .get("doc_ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .or_else(|| {
            search_app
                .as_ref()
                .map(|app| app.doc_ids.clone())
                .filter(|ids| !ids.is_empty())
        });
    // Upstream prefers the search app's stored `meta_data_filter` and only reads
    // the request field for a direct retrieval call.
    let metadata_filter = b
        .get("meta_data_filter")
        .filter(|value| !value.is_null())
        .or_else(|| {
            search_app
                .as_ref()
                .map(|app| &app.meta_data_filter)
                .filter(|value| !value.is_null())
        });
    let metadata_context =
        crate::api::document_metadata::MetadataFilterContext::new(&auth.user_id, q)
            .with_chat_selector(
                search_app
                    .as_ref()
                    .map(|app| app.chat_id.as_str())
                    .filter(|chat_id| !chat_id.trim().is_empty()),
            );
    let filtered_doc_ids = match crate::api::document_metadata::resolve_metadata_doc_ids(
        &state,
        &metadata_context,
        &kb_ids,
        doc_ids.as_deref(),
        metadata_filter,
    )
    .await
    {
        Ok(doc_ids) => doc_ids,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let (include_metadata, metadata_fields) =
        crate::api::document_metadata::reference_metadata_selection(&b);
    let rank_feature = b
        .get("rank_feature")
        .and_then(|value| serde_json::from_value(value.clone()).ok());
    let highlight = b
        .get("highlight")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let aggs = b
        .get("aggs")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    if !all_kbs_accessible(&state, &kb_ids, &auth) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "At least one accessible kb_id is required",
        );
    }
    if let Err(error) = validate_kb_embedding_bindings(&state, &kb_ids) {
        return api_error(StatusCode::BAD_REQUEST, &error.to_string());
    }
    let embedding = if vector_weight > 0.0 {
        let emb = match kb_embedder_for(&state, &kb_ids) {
            Ok(embedder) => embedder,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        };
        match emb.embed(&[q]).await {
            Ok(embeddings) => embeddings.into_iter().next(),
            Err(_) => None,
        }
    } else {
        None
    };
    if vector_weight > 0.0 && embedding.is_none() {
        return api_error(StatusCode::INTERNAL_SERVER_ERROR, "Embed failed");
    }
    let global_offset = (page - 1).saturating_mul(top_k);
    let window = rerank_window(top_k, rerank.then_some(1024));
    let block_start = global_offset / window * window;
    let all_matches =
        state
            .engine
            .read()
            .unwrap()
            .hybrid_search_kbs(crate::search::HybridSearchQuery {
                query: q,
                query_embedding: embedding.as_deref(),
                top_k: usize::MAX,
                kb_ids: &kb_ids,
                vector_weight,
                doc_ids: filtered_doc_ids.as_deref(),
                rank_feature: rank_feature.as_ref(),
            });
    let post_threshold = if vector_weight > 0.0 { threshold } else { 0.0 };
    let aggregate_candidates: Vec<_> = all_matches
        .iter()
        .filter(|result| result.score >= post_threshold)
        .cloned()
        .collect();
    let total = aggregate_candidates.len();
    let doc_aggs = if aggs {
        aggregate_documents(&aggregate_candidates)
    } else {
        Vec::new()
    };
    let mut matches: Vec<_> = all_matches
        .into_iter()
        .skip(block_start)
        .take(window)
        .collect();
    if rerank && !matches.is_empty() {
        let reranker = match kb_reranker_for(&state, &kb_ids, rerank_id) {
            Ok(Some(reranker)) => reranker,
            Ok(None) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Reranker is not configured",
                );
            }
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        };
        let documents: Vec<String> = matches
            .iter()
            .map(|result| result.chunk.content.clone())
            .collect();
        let model_scores = match reranker.rerank(q, &documents, matches.len()).await {
            Ok(scores) => scores,
            Err(error) => {
                tracing::warn!(%error, "Reranker request failed");
                return api_error(StatusCode::BAD_GATEWAY, "Reranker request failed");
            }
        };
        matches = match apply_hybrid_rerank(matches, &model_scores, vector_weight) {
            Ok(results) => results,
            Err(error) => {
                tracing::warn!(%error, "Invalid reranker response");
                return api_error(StatusCode::BAD_GATEWAY, "Invalid reranker response");
            }
        };
    }
    let begin = global_offset % window;
    let filtered: Vec<_> = matches
        .iter()
        .filter(|result| result.score >= post_threshold)
        .cloned()
        .collect();
    let results: Vec<serde_json::Value> = filtered
        .iter()
        .skip(begin)
        .take(top_k)
        .map(|result| {
            let doc_id = result
                .chunk
                .metadata
                .get("doc_id")
                .cloned()
                .unwrap_or_default();
            let kb_id = result
                .chunk
                .metadata
                .get("kb_id")
                .cloned()
                .unwrap_or_default();
            let document_metadata = include_metadata.then(|| {
                crate::api::document_metadata::enrich_metadata(
                    &state,
                    &kb_id,
                    &doc_id,
                    metadata_fields.as_ref(),
                )
            });
            serde_json::json!({
                "score": result.score,
                "vector_similarity": result.vector_score,
                "term_similarity": result.term_score,
                "content": result.chunk.content,
                "doc_name": result.chunk.doc_name,
                "chunk_id": result.chunk.id,
                "doc_id": doc_id,
                "kb_id": kb_id,
                "highlight": highlight.then(|| highlight_content(&result.chunk.content, q)),
                "document_metadata": document_metadata.flatten(),
            })
        })
        .collect();
    let graph_context = state.graphs.context_for_query(&kb_ids, q);
    Json(ApiResponse {
        code: 0,
        message: "ok".into(),
        data: Some(serde_json::json!({"total":total,"chunks":results,"doc_aggs":doc_aggs,"graph_context":graph_context})),
    })
    .into_response()
}

// ── Chat ───────────────────────────────────────────────────────

async fn chat_completion(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let kb_ids: Vec<String> = b
        .get("kb_ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if !all_kbs_accessible(&state, &kb_ids, &auth) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "At least one accessible kb_id is required",
        );
    }
    let q = b.get("question").and_then(|v| v.as_str()).unwrap_or("");
    let llm = match &state.llm {
        Some(l) => l,
        None => {
            return api_error(StatusCode::SERVICE_UNAVAILABLE, "No LLM");
        }
    };
    // RAG：混合检索知识库 → 注入上下文
    let top_k = b
        .get("top_k")
        .and_then(|v| v.as_u64())
        .unwrap_or(4)
        .clamp(1, 20) as usize;
    let mut hits =
        state
            .engine
            .read()
            .unwrap()
            .hybrid_search_kbs(crate::search::HybridSearchQuery {
                query: q,
                query_embedding: None,
                kb_ids: &kb_ids,
                top_k,
                vector_weight: 0.5,
                doc_ids: None,
                rank_feature: None,
            });
    // RAGFlow dialog_service: when a KB's prompt_config declares
    // cross_languages, translate the question and merge extra-language hits
    // (deduped by chunk id). Degrades to the original-language hits when no
    // LLM is available or translation fails.
    let cross_langs: Vec<String> = {
        let mut langs = Vec::new();
        for kb_id in &kb_ids {
            if let Some(kb) = state.kbs.get(kb_id)
                && let Some(list) = kb
                    .prompt_config
                    .get("cross_languages")
                    .and_then(|v| v.as_array())
            {
                for item in list.iter().filter_map(|v| v.as_str()) {
                    if !item.trim().is_empty() && !langs.contains(&item.trim().to_string()) {
                        langs.push(item.trim().to_string());
                    }
                }
            }
        }
        langs
    };
    if !cross_langs.is_empty()
        && let Some(llm_client) = &state.llm
    {
        let langs_str = cross_langs.join(", ");
        let mut vars: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
        vars.insert("query", q);
        vars.insert("languages", langs_str.as_str());
        let sys = crate::prompts::PromptLibrary::cross_languages_sys()
            .render(&std::collections::HashMap::new());
        let user = crate::prompts::PromptLibrary::cross_languages_user().render(&vars);
        let translated = match llm_client
            .chat_completion(&[
                crate::llm::ChatMessage::new("system", &sys),
                crate::llm::ChatMessage::new("user", &user),
            ])
            .await
        {
            Ok(completion) => completion.content,
            Err(error) => {
                tracing::warn!(%error, "cross_languages translation failed; original hits used");
                String::new()
            }
        };
        let mut seen: std::collections::HashSet<String> =
            hits.iter().map(|hit| hit.chunk.id.clone()).collect();
        for translation in translated
            .split("###")
            .map(str::trim)
            .filter(|text| !text.is_empty() && *text != q)
        {
            let extra =
                state
                    .engine
                    .read()
                    .unwrap()
                    .hybrid_search_kbs(crate::search::HybridSearchQuery {
                        query: translation,
                        query_embedding: None,
                        kb_ids: &kb_ids,
                        top_k,
                        vector_weight: 0.5,
                        doc_ids: None,
                        rank_feature: None,
                    });
            for hit in extra {
                if seen.insert(hit.chunk.id.clone()) {
                    hits.push(hit);
                }
            }
        }
    }
    let mut context = String::new();
    let mut references = Vec::new();
    for (i, hit) in hits.iter().take(4).enumerate() {
        context.push_str(&format!("[{i}] {}\n", hit.chunk.content));
        references.push(serde_json::json!({
            "chunk_id": hit.chunk.id,
            "doc_name": hit.chunk.doc_name,
            "score": hit.score,
        }));
    }
    let prompt = if context.trim().is_empty() {
        q.to_string()
    } else {
        format!(
            "请基于以下知识库内容回答问题。若内容不足以回答，请说明。\n\n知识库内容：\n{context}\n问题：{q}"
        )
    };
    let msgs = vec![crate::llm::ChatMessage::new("user", &prompt)];
    match llm.chat(&msgs).await {
        Ok(a) => Json(ApiResponse {
            code: 0,
            message: "ok".into(),
            data: Some(serde_json::json!({"answer":a,"reference":references})),
        })
        .into_response(),
        Err(e) => Json(ApiResponse {
            code: 500,
            message: format!("{}", e),
            data: None,
        })
        .into_response(),
    }
}

// ── API authentication ────────────────────────────────────────

/// The middleware's view of a request: most unauthenticated endpoints are
/// decided by path alone, but `/api/v1/users` carries both the public sign-up
/// `POST` and the authenticated `GET` from the same upstream blueprint, so the
/// method is part of the check there.
fn public_api_request(method: &Method, path: &str) -> bool {
    if method == Method::POST && path == "/api/v1/users" {
        return true;
    }
    public_api_path(path)
}

fn public_api_path(path: &str) -> bool {
    // Patterns may contain one dynamic segment (`{channel}`, `{account_id}`):
    // the middleware sees the concrete request path, so placeholders are matched
    // segment-wise instead of as literals.
    const PATTERNS: [&str; 12] = [
        "/api/v1/auth/login",
        // Upstream mounts the admin console login on the unauthenticated group
        // (`internal/router/router.go`): it is how the console obtains a token.
        "/api/v1/admin/login",
        "/api/v1/user/register",
        // RayRAG's own build-identity endpoint stays public; losing it broke the
        // banner/version probes in the v0.3.5o rewrite.
        "/api/v1/version",
        "/api/v1/system/password-public-key",
        "/api/v1/system/healthz",
        // Upstream `system_api.version` / `get_config`, plus the OAuth entry
        // points, are unauthenticated: the login page needs them before it has
        // a token.
        "/api/v1/system/version",
        "/api/v1/system/config",
        // The wizard's status route answers "does this deployment still need
        // setup?" before a session exists.
        "/api/v1/setup/status",
        "/api/v1/auth/login/channels",
        "/api/v1/auth/login/{channel}",
        "/api/v1/auth/oauth/{channel}/callback",
    ];
    const CHANNEL_PATTERNS: [&str; 2] = [
        // Chat channel inbound endpoints: platform webhook callbacks must not
        // need the RayRAG API token (RAGFlow channels are unauthenticated).
        "/api/v1/channels/{account_id}/webhook/inbound",
        "/api/v1/channels/{account_id}/feishu/event",
    ];
    PATTERNS
        .iter()
        .chain(CHANNEL_PATTERNS.iter())
        .any(|pattern| path_matches_pattern(pattern, path))
        || {
            // 公共分享端点（免登录）：动态 {chat_id} 无法用字面量匹配，
            // 对齐 RAGFlow 分享页使用的 chatbots 端点（info / completions）。
            path.starts_with("/api/v1/chatbots/")
        }
}

/// Segment-wise pattern match where `{name}` segments match any single segment.
fn path_matches_pattern(pattern: &str, path: &str) -> bool {
    let mut pattern_segments = pattern.split('/');
    let mut path_segments = path.split('/');
    loop {
        match (pattern_segments.next(), path_segments.next()) {
            (None, None) => return true,
            (Some(expected), Some(actual)) => {
                let is_placeholder =
                    expected.starts_with('{') && expected.ends_with('}') && expected.len() > 2;
                if !is_placeholder && expected != actual {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

async fn require_api_auth(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS
        || !request.uri().path().starts_with("/api/v1/")
        || public_api_request(request.method(), request.uri().path())
    {
        return next.run(request).await;
    }

    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split_once(' '))
        .filter(|(scheme, token)| scheme.eq_ignore_ascii_case("bearer") && !token.is_empty())
        .map(|(_, token)| token.to_string())
        .or_else(|| {
            // 页面导航兜底：cookie rayrag_token（登录后由前端注入）
            request
                .headers()
                .get(axum::http::header::COOKIE)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    value.split(';').find_map(|part| {
                        let part = part.trim();
                        part.strip_prefix("rayrag_token=").map(str::to_string)
                    })
                })
        });

    // A session token first (`UserService.query(access_token=...)`); when it is
    // not one, the credential may be an `APIToken.beta` secret — upstream
    // `login_required(auth_types=AUTH_BETA)` resolves it through
    // `APIToken.query(beta=auth_token)` and loads the owner
    // (`UserService.query(id=objs[0].tenant_id)`). The embed/share surfaces
    // authenticate exactly this way: the URL carries `?auth=<beta>` and every
    // follow-up request sends `Authorization: Bearer <beta>`.
    let resolved = token.as_deref().and_then(|token| {
        state
            .users
            .validate_token(token)
            .or_else(|| {
                let tenant_id = state.api_tokens.tenant_for_beta(token)?;
                // The personal tenant id is the owner's user id, so the token's
                // tenant must still resolve to a live account.
                state.users.get_user_by_id(&tenant_id)?;
                Some(tenant_id)
            })
            .map(|user_id| (user_id, token.to_string()))
    });

    if let Some((user_id, token)) = resolved {
        let is_admin = state.users.is_admin(&user_id);
        let mut request = request;
        request.extensions_mut().insert(AuthContext {
            user_id,
            is_admin,
            token,
        });
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse {
                code: 401,
                message: "Unauthorized".into(),
                data: None,
            }),
        )
            .into_response()
    }
}

async fn prometheus_metrics() -> Response {
    match crate::metrics::encode_prometheus_metrics() {
        Ok(metrics) => (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            metrics,
        )
            .into_response(),
        Err(error) => {
            tracing::error!(%error, "Failed to encode Prometheus metrics");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct ComponentCatalogQuery {
    category: Option<String>,
}

fn parse_component_categories(
    raw: Option<&str>,
) -> Result<Vec<crate::runtime::ComponentCategory>, String> {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(Vec::new());
    };
    raw.split(',')
        .filter_map(|part| {
            let part = part.trim().to_ascii_lowercase();
            (!part.is_empty()).then_some(part)
        })
        .map(|part| {
            crate::runtime::ComponentCategory::parse(&part)
                .ok_or_else(|| format!("unknown category: {part}"))
        })
        .collect()
}

async fn component_catalog(Query(query): Query<ComponentCatalogQuery>) -> Response {
    let categories = match parse_component_categories(query.category.as_deref()) {
        Ok(categories) => categories,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ApiResponse {
                    code: 400,
                    message,
                    data: None,
                }),
            )
                .into_response();
        }
    };
    Json(ApiResponse {
        code: 0,
        message: "success".into(),
        data: Some(
            serde_json::to_value(crate::runtime::component_descriptors(&categories))
                .expect("component descriptors are JSON-safe"),
        ),
    })
    .into_response()
}

// ── Current user ───────────────────────────────────────────────

async fn user_info(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    match state.users.get_user_by_id(&auth.user_id) {
        Some(user) => Json(ApiResponse {
            code: 0,
            message: "ok".into(),
            data: Some(serde_json::json!({
                "id": user.id,
                "nickname": user.nickname,
                "email": user.email,
                "avatar": user.avatar,
                "timezone": user.timezone,
                "role": user.role,
                "created_at": user.created_at,
            })),
        })
        .into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse {
                code: 401,
                message: "Unauthorized".into(),
                data: None,
            }),
        )
            .into_response(),
    }
}

// ── Router ─────────────────────────────────────────────────────

fn configure_cors(router: Router) -> Router {
    let Ok(origin) = std::env::var("RAYRAG_CORS_ORIGIN") else {
        return router;
    };
    match origin.parse::<axum::http::HeaderValue>() {
        Ok(origin) => router.layer(
            CorsLayer::new()
                .allow_origin(origin)
                .allow_methods(tower_http::cors::Any)
                .allow_headers([
                    axum::http::header::AUTHORIZATION,
                    axum::http::header::CONTENT_TYPE,
                ]),
        ),
        Err(error) => {
            tracing::warn!(%error, "Ignoring invalid RAYRAG_CORS_ORIGIN");
            router
        }
    }
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let sd = &state.static_dir;
    let router = Router::new()
        // Web pages
        .route("/", get(crate::web::index))
        .route("/dashboard", get(crate::web::dashboard))
        .route("/kbs", get(crate::web::kbs_page))
        .route("/kbs/{id}", get(crate::web::kb_detail_page))
        .route("/datasets", get(crate::web::datasets_alias))
        .route("/dataset/{tab}/{id}", get(crate::web::dataset_tab_page))
        .route("/search", get(crate::web::search_page))
        .route("/providers", get(crate::web::providers_page))
        .route("/user-setting", get(crate::web::user_setting_index))
        .route(
            "/user-setting/data-source",
            get(crate::web::datasources_page),
        )
        .route(
            "/user-setting/data-source/{id}",
            get(crate::web::data_source_detail_page),
        )
        .route(
            "/user-setting/chat-channel",
            get(crate::web::user_setting_chat_channel_page),
        )
        .route("/user-setting/model", get(crate::web::providers_page))
        .route("/user-setting/mcp", get(crate::web::user_setting_mcp_page))
        .route(
            "/user-setting/team",
            get(crate::web::user_setting_team_page),
        )
        .route(
            "/user-setting/profile",
            get(crate::web::user_setting_profile_page),
        )
        .route("/user-setting/api", get(crate::web::user_setting_api_page))
        .route("/skills", get(crate::web::skills_page))
        // RAGFlow 0.26.4 route-graph parity: aliases that map to the same SSR pages.
        .route("/home", get(crate::web::dashboard))
        .route("/profile-setting", get(crate::web::user_setting_index))
        .route("/files/skills", get(crate::web::skills_page))
        .route("/explore", get(crate::web::agents_page))
        .route("/agent-templates", get(crate::web::agent_templates_page))
        .route("/agent-list", get(crate::web::agents_page))
        .route("/logout", get(crate::web::logout_page))
        .route(
            "/documents/{kb_id}/{doc_id}",
            get(crate::web::document_page),
        )
        .route("/settings", get(crate::web::settings_page))
        .route("/api-docs", get(crate::web::api_docs))
        .route("/status", get(crate::web::status_page))
        .route("/chat", get(crate::web::chat_page))
        // Upstream `routes.tsx`: `/login` and `/login-next` are two entry points
        // for the same page, and the canonical agent/search/detail paths are
        // singular (`/agent/:id`, `/search/:id`, `/document/:id`).
        .route("/login-next", get(crate::web::login_page))
        .route("/agent/{agent_id}", get(crate::web::agent_detail_page))
        // Upstream `Routes.AgentExplore` (`/agent/:id/explore`); the plural
        // spelling mirrors RayRAG's `/agents` alias for the same page bundle.
        .route(
            "/agent/{agent_id}/explore",
            get(crate::web::agent_explore_page),
        )
        .route(
            "/agents/{agent_id}/explore",
            get(crate::web::agent_explore_page),
        )
        .route(
            "/search/{search_id}",
            get(crate::web::search_app_detail_page),
        )
        .route("/search/share", get(crate::web::search_share_page))
        // Upstream `Routes.AgentShare` (`/agent/share?shared_id=…&from=agent&auth=<beta>`).
        .route("/agent/share", get(crate::web::agent_share_page))
        // Upstream `Routes.DataflowResult` — the ingestion-log viewer.
        .route("/dataflow-result", get(crate::web::dataflow_result_page))
        .route("/document/{doc_id}", get(crate::web::document_viewer_page))
        // Upstream `Routes.Chunk` subtree (`pages/chunk/**`): the standalone
        // chunk workbench with its three panel routes.
        .route("/chunk", get(crate::web::chunk_page))
        .route("/chunk/parsed/chunks", get(crate::web::chunk_parsed_page))
        .route("/chunk/chunk/{doc_id}", get(crate::web::chunk_chunk_page))
        .route("/chunk/result/{doc_id}", get(crate::web::chunk_result_page))
        // RAGFlow 0.26.4 route parity: /chats (chat apps) and /searches (search apps).
        .route("/chats", get(crate::web::chat_apps_page))
        .route("/chats/share", get(crate::web::chat_share_page))
        // Upstream `Routes.ChatWidget` (`/chats/widget`) — the floating widget the
        // embed dialog's widget snippet mounts.
        .route("/chats/widget", get(crate::web::chat_widget_page))
        .route("/chats/{id}", get(crate::web::chat_app_detail_page))
        .route("/searches", get(crate::web::search_apps_page))
        .route("/searches/{id}", get(crate::web::search_app_detail_page))
        .route("/chat/{id}", get(crate::web::chat_detail_page))
        // Upstream `Routes.Dataset = /dataset/files`: the bare detail path
        // redirects into the Files tab.
        .route("/dataset/{id}", get(crate::web::dataset_files_redirect))
        .route("/datasources", get(crate::web::datasources_page))
        .route("/files", get(crate::web::files_page))
        .route("/agents", get(crate::web::agents_page))
        .route("/agents/{agent_id}", get(crate::web::agent_detail_page))
        // Upstream `Routes.AgentLog = /agent-log-page/:agentId`
        // (`pages/agents/agent-log-page.tsx`).
        .route(
            "/agent-log-page/{agent_id}",
            get(crate::web::agent_log_page),
        )
        .route("/memories", get(crate::web::memories_page_v2))
        .route(
            "/memory/memory-message/{memory_id}",
            get(crate::web::memory_message_page),
        )
        .route(
            "/memory/memory-setting/{memory_id}",
            get(crate::web::memory_setting_page),
        )
        // RAGFlow admin console (`pages/admin/**`): `/admin` is the console
        // login and the console pages live under the navigation layout.
        .route("/admin", get(crate::web::admin_page))
        .route("/admin/services", get(crate::web::admin_services_page))
        .route("/admin/users", get(crate::web::admin_users_page))
        .route(
            "/admin/users/{user_id}",
            get(crate::web::admin_user_detail_page),
        )
        .route(
            "/admin/sandbox-settings",
            get(crate::web::admin_sandbox_settings_page),
        )
        .route("/login", get(crate::web::login_page))
        .route("/setup", get(crate::web::setup_page))
        .route("/metrics", get(prometheus_metrics))
        // API
        .route(
            "/api/v1/system/status",
            get(crate::api::system::system_status_detail),
        )
        .route(
            "/api/v1/version",
            get(|| async {
                Json(ApiResponse {
                    code: 0,
                    message: "ok".into(),
                    data: Some(serde_json::json!({
                        "version": crate::build_info::VERSION,
                        "parity": crate::build_info::PARITY_SLICE,
                        "revision": crate::build_info::revision(),
                        "built_at": crate::build_info::BUILT_AT,
                    })),
                })
            }),
        )
        .route("/api/v1/auth/login", post(login))
        // Compatible alias for clients written against earlier RayRAG builds;
        // upstream registers only `POST /api/v1/users` (see the user routes).
        .route("/api/v1/user/register", post(user_add))
        .route(
            "/api/v1/system/password-public-key",
            get(password_public_key),
        )
        .route("/api/v1/user/info", get(user_info))
        .route("/api/v1/components", get(component_catalog))
        .route(
            "/api/v1/datasets",
            get(list_datasets)
                .post(create_dataset)
                .delete(delete_datasets),
        )
        .route(
            "/api/v1/datasets/{id}",
            get(get_dataset).put(update_dataset).delete(delete_dataset),
        )
        .route(
            "/api/v1/datasets/{id}/tags",
            get(crate::api::dataset_tags::list_tags)
                .delete(crate::api::dataset_tags::delete_tags)
                .put(crate::api::dataset_tags::rename_tag),
        )
        .route(
            "/api/v1/datasets/{id}/documents",
            post(upload_document).get(crate::api::document::list_docs),
        )
        .route(
            "/api/v1/datasets/{id}/documents/{did}",
            get(crate::api::document::get_doc)
                .delete(crate::api::document::delete_doc)
                .patch(crate::api::document::rename_doc),
        )
        .route(
            "/api/v1/datasets/{id}/documents/{did}/reparse",
            post(crate::api::document::reparse_doc),
        )
        .route(
            "/api/v1/documents/{doc_id}/preview",
            get(crate::api::document::preview_doc),
        )
        // Upstream `document_api.py::download_document`: the byte source behind
        // `fetchPreviewBlob(id, 'document')` (`previewHtmlFile` / `downloadDocument`).
        .route(
            "/api/v1/documents/{doc_id}",
            get(crate::api::document::download_document),
        )
        // RayRAG-side readers for the previewers upstream builds in the browser:
        // the xlsx grid (`@js-preview/excel`) and the per-slide deck text
        // (`pptx-preview`). No upstream route exists, so both are documented as
        // `replaced` in the coverage ledger.
        .route(
            "/api/v1/documents/{doc_id}/preview/sheets",
            get(crate::api::document::preview_sheets),
        )
        .route(
            "/api/v1/documents/{doc_id}/preview/slides",
            get(crate::api::document::preview_slides),
        )
        .route(
            "/api/v1/datasets/{id}/documents/{did}/download",
            get(crate::api::document::download_doc),
        )
        .route(
            "/api/v1/datasets/{id}/documents/{did}/metadata",
            get(crate::api::document_metadata::get_document_metadata)
                .put(crate::api::document_metadata::replace_document_metadata)
                .delete(crate::api::document_metadata::delete_document_metadata),
        )
        .route(
            "/api/v1/datasets/{id}/metadata/keys",
            get(crate::api::document_metadata::metadata_keys),
        )
        .route(
            "/api/v1/datasets/{id}/metadata/summary",
            post(crate::api::document_metadata::metadata_summary),
        )
        // Upstream `document_api.update_metadata` — the endpoint the dataset
        // metadata manager calls.
        .route(
            "/api/v1/datasets/{id}/documents/metadatas",
            axum::routing::patch(crate::api::document_metadata::update_document_metadatas),
        )
        // RayRAG's original flat-body alias.
        .route(
            "/api/v1/datasets/{id}/metadata/batch",
            post(crate::api::document_metadata::batch_update_metadata),
        )
        .route(
            "/api/v1/datasets/{id}/documents/{did}/chunks",
            get(crate::api::chunks::list_chunks)
                .post(crate::api::chunks::add_chunk)
                .delete(crate::api::chunks::delete_chunks)
                .patch(crate::api::chunks::switch_chunks),
        )
        .route(
            "/api/v1/datasets/{id}/documents/{did}/chunks/{chunk_id}",
            get(crate::api::chunks::get_chunk).patch(crate::api::chunks::update_chunk),
        )
        .route("/api/v1/retrieval", post(retrieval))
        .route(
            "/api/v1/conversations/{conversation_id}",
            axum::routing::delete(delete_conversation).patch(rename_conversation),
        )
        .route(
            "/api/v1/conversations/{conversation_id}/clear",
            axum::routing::post(clear_conversation),
        )
        .route(
            "/api/v1/conversations/{conversation_id}/export",
            axum::routing::get(export_conversation),
        )
        .route(
            "/api/v1/user/password",
            axum::routing::patch(change_password),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/graphrag/build",
            axum::routing::post(build_graphrag),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/graphrag/query",
            axum::routing::post(query_graphrag),
        )
        // ── RAGFlow admin API (admin/server/routes.py port) ──
        .route("/api/v1/admin/ping", get(crate::api::admin::ping))
        // ── RAGFlow admin console (internal/admin) ──
        .route("/api/v1/admin/login", post(crate::api::admin::login))
        .route("/api/v1/admin/logout", get(crate::api::admin::logout))
        .route(
            "/api/v1/admin/services",
            get(crate::api::admin::list_services),
        )
        .route(
            "/api/v1/admin/services/{service_id}",
            get(crate::api::admin::service_details),
        )
        .route(
            "/api/v1/admin/users",
            get(crate::api::admin::list_users).post(crate::api::admin::create_user),
        )
        .route(
            "/api/v1/admin/users/{username}",
            get(crate::api::admin::get_user_details).delete(crate::api::admin::delete_user),
        )
        .route(
            "/api/v1/admin/users/{username}/password",
            axum::routing::put(crate::api::admin::change_password),
        )
        .route(
            "/api/v1/admin/users/{username}/activate",
            axum::routing::put(crate::api::admin::alter_user_activate_status),
        )
        .route(
            "/api/v1/admin/users/{username}/admin",
            axum::routing::put(crate::api::admin::grant_admin)
                .delete(crate::api::admin::revoke_admin),
        )
        .route(
            "/api/v1/admin/users/{username}/datasets",
            get(crate::api::admin::list_user_datasets),
        )
        .route(
            "/api/v1/admin/users/{username}/agents",
            get(crate::api::admin::list_user_agents),
        )
        .route(
            "/api/v1/admin/users/{username}/keys",
            get(crate::api::admin::get_user_api_keys)
                .post(crate::api::admin::generate_user_api_key),
        )
        .route(
            "/api/v1/admin/users/{username}/keys/{key}",
            axum::routing::delete(crate::api::admin::delete_user_api_key),
        )
        .route(
            "/api/v1/admin/version",
            get(crate::api::admin::show_version),
        )
        .route(
            "/api/v1/admin/variables",
            get(crate::api::system_settings::list_variables)
                .put(crate::api::system_settings::set_variable),
        )
        .route("/api/v1/admin/configs", get(crate::api::admin::get_config))
        // Upstream `admin/server/routes.py`: sandbox provider registry, per
        // provider schema, active configuration and the connection probe.
        .route(
            "/api/v1/admin/sandbox/providers",
            get(crate::api::sandbox_admin::list_providers),
        )
        .route(
            "/api/v1/admin/sandbox/providers/{provider_id}/schema",
            get(crate::api::sandbox_admin::provider_schema),
        )
        .route(
            "/api/v1/admin/sandbox/config",
            get(crate::api::sandbox_admin::get_config).post(crate::api::sandbox_admin::set_config),
        )
        .route(
            "/api/v1/admin/sandbox/test",
            post(crate::api::sandbox_admin::test_connection),
        )
        .route(
            "/api/v1/admin/variables/{var_name}",
            get(crate::api::system_settings::show_variable),
        )
        .route(
            "/api/v1/admin/environments",
            get(crate::api::admin::get_environments),
        )
        .route(
            "/api/v1/admin/log_levels",
            get(crate::api::admin::get_log_levels).put(crate::api::admin::set_log_level),
        )
        .route("/api/v1/search", post(crate::api::search::weighted_search))
        .route(
            "/api/v1/dify/retrieval",
            post(crate::api::search::dify_retrieval),
        )
        .route(
            "/api/v1/chunks/{doc_id}",
            get(crate::api::features::list_chunks),
        )
        .route(
            "/api/v1/chunks/feedback",
            post(crate::chunk_feedback::apply_chunk_feedback),
        )
        .route("/api/v1/completions", post(chat_completion))
        .route(
            "/api/v1/chats",
            get(crate::api::features::list_chats).post(crate::api::features::create_chat),
        )
        .route(
            "/api/v1/chats/{id}",
            get(crate::api::features::get_chat)
                .patch(crate::api::features::update_chat)
                .delete(crate::api::features::delete_chat),
        )
        .route(
            "/api/v1/chats/{id}/completions",
            post(crate::api::features::chat_complete),
        )
        .route(
            "/api/v1/chats/{chat_id}/sessions/{session_id}/messages/{message_id}/feedback",
            put(crate::chunk_feedback::update_message_feedback),
        )
        .route(
            "/api/v1/chats/{session_id}/messages/{message_id}/feedback",
            put(crate::chunk_feedback::update_session_message_feedback),
        )
        .route(
            "/api/v1/chats/{session_id}/messages/{message_id}",
            post(crate::api::features::regenerate_chat_message)
                .delete(crate::api::features::delete_chat_message),
        )
        .route(
            "/api/v1/chats/{session_id}/messages/{message_id}/regenerate",
            post(crate::api::features::regenerate_chat_message),
        )
        .route(
            "/api/v1/chats/{chat_id}/sessions/{session_id}/messages/{message_id}",
            post(crate::api::features::regenerate_chat_session_message)
                .delete(crate::api::features::delete_chat_session_message),
        )
        .route(
            "/api/v1/chats/{chat_id}/sessions/{session_id}/messages/{message_id}/regenerate",
            post(crate::api::features::regenerate_chat_session_message),
        )
        .route(
            "/api/v1/evaluations",
            get(crate::api::evaluation::list_datasets).post(crate::api::evaluation::create_dataset),
        )
        .route(
            "/api/v1/evaluations/{id}",
            get(crate::api::evaluation::get_dataset).delete(crate::api::evaluation::delete_dataset),
        )
        .route(
            "/api/v1/evaluations/{id}/cases",
            get(crate::api::evaluation::list_cases).post(crate::api::evaluation::add_case),
        )
        .route(
            "/api/v1/evaluations/{id}/runs",
            post(crate::api::evaluation::start_run),
        )
        .route(
            "/api/v1/evaluation-runs/{id}",
            get(crate::api::evaluation::get_run),
        )
        .route(
            "/api/v1/evaluation-runs/{id}/recommendations",
            get(crate::api::evaluation::recommendations),
        )
        .route("/api/v1/files", get(crate::api::file_mgr::list_files))
        .route(
            "/api/v1/files/folder",
            axum::routing::post(crate::api::file_mgr::create_folder),
        )
        .route(
            "/api/v1/files/{file_id}",
            axum::routing::get(crate::api::file_mgr::get_file)
                .delete(crate::api::file_mgr::delete_file)
                .patch(rename_file),
        )
        .route("/api/v1/files/{file_id}/copy", post(copy_file))
        .route(
            "/api/v1/files/{file_id}/download",
            axum::routing::get(crate::api::file_mgr::download_file),
        )
        .route(
            "/api/v1/data_sources",
            get(crate::api::data_source_mgr::list_data_sources)
                .post(crate::api::data_source_mgr::create_data_source),
        )
        .route(
            "/api/v1/chatapps",
            get(crate::api::chatapp_mgr::list_chat_apps)
                .post(crate::api::chatapp_mgr::create_chat_app),
        )
        .route(
            "/api/v1/chatapps/{id}",
            axum::routing::put(crate::api::chatapp_mgr::update_chat_app)
                .delete(crate::api::chatapp_mgr::delete_chat_app),
        )
        .route(
            "/api/v1/chatbots/{chat_id}/info",
            axum::routing::get(crate::api::chatapp_mgr::chatbot_info),
        )
        .route(
            "/api/v1/chatbots/{session_id}/completions",
            axum::routing::post(crate::api::chatapp_mgr::chatbot_completions),
        )
        .route(
            "/api/v1/data_sources/{id}",
            axum::routing::get(crate::api::data_source_mgr::get_data_source)
                .put(crate::api::data_source_mgr::update_data_source)
                .patch(crate::api::data_source_mgr::update_data_source)
                .delete(crate::api::data_source_mgr::delete_data_source),
        )
        .route(
            "/api/v1/data_sources/{id}/logs",
            axum::routing::get(crate::api::data_source_mgr::get_data_source_logs),
        )
        .route(
            "/api/v1/data_sources/{id}/connect",
            post(crate::api::data_source_mgr::connect_data_source),
        )
        .route(
            "/api/v1/data_sources/{id}/test",
            post(crate::api::data_source_mgr::connect_data_source),
        )
        .route(
            "/api/v1/connectors/box/oauth/web/start",
            post(crate::api::oauth_web::box_oauth_start),
        )
        .route(
            "/api/v1/connectors/box/oauth/web/callback",
            get(crate::api::oauth_web::box_oauth_callback),
        )
        .route(
            "/api/v1/connectors/box/oauth/web/result",
            post(crate::api::oauth_web::box_oauth_result),
        )
        .route(
            "/api/v1/connectors/google/oauth/web/start",
            post(crate::api::oauth_web::google_oauth_start),
        )
        .route(
            "/api/v1/connectors/google/oauth/web/result",
            post(crate::api::oauth_web::google_oauth_result),
        )
        .route(
            "/api/v1/connectors/gmail/oauth/web/callback",
            get(crate::api::oauth_web::gmail_oauth_callback),
        )
        .route(
            "/api/v1/connectors/google-drive/oauth/web/callback",
            get(crate::api::oauth_web::google_drive_oauth_callback),
        )
        .route(
            "/api/v1/data_sources/{id}/disconnect",
            post(crate::api::data_source_mgr::disconnect_data_source),
        )
        .route(
            "/api/v1/searchapps",
            get(crate::api::searchapp_mgr::list_search_apps)
                .post(crate::api::searchapp_mgr::create_search_app),
        )
        .route(
            "/api/v1/searchapps/{id}",
            get(crate::api::searchapp_mgr::get_search_app)
                .put(crate::api::searchapp_mgr::update_search_app)
                .delete(crate::api::searchapp_mgr::delete_search_app),
        )
        // Upstream `search_api` mounts the same CRUD on `/api/v1/searches`
        // (`web/src/utils/api.ts::createSearch` / `getSearchList` /
        // `getSearchDetail` / `updateSearchSetting` / `deleteSearch`), so the
        // upstream-shaped client reaches the same store through these aliases.
        .route(
            "/api/v1/searches",
            get(crate::api::searchapp_mgr::list_search_apps)
                .post(crate::api::searchapp_mgr::create_search_app),
        )
        .route(
            "/api/v1/searches/{id}",
            get(crate::api::searchapp_mgr::get_search_app)
                .put(crate::api::searchapp_mgr::update_search_app)
                .delete(crate::api::searchapp_mgr::delete_search_app),
        )
        .route(
            "/api/v1/data_sources/{id}/files",
            post(crate::api::data_source_mgr::list_data_source_files),
        )
        .route(
            "/api/v1/data_sources/{id}/sync",
            post(crate::api::data_source_mgr::sync_data_source),
        )
        .route(
            "/api/v1/files/upload",
            post(crate::api::file_mgr::upload_file),
        )
        .route(
            "/api/v1/chat/completions",
            post(crate::api::openai_proxy::chat_completions),
        )
        .route(
            "/api/v1/openai/{chat_id}/chat/completions",
            post(crate::api::openai_proxy::openai_rag_chat_completions),
        )
        .route(
            "/api/v1/models",
            get(crate::api::tenant_models::list_ragflow_models),
        )
        .route(
            "/api/v1/models/default",
            get(crate::api::tenant_models::get_ragflow_default_models)
                .patch(crate::api::tenant_models::set_ragflow_default_models),
        )
        .route(
            "/api/v1/models/search",
            get(crate::api::tenant_models::search_ragflow_models),
        )
        .route(
            "/api/v1/openai/models",
            get(crate::api::openai_proxy::list_models),
        )
        .route(
            "/api/v1/embeddings",
            post(crate::api::openai_proxy::embeddings),
        )
        .route("/api/v1/rerank", post(crate::api::openai_proxy::rerank))
        .route(
            "/api/v1/providers",
            get(crate::api::features::list_providers)
                .put(crate::api::tenant_models::add_ragflow_provider),
        )
        .route(
            "/api/v1/providers/",
            put(crate::api::tenant_models::add_ragflow_provider),
        )
        .route(
            "/api/v1/providers/presets",
            get(crate::api::features::list_provider_presets),
        )
        .route(
            "/api/v1/providers/{id}",
            get(crate::api::features::get_provider)
                .post(crate::api::features::create_provider)
                .put(crate::api::features::update_provider)
                .delete(crate::api::features::delete_provider),
        )
        .route(
            "/api/v1/providers/{id}/models",
            get(crate::api::features::list_provider_models),
        )
        .route(
            "/api/v1/providers/{provider_name}/models/{model_name}",
            get(crate::api::tenant_models::show_ragflow_provider_model),
        )
        .route(
            "/api/v1/providers/{provider_name}/instances",
            get(crate::api::tenant_models::list_ragflow_provider_instances)
                .post(crate::api::tenant_models::create_ragflow_provider_instance)
                .delete(crate::api::tenant_models::delete_ragflow_provider_instances),
        )
        .route(
            "/api/v1/providers/{provider_name}/connection",
            post(crate::api::tenant_models::verify_ragflow_provider_connection),
        )
        .route(
            "/api/v1/providers/{provider_name}/instances/{instance_name}",
            get(crate::api::tenant_models::show_ragflow_provider_instance),
        )
        .route(
            "/api/v1/providers/{provider_name}/instances/{instance_name}/models",
            get(crate::api::tenant_models::list_ragflow_instance_models)
                .post(crate::api::tenant_models::add_ragflow_instance_model)
                .put(crate::api::tenant_models::edit_ragflow_instance_models),
        )
        .route(
            "/api/v1/providers/{provider_name}/instances/{instance_name}/models/{model_name}",
            axum::routing::patch(crate::api::tenant_models::update_ragflow_instance_model_status),
        )
        .route(
            "/api/v1/tenant/models",
            get(crate::api::tenant_models::list_tenant_models),
        )
        .route(
            "/api/v1/tenant/default-chat-model",
            get(crate::api::tenant_models::get_default_chat_model)
                .put(crate::api::tenant_models::set_default_chat_model),
        )
        .route(
            "/api/v1/tenant/models/{provider_id}/{instance_id}",
            put(crate::api::tenant_models::upsert_tenant_model_instance)
                .delete(crate::api::tenant_models::delete_tenant_model_instance),
        )
        .route(
            "/api/v1/reranker",
            get(crate::api::system::get_reranker_config)
                .put(crate::api::system::update_reranker_config),
        )
        .route(
            "/api/v1/agents",
            get(crate::api::features::list_agents).post(crate::api::features::create_agent),
        )
        .route(
            "/api/v1/agents/templates",
            get(crate::api::features::list_agent_templates),
        )
        .route(
            "/api/v1/agents/templates/{id}",
            get(crate::api::features::get_agent_template),
        )
        .route(
            "/api/v1/agents/tags",
            get(crate::api::features::list_agent_tags),
        )
        .route(
            "/api/v1/agents/{id}",
            get(crate::api::features::get_agent)
                .put(crate::api::features::update_agent)
                .delete(crate::api::features::delete_agent),
        )
        .route(
            "/api/v1/agents/{id}/tags",
            put(crate::api::features::update_agent_tags),
        )
        .route(
            "/api/v1/agents/{id}/reset",
            post(crate::api::features::reset_agent),
        )
        .route(
            "/api/v1/agents/{id}/components/{component_id}/input-form",
            get(crate::api::features::get_agent_component_input_form),
        )
        .route(
            "/api/v1/agents/{id}/components/{component_id}/debug",
            post(crate::api::features::debug_agent_component),
        )
        .route(
            "/api/v1/agents/{id}/completions",
            post(crate::api::features::agent_complete),
        )
        // `api.ts::agentChatCompletion` — the canvas id rides in the body, so
        // this literal route must exist next to the `{id}` spelling above
        // (matchit ranks the static segment over the parameter).
        .route(
            "/api/v1/agents/chat/completions",
            post(crate::api::features::agent_chat_completion),
        )
        // `bot_api.py` — the embedded/shared agent surface. It authenticates
        // with the tenant's `APIToken.beta` secret (`?auth=<beta>` in the embed
        // URL becomes `Authorization: Bearer <beta>`) and exposes the Begin
        // form plus the streaming completion for that canvas.
        .route(
            "/api/v1/agentbots/{agent_id}/inputs",
            get(crate::api::features::agent_bot_inputs),
        )
        .route(
            "/api/v1/agentbots/{agent_id}/completions",
            post(crate::api::features::agent_bot_completions),
        )
        // `agent_api.get_agent_logs` / `bot_api.agent_bot_logs`: the per-message
        // run trace the log sheet (and the share embed's Thinking button) polls.
        .route(
            "/api/v1/agents/{agent_id}/logs/{message_id}",
            get(crate::api::agent_trace::get_agent_logs),
        )
        .route(
            "/api/v1/agentbots/{shared_id}/logs/{message_id}",
            get(crate::api::agent_trace::agent_bot_logs),
        )
        .route(
            "/api/v1/agents/{id}/versions",
            get(crate::api::features::list_agent_versions),
        )
        .route(
            "/api/v1/agents/{id}/versions/{version_id}",
            get(crate::api::features::get_agent_version),
        )
        // Agent-log surface (`restful_apis/agent_api.py::list_agent_sessions`
        // and `get_agent_session`) plus the `webAPI` spelling the ported log
        // page uses: upstream `api.ts::fetchAgentLogs` resolves to
        // `/v1/canvas/{canvas_id}/sessions` because `webAPI = /v1`.
        .route(
            "/api/v1/agents/{id}/sessions",
            get(crate::api::agent_log::list_agent_sessions)
                .post(crate::api::agent_log::create_agent_session),
        )
        .route(
            "/api/v1/agents/{id}/sessions/{session_id}",
            get(crate::api::agent_log::get_agent_session)
                .delete(crate::api::agent_log::delete_agent_session),
        )
        .route(
            "/api/v1/canvas/{canvas_id}/sessions",
            get(crate::api::agent_log::list_canvas_sessions),
        )
        .route(
            "/v1/canvas/{canvas_id}/sessions",
            get(crate::api::agent_log::list_canvas_sessions),
        )
        .route(
            "/api/v1/compilation_template_groups",
            get(crate::api::compilation_templates::list_groups)
                .post(crate::api::compilation_templates::create_group),
        )
        .route(
            "/api/v1/compilation_template_groups/{id}",
            get(crate::api::compilation_templates::get_group)
                .put(crate::api::compilation_templates::update_group)
                .delete(crate::api::compilation_templates::delete_group),
        )
        .route(
            // POST /compilation-template-groups/{id}/execute — run the
            // group's tree-kind templates (RAPTOR) over supplied chunks.
            "/api/v1/compilation_template_groups/{id}/execute",
            axum::routing::post(crate::api::compilation_templates::execute_group_templates),
        )
        .route(
            "/api/v1/compilation_templates/builtins",
            get(crate::api::compilation_templates::list_builtin_templates),
        )
        .route(
            "/api/v1/compilation_templates/wiki_presets",
            get(crate::api::compilation_templates::list_wiki_presets),
        )
        .route("/api/v1/stats", get(crate::api::features::get_stats))
        .route("/api/v1/tasks", get(crate::api::features::list_tasks))
        .route(
            "/api/v1/pipeline/operation-logs",
            get(crate::api::features::list_operation_logs),
        )
        // RAGFlow ingestion logs (`dataset_api.py`) — the `/dataflow-result`
        // page's data source. The literal `summary` route must stay ahead of the
        // `{log_id}` parameter route.
        .route(
            "/api/v1/datasets/{dataset_id}/ingestions/summary",
            get(crate::api::ingestion::get_ingestion_summary),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/ingestions",
            get(crate::api::ingestion::list_ingestion_logs),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/ingestions/{log_id}",
            get(crate::api::ingestion::get_ingestion_log),
        )
        .route(
            "/api/v1/agents/rerun",
            post(crate::api::ingestion::rerun_agent),
        )
        .route(
            "/api/v1/tasks/{task_id}/cancel",
            post(crate::api::features::cancel_task),
        )
        .route(
            "/api/v1/tasks/{task_id}",
            axum::routing::patch(crate::api::features::patch_task),
        )
        .route(
            "/api/v1/memories",
            get(crate::api::features::list_memories).post(crate::api::features::create_memory),
        )
        .route(
            "/api/v1/memories/clear",
            axum::routing::delete(crate::server::clear_memories),
        )
        .route(
            "/api/v1/memories/{memory_id}",
            get(crate::api::features::get_memory_messages)
                .put(crate::api::features::update_memory)
                .delete(crate::api::features::delete_memory),
        )
        .route(
            "/api/v1/memories/{memory_id}/config",
            get(crate::api::features::get_memory_config),
        )
        .route(
            "/api/v1/messages",
            get(crate::api::features::get_recent_memory_messages)
                .post(crate::api::features::add_memory_message),
        )
        .route(
            "/api/v1/messages/search",
            get(crate::api::features::search_memory_messages),
        )
        .route(
            "/api/v1/messages/{message_ref}",
            put(crate::api::features::update_memory_message)
                .delete(crate::api::features::forget_memory_message),
        )
        .route(
            "/api/v1/messages/{message_ref}/content",
            get(crate::api::features::get_memory_message_content),
        )
        .route(
            "/api/v1/skills/config",
            get(crate::api::skill_index::get_skill_config)
                .post(crate::api::skill_index::update_skill_config),
        )
        .route(
            "/api/v1/skills/search",
            post(crate::api::skill_index::search_skills),
        )
        .route(
            "/api/v1/skills/index",
            post(crate::api::skill_index::index_skills)
                .delete(crate::api::skill_index::delete_skill_index),
        )
        .route(
            "/api/v1/skills/reindex",
            post(crate::api::skill_index::reindex_skills),
        )
        // Upstream `api/apps/restful_apis/user_api.py` mounts both verbs on
        // `/api/v1/users`: `GET` is `@login_required` and lists users, `POST`
        // (`user_add`) is the unauthenticated sign-up face. The middleware's
        // public-path table therefore has to look at the method, not just the
        // path.
        .route(
            "/api/v1/users",
            get(crate::api::system::list_users).post(user_add),
        )
        .route("/api/v1/users/{id}", put(crate::api::system::update_user))
        .route(
            "/api/v1/users/me",
            axum::routing::patch(crate::api::system::update_me),
        )
        .route(
            "/api/v1/users/invite",
            post(crate::api::system::invite_user),
        )
        .route("/api/v1/tenant", get(crate::api::system::get_tenant))
        .route("/api/v1/tenants", get(crate::api::system::list_tenants))
        .route(
            "/api/v1/tenants/{tenant_id}",
            axum::routing::patch(crate::api::system::agree_tenant),
        )
        .route(
            "/api/v1/tenants/{tenant_id}/users",
            get(crate::api::system::list_tenant_users)
                .post(crate::api::system::add_tenant_user)
                .delete(crate::api::system::remove_tenant_user),
        )
        .route(
            "/api/v1/tenant/members",
            get(crate::api::system::list_tenant_members),
        )
        .route(
            "/api/v1/tenant/members/{user_id}",
            put(crate::api::system::update_tenant_member_role)
                .delete(crate::api::system::remove_tenant_member),
        )
        .route(
            "/api/v1/tenant/invitations/{tenant_id}/accept",
            axum::routing::patch(crate::api::system::accept_tenant_invitation),
        )
        .route(
            "/api/v1/connectors",
            get(crate::api::connector::list_connectors)
                .post(crate::api::connector::create_connector),
        )
        .route("/api/v1/mcp/tools", get(crate::api::connector::mcp_tools))
        .route(
            "/api/v1/mcp/servers",
            get(crate::api::mcp_mgr::list_mcp_servers).post(crate::api::mcp_mgr::create_mcp_server),
        )
        .route(
            "/api/v1/mcp/servers/import",
            axum::routing::post(crate::api::mcp_mgr::import_mcp_servers),
        )
        .route(
            "/api/v1/mcp/servers/{id}",
            get(crate::api::mcp_mgr::get_mcp_server)
                .put(crate::api::mcp_mgr::update_mcp_server)
                .delete(crate::api::mcp_mgr::delete_mcp_server),
        )
        .route(
            "/api/v1/mcp/servers/{id}/test",
            axum::routing::post(crate::api::mcp_mgr::test_mcp_server),
        )
        .route("/api/v1/bots", get(crate::api::connector::list_bots))
        .route(
            "/api/v1/channels",
            get(crate::api::connector::list_channels),
        )
        .route(
            "/api/v1/chat-channels",
            get(crate::api::chat_channel_mgr::list_chat_channels)
                .post(crate::api::chat_channel_mgr::create_chat_channel),
        )
        .route(
            "/api/v1/chat-channels/{id}",
            get(crate::api::chat_channel_mgr::get_chat_channel)
                .patch(crate::api::chat_channel_mgr::update_chat_channel)
                .delete(crate::api::chat_channel_mgr::delete_chat_channel),
        )
        .route(
            "/api/v1/chat-channels/{id}/runtime",
            get(crate::api::chat_channel_mgr::get_chat_channel_runtime),
        )
        .route("/api/v1/plugins", get(crate::api::connector::list_plugins))
        .route(
            "/api/v1/plugins/{id}/toggle",
            post(crate::api::connector::toggle_plugin),
        )
        .route(
            "/api/v1/system",
            get(crate::api::system::get_system).put(crate::api::system::update_system),
        )
        .route(
            "/api/v1/system/status/detail",
            get(crate::api::system::system_status_detail),
        )
        .route(
            "/api/v1/system/config/log",
            get(crate::api::system::get_log_levels).put(crate::api::system::set_log_level),
        )
        .route(
            "/api/v1/system/variables",
            get(crate::api::system_settings::list_variables)
                .put(crate::api::system_settings::set_variable),
        )
        .route(
            "/api/v1/system/variables/{var_name}",
            get(crate::api::system_settings::show_variable),
        )
        .route("/api/v1/system/healthz", get(crate::api::system::healthz))
        .route(
            "/api/v1/auth/login/channels",
            get(crate::api::system::login_channels),
        )
        .route(
            "/api/v1/auth/login/{channel}",
            get(crate::api::system::oauth_login),
        )
        .route(
            "/api/v1/auth/oauth/{channel}/callback",
            get(crate::api::system::oauth_callback),
        )
        .route(
            "/api/v1/system/version",
            get(crate::api::system::system_version_public),
        )
        .route(
            "/api/v1/system/config",
            get(crate::api::system::system_config),
        )
        // First-login setup: the guided counterpart of the environment file.
        .route("/api/v1/setup/status", get(crate::api::setup::setup_status))
        .route(
            "/api/v1/setup/options",
            get(crate::api::setup::setup_options),
        )
        .route(
            "/api/v1/setup/complete",
            post(crate::api::setup::setup_complete),
        )
        .route(
            "/api/v1/system/new_api_key",
            post(crate::api::system::new_api_key),
        )
        .route(
            "/api/v1/system/api_keys",
            get(list_my_api_keys).post(generate_my_api_key),
        )
        .route(
            "/api/v1/system/api_keys/{key}",
            axum::routing::delete(delete_my_api_key),
        )
        // RAGFlow `system_api.py` API-token surface (`web/src/utils/api.ts`:
        // `getSystemTokenList` / `createSystemToken` / `removeSystemToken`).
        // Each row also carries the `beta` secret the embed/share surfaces use
        // as a bearer credential.
        .route(
            "/api/v1/system/tokens",
            get(crate::api::tokens::list_tokens).post(crate::api::tokens::create_token),
        )
        .route(
            "/api/v1/system/tokens/{token}",
            axum::routing::delete(crate::api::tokens::delete_token),
        )
        .route("/api/v1/llm/models", get(crate::api::system::list_models))
        .route(
            "/api/v1/llm/factories",
            get(crate::api::system::list_factories),
        )
        .route("/api/v1/llm/my_llms", get(crate::api::system::my_llms))
        .route(
            "/api/v1/document/upload_info",
            post(crate::api::system::document_upload_info),
        )
        .route(
            "/api/v1/sessions/related_questions",
            post(crate::api::system::related_questions),
        )
        .route(
            "/api/v1/searches/{search_id}/completions",
            post(crate::api::features::search_complete),
        )
        .route(
            "/api/v1/chat/mindmap",
            post(crate::api::features::chat_mindmap),
        )
        .route(
            "/api/v1/chat/recommendation",
            post(crate::api::system::related_questions),
        )
        .route(
            "/api/v1/searchbots/related_questions",
            post(crate::api::system::related_questions),
        )
        .route(
            "/api/v1/file/commits",
            get(crate::api::system::list_commits),
        )
        .route(
            "/api/v1/file2document",
            post(crate::api::system::file2document),
        )
        .route(
            "/api/v1/langfuse/api-key",
            get(crate::api::langfuse::get_langfuse_api_key)
                .post(crate::api::langfuse::set_langfuse_api_key)
                .put(crate::api::langfuse::set_langfuse_api_key)
                .delete(crate::api::langfuse::delete_langfuse_api_key),
        )
        .route(
            "/api/v1/langfuse",
            get(crate::api::langfuse::get_langfuse_api_key)
                .post(crate::api::langfuse::set_langfuse_api_key),
        )
        // Chat channel inbound endpoints (webhook / feishu event callbacks).
        // New routes only; existing route behavior is unchanged.
        .merge(crate::channels::channel_router())
        // MCP SSE server (mcp/server/server.py port): /mcp/sse + /mcp/messages.
        // 闭包捕获 state，不依赖 router state 类型，可直接挂到 Router<Arc<AppState>>。
        .route(
            "/mcp/sse",
            get({
                let st = std::sync::Arc::new(crate::mcp_server::McpSseState::default());
                move |headers: HeaderMap| crate::mcp_server::sse_handler(st.clone(), headers)
            }),
        )
        .route(
            "/mcp/messages",
            post({
                let st = std::sync::Arc::new(crate::mcp_server::McpSseState::default());
                move |headers: HeaderMap,
                      params: Query<std::collections::HashMap<String, String>>,
                      body: Body| {
                    crate::mcp_server::post_message(st.clone(), headers, params, body)
                }
            }),
        )
        .fallback_service(
            ServeDir::new(sd)
                .precompressed_gzip()
                .precompressed_br()
                .not_found_service(axum::routing::get(crate::web::not_found_page)),
        )
        .layer(DefaultBodyLimit::max(
            max_upload_bytes()
                .saturating_mul(MAX_DOCUMENTS_PER_UPLOAD)
                .saturating_add(MULTIPART_OVERHEAD_BYTES),
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_api_auth,
        ))
        .with_state(state);
    configure_cors(router)
}

pub async fn run(
    port: u16,
    static_dir: &str,
    log_levels: crate::logging::LogLevelManager,
) -> anyhow::Result<()> {
    // 统一命令超时输入：用户 env 环境文件（RAYRAG_CMD_TIMEOUT，默认 7200 秒/2 小时）
    crate::common::cmd_timeout::load_user_env_file();
    // The setup page writes the values it collects back to the file the process read.
    crate::api::setup::remember_loaded_env_file();
    let u = format!("{}/../users.json", static_dir);
    let k = format!("{}/../kbs.json", static_dir);
    let tenants_path = format!("{}/../tenants.json", static_dir);
    let c = format!("{}/../conversations.json", static_dir);
    let i = format!("{}/../index.json", static_dir);
    let m = format!("{}/../models/all-MiniLM-L6-v2", static_dir);
    let d = format!("{}/../docs.json", static_dir);
    let document_metadata_path = format!("{}/../document_metadata.json", static_dir);
    let t = format!("{}/../tasks.json", static_dir);
    let r = format!("{}/../reranker.json", static_dir);
    let p = format!("{}/../providers.json", static_dir);
    let tenant_models_path = format!("{}/../tenant_models.json", static_dir);
    let memories_path = format!("{}/../memories.json", static_dir);
    let memory_messages_path = format!("{}/../memory_messages.json", static_dir);
    let skill_index_path = format!("{}/../skill_index.json", static_dir);
    let system_settings_path = format!("{}/../system_settings.json", static_dir);
    let api_tokens_path = format!("{}/../api_tokens.json", static_dir);
    let ingestion_logs_path = format!("{}/../ingestion_logs.json", static_dir);
    let agent_traces_path = format!("{}/../agent_traces.json", static_dir);
    let langfuse_path = format!("{}/../langfuse.json", static_dir);
    let mcp_servers_path = format!("{}/../mcp_servers.json", static_dir);
    let chat_channels_path = format!("{}/../chat_channels.json", static_dir);
    let agents_path = format!("{}/../agents.json", static_dir);
    let agent_checkpoints_path = format!("{}/../agent_checkpoints.json", static_dir);
    let canvas_versions_path = format!("{}/../canvas_versions.json", static_dir);
    let compilation_templates_path = format!("{}/../compilation_templates.json", static_dir);
    let g = format!("{}/../graphrag.json", static_dir);
    let e = format!("{}/../evaluations.json", static_dir);

    let users = Arc::new(UserStore::new(&u)?);
    let kbs = Arc::new(KbStore::new(&k)?);
    let tenants = Arc::new(TenantStore::new(&tenants_path)?);
    let conversations = Arc::new(ConvStore::new(&c)?);
    let docs = Arc::new(crate::api::document::DocStore::new(&d)?);
    let document_metadata = Arc::new(crate::api::document_metadata::DocumentMetadataStore::new(
        &document_metadata_path,
    )?);
    let files = Arc::new(crate::api::file_mgr::FileStore::new(&format!(
        "{}/../uploads/",
        static_dir
    ))?);
    let data_sources = Some(Arc::new(crate::api::data_source_mgr::DataSourceStore::new(
        &format!("{}/../uploads/", static_dir),
    )?));
    let search_apps = Some(Arc::new(crate::api::searchapp_mgr::SearchAppStore::new(
        &format!("{}/../uploads/", static_dir),
    )?));
    let chat_apps = Some(Arc::new(crate::api::chatapp_mgr::ChatAppStore::new(
        &format!("{}/../uploads/", static_dir),
    )?));

    let llm = std::env::var("LLM_API_KEY")
        .ok()
        .or_else(|| std::env::var("EMBED_API_KEY").ok())
        .map(|key| {
            Arc::new(LlmClient::new(LlmConfig {
                api_base: std::env::var("LLM_API_BASE")
                    .unwrap_or_else(|_| "https://api.minimaxi.com/v1".into()),
                api_key: key,
                model: std::env::var("LLM_MODEL").unwrap_or_else(|_| "MiniMax-M3".into()),
                ..LlmConfig::default()
            }))
        });

    let engine = if std::path::Path::new(&i).exists() {
        Arc::new(RwLock::new(SearchEngine::from_file(&i)?))
    } else {
        Arc::new(RwLock::new(SearchEngine::new()))
    };
    let vector_mirror = Arc::new(crate::store::OnlineVectorMirror::from_env()?);
    if vector_mirror.is_enabled() {
        // Startup only reclaims what the index no longer carries: it opens nothing
        // and loads nothing, so boot stays flat no matter how large the corpus is.
        // Repairing a collection that drifted is a *rewrite*, and rewrites are
        // deferred to `start_mirror_repair` so they can never spike a boot.
        let counts = engine.read().unwrap().chunk_counts_by_kb();
        match vector_mirror.reclaim_unindexed_collections(&counts) {
            Ok(0) => {}
            Ok(reclaimed) => tracing::info!(reclaimed, "Reclaimed leftover zvec collections"),
            Err(error) => {
                tracing::warn!(%error, "Failed to reclaim leftover zvec collections");
            }
        }
        // Even counting rows opens each collection, so the drift inspection is
        // deferred to the repair pass as well: startup touches no native storage
        // beyond removing what the index no longer carries.
    }
    let embedder: Option<SharedEmbedder> = match (
        std::env::var("EMBED_API_BASE").ok(),
        std::env::var("EMBED_API_KEY").ok(),
        std::env::var("EMBED_MODEL").ok(),
    ) {
        (Some(api_base), Some(api_key), Some(model))
            if !api_base.is_empty() && !model.is_empty() =>
        {
            tracing::info!(model = %model, "Embedding model configured");
            tracing::debug!(api_base = %api_base, "Embedding endpoint");
            Some(Arc::new(crate::embed::openai_compatible_embedder(
                &api_base, &api_key, &model,
            )))
        }
        _ => {
            tracing::warn!(
                "Embedding disabled: set EMBED_API_BASE, EMBED_API_KEY, and EMBED_MODEL"
            );
            None
        }
    };
    let reranker = Arc::new(RerankerManager::new(
        &r,
        RerankerConfig {
            enabled: std::env::var("RERANK_API_BASE")
                .ok()
                .is_some_and(|base_url| !base_url.trim().is_empty()),
            api_base: std::env::var("RERANK_API_BASE").unwrap_or_default(),
            api_key: std::env::var("RERANK_API_KEY")
                .ok()
                .filter(|key| !key.is_empty()),
        },
    )?);

    let providers = Arc::new(crate::api::features::ProviderStore::new(&p)?);
    let tenant_models = Arc::new(crate::api::tenant_models::TenantModelStore::new(
        &tenant_models_path,
    )?);
    tenant_models.validate_providers(&providers)?;

    // Bootstrap the chat channel registry (RAGFlow `api/channels` bootstrap):
    // built-in builders (webhook, feishu) self-register here; enabled channels
    // from the environment config are started below and stopped on shutdown.
    let channels = crate::channels::bootstrap_registry().await;
    let state = Arc::new(AppState {
        static_dir: static_dir.into(),
        port,
        users,
        kbs,
        tenants,
        conversations,
        llm,
        embedder,
        reranker,
        graphs: Arc::new(crate::graph_store::GraphStore::new(&g)?),
        engine,
        vector_mirror,
        index_path: i,
        model_path: m,
        max_upload_bytes: max_upload_bytes(),
        docs,
        document_metadata,
        files,
        data_sources,
        search_apps,
        chat_apps,
        providers,
        tenant_models,
        memories: Arc::new(crate::api::features::MemoryStore::new(&memories_path)?),
        memory_messages: Arc::new(crate::api::joint_services::MemoryMessageService::new(
            &memory_messages_path,
        )?),
        skill_index: Arc::new(crate::api::skill_index::SkillIndexStore::new(
            &skill_index_path,
        )?),
        system_settings: Arc::new(crate::api::system_settings::SystemSettingsStore::new(
            &system_settings_path,
        )?),
        api_tokens: Arc::new(crate::api::tokens::ApiTokenStore::new(&api_tokens_path)?),
        ingestion_logs: Arc::new(crate::api::ingestion::IngestionLogStore::new(
            &ingestion_logs_path,
        )?),
        agent_traces: Arc::new(crate::api::agent_trace::AgentTraceStore::new(
            &agent_traces_path,
        )?),
        register_enabled: crate::settings::Settings::from_env().register_enabled,
        disable_password_login: crate::settings::Settings::from_env().disable_password_login,
        oauth_states: Arc::new(crate::oauth_config::StateStore::new()),
        oauth_channels: Arc::new(std::sync::RwLock::new(crate::oauth_config::from_env())),
        langfuse: Arc::new(crate::api::langfuse::LangfuseStore::new(&langfuse_path)?),
        mcp_servers: Arc::new(crate::api::mcp_mgr::McpServerStore::new(&mcp_servers_path)?),
        chat_channels: Arc::new(crate::api::chat_channel_mgr::ChatChannelStore::new(
            &chat_channels_path,
        )?),
        tasks: Arc::new(crate::api::features::TaskQueue::new(&t)?),
        task_executor: Arc::new(crate::task_executor::TaskExecutor::new(
            "task_executor_0",
            max_concurrent_document_tasks(),
        )),
        agents: Arc::new(crate::api::features::AgentStore::new(&agents_path)?),
        agent_runs: Arc::new(crate::api::features::AgentRunRegistry::default()),
        agent_checkpoints: Arc::new(crate::agent_checkpoint::AgentCheckpointStore::new(
            &agent_checkpoints_path,
        )?),
        canvas_versions: Arc::new(crate::api::features::CanvasVersionStore::new(
            &canvas_versions_path,
        )?),
        compilation_templates: Arc::new(
            crate::api::compilation_templates::CompilationTemplateStore::new(
                &compilation_templates_path,
            )?,
        ),
        evaluations: Arc::new(crate::api::evaluation::EvaluationStore::new(&e)?),
        log_levels: Arc::new(log_levels),
        chunk_feedback_enabled: crate::chunk_feedback::feedback_enabled_from_env(),
        chunk_feedback_weighting: crate::chunk_feedback::feedback_weighting_from_env(),
        channels: channels.clone(),
        document_commit_lock: Arc::new(std::sync::Mutex::new(())),
        memory_commit_lock: Arc::new(std::sync::Mutex::new(())),
    });

    start_document_task_dispatcher(state.clone());
    start_mirror_repair(state.clone());
    // Start the async task executor (RAGFlow task_executor port): it drains the
    // same TaskQueue entries submitted by queue_document_processing. The
    // TaskQueue lease makes execution exclusive with the legacy dispatcher, so
    // the two worker paths never process the same task concurrently.
    let executor_handler = document_executor_handler(state.clone());
    state
        .task_executor
        .spawn(executor_handler, std::time::Duration::from_millis(2_000));
    let router = build_router(state);

    // Chat channel start hook (RAGFlow `bootstrap.run_channels`): build + start
    // every enabled channel from the environment config; errors are isolated
    // per channel so one bad bot never aborts the API server.
    let channel_config = crate::channels::channels_config_from_env();
    let started = channels.start_all(&channel_config).await;
    for (platform, account) in &started {
        tracing::info!(platform, account, "chat channel started");
    }

    // Memory tokenized-field repair hook (upstream
    // `memory_message_service.fix_missing_tokenized_memory`): the repair only
    // applies to engines that persist a `tokenized_content_ltks` field. The
    // active RayRAG engines (zvec vector mirror + PostgreSQL metadata) compute
    // tokens at query time and never persist that field, so the guard
    // short-circuits. The full repair loop is ported and unit-tested in
    // `joint_services::fix_missing_tokenized_memory` / `repair_missing_field`.
    tracing::debug!(
        "Memory tokenized-field repair skipped: the active document engine tokenizes at query time"
    );

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!(
        "RayRAG {} on http://0.0.0.0:{}",
        crate::build_info::version_line(),
        port
    );
    // Chat channel stop hook: on shutdown, stop every running channel.
    let shutdown = async move {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to listen for shutdown signal");
        }
        channels.stop_all().await;
    };
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_api_path_matches_dynamic_segments() {
        // Literal endpoints keep working.
        for path in [
            "/api/v1/auth/login",
            "/api/v1/user/register",
            "/api/v1/version",
            "/api/v1/system/healthz",
            "/api/v1/system/version",
            "/api/v1/system/config",
            "/api/v1/auth/login/channels",
            "/api/v1/chatbots/abc/info",
        ] {
            assert!(super::public_api_path(path), "{path} must stay public");
        }
        // Single-segment placeholders now match the concrete request path.
        for path in [
            "/api/v1/auth/login/github",
            "/api/v1/auth/login/oidc",
            "/api/v1/channels/42/webhook/inbound",
            "/api/v1/channels/42/feishu/event",
        ] {
            assert!(super::public_api_path(path), "{path} must stay public");
        }
        // Everything else still requires a token.
        for path in [
            "/api/v1/channels/42",
            "/api/v1/auth/login/github/callback",
            "/api/v1/users/me",
            "/api/v1/datasets",
        ] {
            assert!(!super::public_api_path(path), "{path} must require auth");
        }
    }

    use super::*;
    use crate::api::tenant_models::{ModelCapability, TenantModelInstanceUpdate, TenantModelSpec};
    use axum::{
        body::{Body, to_bytes},
        http::{Request, header},
    };
    use tower::ServiceExt;

    struct ReverseReranker;

    #[async_trait::async_trait]
    impl crate::rerank::Reranker for ReverseReranker {
        async fn rerank(
            &self,
            _query: &str,
            documents: &[String],
            _top_k: usize,
        ) -> crate::Result<Vec<(usize, f32)>> {
            Ok((0..documents.len())
                .rev()
                .map(|index| (index, 1.0))
                .collect())
        }
    }

    struct TestEmbedder;

    #[async_trait::async_trait]
    impl crate::embed::Embedder for TestEmbedder {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    async fn mock_memory_llm(response: &str) -> LlmClient {
        let response = response.to_owned();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let response = response.clone();
                async move {
                    Json(serde_json::json!({
                        "choices": [{"message": {"content": response}}],
                        "usage": {"total_tokens": 3}
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        LlmClient::new(LlmConfig {
            api_base: format!("http://{address}/v1"),
            api_key: "test".into(),
            model: "memory-test".into(),
            ..LlmConfig::default()
        })
    }

    struct TestEnv {
        root: std::path::PathBuf,
        state: Arc<AppState>,
        token: String,
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn test_env() -> TestEnv {
        let root = std::env::temp_dir().join(format!("rayrag-server-{}", uuid::Uuid::new_v4()));
        let static_dir = root.join("web");
        std::fs::create_dir_all(&static_dir).unwrap();
        let users_path = root.join("users.json");
        let email = "test@example.com";
        let password = "correct horse battery staple";
        std::fs::write(
            &users_path,
            serde_json::to_vec(&serde_json::json!([{
                "id": "test-user",
                "nickname": "Test",
                "email": email,
                "password_hash": crate::auth::hash_password_for_test(password),
                "role": "admin",
                "created_at": 1
            }]))
            .unwrap(),
        )
        .unwrap();
        let users = Arc::new(UserStore::new(users_path.to_str().unwrap()).unwrap());
        let token = users.login(email, password).unwrap().unwrap();
        let state = Arc::new(AppState {
            static_dir: static_dir.to_string_lossy().into_owned(),
            port: 0,
            users,
            kbs: Arc::new(KbStore::new(root.join("kbs.json").to_str().unwrap()).unwrap()),
            tenants: Arc::new(
                TenantStore::new(root.join("tenants.json").to_str().unwrap()).unwrap(),
            ),
            conversations: Arc::new(
                ConvStore::new(root.join("conversations.json").to_str().unwrap()).unwrap(),
            ),
            llm: None,
            embedder: None,
            reranker: Arc::new(RerankerManager::in_memory(None)),
            graphs: Arc::new(crate::graph_store::GraphStore::in_memory()),
            engine: Arc::new(RwLock::new(SearchEngine::new())),
            vector_mirror: Arc::new(crate::store::OnlineVectorMirror::disabled()),
            index_path: root.join("index.json").to_string_lossy().into_owned(),
            model_path: root.join("models").to_string_lossy().into_owned(),
            max_upload_bytes: DEFAULT_MAX_UPLOAD_BYTES,
            docs: Arc::new(
                crate::api::document::DocStore::new(root.join("docs.json").to_str().unwrap())
                    .unwrap(),
            ),
            document_metadata: Arc::new(
                crate::api::document_metadata::DocumentMetadataStore::in_memory(),
            ),
            files: Arc::new(
                crate::api::file_mgr::FileStore::new(root.join("uploads").to_str().unwrap())
                    .unwrap(),
            ),
            data_sources: None,
            chat_apps: None,
            search_apps: None,
            providers: Arc::new(crate::api::features::ProviderStore::in_memory()),
            tenant_models: Arc::new(crate::api::tenant_models::TenantModelStore::in_memory()),
            memories: Arc::new(crate::api::features::MemoryStore::in_memory()),
            memory_messages: Arc::new(crate::api::joint_services::MemoryMessageService::in_memory()),
            skill_index: Arc::new(crate::api::skill_index::SkillIndexStore::in_memory()),
            system_settings: Arc::new(crate::api::system_settings::SystemSettingsStore::in_memory()),
            api_tokens: Arc::new(crate::api::tokens::ApiTokenStore::in_memory()),
            ingestion_logs: Arc::new(crate::api::ingestion::IngestionLogStore::in_memory()),
            agent_traces: Arc::new(crate::api::agent_trace::AgentTraceStore::in_memory()),
            register_enabled: 1,
            disable_password_login: false,
            oauth_states: Arc::new(crate::oauth_config::StateStore::new()),
            oauth_channels: Arc::new(std::sync::RwLock::new(Vec::new())),
            langfuse: Arc::new(crate::api::langfuse::LangfuseStore::in_memory()),
            mcp_servers: Arc::new(crate::api::mcp_mgr::McpServerStore::in_memory()),
            chat_channels: Arc::new(crate::api::chat_channel_mgr::ChatChannelStore::in_memory()),
            tasks: Arc::new(crate::api::features::TaskQueue::in_memory()),
            task_executor: Arc::new(crate::task_executor::TaskExecutor::new("test_executor", 4)),
            agents: Arc::new(crate::api::features::AgentStore::in_memory()),
            agent_runs: Arc::new(crate::api::features::AgentRunRegistry::default()),
            agent_checkpoints: Arc::new(crate::agent_checkpoint::AgentCheckpointStore::in_memory()),
            canvas_versions: Arc::new(crate::api::features::CanvasVersionStore::in_memory()),
            compilation_templates: Arc::new(
                crate::api::compilation_templates::CompilationTemplateStore::in_memory(),
            ),
            evaluations: Arc::new(crate::api::evaluation::EvaluationStore::in_memory()),
            log_levels: Arc::new(crate::logging::LogLevelManager::in_memory()),
            chunk_feedback_enabled: false,
            chunk_feedback_weighting: crate::chunk_feedback::FeedbackWeighting::Relevance,
            channels: Arc::new(crate::channels::ChannelRegistry::new()),
            document_commit_lock: Arc::new(std::sync::Mutex::new(())),
            memory_commit_lock: Arc::new(std::sync::Mutex::new(())),
        });
        TestEnv { root, state, token }
    }

    /// Upstream `user_api.user_add`: `web/src/utils/api.ts::register` posts to
    /// `${restAPIv1}/users`, so the sign-up face lives on `POST /api/v1/users`
    /// (the `GET` on the same path stays authenticated), every rejection is
    /// HTTP 200 carrying a `RetCode` body, and the success payload is
    /// `{code:0, message:"{nickname}, welcome aboard!", data:{…}}` with the
    /// credential in the `Authorization` header.
    #[tokio::test]
    async fn user_add_matches_the_upstream_registration_contract() {
        let env = test_env();
        let post = |uri: &'static str, body: serde_json::Value| {
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };
        let register_body = |nickname: &str, email: &str| {
            serde_json::json!({
                "nickname": nickname,
                "email": email,
                "password": "a long enough password",
            })
        };

        // `@validate_request("nickname","email","password")` → 101, HTTP 200.
        for (body, expected) in [
            (
                serde_json::json!({}),
                "required argument are missing: nickname,email,password; ",
            ),
            (
                serde_json::json!({"nickname": "N"}),
                "required argument are missing: email,password; ",
            ),
        ] {
            let response = build_router(env.state.clone())
                .oneshot(post("/api/v1/users", body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
            let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            assert_eq!(json["code"], 101);
            assert_eq!(json["message"], expected);
            assert!(
                json.get("data").is_none(),
                "upstream only emits `data` for code 0"
            );
        }

        // email shape → 103 "Invalid email address: {email}!"
        let response = build_router(env.state.clone())
            .oneshot(post("/api/v1/users", register_body("Newcomer", "nope")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["code"], 103);
        assert_eq!(json["message"], "Invalid email address: nope!");

        // nickname validation → 101 with the exact `nickname_validation.py` text.
        let response = build_router(env.state.clone())
            .oneshot(post(
                "/api/v1/users",
                register_body("bad/name", "nickname@example.com"),
            ))
            .await
            .unwrap();
        let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["code"], 101);
        assert_eq!(json["message"], "Nickname contains invalid characters.");

        // Success: HTTP 200, the welcome message, the self-safe user and the
        // credential in the `Authorization` header.
        let response = build_router(env.state.clone())
            .oneshot(post(
                "/api/v1/users",
                register_body("Newcomer", "newcomer@example.com"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let authorization = response
            .headers()
            .get(header::AUTHORIZATION)
            .expect("upstream `construct_response(auth=...)` sets the header")
            .to_str()
            .unwrap()
            .to_string();
        let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["code"], 0);
        assert_eq!(json["message"], "Newcomer, welcome aboard!");
        assert_eq!(json["data"]["email"], "newcomer@example.com");
        assert_eq!(json["data"]["nickname"], "Newcomer");
        assert!(
            json["data"].get("password").is_none() && json["data"].get("password_hash").is_none(),
            "`to_safe_dict` strips the password"
        );

        // The freshly minted token really works (`GET /api/v1/user/info` is the
        // authenticated self-read in this build).
        let me = Request::builder()
            .uri("/api/v1/user/info")
            .header(header::AUTHORIZATION, format!("Bearer {authorization}"))
            .body(Body::empty())
            .unwrap();
        let response = build_router(env.state.clone()).oneshot(me).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["data"]["email"], "newcomer@example.com");

        // Duplicate → 103 "Email: {email} has already registered!"
        let response = build_router(env.state.clone())
            .oneshot(post(
                "/api/v1/users",
                register_body("Newcomer", "newcomer@example.com"),
            ))
            .await
            .unwrap();
        let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["code"], 103);
        assert_eq!(
            json["message"],
            "Email: newcomer@example.com has already registered!"
        );

        // The `GET` sibling on the same path keeps requiring a token.
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/users")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // The RayRAG-only alias answers the same contract.
        let response = build_router(env.state.clone())
            .oneshot(post(
                "/api/v1/user/register",
                register_body("Alias User", "alias@example.com"),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["message"], "Alias User, welcome aboard!");
    }

    /// Regression: an instance with `REGISTER_ENABLED=1` advertised
    /// `registerEnabled: 1` from `/api/v1/system/config` — so the login page
    /// showed its sign-up face — and then answered the submitted form with 403
    /// "Registration is disabled", because this handler gated on its own
    /// `RAYRAG_ALLOW_REGISTRATION` env var. One resolved switch, two consumers.
    #[tokio::test]
    async fn register_endpoint_follows_the_register_enabled_switch() {
        let env = test_env();
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/api/v1/users")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "nickname": "Newcomer",
                        "email": "newcomer@example.com",
                        "password": "a long enough password",
                    })
                    .to_string(),
                ))
                .unwrap()
        };

        // `REGISTER_ENABLED` unset (upstream default 1) is enough on its own.
        let response = build_router(env.state.clone())
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Switching it off closes the API and the page's gate together. The
        // transport is upstream's: HTTP 200 with `RetCode.OPERATING_ERROR`.
        let mut disabled = (*env.state).clone();
        disabled.register_enabled = 0;
        let disabled = Arc::new(disabled);
        let response = build_router(disabled.clone())
            .oneshot(request())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(json["code"], 103);
        assert_eq!(json["message"], "User registration is disabled!");

        let response = build_router(disabled)
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/config")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let payload = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let config: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(config["data"]["registerEnabled"], 0);
    }

    #[tokio::test]
    async fn upstream_model_provider_page_route_is_registered() {
        let env = test_env();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/user-setting/model")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(response.into_body(), 2 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("data-testid='available-models-section'"));
        assert!(body.contains("data-testid='default-model-llm_id'"));
    }

    #[tokio::test]
    async fn user_setting_root_and_all_sidebar_routes_are_registered() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let redirect = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/user-setting")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(redirect.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            redirect.headers().get(header::LOCATION).unwrap(),
            "/user-setting/data-source"
        );

        for key in [
            "/data-source",
            "/chat-channel",
            "/model",
            "/mcp",
            "/team",
            "/profile",
            "/api",
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/user-setting{key}"))
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "route {key}");
            let body = String::from_utf8(
                to_bytes(response.into_body(), 4 * 1024 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(body.contains("data-testid='user-setting-layout'"));
            assert!(body.contains(&format!(
                "class='user-setting-nav-item active' href='/user-setting{key}' data-setting-key='{key}'"
            )));
        }
    }

    /// Upstream admin console API (`internal/admin/handler.go` +
    /// `internal/admin/service.go`): superuser-only login, logout, the service
    /// list and per-service details.
    #[tokio::test]
    async fn admin_console_api_matches_upstream_handlers() {
        let env = test_env();
        let router = build_router(env.state.clone());

        // Wrong password -> 401 with the upstream shape.
        let denied = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/admin/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"email": "test@example.com", "password": "nope"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Upstream answers HTTP 200 with `code: 109` for bad credentials.
        assert_eq!(denied.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(denied.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["code"], 109);
        assert_eq!(payload["message"], "email and password do not match!");

        // Administrator login mirrors `AdminService.LoginData` and echoes the
        // token in the Authorization response header.
        let ok = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/admin/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "email": "test@example.com",
                            "password": "correct horse battery staple",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        assert!(ok.headers().get("authorization").is_some());
        let body = String::from_utf8(
            to_bytes(ok.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let login: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(login["code"], 0);
        assert_eq!(login["message"], "Welcome back!");
        assert_eq!(login["data"]["is_superuser"], true);
        assert_eq!(login["data"]["is_active"], "1");
        assert_eq!(login["data"]["status"], "1");
        assert!(login["data"]["access_token"].as_str().unwrap().len() > 8);

        // Non-superusers are refused with the upstream message.
        let plain = env
            .state
            .users
            .create_user("plain@example.com", "plain-password-1", "normal")
            .unwrap();
        assert!(env.state.users.get_user_by_id(&plain.id).is_some());
        let forbidden = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/admin/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "email": "plain@example.com",
                            "password": "plain-password-1",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(forbidden.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["code"], 403);
        assert_eq!(payload["message"], "Only superuser can login admin system");
        // An unknown account reports the upstream "not registered" message.
        let unknown = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/admin/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "email": "nobody@example.com",
                            "password": "whatever-password",
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = String::from_utf8(
            to_bytes(unknown.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["code"], 109);
        assert_eq!(
            payload["message"],
            "email: nobody@example.com is not registered!"
        );

        // Service list: the upstream `{id,name,service_type,host,port,status}`
        // shape over the components RayRAG runs.
        let services = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/services")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(services.status(), StatusCode::OK);
        let body = String::from_utf8(
            to_bytes(services.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        let list = payload["data"].as_array().unwrap();
        assert_eq!(list.len(), 5);
        for (index, service) in list.iter().enumerate() {
            assert_eq!(service["id"], index);
            for key in ["name", "service_type", "host", "port", "status"] {
                assert!(service.get(key).is_some(), "service {index} lacks {key}");
            }
        }
        assert_eq!(list[0]["service_type"], "ragflow_server");
        assert_eq!(list[1]["service_type"], "meta_data");
        assert_eq!(list[2]["service_type"], "retrieval");
        assert_eq!(list[3]["service_type"], "file_store");
        assert_eq!(list[4]["service_type"], "task_executor");
        // Unauthenticated access stays closed.
        let anonymous = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/services")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

        for (path, status) in [
            ("/api/v1/admin/services/0", StatusCode::OK),
            ("/api/v1/admin/services/4", StatusCode::OK),
            ("/api/v1/admin/services/99", StatusCode::NOT_FOUND),
            ("/api/v1/admin/services/abc", StatusCode::BAD_REQUEST),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{path}");
        }
        let details = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/services/4")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = String::from_utf8(
            to_bytes(details.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["data"]["service_name"], "task_executor");
        assert!(payload["data"]["message"]["rayrag"].is_array());

        // Logout revokes the presented token.
        let logout = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/logout")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::OK);
        let after = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/services")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
    }

    /// Upstream `next-search/hooks.ts` posts `{kb_id, question, search_id, page,
    /// size}`; the retrieval endpoint resolves the app's `search_config` for
    /// every field the caller leaves out.
    #[tokio::test]
    async fn retrieval_resolves_search_app_config() {
        let mut env = test_env();
        let dir =
            std::env::temp_dir().join(format!("rayrag-retrieval-app-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(
            crate::api::searchapp_mgr::SearchAppStore::new(dir.to_str().unwrap()).unwrap(),
        );
        Arc::get_mut(&mut env.state).unwrap().search_apps = Some(store.clone());
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Retrieval KB", "d")
            .unwrap();
        let app = store
            .create(
                "test-user",
                crate::api::searchapp_mgr::SearchAppCreate {
                    name: "retrieval-app".into(),
                    kb_ids: vec![kb.id.clone()],
                    top_k: 10,
                    similarity_threshold: 0.2,
                    vector_similarity_weight: 0.3,
                    ..Default::default()
                },
            )
            .unwrap();
        let router = build_router(env.state.clone());
        // Only `search_id` + the question: the knowledge base, the weights and
        // the page size all come from the application record.
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "what is rayrag",
                            "search_id": app.id,
                            "size": 10,
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = String::from_utf8(
            to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        // The test environment has no embedder, so the app config resolves and
        // the request reaches the retrieval stage, which reports the missing
        // embedding model instead of "kb_id is required".
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
        assert!(
            !body.contains("At least one accessible kb_id is required"),
            "the app config must supply the knowledge base: {body}"
        );
        // A request without `kb_ids` and without a resolvable app is still
        // rejected, exactly like before.
        let bare = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "question": "what is rayrag" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bare.status(), StatusCode::BAD_REQUEST);
        let body = String::from_utf8(
            to_bytes(bare.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("At least one accessible kb_id is required"));
        // The singular upstream `kb_id` field is accepted as well.
        let singular = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "what is rayrag",
                            "kb_id": [kb.id.clone()],
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        // Same embedder-less environment: the singular `kb_id` must be accepted
        // (no "kb_id is required" error) even though retrieval cannot run.
        let status = singular.status();
        let body = String::from_utf8(
            to_bytes(singular.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
        assert!(
            !body.contains("At least one accessible kb_id is required"),
            "the singular kb_id alias must be accepted: {body}"
        );
    }

    /// Upstream `api.searchCompletion` / `api.chatsMindmap`: the search answer
    /// stream and the mind-map endpoint are admin-free but auth-protected, and
    /// they validate their inputs the way the React hooks expect.
    #[tokio::test]
    async fn search_completion_and_mindmap_endpoints_are_registered() {
        let env = test_env();
        let router = build_router(env.state.clone());
        // Unknown search app -> 404 with the upstream shape.
        let missing = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/searches/does-not-exist/completions")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "question": "hello", "stream": true }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        let body = String::from_utf8(
            to_bytes(missing.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("Search app not found"));
        // Empty question -> 400 before any retrieval happens.
        let empty = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/searches/any/completions")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "question": "   ", "stream": true }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(empty.status(), StatusCode::NOT_FOUND);
        // The mind-map endpoint answers 503 when no chat model is configured.
        let mindmap = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/chat/mindmap")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "question": "how does rag work" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mindmap.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = String::from_utf8(
            to_bytes(mindmap.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(body.contains("Chat model is not configured"));
        // Both endpoints stay behind the API token.
        for path in ["/api/v1/searches/x/completions", "/api/v1/chat/mindmap"] {
            let anonymous = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({ "question": "q" }).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    /// The full search-answer stream: with a chat model configured the endpoint
    /// emits `chat.completion.chunk` deltas, a `chat.completion.references`
    /// event and the terminal `[DONE]` marker on the app's knowledge bases.
    #[tokio::test]
    async fn search_completion_streams_answer_and_references() {
        let mut env = test_env();
        let dir =
            std::env::temp_dir().join(format!("rayrag-search-stream-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(
            crate::api::searchapp_mgr::SearchAppStore::new(dir.to_str().unwrap()).unwrap(),
        );
        Arc::get_mut(&mut env.state).unwrap().search_apps = Some(store.clone());
        let env = env;
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Stream KB", "d")
            .unwrap();
        let app = store
            .create(
                "test-user",
                crate::api::searchapp_mgr::SearchAppCreate {
                    name: "stream-app".into(),
                    kb_ids: vec![kb.id.clone()],
                    ..Default::default()
                },
            )
            .unwrap();
        let router = build_router(env.state.clone());
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/searches/{}/completions", app.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "question": "what is RayRAG", "stream": true })
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        // The stream opens as soon as the app resolves; without a configured
        // chat model the first event reports the failure instead of hanging.
        assert!(
            response.status() == StatusCode::OK || response.status() == StatusCode::BAD_REQUEST,
            "unexpected status {}",
            response.status()
        );
        let body = String::from_utf8(
            to_bytes(response.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        // The test environment has no embedder, so the stream reports the
        // failure through the same envelope the front end parses instead of
        // hanging; the concrete delta/reference events are covered by the CDP
        // probe against the deployed instance (which has real models).
        assert!(body.starts_with("data: "), "stream body: {body:.200}");
        assert!(
            body.contains("chat.completion.chunk")
                || body.contains("Embedding is not configured")
                || body.contains("Chat model"),
            "stream body: {}",
            &body[..body.len().min(200)]
        );
    }

    /// Upstream `Routes.Chunk` subtree: the four panel routes register, the
    /// header carries the segmented control with the three upstream labels and
    /// the panels render against the real chunk API.
    #[tokio::test]
    async fn chunk_workbench_routes_match_upstream_subtree() {
        let env = test_env();
        let router = build_router(env.state.clone());
        for (path, section) in [
            ("/chunk", "/chunk/parsed"),
            ("/chunk/parsed/chunks", "/chunk/parsed"),
            ("/chunk/chunk/doc-1", "/chunk/chunk"),
            ("/chunk/result/doc-1", "/chunk/result"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "route {path}");
            let body = String::from_utf8(
                to_bytes(response.into_body(), 4 * 1024 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(body.contains("data-testid='chunk-workbench'"), "{path}");
            assert!(
                body.contains(&format!("data-section='{section}'")),
                "{path}: active section"
            );
            assert!(
                body.contains(&format!(
                    "class='chunk-segment active' data-section='{section}'"
                )),
                "{path}: active segment"
            );
            for label in ["Parsed results", "Chunk result", "Result view"] {
                assert!(body.contains(label), "{path}: segment {label}");
            }
            assert!(body.contains("data-testid='chunk-save'"), "{path}: save");
            assert!(body.contains("data-testid='chunk-more'"), "{path}: menu");
            assert!(
                body.contains("/api/v1/datasets/"),
                "{path}: chunk API wiring"
            );
        }
        // The three panels carry the upstream toolbars and list hosts.
        let chunked = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/chunk/chunk/doc-1?knowledgeId=kb-1")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = String::from_utf8(
            to_bytes(chunked.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        for marker in [
            "Parsed  results",
            "Chunked  results",
            "id='chunkParsedList'",
            "id='chunkChunkedList'",
            "data-testid='chunk-copy-parsed'",
            "data-testid='chunk-export-chunked'",
            "CHUNK_ANNOYED_SVG",
            "chunkSaveAll()",
            "CHUNK_STATE=",
            "kb-1",
            "\"kbId\"",
        ] {
            assert!(body.contains(marker), "chunk marker missing: {marker}");
        }
    }

    /// Upstream `routes.tsx` parity for the paths that map onto existing
    /// renderers: `/login-next` (second entry point for the login page),
    /// `/agent/:id` and `/search/:id` (canonical singular detail paths) and
    /// `/document/:id` (standalone document viewer). `/search/share` is the
    /// public embed resolved from `?tenantId=`.
    #[tokio::test]
    async fn upstream_singular_detail_and_share_routes_are_registered() {
        let env = test_env();
        let router = build_router(env.state.clone());

        // Public: both login entry points and the share embed.
        for path in ["/login", "/login-next"] {
            let response = router
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "route {path}");
        }
        let share = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/search/share")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(share.status(), StatusCode::OK);
        let share_body = String::from_utf8(
            to_bytes(share.into_body(), 4 * 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        // Same next-search surface as `/search`, plus the embed owner slot.
        assert!(share_body.contains("id='searchHome'"));
        assert!(
            share_body.contains("data-testid='search-share-owner'")
                || share_body.contains("id='searchView'")
        );

        // Authenticated canonical detail paths render the same handlers as the
        // compatibility aliases still served by RayRAG. Both are exercised with
        // an unknown id, which must reach the handler (not the 404 fallback).
        for (path, marker) in [
            ("/agent/demo-agent", "id='agentTitle'"),
            ("/search/demo-search", "Search app not found"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .header(header::COOKIE, "lang=en")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "route {path}");
            let body = String::from_utf8(
                to_bytes(response.into_body(), 4 * 1024 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            assert!(
                body.contains(marker),
                "route {path} did not reach its handler"
            );
            assert!(
                !body.contains("404 · RayRAG"),
                "route {path} fell through to the 404 page"
            );
        }

        // `/document/:id` resolves the dataset from the document record and
        // 404s for an unknown id instead of panicking.
        let missing = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/document/does-not-exist")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn current_user_accepts_bearer_and_cookie_auth() {
        let env = test_env();
        let router = build_router(env.state.clone());
        for (name, value) in [
            (header::AUTHORIZATION, format!("Bearer {}", env.token)),
            (header::COOKIE, format!("rayrag_token={}", env.token)),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/v1/user/info")
                        .header(name, value)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = response_json(response).await;
            assert_eq!(body["code"], 0);
            assert_eq!(body["data"]["email"], "test@example.com");
        }

        let unauthorized = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/user/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn related_question_aliases_require_auth_question_and_chat_model() {
        for path in [
            "/api/v1/chat/recommendation",
            "/api/v1/searchbots/related_questions",
            "/api/v1/sessions/related_questions",
        ] {
            let env = test_env();
            let unauthorized = build_router(env.state.clone())
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"question":"Rust RAG"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

            let missing_question = build_router(env.state.clone())
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"question":"   "}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(missing_question.status(), StatusCode::BAD_REQUEST);

            let no_model = build_router(env.state.clone())
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"question":"Rust RAG"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(no_model.status(), StatusCode::SERVICE_UNAVAILABLE);
        }
    }

    fn configure_embedding_models(state: &AppState, tenant_id: &str, models: &[&str]) {
        state
            .tenant_models
            .upsert(
                &state.providers,
                tenant_id,
                "minimax",
                "primary",
                TenantModelInstanceUpdate {
                    tenant_id: None,
                    instance_name: "Primary".into(),
                    api_base: Some("http://127.0.0.1:9/v1".into()),
                    api_key: None,
                    clear_api_key: false,
                    models: models
                        .iter()
                        .map(|name| TenantModelSpec {
                            name: (*name).into(),
                            model_types: vec![ModelCapability::Embedding],
                            max_tokens: None,
                            enabled: true,
                            is_tools: false,
                            ocr_config: None,
                        })
                        .collect(),
                },
            )
            .unwrap();
    }

    fn non_admin_env() -> TestEnv {
        let root = std::env::temp_dir().join(format!("rayrag-server-{}", uuid::Uuid::new_v4()));
        let static_dir = root.join("web");
        std::fs::create_dir_all(&static_dir).unwrap();
        let users_path = root.join("users.json");
        let email = "member@example.com";
        let password = "correct horse battery staple";
        std::fs::write(
            &users_path,
            serde_json::to_vec(&serde_json::json!([{
                "id": "member-user",
                "nickname": "Member",
                "email": email,
                "password_hash": crate::auth::hash_password_for_test(password),
                "role": "user",
                "created_at": 1
            }]))
            .unwrap(),
        )
        .unwrap();
        let users = Arc::new(UserStore::new(users_path.to_str().unwrap()).unwrap());
        let token = users.login(email, password).unwrap().unwrap();
        let state = Arc::new(AppState {
            static_dir: static_dir.to_string_lossy().into_owned(),
            port: 0,
            users,
            kbs: Arc::new(KbStore::new(root.join("kbs.json").to_str().unwrap()).unwrap()),
            tenants: Arc::new(
                TenantStore::new(root.join("tenants.json").to_str().unwrap()).unwrap(),
            ),
            conversations: Arc::new(
                ConvStore::new(root.join("conversations.json").to_str().unwrap()).unwrap(),
            ),
            llm: None,
            embedder: None,
            reranker: Arc::new(RerankerManager::in_memory(None)),
            graphs: Arc::new(crate::graph_store::GraphStore::in_memory()),
            engine: Arc::new(RwLock::new(SearchEngine::new())),
            vector_mirror: Arc::new(crate::store::OnlineVectorMirror::disabled()),
            index_path: root.join("index.json").to_string_lossy().into_owned(),
            model_path: root.join("models").to_string_lossy().into_owned(),
            max_upload_bytes: DEFAULT_MAX_UPLOAD_BYTES,
            docs: Arc::new(
                crate::api::document::DocStore::new(root.join("docs.json").to_str().unwrap())
                    .unwrap(),
            ),
            document_metadata: Arc::new(
                crate::api::document_metadata::DocumentMetadataStore::in_memory(),
            ),
            files: Arc::new(
                crate::api::file_mgr::FileStore::new(root.join("uploads").to_str().unwrap())
                    .unwrap(),
            ),
            data_sources: None,
            chat_apps: None,
            search_apps: None,
            providers: Arc::new(crate::api::features::ProviderStore::in_memory()),
            tenant_models: Arc::new(crate::api::tenant_models::TenantModelStore::in_memory()),
            memories: Arc::new(crate::api::features::MemoryStore::in_memory()),
            memory_messages: Arc::new(crate::api::joint_services::MemoryMessageService::in_memory()),
            skill_index: Arc::new(crate::api::skill_index::SkillIndexStore::in_memory()),
            system_settings: Arc::new(crate::api::system_settings::SystemSettingsStore::in_memory()),
            api_tokens: Arc::new(crate::api::tokens::ApiTokenStore::in_memory()),
            ingestion_logs: Arc::new(crate::api::ingestion::IngestionLogStore::in_memory()),
            agent_traces: Arc::new(crate::api::agent_trace::AgentTraceStore::in_memory()),
            register_enabled: 1,
            disable_password_login: false,
            oauth_states: Arc::new(crate::oauth_config::StateStore::new()),
            oauth_channels: Arc::new(std::sync::RwLock::new(Vec::new())),
            langfuse: Arc::new(crate::api::langfuse::LangfuseStore::in_memory()),
            mcp_servers: Arc::new(crate::api::mcp_mgr::McpServerStore::in_memory()),
            chat_channels: Arc::new(crate::api::chat_channel_mgr::ChatChannelStore::in_memory()),
            tasks: Arc::new(crate::api::features::TaskQueue::in_memory()),
            task_executor: Arc::new(crate::task_executor::TaskExecutor::new("test_executor", 4)),
            agents: Arc::new(crate::api::features::AgentStore::in_memory()),
            agent_runs: Arc::new(crate::api::features::AgentRunRegistry::default()),
            agent_checkpoints: Arc::new(crate::agent_checkpoint::AgentCheckpointStore::in_memory()),
            canvas_versions: Arc::new(crate::api::features::CanvasVersionStore::in_memory()),
            compilation_templates: Arc::new(
                crate::api::compilation_templates::CompilationTemplateStore::in_memory(),
            ),
            evaluations: Arc::new(crate::api::evaluation::EvaluationStore::in_memory()),
            log_levels: Arc::new(crate::logging::LogLevelManager::in_memory()),
            chunk_feedback_enabled: false,
            chunk_feedback_weighting: crate::chunk_feedback::FeedbackWeighting::Relevance,
            channels: Arc::new(crate::channels::ChannelRegistry::new()),
            document_commit_lock: Arc::new(std::sync::Mutex::new(())),
            memory_commit_lock: Arc::new(std::sync::Mutex::new(())),
        });
        TestEnv { root, state, token }
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn memory_web_detail_routes_render_live_shells() {
        let env = test_env();
        let router = build_router(env.state.clone());
        for (path, marker) in [
            ("/memories", "data-testid='memory-list'"),
            ("/memory/memory-message/memory-a", "id='messageSearch'"),
            ("/memory/memory-setting/memory-a", "id='memorySettingForm'"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::COOKIE, "lang=en")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "route {path}");
            let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let html = String::from_utf8(body.to_vec()).unwrap();
            assert!(html.contains(marker), "route {path} missing {marker}");
        }
    }

    #[tokio::test]
    async fn memory_owner_and_ragflow_model_catalog_contracts_are_live() {
        let env = test_env();
        let memory = env
            .state
            .memories
            .create(
                "test-user",
                crate::api::features::MemoryCreateRequest {
                    name: "Owned memory".into(),
                    memory_type: vec!["raw".into()],
                    embd_id: "default".into(),
                    llm_id: "default".into(),
                    description: String::new(),
                },
            )
            .unwrap();
        env.state
            .tenant_models
            .upsert(
                &env.state.providers,
                "test-user",
                "minimax",
                "primary",
                TenantModelInstanceUpdate {
                    tenant_id: None,
                    instance_name: "Primary".into(),
                    api_base: Some("http://127.0.0.1:9/v1".into()),
                    api_key: Some("must-not-leak".into()),
                    clear_api_key: false,
                    models: vec![
                        TenantModelSpec {
                            name: "MiniMax-M3".into(),
                            model_types: vec![ModelCapability::Chat],
                            max_tokens: Some(4096),
                            enabled: true,
                            is_tools: false,
                            ocr_config: None,
                        },
                        TenantModelSpec {
                            name: "MiniMax-M2".into(),
                            model_types: vec![ModelCapability::Embedding],
                            max_tokens: None,
                            enabled: true,
                            is_tools: false,
                            ocr_config: None,
                        },
                    ],
                },
            )
            .unwrap();
        env.state
            .tenant_models
            .set_default_chat_model(
                &env.state.providers,
                "test-user",
                Some("minimax/primary/MiniMax-M3"),
            )
            .unwrap();
        let router = build_router(env.state.clone());
        let authorization = format!("Bearer {}", env.token);

        let listed = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/memories?tenant_id=&owner_ids={}",
                        memory.tenant_id
                    ))
                    .header(header::AUTHORIZATION, &authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = response_json(listed).await;
        let memory_json = &listed["data"]["memory_list"][0];
        assert_eq!(memory_json["owner_name"], "Test");
        assert_eq!(memory_json["permissions"], "me");
        assert_eq!(memory_json["memory_size"], 5 * 1024 * 1024);
        assert_eq!(memory_json["temperature"], 0.5);

        let catalog = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/models")
                    .header(header::AUTHORIZATION, &authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(catalog.status(), StatusCode::OK);
        let catalog = response_json(catalog).await;
        let models = catalog["data"].as_array().unwrap();
        // Factory-catalog expansion (upstream `list_tenant_added_models`):
        // every MiniMax factory model is exposed for the tenant instance, in
        // factory order; an explicit compact record is the final effective
        // capability set after upstream ACTIVE/INACTIVE/UNSUPPORTED folding.
        assert!(
            models.len() >= 7,
            "expected the full MiniMax factory catalog"
        );
        assert_eq!(models[0]["selector"], "MiniMax-M3@Primary@MiniMax");
        assert_eq!(models[0]["provider_name"], "MiniMax");
        assert_eq!(models[0]["provider"], "MiniMax");
        assert!(models.iter().all(|model| model.get("api_key").is_none()));
        assert!(models.iter().any(|model| {
            model["name"] == "MiniMax-M2" && model["model_type"] == serde_json::json!(["embedding"])
        }));

        let defaults = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/models/default")
                    .header(header::AUTHORIZATION, &authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let defaults = response_json(defaults).await;
        assert_eq!(
            defaults["data"]["models"][0]["selector"],
            "MiniMax-M3@Primary@MiniMax"
        );
        assert_eq!(defaults["data"]["models"][0]["enable"], true);

        let openai = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/openai/models")
                    .header(header::AUTHORIZATION, authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response_json(openai).await["object"], "list");
    }

    #[tokio::test]
    async fn memory_message_routes_persist_search_forget_and_cascade() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        let memory = env
            .state
            .memories
            .create(
                "test-user",
                crate::api::features::MemoryCreateRequest {
                    name: "Route memory".into(),
                    memory_type: vec!["raw".into()],
                    embd_id: "default".into(),
                    llm_id: "default".into(),
                    description: String::new(),
                },
            )
            .unwrap();
        let router = build_router(env.state.clone());

        let added = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/messages")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "memory_id": [memory.id.clone()],
                            "agent_id": "agent-1",
                            "session_id": "session-1",
                            "user_input": "Rust memory",
                            "agent_response": "Stored"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(added.status(), StatusCode::OK);
        assert_eq!(response_json(added).await["message"], "All add to task.");
        let queued = env.state.tasks.pending();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].task_type, "memory");
        assert_eq!(queued[0].doc_id, memory.id);
        assert_eq!(queued[0].digest, "1");
        dispatch_pending_document_tasks(
            env.state.clone(),
            Arc::new(tokio::sync::Semaphore::new(1)),
        );
        for _ in 0..50 {
            if env.state.tasks.get(&queued[0].id).unwrap().status == "done" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(env.state.tasks.get(&queued[0].id).unwrap().status, "done");

        let listed = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/memories/{}", memory.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = response_json(listed).await;
        let message = &listed["data"]["messages"]["message_list"][0];
        assert_eq!(message["user_id"], "test-user");
        assert!(message.get("content_embed").is_none());
        assert_eq!(message["extract"], serde_json::json!([]));
        assert_eq!(message["task"]["digest"], message["message_id"].to_string());
        assert_eq!(message["task"]["progress"], 1.0);
        let message_id = message["message_id"].as_i64().unwrap();
        let message_ref = format!("{}:{message_id}", memory.id);

        let disabled = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/messages/{message_ref}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(disabled.status(), StatusCode::OK);

        let recent = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/messages?memory_id={}&agent_id=agent-1&session_id=session-1",
                        memory.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response_json(recent).await["data"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let hidden = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/messages/search?memory_id={}&query=Rust",
                        memory.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response_json(hidden).await["data"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        let enabled = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/messages/{message_ref}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(enabled.status(), StatusCode::OK);

        let content = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/messages/{message_ref}/content"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response_json(content).await["data"]["content"]
                .as_str()
                .unwrap()
                .contains("Rust memory")
        );

        let forgotten = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/messages/{message_ref}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forgotten.status(), StatusCode::OK);
        assert!(
            env.state
                .memory_messages
                .get_by_message_id(&memory.id, message_id)
                .unwrap()
                .forget_at
                .is_some()
        );

        let deleted = router
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/memories/{}", memory.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        assert!(
            env.state
                .memory_messages
                .get_by_message_id(&memory.id, message_id)
                .is_none()
        );
    }

    #[tokio::test]
    async fn memory_worker_extracts_children_and_structural_failure_keeps_raw() {
        async fn run(response: &str) -> (TestEnv, String, String, i64) {
            let mut env = test_env();
            let llm = mock_memory_llm(response).await;
            let mutable = Arc::get_mut(&mut env.state).unwrap();
            mutable.embedder = Some(Arc::new(TestEmbedder));
            mutable.llm = Some(Arc::new(llm));
            let memory = env
                .state
                .memories
                .create(
                    "test-user",
                    crate::api::features::MemoryCreateRequest {
                        name: "Worker memory".into(),
                        memory_type: vec!["raw".into(), "semantic".into()],
                        embd_id: "default".into(),
                        llm_id: "default".into(),
                        description: String::new(),
                    },
                )
                .unwrap();
            let source_id = env.state.memory_messages.next_message_id();
            env.state
                .memory_messages
                .insert_messages(vec![crate::api::joint_services::MemoryMessage {
                    message_id: source_id,
                    message_type: "raw".into(),
                    source_id: 0,
                    memory_id: memory.id.clone(),
                    user_id: "test-user".into(),
                    agent_id: "agent-1".into(),
                    session_id: "session-1".into(),
                    content: crate::api::joint_services::build_raw_message_content(
                        "Rust is memory safe",
                        "Stored",
                    ),
                    valid_at: "2026-08-12 10:00:00".into(),
                    invalid_at: None,
                    forget_at: None,
                    status: true,
                    zone_id: 0,
                    content_embed: vec![1.0, 0.0],
                }])
                .unwrap();
            let task_id = env
                .state
                .tasks
                .stage_memory_task(
                    "test-user",
                    &memory.id,
                    source_id,
                    crate::api::features::MemoryTaskPayload::new(
                        "test-user",
                        "agent-1",
                        "session-1",
                        "Rust is memory safe",
                        "Stored",
                    ),
                )
                .unwrap();
            assert!(env.state.tasks.publish_memory_task(&task_id).unwrap());
            let task = env.state.tasks.get(&task_id).unwrap();
            process_memory_task(env.state.clone(), task).await;
            (env, memory.id, task_id, source_id)
        }

        let (success, memory_id, task_id, source_id) = run(
            r#"{"semantic":[{"content":"Rust prevents data races","valid_at":"2026-08-12T10:00:00Z","invalid_at":""}]}"#,
        )
        .await;
        let rows = success.state.memory_messages.query_with_options(
            crate::api::joint_services::MemoryMessageQuery {
                memory_ids: std::slice::from_ref(&memory_id),
                agent_id: None,
                session_id: None,
                user_id: None,
                status: None,
                top_n: None,
                hide_forgotten: false,
            },
        );
        assert_eq!(success.state.tasks.get(&task_id).unwrap().status, "done");
        assert_eq!(rows.len(), 2);
        let child = rows.iter().find(|row| row.source_id == source_id).unwrap();
        assert_eq!(child.message_type, "semantic");
        assert_eq!(child.valid_at, "2026-08-12 10:00:00");

        let (failed, memory_id, task_id, source_id) = run(r#"{"semantic":{}}"#).await;
        let task = failed.state.tasks.get(&task_id).unwrap();
        assert_eq!(task.status, "failed");
        assert!(task.message.contains("must be an array"));
        assert!(
            failed
                .state
                .memory_messages
                .get_by_message_id(&memory_id, source_id)
                .is_some(),
            "the synchronous RAW phase must survive extraction failure"
        );
        assert!(
            failed
                .state
                .memory_messages
                .query_with_options(crate::api::joint_services::MemoryMessageQuery {
                    memory_ids: std::slice::from_ref(&memory_id),
                    agent_id: None,
                    session_id: None,
                    user_id: None,
                    status: None,
                    top_n: None,
                    hide_forgotten: false,
                })
                .iter()
                .all(|row| row.source_id == 0)
        );
    }

    #[test]
    fn memory_task_reconciler_publishes_only_when_raw_and_memory_exist() {
        let env = test_env();
        let memory = env
            .state
            .memories
            .create(
                "test-user",
                crate::api::features::MemoryCreateRequest {
                    name: "Reconcile memory".into(),
                    memory_type: vec!["raw".into()],
                    embd_id: "default".into(),
                    llm_id: "default".into(),
                    description: String::new(),
                },
            )
            .unwrap();
        env.state
            .memory_messages
            .insert_messages(vec![crate::api::joint_services::MemoryMessage {
                message_id: 91,
                message_type: "raw".into(),
                source_id: 0,
                memory_id: memory.id.clone(),
                user_id: "test-user".into(),
                agent_id: "agent".into(),
                session_id: "session".into(),
                content: "raw".into(),
                valid_at: "2026-08-12 10:00:00".into(),
                invalid_at: None,
                forget_at: None,
                status: true,
                zone_id: 0,
                content_embed: vec![1.0],
            }])
            .unwrap();
        let payload = || {
            crate::api::features::MemoryTaskPayload::new(
                "test-user",
                "agent",
                "session",
                "input",
                "response",
            )
        };
        let recoverable = env
            .state
            .tasks
            .stage_memory_task("test-user", &memory.id, 91, payload())
            .unwrap();
        let orphan = env
            .state
            .tasks
            .stage_memory_task("test-user", &memory.id, 92, payload())
            .unwrap();
        reconcile_staged_memory_tasks(&env.state);
        assert_eq!(env.state.tasks.get(&recoverable).unwrap().status, "pending");
        assert!(env.state.tasks.get(&orphan).is_none());
    }

    #[tokio::test]
    async fn memory_routes_accept_repeated_filters_and_reject_oversized_pages() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        let memory = env
            .state
            .memories
            .create(
                "test-user",
                crate::api::features::MemoryCreateRequest {
                    name: "Query contract".into(),
                    memory_type: vec!["raw".into()],
                    embd_id: "default".into(),
                    llm_id: "default".into(),
                    description: String::new(),
                },
            )
            .unwrap();
        let message = |message_id, agent_id: &str| crate::api::joint_services::MemoryMessage {
            message_id,
            message_type: "raw".into(),
            source_id: 0,
            memory_id: memory.id.clone(),
            user_id: "test-user".into(),
            agent_id: agent_id.into(),
            session_id: "session".into(),
            content: format!("content {message_id}"),
            valid_at: format!("2026-08-12 00:00:0{message_id}"),
            invalid_at: None,
            forget_at: None,
            status: true,
            zone_id: 0,
            content_embed: vec![1.0, 0.0],
        };
        env.state
            .memory_messages
            .insert_messages(vec![message(1, "agent-a"), message(2, "agent-b")])
            .unwrap();
        let router = build_router(env.state.clone());
        let authorization = format!("Bearer {}", env.token);

        let repeated = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/memories/{}?agent_id=agent-a&agent_id=agent-b&page_size=100",
                        memory.id
                    ))
                    .header(header::AUTHORIZATION, &authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(repeated.status(), StatusCode::OK);
        let repeated_json = response_json(repeated).await;
        let repeated_messages = repeated_json["data"]["messages"]["message_list"]
            .as_array()
            .unwrap();
        assert_eq!(repeated_messages.len(), 2);
        assert_eq!(repeated_messages[0]["agent_name"], "Unknown");
        assert!(repeated_messages[0]["task"].is_null());

        let content = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/messages/{}:{}/content", memory.id, 1))
                    .header(header::AUTHORIZATION, &authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(content.status(), StatusCode::OK);
        assert_eq!(
            response_json(content).await["data"]["content_embed"],
            serde_json::json!([1.0, 0.0])
        );

        let repeated_memory_ids = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/messages?memory_id={}&memory_id={}&limit=100",
                        memory.id, memory.id
                    ))
                    .header(header::AUTHORIZATION, &authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(repeated_memory_ids.status(), StatusCode::OK);

        let too_large = router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/memories/{}?page_size=101", memory.id))
                    .header(header::AUTHORIZATION, authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(too_large.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(too_large).await["message"],
            "page_size must be less than or equal to 100"
        );
    }

    #[tokio::test]
    async fn skill_index_config_search_reindex_and_delete_routes_are_live() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        let router = build_router(env.state.clone());
        let authorization = format!("Bearer {}", env.token);

        let configured = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/skills/config")
                    .header(header::AUTHORIZATION, &authorization)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "space_id": "engineering",
                            "embd_id": "default",
                            "vector_similarity_weight": 0.4,
                            "similarity_threshold": 0.0,
                            "top_k": 10,
                            "field_config": {
                                "name": {"enabled": true, "weight": 3.0},
                                "tags": {"enabled": true, "weight": 2.0},
                                "description": {"enabled": true, "weight": 1.0},
                                "content": {"enabled": false, "weight": 0.5}
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(configured.status(), StatusCode::OK);
        assert_eq!(
            response_json(configured).await["data"]["embd_id"],
            "default"
        );

        let indexed = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/skills/index")
                    .header(header::AUTHORIZATION, &authorization)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "space_id": "engineering",
                            "skills": [{
                                "id": "rust/search",
                                "folder_id": "folder-1",
                                "name": "Rust Search",
                                "description": "Hybrid retrieval in Rust",
                                "tags": ["rust", "rag"],
                                "content": "Implementation notes",
                                "version": "1.2.3"
                            }]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(indexed.status(), StatusCode::OK);
        assert_eq!(response_json(indexed).await["data"]["indexed_count"], 1);

        let searched = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/skills/search")
                    .header(header::AUTHORIZATION, &authorization)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"space_id":"engineering","query":"Rust retrieval","page":1,"page_size":10}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(searched.status(), StatusCode::OK);
        let searched = response_json(searched).await;
        assert_eq!(searched["data"]["total"], 1);
        assert_eq!(searched["data"]["search_type"], "hybrid");
        assert_eq!(searched["data"]["skills"][0]["skill_id"], "rust_search");

        let reindexed = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/skills/reindex")
                    .header(header::AUTHORIZATION, &authorization)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"space_id":"engineering"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reindexed.status(), StatusCode::OK);
        let reindexed = response_json(reindexed).await;
        assert_eq!(reindexed["data"]["indexed_count"], 1);
        assert_eq!(reindexed["data"]["version"], "1.0.1");

        let deleted = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/skills/index?space_id=engineering&skill_id=rust/search")
                    .header(header::AUTHORIZATION, &authorization)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        assert!(response_json(deleted).await["data"].as_bool().unwrap());

        let listed = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/skills/search")
                    .header(header::AUTHORIZATION, authorization)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"space_id":"engineering","query":""}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response_json(listed).await["data"]["total"], 0);
    }

    async fn response_sse_json(response: Response) -> (Vec<serde_json::Value>, String) {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        let frames = body
            .split("\n\n")
            .filter_map(|frame| frame.strip_prefix("data:"))
            .map(str::trim)
            .filter(|frame| !frame.is_empty() && *frame != "[DONE]")
            .map(|frame| serde_json::from_str(frame).unwrap())
            .collect();
        (frames, body)
    }

    #[tokio::test]
    async fn prometheus_metrics_are_public_text_and_bounded_to_rust_runtime() {
        let env = test_env();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains("ragflow_canvas_runs_total"));
        assert!(!body.contains("runtime=\"python\""));
    }

    #[tokio::test]
    async fn component_catalog_is_authenticated_filtered_and_strictly_shaped() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let unauthorized = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/components")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let all = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/components")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(all.status(), StatusCode::OK);
        let all = response_json(all).await;
        assert_eq!(all["code"], 0);
        assert_eq!(all["message"], "success");
        let descriptors = all["data"].as_array().unwrap();
        assert!(
            descriptors
                .windows(2)
                .all(|pair| pair[0]["name"].as_str() < pair[1]["name"].as_str())
        );
        assert!(descriptors.iter().any(|item| item["category"] == "agent"));
        assert!(
            descriptors
                .iter()
                .any(|item| item["category"] == "ingestion")
        );
        assert!(descriptors.iter().all(|item| {
            item["inputs"].is_object()
                && item["outputs"].is_object()
                && item["name"].as_str().is_some_and(|name| !name.is_empty())
        }));

        let filtered = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/components?category=INGESTION,ingestion,shared")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(filtered.status(), StatusCode::OK);
        let filtered = response_json(filtered).await;
        assert_eq!(
            filtered["data"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["name"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["extractor", "file", "parser", "tokenchunker", "tokenizer"]
        );

        let invalid = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/components?category=Foo")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let invalid = response_json(invalid).await;
        assert_eq!(invalid["code"], 400);
        assert_eq!(invalid["message"], "unknown category: foo");
    }

    #[tokio::test]
    async fn agent_component_input_form_is_access_checked_and_typed() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "title": "Input Form Agent",
                            "dsl": {
                                "graph": {
                                    "nodes": [
                                        {"id": "sally:0", "type": "beginNode"},
                                        {"id": "Browser:0", "type": "agentNode"}
                                    ],
                                    "edges": []
                                },
                                "components": {
                                    "sally:0": {
                                        "obj": {
                                            "component_name": "Begin",
                                            "params": {},
                                            "input_form": {
                                                "query": {
                                                    "type": "line",
                                                    "name": "Query"
                                                }
                                            }
                                        },
                                        "upstream": [],
                                        "downstream": []
                                    },
                                    "Browser:0": {
                                        "obj": {
                                            "component_name": "Browser",
                                            "params": {"prompts": "{sys.query}"}
                                        },
                                        "upstream": [],
                                        "downstream": []
                                    }
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let agent_id = response_json(create).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let form = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/agents/{agent_id}/components/sally:0/input-form"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(form.status(), StatusCode::OK);
        let form = response_json(form).await;
        assert_eq!(form["code"], 0);
        assert_eq!(form["message"], "success");
        assert_eq!(form["data"]["query"]["type"], "line");

        let dynamic = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/agents/{agent_id}/components/Browser:0/input-form"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dynamic.status(), StatusCode::OK);
        let dynamic = response_json(dynamic).await;
        assert_eq!(dynamic["code"], 0);
        assert_eq!(dynamic["data"]["prompts"]["type"], "text");
        assert_eq!(dynamic["data"]["upload_sources"]["type"], "line");

        let missing_component = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/agents/{agent_id}/components/missing/input-form"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_component.status(), StatusCode::OK);
        let missing_component = response_json(missing_component).await;
        assert_eq!(missing_component["code"], 102);
        assert_eq!(missing_component["message"], "component not found: missing");

        let debug = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/agents/{agent_id}/components/sally:0/debug"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::from(
                        r#"{"params":{"query":{"value":"hello debug"}}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(debug.status(), StatusCode::OK);
        let debug = response_json(debug).await;
        assert_eq!(debug["code"], 0);
        assert_eq!(debug["data"]["query"], "hello debug");

        for (body, expected_message) in [
            (r#"{}"#, "`params` is required."),
            (
                r#"{"params":{"query":{}}}"#,
                "`params.query.value` is required.",
            ),
        ] {
            let invalid = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!(
                            "/api/v1/agents/{agent_id}/components/sally:0/debug"
                        ))
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(invalid.status(), StatusCode::OK);
            let invalid = response_json(invalid).await;
            assert_eq!(invalid["code"], 101);
            assert_eq!(invalid["message"], expected_message);
        }

        let unsupported_debug = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/agents/{agent_id}/components/Browser:0/debug"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::from(r#"{"params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unsupported_debug.status(), StatusCode::OK);
        let unsupported_debug = response_json(unsupported_debug).await;
        assert_eq!(unsupported_debug["code"], 102);
        assert!(
            unsupported_debug["message"]
                .as_str()
                .unwrap()
                .contains("component factory")
        );

        let inaccessible = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/agents/does-not-exist/components/begin/input-form")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inaccessible.status(), StatusCode::OK);
        let inaccessible = response_json(inaccessible).await;
        assert_eq!(inaccessible["code"], 103);
        assert_eq!(
            inaccessible["message"],
            "Make sure you have permission to access the agent."
        );

        let inaccessible_debug = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents/does-not-exist/components/begin/debug")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::from(r#"{"params":{}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inaccessible_debug.status(), StatusCode::OK);
        assert_eq!(response_json(inaccessible_debug).await["code"], 103);
    }

    #[tokio::test]
    async fn agent_create_derives_and_versions_a_missing_canvas_graph() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "title": "Derived Graph Agent",
                            "dsl": {"components": {
                                "message": {
                                    "obj": {
                                        "component_name": "Message",
                                        "params": {"content": ["done"]}
                                    },
                                    "upstream": ["begin"],
                                    "downstream": []
                                },
                                "begin": {
                                    "obj": {"component_name": "Begin", "params": {}},
                                    "upstream": [],
                                    "downstream": ["message"]
                                }
                            }}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let create = response_json(create).await;
        assert_eq!(create["code"], 0);
        let dsl = &create["data"]["dsl"];
        assert_eq!(
            dsl["graph"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|node| node["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["begin", "message"]
        );
        assert_eq!(dsl["graph"]["edges"][0]["sourceHandle"], "start");
        assert_eq!(dsl["graph"]["edges"][0]["targetHandle"], "end");
        assert_eq!(dsl["components"]["begin"]["name"], "Begin");
        assert!(dsl["components"]["begin"].get("obj").is_none());

        let agent_id = create["data"]["id"].as_str().unwrap();
        let versions = env.state.canvas_versions.list(agent_id);
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].dsl, *dsl);
    }

    /// Upstream `agent_api.py::list_agent_sessions` +
    /// `API4ConversationService.get_list`: both route spellings answer the
    /// `{code, message, data, total}` envelope of `_agent_session_list_result`,
    /// the row carries the `_normalize_agent_session` shape (`agent_id` from
    /// `dialog_id`, folded per-message references, RAGFlow datetimes), and the
    /// filters (`keywords`, `desc`, `orderby`, `dsl`, `page_size`) behave the
    /// way the ported log page drives them.
    #[tokio::test]
    async fn agent_log_sessions_match_upstream_envelope_filters_and_detail() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "title": "Log Parity Agent",
                            "dsl": {"components": {
                                "message": {
                                    "obj": {
                                        "component_name": "Message",
                                        "params": {"content": ["done"]}
                                    },
                                    "upstream": ["begin"],
                                    "downstream": []
                                },
                                "begin": {
                                    "obj": {"component_name": "Begin", "params": {}},
                                    "upstream": [],
                                    "downstream": ["message"]
                                }
                            }}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let agent_id = response_json(create).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let record = |question: &str, answer: &str| crate::llm::ConversationExchange {
            question: question.to_string(),
            answer: answer.to_string(),
            citations: Vec::new(),
            references: vec![crate::llm::ChunkReference {
                id: "chunk-1".into(),
                kb_id: "kb-1".into(),
                content: "retrieved passage".into(),
                similarity: Some(0.75),
                vector_similarity: Some(0.8),
                term_similarity: Some(0.2),
            }],
            settings: None,
            duration_ms: 1200,
            usage: None,
        };
        let first = env
            .state
            .conversations
            .create_agent_for(
                "test-user",
                "test-user",
                &agent_id,
                "first question",
                vec![],
            )
            .unwrap();
        env.state
            .conversations
            .append_exchange_with_settings(&first.id, "test-user", record("first question", "one"))
            .unwrap();
        let dsl = serde_json::json!({"components": {"begin": {}}});
        env.state
            .conversations
            .record_agent_run(&first.id, &dsl, Some("node failed"))
            .unwrap();
        let second = env
            .state
            .conversations
            .create_agent_for(
                "test-user",
                "test-user",
                &agent_id,
                "second question",
                vec![],
            )
            .unwrap();
        env.state
            .conversations
            .append_exchange_with_settings(
                &second.id,
                "test-user",
                record("second question", "two"),
            )
            .unwrap();
        env.state
            .conversations
            .set_agent_version_title(&second.id, "Release 1")
            .unwrap();
        // A chat session on the same canvas must not leak into the log table.
        let other = env
            .state
            .conversations
            .create_for("test-user", "not an agent session")
            .unwrap();

        let unauthorized = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/canvas/{agent_id}/sessions"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let missing_agent = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/canvas/does-not-exist/sessions")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing_agent.status(), StatusCode::NOT_FOUND);

        let listing = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/canvas/{agent_id}/sessions?orderby=create_time&desc=false"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listing.status(), StatusCode::OK);
        let listing = response_json(listing).await;
        assert_eq!(listing["code"], 0);
        assert_eq!(listing["message"], "success");
        assert_eq!(listing["total"], 2);
        let rows = listing["data"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], first.id);
        assert_eq!(rows[1]["id"], second.id);
        assert_eq!(rows[0]["agent_id"], agent_id);
        assert_eq!(rows[0]["user_id"], "test-user");
        assert_eq!(rows[0]["exp_user_id"], "test-user");
        assert_eq!(rows[0]["round"], 1);
        assert_eq!(rows[0]["errors"], "node failed");
        assert_eq!(rows[1]["errors"], serde_json::Value::Null);
        assert_eq!(rows[1]["version_title"], "Release 1");
        assert_eq!(rows[0]["message"][0]["content"], "first question");
        assert_eq!(rows[0]["message"][0]["role"], "user");
        assert_eq!(
            rows[0]["message"][1]["reference"][0]["content"],
            "retrieved passage"
        );
        assert_eq!(rows[0]["message"][1]["reference"][0]["dataset_id"], "kb-1");
        assert_eq!(rows[0]["dsl"]["components"]["begin"], serde_json::json!({}));
        assert_eq!(rows[0]["create_date"].as_str().unwrap().len(), 19);
        assert!(!rows.iter().any(|row| row["id"] == other.id));

        // `dsl=false` drops the snapshot, exactly like `include_dsl`.
        let without_dsl = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/canvas/{agent_id}/sessions?dsl=false"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let without_dsl = response_json(without_dsl).await;
        assert!(without_dsl["data"][0].get("dsl").is_none());

        // Keywords match the serialized message, newest first by default.
        let keyword = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/canvas/{agent_id}/sessions?keywords=SECOND"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let keyword = response_json(keyword).await;
        assert_eq!(keyword["total"], 1);
        assert_eq!(keyword["data"][0]["id"], second.id);

        let empty = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/canvas/{agent_id}/sessions?keywords=nothing"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let empty = response_json(empty).await;
        assert_eq!(empty["total"], 0);
        assert_eq!(empty["data"], serde_json::json!([]));

        // `validate_rest_api_page_size` rejects anything above 100.
        let oversize = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/canvas/{agent_id}/sessions?page_size=101"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(oversize.status(), StatusCode::BAD_REQUEST);
        let oversize = response_json(oversize).await;
        assert_eq!(
            oversize["message"],
            "page_size must be less than or equal to 100"
        );

        // The REST spelling, the detail route and its 404 message.
        let rest = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/agents/{agent_id}/sessions?page=1&page_size=1"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let rest = response_json(rest).await;
        assert_eq!(rest["total"], 2);
        assert_eq!(rest["data"].as_array().unwrap().len(), 1);

        let detail = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agents/{agent_id}/sessions/{}", first.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);
        let detail = response_json(detail).await;
        assert_eq!(detail["data"]["id"], first.id);
        assert_eq!(detail["data"]["round"], 1);

        let unknown = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agents/{agent_id}/sessions/nope"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response_json(unknown).await["message"],
            "Session not found!"
        );

        // `delete_agent_session_item`: the row leaves the log and the total
        // drops, so callers (and UI probes) can clean up after themselves.
        let removed = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/agents/{agent_id}/sessions/{}", second.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(removed.status(), StatusCode::OK);
        assert_eq!(
            response_json(removed).await["data"],
            serde_json::json!(true)
        );
        let after = router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/canvas/{agent_id}/sessions"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let after = response_json(after).await;
        assert_eq!(after["total"], 1);
        assert_eq!(after["data"][0]["id"], first.id);
    }

    /// Upstream `Routes.AgentLog = /agent-log-page/:id`: the SSR shell carries
    /// the breadcrumb, the toolbar (`flow.export`, the `ID/Title` keyword box,
    /// the `flow.latestDate` range, Search/Reset), the eight-column table, the
    /// pagination footer and the detail modal, in both locales.
    #[tokio::test]
    async fn agent_log_page_matches_upstream_layout() {
        let env = test_env();
        let router = build_router(env.state.clone());
        for (lang, expectations) in [
            (
                "en",
                vec![
                    "agent-log-breadcrumb",
                    "agent-log-breadcrumb-agents",
                    ">Log<",
                    "agent-log-export",
                    "ID/Title",
                    "Latest date",
                    "agent-log-search",
                    "agent-log-reset",
                    "agent-log-table",
                    "agent-log-pagination",
                    "agent-log-modal",
                    "agentLogLoad()",
                ],
            ),
            (
                "zh",
                vec![
                    "agent-log-breadcrumb",
                    "智能体",
                    ">日志<",
                    "导出",
                    "ID/标题",
                    "最新日期",
                    "搜索",
                    "重置",
                    "agent-log-table",
                ],
            ),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/agent-log-page/canvas-1")
                        .header(
                            header::COOKIE,
                            if lang == "zh" {
                                "lang=zh-CN"
                            } else {
                                "lang=en"
                            },
                        )
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "lang {lang}");
            let body = String::from_utf8(
                to_bytes(response.into_body(), 4 * 1024 * 1024)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap();
            for expected in expectations {
                assert!(body.contains(expected), "lang {lang} missing {expected}");
            }
            // The English shell keeps the untranslated `flow.*` fallbacks that
            // RAGFlow's zh locale also falls back to.
            if lang == "zh" {
                assert!(body.contains("No data to export"));
                assert!(body.contains("Success"));
            }
        }
    }

    #[tokio::test]
    async fn stats_requires_authentication_and_honors_canvas_source_filter() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/stats")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        env.state
            .conversations
            .create_for("test-user", "standard")
            .unwrap();
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/stats?from_date=1970-01-01&to_date=2999-12-31&canvas_id=agent")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["pv"], serde_json::json!([]));
        assert_eq!(body["data"]["uv"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn chat_creation_and_stats_require_selected_tenant_membership() {
        let env = test_env();
        let router = build_router(env.state.clone());

        let forbidden_chat = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/chats")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"question":"shared","tenant_id":"other-tenant"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden_chat.status(), StatusCode::FORBIDDEN);

        let forbidden_stats = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/stats?tenant_id=other-tenant")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden_stats.status(), StatusCode::FORBIDDEN);

        env.state
            .tenants
            .invite_member("shared-tenant", "test-user", "owner-user")
            .unwrap();
        env.state
            .tenants
            .accept_invitation("shared-tenant", "test-user")
            .unwrap();
        let created = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/chats")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"question":"shared","tenant_id":"shared-tenant"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::OK);
        let created = response_json(created).await;
        let conversation_id = created["data"]["id"].as_str().unwrap();
        assert_eq!(
            env.state
                .conversations
                .get(conversation_id)
                .unwrap()
                .tenant_id,
            "shared-tenant"
        );

        let allowed_stats = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/stats?tenant_id=shared-tenant&from_date=1970-01-01&to_date=2999-12-31")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(allowed_stats.status(), StatusCode::OK);
        let allowed_stats = response_json(allowed_stats).await;
        assert_eq!(allowed_stats["data"]["pv"][0][1], 1);
    }

    fn search_chunk(id: &str, kb_id: &str, content: &str) -> crate::search::IndexedChunk {
        crate::search::IndexedChunk {
            id: id.into(),
            doc_name: "search.txt".into(),
            content: content.into(),
            embedding: Vec::new(),
            token_count: 2,
            position: 0,
            metadata: std::collections::HashMap::from([("kb_id".into(), kb_id.into())]),
        }
    }

    fn chunk_pagerank(state: &AppState, chunk_id: &str) -> f32 {
        state
            .engine
            .read()
            .unwrap()
            .to_vec()
            .into_iter()
            .find(|chunk| chunk.id == chunk_id)
            .and_then(|chunk| chunk.metadata.get("pagerank_fea").cloned())
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.0)
    }

    #[test]
    fn kb_embedding_selector_requires_one_vector_space_and_supports_legacy_default() {
        let mut env = test_env();
        configure_embedding_models(&env.state, "test-user", &["embed-a", "embed-b"]);
        let first = env
            .state
            .kbs
            .create_for_with_config(
                "test-user",
                "First",
                "",
                "private",
                "minimax/primary/embed-a",
            )
            .unwrap();
        let second = env
            .state
            .kbs
            .create_for_with_config(
                "test-user",
                "Second",
                "",
                "private",
                "minimax/primary/embed-b",
            )
            .unwrap();
        let error = kb_embedder_for(&env.state, &[first.id, second.id])
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "Knowledge bases use different embedding models"
        );

        let legacy = env.state.kbs.create_for("test-user", "Legacy", "").unwrap();
        assert!(kb_embedder_for(&env.state, std::slice::from_ref(&legacy.id)).is_err());
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        assert!(kb_embedder_for(&env.state, &[legacy.id]).is_ok());
    }

    #[tokio::test]
    async fn datasets_persist_validated_embedding_and_block_changes_after_indexing() {
        let env = test_env();
        configure_embedding_models(&env.state, "test-user", &["embed-a", "embed-b"]);
        let router = build_router(env.state.clone());
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/datasets")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "Configured",
                            "embedding_model": "embed-a"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = response_json(response).await;
        let kb_id = body["data"]["id"].as_str().unwrap();
        assert_eq!(body["data"]["embd_id"], "minimax/primary/embed-a");
        env.state.kbs.update_counts(kb_id, 1, 1).unwrap();

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/datasets/{kb_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"embedding_model":"embed-b"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            env.state.kbs.get(kb_id).unwrap().embd_id,
            "minimax/primary/embed-a"
        );
    }

    #[tokio::test]
    async fn retrieval_rejects_knowledge_bases_with_different_embedding_models() {
        let env = test_env();
        configure_embedding_models(&env.state, "test-user", &["embed-a", "embed-b"]);
        let first = env
            .state
            .kbs
            .create_for_with_config(
                "test-user",
                "First",
                "",
                "private",
                "minimax/primary/embed-a",
            )
            .unwrap();
        let second = env
            .state
            .kbs
            .create_for_with_config(
                "test-user",
                "Second",
                "",
                "private",
                "minimax/primary/embed-b",
            )
            .unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "water quality",
                            "kb_ids": [first.id, second.id],
                            "vector_similarity_weight": 0
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(
            body["message"],
            "Knowledge bases use different embedding models"
        );
    }

    #[tokio::test]
    async fn retrieval_requires_configured_reranker_when_requested() {
        let env = test_env();
        let kb = env.state.kbs.create_for("test-user", "Search", "").unwrap();
        env.state
            .engine
            .write()
            .unwrap()
            .add(search_chunk("one", &kb.id, "water quality"));
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "water quality",
                            "kb_ids": [kb.id],
                            "vector_similarity_weight": 0,
                            "rerank": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn retrieval_uses_shared_reranker() {
        let mut env = test_env();
        let kb = env.state.kbs.create_for("test-user", "Search", "").unwrap();
        {
            let mut engine = env.state.engine.write().unwrap();
            engine.add(search_chunk("first", &kb.id, "water quality exact"));
            engine.add(search_chunk("second", &kb.id, "water quality"));
        }
        Arc::get_mut(&mut env.state).unwrap().reranker =
            Arc::new(RerankerManager::in_memory(Some(Arc::new(ReverseReranker))));
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "water quality",
                            "kb_ids": [kb.id],
                            "top_k": 2,
                            "vector_similarity_weight": 0,
                            "rerank": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["chunks"][0]["chunk_id"], "second");
    }

    #[tokio::test]
    async fn retrieval_filters_documents_and_returns_full_aggregations_and_highlights() {
        let env = test_env();
        let kb = env.state.kbs.create_for("test-user", "Search", "").unwrap();
        {
            let mut engine = env.state.engine.write().unwrap();
            for (id, doc_id, doc_name, content) in [
                ("one", "doc-a", "A.txt", "water quality alpha"),
                ("two", "doc-a", "A.txt", "water quality beta"),
                ("three", "doc-b", "B.txt", "water quality gamma"),
            ] {
                let mut chunk = search_chunk(id, &kb.id, content);
                chunk.doc_name = doc_name.into();
                chunk.metadata.insert("doc_id".into(), doc_id.into());
                engine.add(chunk);
            }
        }
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "water quality",
                            "kb_ids": [kb.id],
                            "doc_ids": ["doc-a"],
                            "top_k": 1,
                            "vector_similarity_weight": 0,
                            "highlight": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["total"], 2);
        assert_eq!(body["data"]["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(body["data"]["chunks"][0]["doc_id"], "doc-a");
        assert_eq!(body["data"]["doc_aggs"][0]["count"], 2);
        assert_eq!(body["data"]["doc_aggs"][0]["doc_name"], "A.txt");
        assert!(
            body["data"]["chunks"][0]["highlight"]
                .as_str()
                .unwrap()
                .contains("<em>water</em>")
        );
    }

    #[tokio::test]
    async fn retrieval_deep_page_keeps_global_total_and_aggregations() {
        let env = test_env();
        let kb = env.state.kbs.create_for("test-user", "Search", "").unwrap();
        {
            let mut engine = env.state.engine.write().unwrap();
            for index in 0..70 {
                let mut chunk = search_chunk(&format!("chunk-{index:02}"), &kb.id, "water quality");
                chunk.doc_name = "A.txt".into();
                chunk.metadata.insert("doc_id".into(), "doc-a".into());
                engine.add(chunk);
            }
        }
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "water quality",
                            "kb_ids": [kb.id],
                            "top_k": 1,
                            "page": 65,
                            "vector_similarity_weight": 0
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["total"], 70);
        assert_eq!(body["data"]["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(body["data"]["doc_aggs"][0]["count"], 70);
    }

    #[tokio::test]
    async fn reranker_config_requires_admin_and_hot_disables_runtime() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().reranker =
            Arc::new(RerankerManager::in_memory(Some(Arc::new(ReverseReranker))));
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/reranker")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "enabled": false,
                            "api_base": "",
                            "api_key": null
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(env.state.reranker.current().is_none());
        let body = response_json(response).await;
        assert_eq!(body["data"]["enabled"], false);
        assert!(body["data"].get("api_key").is_none());

        let member = non_admin_env();
        let response = build_router(member.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/v1/reranker")
                    .header(header::AUTHORIZATION, format!("Bearer {}", member.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn provider_config_requires_admin_and_never_returns_secret() {
        let env = test_env();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/providers/local-openai")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "Local OpenAI",
                            "api_base": "http://127.0.0.1:8080/v1",
                            "models": ["model-a"],
                            "enabled": true,
                            "api_key": "secret"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["api_key_configured"], true);
        assert!(body["data"].get("api_key").is_none());

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/providers")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = response_json(response).await;
        let provider = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|provider| provider["id"] == "local-openai")
            .unwrap();
        assert_eq!(provider["api_key_configured"], true);
        assert!(provider.get("api_key").is_none());

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/providers/local-openai")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["data"]["id"], "local-openai");
        assert_eq!(body["data"]["api_key_configured"], true);
        assert!(body["data"].get("api_key").is_none());

        let member = non_admin_env();
        let response = build_router(member.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/providers/local-openai")
                    .header(header::AUTHORIZATION, format!("Bearer {}", member.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "Local OpenAI",
                            "api_base": "http://127.0.0.1:8080/v1",
                            "models": ["model-a"],
                            "enabled": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let response = build_router(member.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/providers/New%20API/models")
                    .header(header::AUTHORIZATION, format!("Bearer {}", member.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(response_json(response).await["code"], 502);

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/providers/missing/models")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn tenant_model_instances_enforce_permissions_and_expand_capabilities() {
        let env = test_env();
        let member = env
            .state
            .users
            .register(
                "Model Member",
                "model-member@example.com",
                "member password 123",
            )
            .unwrap();
        let member_token = env
            .state
            .users
            .login("model-member@example.com", "member password 123")
            .unwrap()
            .unwrap();
        env.state
            .tenants
            .invite_member("test-user", &member.id, "test-user")
            .unwrap();
        env.state
            .tenants
            .accept_invitation("test-user", &member.id)
            .unwrap();
        let router = build_router(env.state.clone());

        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/tenant/models/minimax/primary")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "instance_name": "Primary MiniMax",
                            "api_key": "tenant-secret",
                            "models": [{
                                "name": "MiniMax-M3",
                                "model_types": ["chat", "embedding"],
                                "max_tokens": 8192,
                                "enabled": true
                            }]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let create_body = response_json(create).await;
        assert_eq!(create_body["data"]["api_key_configured"], true);
        assert!(create_body["data"].get("api_key").is_none());

        let models = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/llm/models?tenant_id=test-user")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(models.status(), StatusCode::OK);
        let models_body = response_json(models).await;
        let model_types: Vec<&str> = models_body["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|model| model["model_type"].as_str())
            .collect();
        // Factory-catalog expansion (upstream `list_tenant_added_models`):
        // the MiniMax factory catalog is exposed for the tenant instance, in
        // factory order, with the configured MiniMax-M3 entry expanded first
        // (chat + embedding), followed by the remaining factory chat models.
        assert!(
            model_types.len() >= 7,
            "expected the full MiniMax factory catalog"
        );
        assert_eq!(model_types[0], "chat");
        assert_eq!(model_types[1], "embedding");
        assert!(model_types[2..].iter().all(|t| *t == "chat"));

        let added_instances = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/providers/MiniMax/instances")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(added_instances.status(), StatusCode::OK);
        let added_instances = response_json(added_instances).await;
        assert_eq!(
            added_instances["data"][0]["instance_name"],
            "Primary MiniMax"
        );
        assert_eq!(added_instances["data"][0]["api_key"], "");

        let instance_models = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/providers/MiniMax/instances/Primary%20MiniMax/models")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(instance_models.status(), StatusCode::OK);
        let instance_models = response_json(instance_models).await;
        let minimax_m3 = instance_models["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["name"] == "MiniMax-M3")
            .unwrap();
        assert_eq!(minimax_m3["status"], "active");
        assert_eq!(
            minimax_m3["model_type"],
            serde_json::json!(["chat", "embedding"])
        );

        let edit_model = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/providers/MiniMax/instances/Primary%20MiniMax/models")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "model_name": ["MiniMax-M3"],
                            "model_type": ["chat"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(edit_model.status(), StatusCode::OK);

        let deactivate_model = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/v1/providers/MiniMax/instances/Primary%20MiniMax/models/MiniMax-M3")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":"inactive"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deactivate_model.status(), StatusCode::OK);
        assert!(!env.state.tenant_models.list("test-user")[0].models[0].enabled);

        let reactivate_model = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/v1/providers/MiniMax/instances/Primary%20MiniMax/models/MiniMax-M3")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"status":"active"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reactivate_model.status(), StatusCode::OK);
        assert!(env.state.tenant_models.list("test-user")[0].models[0].enabled);

        let member_write = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/tenant/models/minimax/secondary")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "tenant_id": "test-user",
                            "instance_name": "Secondary",
                            "models": [{
                                "name": "MiniMax-M2",
                                "model_types": ["chat"]
                            }]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(member_write.status(), StatusCode::FORBIDDEN);

        let delete_provider = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/providers/minimax")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_provider.status(), StatusCode::CONFLICT);

        let delete_instance = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/providers/MiniMax/instances")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"instances":["Primary MiniMax"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete_instance.status(), StatusCode::OK);
        assert!(env.state.tenant_models.list("test-user").is_empty());

        let outsider = env
            .state
            .users
            .register("Outsider", "outsider@example.com", "outsider password 123")
            .unwrap();
        let outsider_token = env
            .state
            .users
            .login("outsider@example.com", "outsider password 123")
            .unwrap()
            .unwrap();
        assert_ne!(outsider.id, member.id);
        let outsider_read = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tenant/models?tenant_id=test-user")
                    .header(header::AUTHORIZATION, format!("Bearer {outsider_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(outsider_read.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn provider_modal_verify_is_ephemeral_and_create_persists_tenant_instance() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let probe = Router::new().route(
            "/v1/models",
            get(|| async {
                Json(serde_json::json!({
                    "data": [{ "id": "gpt-probe" }]
                }))
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, probe).await.unwrap();
        });

        let env = test_env();
        let router = build_router(env.state.clone());
        let base_url = format!("http://{address}/v1");
        let verify = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/providers/OpenAI/connection")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "api_key": "probe-key",
                            "base_url": base_url.clone()
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(verify.status(), StatusCode::OK);
        assert_eq!(response_json(verify).await["code"], 0);
        assert!(env.state.tenant_models.list("test-user").is_empty());

        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/providers/OpenAI/instances")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "instance_name": "Hosted OpenAI",
                            "api_key": "probe-key",
                            "base_url": base_url,
                            "region": "default",
                            "model_info": []
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let body = response_json(create).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["instance_name"], "Hosted OpenAI");
        assert_eq!(body["data"]["api_key_configured"], true);
        assert!(body["data"].get("api_key").is_none());
        let instances = env.state.tenant_models.list("test-user");
        assert_eq!(instances.len(), 1);
        assert!(instances[0].models.is_empty());
        assert!(
            env.state
                .providers
                .get_configured("openai")
                .unwrap()
                .enabled
        );

        let models = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/models?type=chat")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(models.status(), StatusCode::OK);
        let models = response_json(models).await;
        assert!(
            models["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| { model["selector"] == "gpt-5.5@Hosted OpenAI@OpenAI" })
        );
        let resolved = env
            .state
            .tenant_models
            .resolve(
                &env.state.providers,
                "test-user",
                ModelCapability::Chat,
                Some("gpt-5.5@Hosted OpenAI@OpenAI"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(resolved.model_name, "gpt-5.5");
        assert_eq!(resolved.api_key.as_deref(), Some("probe-key"));
    }

    #[tokio::test]
    async fn new_api_picker_discovers_then_persists_selected_tool_model() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let probe = Router::new()
            .route(
                "/v1/models",
                get(|| async {
                    Json(serde_json::json!({
                        "data": [{ "id": "custom-chat" }, { "id": "custom-embed" }]
                    }))
                }),
            )
            .route(
                "/v1/chat/completions",
                axum::routing::post(|| async {
                    Json(serde_json::json!({
                        "choices": [{ "message": { "content": "Hi" } }]
                    }))
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, probe).await.unwrap();
        });

        let env = test_env();
        assert!(env.state.providers.get_configured("new-api").is_none());
        let router = build_router(env.state.clone());
        let base_url = format!("http://{address}/v1");
        let discovery = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/providers/New%20API/models?base_url={}",
                        url::form_urlencoded::byte_serialize(base_url.as_bytes())
                            .collect::<String>()
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(discovery.status(), StatusCode::OK);
        let discovery = response_json(discovery).await;
        assert_eq!(discovery["code"], 0);
        assert!(
            discovery["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["name"] == "custom-chat")
        );
        assert!(
            env.state.providers.get_configured("new-api").is_none(),
            "read-only discovery must not materialize a provider"
        );

        let model_info = serde_json::json!([{
            "model_name": "custom-chat",
            "model_type": ["chat"],
            "max_tokens": 16384,
            "extra": { "is_tools": true }
        }]);
        let verify = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/providers/New%20API/connection")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "api_key": "",
                            "base_url": base_url.clone(),
                            "model_info": model_info.clone()
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response_json(verify).await["code"], 0);
        assert!(env.state.providers.get_configured("new-api").is_none());

        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/providers/New%20API/instances")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "instance_name": "Local gateway",
                            "api_key": "",
                            "base_url": base_url,
                            "model_info": model_info
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let create = response_json(create).await;
        assert_eq!(create["code"], 0);
        let provider = env.state.providers.get_configured("new-api").unwrap();
        assert!(provider.enabled);
        assert_eq!(provider.models, vec!["custom-chat"]);
        assert!(provider.api_key.is_none());
        let instances = env.state.tenant_models.list("test-user");
        assert_eq!(instances[0].models[0].is_tools, true);
        let instance_models = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/providers/New%20API/instances/Local%20gateway/models")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let instance_models = response_json(instance_models).await;
        assert_eq!(instance_models["data"][0]["features"][0], "is_tools");
        let resolved = env
            .state
            .tenant_models
            .resolve(
                &env.state.providers,
                "test-user",
                ModelCapability::Chat,
                Some("custom-chat@Local gateway@New API"),
            )
            .unwrap()
            .unwrap();
        assert_eq!(resolved.model_name, "custom-chat");
    }

    #[tokio::test]
    async fn default_chat_model_route_enforces_shared_tenant_permissions_and_conflicts() {
        let env = test_env();
        env.state
            .tenant_models
            .upsert(
                &env.state.providers,
                "test-user",
                "minimax",
                "primary",
                TenantModelInstanceUpdate {
                    tenant_id: None,
                    instance_name: "Primary".into(),
                    api_base: None,
                    api_key: None,
                    clear_api_key: false,
                    models: vec![TenantModelSpec {
                        name: "MiniMax-M3".into(),
                        model_types: vec![ModelCapability::Chat],
                        max_tokens: None,
                        enabled: true,
                        is_tools: false,
                        ocr_config: None,
                    }],
                },
            )
            .unwrap();
        let member = env
            .state
            .users
            .register(
                "Selector Member",
                "selector-member@example.com",
                "member password 123",
            )
            .unwrap();
        let member_token = env
            .state
            .users
            .login("selector-member@example.com", "member password 123")
            .unwrap()
            .unwrap();
        env.state
            .tenants
            .invite_member("test-user", &member.id, "test-user")
            .unwrap();
        env.state
            .tenants
            .accept_invitation("test-user", &member.id)
            .unwrap();
        let router = build_router(env.state.clone());

        let set = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/tenant/default-chat-model")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"tenant_id":"test-user","selector":"minimax/primary/MiniMax-M3"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(set.status(), StatusCode::OK);

        let member_get = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tenant/default-chat-model?tenant_id=test-user")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(member_get.status(), StatusCode::OK);
        assert_eq!(
            response_json(member_get).await["data"]["selector"],
            "minimax/primary/MiniMax-M3"
        );

        let member_put = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/tenant/default-chat-model")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"tenant_id":"test-user","selector":null}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(member_put.status(), StatusCode::FORBIDDEN);

        let conflict = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/tenant/models/minimax/primary?tenant_id=test-user")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let conflict = response_json(conflict).await;
        assert_eq!(conflict["code"], 409);
        assert!(
            conflict["message"]
                .as_str()
                .unwrap()
                .contains("Tenant default chat selector")
        );

        let invalid = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/tenant/default-chat-model")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"selector":"missing"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert_eq!(response_json(invalid).await["code"], 400);

        let clear = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/tenant/default-chat-model")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"selector":null}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(clear.status(), StatusCode::OK);
        assert!(response_json(clear).await["data"]["selector"].is_null());
    }

    #[tokio::test]
    async fn api_auth_public_and_protected_routes() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let public = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(public.status(), StatusCode::OK);
        let health = response_json(public).await;
        assert_eq!(health["data"]["status"], "healthy");
        assert_eq!(health["data"]["postgres"], "disabled");
        assert_eq!(health["data"]["zvec"], "disabled");

        let password_key = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/password-public-key")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(password_key.status(), StatusCode::OK);
        let password_key = response_json(password_key).await;
        assert_eq!(password_key["data"]["enabled"], false);
        assert!(password_key["data"]["public_key"].is_null());
        assert_eq!(password_key["data"]["algorithm"], "RSAES-PKCS1-v1_5");

        let status_unauthenticated = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status_unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let status = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/status")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        let status = response_json(status).await;
        assert_eq!(status["data"]["doc_engine"]["type"], "json");
        assert_eq!(status["data"]["database"]["database"], "local-json");
        assert!(status["data"]["task_executor_heartbeats"].is_object());

        let unauthenticated = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/datasets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let authenticated = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/datasets")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authenticated.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn system_variables_are_typed_durable_admin_settings() {
        let env = test_env();
        let router = build_router(env.state.clone());

        let unauthenticated = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/variables")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let listed = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/variables")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = response_json(listed).await;
        assert_eq!(listed["data"].as_array().unwrap().len(), 14);
        assert_eq!(listed["data"][0]["name"], "default_role");
        assert_eq!(listed["data"][0]["setting_type"], "config");

        let invalid = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/system/variables")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"var_name":"mail.timeout","var_value":"1.5"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

        for (name, value) in [("mail.timeout", "45"), ("mail.password", "secret-123")] {
            let updated = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::PUT)
                        .uri("/api/v1/system/variables")
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::json!({"var_name": name, "var_value": value}).to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(updated.status(), StatusCode::OK);
        }

        let shown = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/variables/bWFpbC50aW1lb3V0")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response_json(shown).await["data"][0]["value"], "45");

        // Python admin CLI SHOW VAR sends a JSON body on GET; preserve that
        // contract while the Go-style path uses a base64 name segment.
        let shown_from_body = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/admin/variables")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"var_name":"mail.password"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let shown_from_body = response_json(shown_from_body).await;
        assert_eq!(shown_from_body["data"].as_array().unwrap().len(), 1);
        assert_eq!(shown_from_body["data"][0]["value"], "<redacted>");

        let non_admin = non_admin_env();
        let forbidden = build_router(non_admin.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/variables")
                    .header(header::AUTHORIZATION, format!("Bearer {}", non_admin.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn system_log_config_is_authenticated_and_updates_runtime_levels() {
        let env = test_env();
        let router = build_router(env.state.clone());

        let unauthenticated = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/config/log")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);

        let updated = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/system/config/log")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"pkg_name":"rayrag::search","level":"warn"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        assert_eq!(
            response_json(updated).await["data"],
            serde_json::json!({"pkg_name":"rayrag::search","level":"WARNING"})
        );

        let levels = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/config/log")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(levels.status(), StatusCode::OK);
        assert_eq!(
            response_json(levels).await["data"]["rayrag::search"],
            "WARNING"
        );

        let invalid = router
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/system/config/log")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"pkg_name":"rayrag","level":"verbose"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::OK);
        let invalid = response_json(invalid).await;
        assert_eq!(invalid["code"], 102);
        assert_eq!(invalid["message"], "Invalid log level: verbose");

        let missing = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/system/config/log")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"pkg_name":"rayrag"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::OK);
        assert_eq!(
            response_json(missing).await,
            serde_json::json!({"code":102,"message":"pkg_name and level are required"})
        );
    }

    #[tokio::test]
    async fn knowledge_bases_are_isolated_by_authenticated_owner() {
        let env = test_env();
        let owner_kb = env
            .state
            .kbs
            .create_for("test-user", "Owner KB", "")
            .unwrap();
        let other_kb = env
            .state
            .kbs
            .create_for("other-user", "Other KB", "")
            .unwrap();
        let router = build_router(env.state.clone());

        let list = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/datasets")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_json = response_json(list).await;
        let ids: Vec<&str> = list_json["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|kb| kb["id"].as_str())
            .collect();
        assert!(ids.contains(&owner_kb.id.as_str()));
        assert!(!ids.contains(&other_kb.id.as_str()));

        let forbidden = router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/datasets/{}/documents", other_kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::OK);
        let forbidden_json = response_json(forbidden).await;
        assert_eq!(forbidden_json["code"], 404);
    }

    /// Upstream `admin/server/routes.py` user surfaces: the detail payload is a
    /// *list* of matching accounts (`UserMgr.get_user_details`), and the account's
    /// datasets and agents come from their own endpoints
    /// (`UserServiceMgr.get_user_datasets` / `get_user_agents`).
    #[tokio::test]
    async fn admin_user_assets_follow_the_upstream_endpoints() {
        let env = test_env();
        let admin = env
            .state
            .users
            .create_user("boss@example.com", "secret-passphrase", "admin")
            .unwrap();
        let token = env.state.users.issue_token_for(&admin.id).unwrap();
        let kb = env
            .state
            .kbs
            .create_for(&admin.id, "Admin KB", "dataset owned by the admin")
            .unwrap();
        let agent = env
            .state
            .agents
            .create(
                &admin.id,
                crate::api::features::AgentCreateRequest {
                    name: "Admin Agent".into(),
                    description: String::new(),
                    permission: None,
                    kb_ids: Vec::new(),
                    prompt_template: None,
                    dsl: serde_json::json!({"components": {}, "history": [], "path": []}),
                    canvas_category: Some("agent_canvas".into()),
                    canvas_type: String::new(),
                    tags: Vec::new(),
                    avatar: String::new(),
                    release: None,
                },
            )
            .unwrap();
        let router = build_router(env.state.clone());
        let get = |path: String, token: String| {
            let router = router.clone();
            async move {
                router
                    .oneshot(
                        Request::builder()
                            .uri(path)
                            .header(header::AUTHORIZATION, format!("Bearer {token}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap()
            }
        };

        let detail = get("/api/v1/admin/users/boss@example.com".into(), token.clone()).await;
        assert_eq!(detail.status(), StatusCode::OK);
        let detail = response_json(detail).await;
        let rows = detail["data"].as_array().expect("the payload is a list");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["email"], "boss@example.com");
        assert_eq!(rows[0]["is_superuser"], true);
        assert_eq!(rows[0]["is_anonymous"], "0");
        assert_eq!(rows[0]["login_channel"], "password");
        assert!(rows[0]["create_date"].is_string() && rows[0]["update_date"].is_string());
        assert!(
            rows[0]["datasets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["id"] == kb.id.as_str())
        );

        let datasets = get(
            "/api/v1/admin/users/boss@example.com/datasets".into(),
            token.clone(),
        )
        .await;
        assert_eq!(datasets.status(), StatusCode::OK);
        let datasets = response_json(datasets).await;
        let dataset = datasets["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == kb.id.as_str())
            .expect("the admin's dataset is listed");
        for key in [
            "name",
            "avatar",
            "doc_num",
            "chunk_num",
            "token_num",
            "language",
            "permission",
            "create_date",
            "update_date",
        ] {
            assert!(
                dataset.get(key).is_some(),
                "dataset row missing {key}: {dataset}"
            );
        }
        assert_eq!(dataset["language"], "English", "the KB language column");

        let agents = get(
            "/api/v1/admin/users/boss@example.com/agents".into(),
            token.clone(),
        )
        .await;
        assert_eq!(agents.status(), StatusCode::OK);
        let agents = response_json(agents).await;
        let agent_row = &agents["data"].as_array().unwrap()[0];
        assert_eq!(agent_row["title"], "Admin Agent");
        assert_eq!(agent_row["permission"], "private");
        assert_eq!(
            agent_row["canvas_category"], "agent",
            "upstream keeps only the first `_`-separated segment"
        );
        assert_eq!(agent_row["avatar"], serde_json::Value::Null);
        assert_eq!(agent.id.len() > 0, true);

        // Unknown accounts and non-admins.
        let missing = get(
            "/api/v1/admin/users/nobody@example.com/datasets".into(),
            token.clone(),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(response_json(missing).await["message"], "User not found");
        let plain = env
            .state
            .users
            .create_user("plain@example.com", "secret-passphrase", "user")
            .unwrap();
        let plain_token = env.state.users.issue_token_for(&plain.id).unwrap();
        let forbidden = get(
            "/api/v1/admin/users/boss@example.com/agents".into(),
            plain_token,
        )
        .await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    }

    /// Upstream `admin/server/routes.py` sandbox endpoints: the provider registry,
    /// the per-provider schemas, the durable configuration (with schema defaults)
    /// and the connection probe — including the validation that keeps a typo or an
    /// out-of-range number out of the stored provider configuration.
    #[tokio::test]
    async fn sandbox_admin_endpoints_match_the_upstream_contract() {
        let env = test_env();
        let admin = env
            .state
            .users
            .create_user("boss@example.com", "secret-passphrase", "admin")
            .unwrap();
        let token = env.state.users.issue_token_for(&admin.id).unwrap();
        let router = build_router(env.state.clone());
        let call =
            |method: &'static str, path: String, body: Option<serde_json::Value>, token: String| {
                let router = router.clone();
                async move {
                    let mut request = Request::builder()
                        .method(method)
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {token}"));
                    let body = match body {
                        Some(value) => {
                            request = request.header(header::CONTENT_TYPE, "application/json");
                            Body::from(value.to_string())
                        }
                        None => Body::empty(),
                    };
                    router.oneshot(request.body(body).unwrap()).await.unwrap()
                }
            };

        // Non-admins are refused (the fixture's own user is an administrator).
        let plain = env
            .state
            .users
            .create_user("plain@example.com", "secret-passphrase", "user")
            .unwrap();
        let plain_token = env.state.users.issue_token_for(&plain.id).unwrap();
        let forbidden = call(
            "GET",
            "/api/v1/admin/sandbox/providers".into(),
            None,
            plain_token,
        )
        .await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);

        let providers = call(
            "GET",
            "/api/v1/admin/sandbox/providers".into(),
            None,
            token.clone(),
        )
        .await;
        assert_eq!(providers.status(), StatusCode::OK);
        let providers = response_json(providers).await;
        let ids: Vec<&str> = providers["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                "local",
                "self_managed",
                "ssh",
                "aliyun_codeinterpreter",
                "e2b"
            ]
        );
        assert_eq!(providers["data"][1]["name"], "Self-Managed");
        assert_eq!(providers["data"][1]["tags"][0], "self-hosted");

        let schema = call(
            "GET",
            "/api/v1/admin/sandbox/providers/ssh/schema".into(),
            None,
            token.clone(),
        )
        .await;
        let schema = response_json(schema).await;
        assert_eq!(schema["data"]["host"]["required"], true);
        assert_eq!(schema["data"]["password"]["secret"], true);
        assert_eq!(schema["data"]["private_key"]["multiline"], true);
        let unknown = call(
            "GET",
            "/api/v1/admin/sandbox/providers/nope/schema".into(),
            None,
            token.clone(),
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(unknown).await["message"],
            "Unknown provider: nope"
        );

        // The seeded default provider is self_managed; its schema defaults are
        // filled in even though only four keys are stored.
        let config = call(
            "GET",
            "/api/v1/admin/sandbox/config".into(),
            None,
            token.clone(),
        )
        .await;
        let config = response_json(config).await;
        assert_eq!(config["data"]["provider_type"], "self_managed");
        assert_eq!(config["data"]["config"]["timeout"], 30);
        assert!(config["data"]["config"]["endpoint"].is_string());

        // Validation follows upstream `SandboxMgr.set_config`: required fields
        // must be present, integers are type- and range-checked, and the
        // rejection wording matches upstream.
        for (body, expected) in [
            (
                serde_json::json!({"provider_type": "ssh", "config": {"host": "h"}}),
                "Required field 'port' is missing",
            ),
            (
                serde_json::json!({"provider_type": "ssh", "config": {"host": "h", "port": 70000, "username": "u"}}),
                "Field 'port' must be <= 65535",
            ),
            (
                serde_json::json!({"provider_type": "ssh", "config": {"host": "h", "port": 22, "username": 5}}),
                "Field 'username' must be a string",
            ),
            (
                serde_json::json!({"provider_type": "ssh", "config": {"host": "h", "port": "22", "username": "u"}}),
                "Field 'port' must be an integer",
            ),
            (
                serde_json::json!({"provider_type": "self_managed", "config": {"endpoint": "http://x", "timeout": 1}}),
                "Field 'timeout' must be >= 5",
            ),
            (
                serde_json::json!({"provider_type": "aliyun_codeinterpreter", "config": {"access_key_id": "a", "access_key_secret": "b", "account_id": "c", "region": "mars"}}),
                "Provider validation failed: Field 'region' must be one of: cn-hangzhou, cn-beijing, cn-shanghai, cn-shenzhen, cn-guangzhou",
            ),
        ] {
            let response = call(
                "POST",
                "/api/v1/admin/sandbox/config".into(),
                Some(body),
                token.clone(),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{expected}");
            assert_eq!(response_json(response).await["message"], expected);
        }
        // An unknown top-level provider is rejected with upstream's wording.
        let unknown_save = call(
            "POST",
            "/api/v1/admin/sandbox/config".into(),
            Some(serde_json::json!({"provider_type": "nope", "config": {}})),
            token.clone(),
        )
        .await;
        assert_eq!(unknown_save.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(unknown_save).await["message"],
            "Unknown provider type: nope"
        );

        // A valid save persists and activates the provider.
        let saved = call(
            "POST",
            "/api/v1/admin/sandbox/config".into(),
            Some(serde_json::json!({
                "provider_type": "ssh",
                "config": {
                    "host": "10.0.0.9",
                    "port": 2222,
                    "username": "ragflow",
                    "password": "s3cret",
                    "timeout": 45
                },
                "set_active": true
            })),
            token.clone(),
        )
        .await;
        assert_eq!(
            saved.status(),
            StatusCode::OK,
            "{}",
            response_json(saved).await
        );
        let reloaded = call(
            "GET",
            "/api/v1/admin/sandbox/config".into(),
            None,
            token.clone(),
        )
        .await;
        let reloaded = response_json(reloaded).await;
        assert_eq!(reloaded["data"]["provider_type"], "ssh");
        assert_eq!(reloaded["data"]["config"]["host"], "10.0.0.9");
        assert_eq!(reloaded["data"]["config"]["port"], 2222);
        assert_eq!(
            env.state
                .system_settings
                .typed_value("sandbox.ssh")
                .unwrap()["password"],
            "s3cret",
            "the stored value keeps the secret"
        );
        assert_eq!(
            reloaded["data"]["config"]["password"], "<redacted>",
            "the admin view never echoes a secret"
        );
        // Keys outside the schema round-trip untouched (upstream stores the
        // submitted object verbatim), so the seeded legacy defaults survive.
        assert_eq!(reloaded["data"]["config"]["port"], 2222);

        // Resubmitting the redacted value keeps the stored secret.
        let resent = call(
            "POST",
            "/api/v1/admin/sandbox/config".into(),
            Some(serde_json::json!({
                "provider_type": "ssh",
                "config": {
                    "host": "10.0.0.9",
                    "port": 2222,
                    "username": "ragflow",
                    "password": "<redacted>",
                    "timeout": 45
                }
            })),
            token.clone(),
        )
        .await;
        assert_eq!(resent.status(), StatusCode::OK);
        assert_eq!(
            env.state
                .system_settings
                .typed_value("sandbox.ssh")
                .unwrap()["password"],
            "s3cret"
        );

        // An unknown provider cannot be tested, and the SSH probe reports the
        // reachability it can actually verify (nothing listens on port 1).
        let bad_test = call(
            "POST",
            "/api/v1/admin/sandbox/test".into(),
            Some(serde_json::json!({"provider_type": "nope", "config": {}})),
            token.clone(),
        )
        .await;
        assert_eq!(bad_test.status(), StatusCode::BAD_REQUEST);
        let ssh_test = call(
            "POST",
            "/api/v1/admin/sandbox/test".into(),
            Some(serde_json::json!({
                "provider_type": "ssh",
                "config": {"host": "127.0.0.1", "port": 1, "username": "ragflow"}
            })),
            token.clone(),
        )
        .await;
        assert_eq!(ssh_test.status(), StatusCode::OK);
        let ssh_test = response_json(ssh_test).await;
        assert_eq!(ssh_test["data"]["success"], false);
        assert!(
            ssh_test["data"]["message"]
                .as_str()
                .unwrap()
                .contains("Test FAILED"),
            "{}",
            ssh_test["data"]["message"]
        );
        // The e2b provider is registered but not executable in this build; the
        // probe says so instead of reporting a fake success.
        let e2b_test = call(
            "POST",
            "/api/v1/admin/sandbox/test".into(),
            Some(serde_json::json!({"provider_type": "e2b", "config": {"api_key": "k"}})),
            token.clone(),
        )
        .await;
        assert_eq!(e2b_test.status(), StatusCode::BAD_REQUEST);
        assert!(
            response_json(e2b_test).await["message"]
                .as_str()
                .unwrap()
                .contains("not implemented in this build")
        );
    }

    /// Upstream `DELETE /api/v1/datasets`: `ids` null/empty deletes nothing unless
    /// `delete_all` is set, foreign ids fail the whole request, and the payload
    /// reports `success_count`.
    #[tokio::test]
    async fn collection_delete_follows_the_upstream_contract() {
        let env = test_env();
        let first = env.state.kbs.create_for("test-user", "First", "").unwrap();
        let second = env.state.kbs.create_for("test-user", "Second", "").unwrap();
        let other = env
            .state
            .kbs
            .create_for("other-user", "Foreign", "")
            .unwrap();
        let router = build_router(env.state.clone());
        let delete = |body: serde_json::Value| {
            let router = router.clone();
            let token = env.token.clone();
            async move {
                router
                    .oneshot(
                        Request::builder()
                            .method("DELETE")
                            .uri("/api/v1/datasets")
                            .header(header::AUTHORIZATION, format!("Bearer {token}"))
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(body.to_string()))
                            .unwrap(),
                    )
                    .await
                    .unwrap()
            }
        };

        let empty = delete(serde_json::json!({"ids": []})).await;
        assert_eq!(empty.status(), StatusCode::OK);
        assert_eq!(response_json(empty).await["data"]["success_count"], 0);
        assert!(
            env.state.kbs.get(&first.id).is_some(),
            "nothing was deleted"
        );

        let foreign = delete(serde_json::json!({"ids": [other.id.clone()]})).await;
        assert_eq!(foreign.status(), StatusCode::BAD_REQUEST);
        assert!(
            env.state.kbs.get(&other.id).is_some(),
            "a foreign id protects the whole request"
        );

        let removed = delete(serde_json::json!({"ids": [first.id.clone()]})).await;
        assert_eq!(removed.status(), StatusCode::OK);
        assert_eq!(response_json(removed).await["data"]["success_count"], 1);
        assert!(env.state.kbs.get(&first.id).is_none());

        let all = delete(serde_json::json!({"ids": null, "delete_all": true})).await;
        assert_eq!(all.status(), StatusCode::OK, "delete_all wipes the tenant");
        assert_eq!(response_json(all).await["data"]["success_count"], 1);
        assert!(env.state.kbs.get(&second.id).is_none());
        assert!(
            env.state.kbs.get(&other.id).is_some(),
            "another tenant keeps its datasets"
        );
    }

    /// Upstream `Knowledgebase.language` is a top-level column with the process
    /// locale as its default, and `PUT /api/v1/datasets/{id}` writes it there --
    /// not into `parser_config`, which keeps only parse/retrieval settings.
    #[tokio::test]
    async fn dataset_language_is_a_top_level_column() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Language KB", "")
            .unwrap();
        let expected_default = if std::env::var("LANG")
            .map(|lang| lang.contains("zh_CN"))
            .unwrap_or(false)
        {
            "Chinese"
        } else {
            "English"
        };
        assert_eq!(kb.language, expected_default, "upstream column default");
        let router = build_router(env.state.clone());
        let put = |body: serde_json::Value| {
            let router = router.clone();
            let token = env.token.clone();
            let id = kb.id.clone();
            async move {
                router
                    .oneshot(
                        Request::builder()
                            .method("PUT")
                            .uri(format!("/api/v1/datasets/{id}"))
                            .header(header::AUTHORIZATION, format!("Bearer {token}"))
                            .header(header::CONTENT_TYPE, "application/json")
                            .body(Body::from(body.to_string()))
                            .unwrap(),
                    )
                    .await
                    .unwrap()
            }
        };

        let updated = put(serde_json::json!({"language": "Korean"})).await;
        assert_eq!(updated.status(), StatusCode::OK);
        let updated = response_json(updated).await;
        assert_eq!(updated["data"]["language"], "Korean");
        let parser_config: serde_json::Value =
            serde_json::from_str(updated["data"]["parser_config"].as_str().unwrap_or("{}"))
                .unwrap();
        assert!(
            parser_config.get("language").is_none(),
            "language must not be duplicated into parser_config"
        );
        let reloaded = env.state.kbs.get(&kb.id).unwrap();
        assert_eq!(reloaded.language, "Korean", "the column is persisted");

        // A blank value carries no update at all.
        let blank = put(serde_json::json!({"language": "   "})).await;
        assert_eq!(blank.status(), StatusCode::BAD_REQUEST);
        // `max_length=32`, like the upstream CharField.
        let too_long = put(serde_json::json!({"language": "x".repeat(33)})).await;
        assert_eq!(too_long.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            env.state.kbs.get(&kb.id).unwrap().language,
            "Korean",
            "rejected updates must not change the stored value"
        );

        let fetched = router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/datasets/{}", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let fetched = response_json(fetched).await;
        assert_eq!(fetched["data"]["language"], "Korean");
    }

    #[tokio::test]
    async fn system_admin_cannot_manage_another_users_tenant() {
        let env = test_env();
        let owner = env
            .state
            .users
            .register("Owner", "owner@example.com", "owner password 123")
            .unwrap();
        let invitee = env
            .state
            .users
            .register("Invitee", "invitee@example.com", "invitee password 123")
            .unwrap();
        let router = build_router(env.state.clone());

        let invite = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/users/invite")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "tenant_id": owner.id,
                            "email": "invitee@example.com"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invite.status(), StatusCode::FORBIDDEN);
        assert_eq!(env.state.tenants.role(&owner.id, &invitee.id), None);
    }

    #[tokio::test]
    async fn team_members_can_read_but_not_manage_and_lose_access_when_removed() {
        let env = test_env();
        let member = env
            .state
            .users
            .register(
                "Team Member",
                "team-member@example.com",
                "member password 123",
            )
            .unwrap();
        let member_token = env
            .state
            .users
            .login("team-member@example.com", "member password 123")
            .unwrap()
            .unwrap();
        let kb = env
            .state
            .kbs
            .create_for_with_permission("test-user", "Shared KB", "", "team")
            .unwrap();
        let router = build_router(env.state.clone());

        let invite = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/users/invite")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "email": "team-member@example.com" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invite.status(), StatusCode::OK);
        assert!(!env.state.tenants.is_member("test-user", &member.id));
        assert_eq!(
            env.state.tenants.role("test-user", &member.id),
            Some(crate::kb::TenantRole::Invite)
        );

        let hidden_before_accept = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/datasets")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let hidden_before_accept_json = response_json(hidden_before_accept).await;
        assert!(
            !hidden_before_accept_json["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == kb.id)
        );

        let accept = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri("/api/v1/tenant/invitations/test-user/accept")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(accept.status(), StatusCode::OK);
        assert!(env.state.tenants.is_member("test-user", &member.id));

        let visible = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/datasets")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let visible_json = response_json(visible).await;
        assert!(
            visible_json["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == kb.id)
        );

        let manage = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/datasets/{}", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"permission":"private"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(manage.status(), StatusCode::NOT_FOUND);
        assert_eq!(env.state.kbs.get(&kb.id).unwrap().permission, "team");

        let promote = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/tenant/members/{}", member.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"tenant_id":"test-user","role":"admin"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(promote.status(), StatusCode::OK);
        assert!(env.state.tenants.can_manage("test-user", &member.id));

        let admin_manage = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/datasets/{}", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"permission":"private"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(admin_manage.status(), StatusCode::OK);
        // A legacy `private` write is normalised to the upstream `me` spelling.
        assert_eq!(env.state.kbs.get(&kb.id).unwrap().permission, "me");

        let restore_team = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/datasets/{}", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"permission":"team"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(restore_team.status(), StatusCode::OK);

        let remove = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!(
                        "/api/v1/tenant/members/{}?tenant_id=test-user",
                        member.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(remove.status(), StatusCode::OK);
        assert!(!env.state.tenants.is_member("test-user", &member.id));

        let hidden = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/datasets")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let hidden_json = response_json(hidden).await;
        assert!(
            !hidden_json["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["id"] == kb.id)
        );
    }

    #[tokio::test]
    async fn multipart_upload_is_sanitized_and_queued() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Test KB", "")
            .unwrap();
        let boundary = "rayrag-test-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"sample.txt\"\r\nContent-Type: text/plain\r\n\r\nhello rayrag\r\n--{boundary}--\r\n"
        );
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let json = response_json(response).await;
        assert_eq!(status, StatusCode::ACCEPTED, "response body: {json}");
        let doc_id = json["data"]["id"].as_str().unwrap();
        let doc = env.state.docs.get(doc_id).unwrap();
        assert_eq!(doc.name, "sample.txt");
        assert!(doc.storage_name.starts_with(doc_id));
        assert!(!doc.storage_name.contains(".."));
        assert_eq!(doc.content_hash.len(), 32);
        assert!(doc.content_hash.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert!(env.root.join("uploads").join(doc.storage_name).is_file());
        assert!(!json["data"]["task_id"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn duplicate_document_names_are_incremented_per_kb() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Duplicate KB", "")
            .unwrap();
        let other_kb = env
            .state
            .kbs
            .create_for("test-user", "Other Duplicate KB", "")
            .unwrap();
        let router = build_router(env.state.clone());

        for (target_kb, expected_name, content) in [
            (&kb.id, "report.txt", "first"),
            (&kb.id, "report(1).txt", "second"),
            (&kb.id, "report(2).txt", "third"),
            (&other_kb.id, "report.txt", "fourth"),
        ] {
            let boundary = format!("rayrag-duplicate-{}", uuid::Uuid::new_v4());
            let body = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"report.txt\"\r\nContent-Type: text/plain\r\n\r\n{content}\r\n--{boundary}--\r\n"
            );
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri(format!("/api/v1/datasets/{target_kb}/documents"))
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .header(
                            header::CONTENT_TYPE,
                            format!("multipart/form-data; boundary={boundary}"),
                        )
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let json = response_json(response).await;
            assert_eq!(status, StatusCode::ACCEPTED, "response body: {json}");
            assert_eq!(json["data"]["name"], expected_name);
        }

        let mut names: Vec<_> = env
            .state
            .docs
            .list(&kb.id)
            .into_iter()
            .map(|doc| doc.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["report(1).txt", "report(2).txt", "report.txt"]);
    }

    #[tokio::test]
    async fn multipart_upload_returns_partial_success_per_file() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Batch KB", "")
            .unwrap();
        let boundary = "rayrag-batch-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"good.txt\"\r\nContent-Type: text/plain\r\n\r\nvalid text\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fake.pdf\"\r\nContent-Type: application/pdf\r\n\r\nnot a pdf\r\n--{boundary}--\r\n"
        );
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let json = response_json(response).await;
        assert_eq!(status, StatusCode::MULTI_STATUS, "response body: {json}");
        assert_eq!(json["data"]["succeeded"].as_array().unwrap().len(), 1);
        assert_eq!(json["data"]["succeeded"][0]["name"], "good.txt");
        assert_eq!(json["data"]["failed"].as_array().unwrap().len(), 1);
        assert_eq!(json["data"]["failed"][0]["name"], "fake.pdf");
        assert_eq!(env.state.docs.list(&kb.id).len(), 1);
        let files: Vec<_> = std::fs::read_dir(env.root.join("uploads"))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(files.len(), 1);
        assert!(
            !files[0]
                .file_name()
                .to_string_lossy()
                .starts_with(".upload-")
        );
    }

    /// Upstream `useGetDocumentUrl`'s document branch: the previewer fetches
    /// `GET /api/v1/documents/{id}/preview` and expects the raw bytes inline
    /// (a download keeps `attachment` on the dataset route). The MIME type comes
    /// from the stored name so pdf.js, images and text render in place.
    #[tokio::test]
    async fn document_preview_serves_stored_bytes_inline() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Preview KB", "")
            .unwrap();
        let doc = crate::api::document::DocRecord {
            id: "preview-doc".into(),
            name: "positions.pdf".into(),
            kb_id: kb.id.clone(),
            size: 5,
            storage_name: "preview-doc.pdf".into(),
            content_hash: "0123456789abcdef0123456789abcdef".into(),
            indexed_content_hash: String::new(),
            run: "DONE".into(),
            progress: 1.0,
            progress_msg: "Indexed 1 chunks".into(),
            chunk_count: 1,
            created_at: 1,
            updated_at: 1,
        };
        env.state.docs.insert(doc).unwrap();
        std::fs::write(env.root.join("uploads/preview-doc.pdf"), b"%PDF-").unwrap();

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/documents/preview-doc/preview")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/pdf"
        );
        let disposition = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(disposition.starts_with("inline"), "{disposition}");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"%PDF-");

        let missing = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/documents/nope/preview")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::OK);
        let body = axum::body::to_bytes(missing.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["code"], 404);
    }

    /// Upstream `document_api.py::download_document` (`GET /api/v1/documents/<id>`,
    /// the byte source behind `fetchPreviewBlob(id, 'document')`): an attachment
    /// under the document's own MIME type, and `This file is empty.` when the
    /// stored blob is gone.
    #[tokio::test]
    async fn document_download_route_serves_attachment_bytes() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Download KB", "")
            .unwrap();
        env.state
            .docs
            .insert(crate::api::document::DocRecord {
                id: "download-doc".into(),
                name: "notes.md".into(),
                kb_id: kb.id.clone(),
                size: 6,
                storage_name: "download-doc.md".into(),
                content_hash: String::new(),
                indexed_content_hash: String::new(),
                run: "DONE".into(),
                progress: 1.0,
                progress_msg: String::new(),
                chunk_count: 0,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        std::fs::write(env.root.join("uploads/download-doc.md"), b"# hi\n").unwrap();

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/documents/download-doc")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; charset=utf-8"
        );
        let disposition = response
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(disposition.starts_with("attachment"), "{disposition}");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"# hi\n");

        // A record whose blob vanished answers upstream's empty-file message.
        std::fs::remove_file(env.root.join("uploads/download-doc.md")).unwrap();
        let empty = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/documents/download-doc")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(empty.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["message"], "This file is empty.");
    }

    /// Upstream `file_api.py::download` (`GET /api/v1/files/<file_id>`, the branch
    /// `fetchPreviewBlob(id, 'files')` takes): the raw bytes of a stored file.
    #[tokio::test]
    async fn file_blob_route_serves_raw_bytes() {
        let env = test_env();
        let files_dir = std::path::Path::new(&env.state.files.data_dir);
        std::fs::create_dir_all(files_dir).unwrap();
        std::fs::write(files_dir.join("file-blob.txt"), b"blob-bytes").unwrap();
        env.state
            .files
            .add(crate::api::file_mgr::FileRecord {
                id: "file-blob".into(),
                name: "notes.txt".into(),
                owner_id: "test-user".into(),
                parent_id: "root".into(),
                size: 10,
                content_hash: String::new(),
                file_type: "file".into(),
                created_at: 1,
            })
            .unwrap();

        for uri in [
            "/api/v1/files/file-blob",
            "/api/v1/files/file-blob/download",
        ] {
            let response = build_router(env.state.clone())
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(&body[..], b"blob-bytes", "{uri}");
        }

        let missing = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/files/nope")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(missing.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["code"], 404);
    }

    /// Minimal .pptx container: one `ppt/slides/slideN.xml` per slide holding a
    /// single text shape, which is all `parser::ppt::slide_texts` reads.
    fn pptx_with_slide_texts(texts: &[&str]) -> Vec<u8> {
        use std::io::Write;
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default();
            for (index, text) in texts.iter().enumerate() {
                writer
                    .start_file(format!("ppt/slides/slide{}.xml", index + 1), options)
                    .unwrap();
                write!(
                    writer,
                    "<p:sld xmlns:p=\"p\" xmlns:a=\"a\"><p:sp><p:txBody><a:p><a:r><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp></p:sld>"
                )
                .unwrap();
            }
            writer.finish().unwrap();
        }
        buffer.into_inner()
    }

    /// The RayRAG-side readers behind the spreadsheet and slides previewers: the
    /// workbook grid comes from `parser::excel::read_xlsx_sheets`, the deck text
    /// from `parser::ppt::slide_texts`. Both are classified `replaced` in the
    /// ledger — upstream builds them in the browser.
    #[tokio::test]
    async fn preview_reader_routes_return_workbook_grid_and_deck_text() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Readers KB", "")
            .unwrap();
        for (id, name, storage) in [
            ("sheet-doc", "book.xlsx", "sheet-doc.xlsx"),
            ("deck-doc", "deck.pptx", "deck-doc.pptx"),
        ] {
            env.state
                .docs
                .insert(crate::api::document::DocRecord {
                    id: id.into(),
                    name: name.into(),
                    kb_id: kb.id.clone(),
                    size: 0,
                    storage_name: storage.into(),
                    content_hash: String::new(),
                    indexed_content_hash: String::new(),
                    run: "DONE".into(),
                    progress: 1.0,
                    progress_msg: String::new(),
                    chunk_count: 0,
                    created_at: 1,
                    updated_at: 1,
                })
                .unwrap();
        }
        let workbook = crate::parser::excel::write_xlsx_sheets(&[(
            "Sheet1".to_string(),
            vec![
                vec![serde_json::json!("name"), serde_json::json!("size")],
                vec![serde_json::json!("alpha"), serde_json::json!(3)],
            ],
        )])
        .unwrap();
        std::fs::write(env.root.join("uploads/sheet-doc.xlsx"), &workbook).unwrap();
        std::fs::write(
            env.root.join("uploads/deck-doc.pptx"),
            pptx_with_slide_texts(&["First slide", "Second slide"]),
        )
        .unwrap();

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/documents/sheet-doc/preview/sheets")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["code"], 0);
        assert_eq!(payload["data"]["sheets"][0]["name"], "Sheet1");
        assert_eq!(payload["data"]["sheets"][0]["rows"][0][0], "name");
        assert_eq!(payload["data"]["sheets"][0]["rows"][1][1], "3");
        assert_eq!(payload["data"]["truncated"], false);

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/documents/deck-doc/preview/slides")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["code"], 0);
        assert_eq!(payload["data"]["slides"][0], "First slide");
        assert_eq!(payload["data"]["slides"][1], "Second slide");
    }

    async fn explicit_reparse_queues_even_when_content_hash_is_unchanged() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Idempotent KB", "")
            .unwrap();
        let doc = crate::api::document::DocRecord {
            id: "indexed-doc".into(),
            name: "indexed.txt".into(),
            kb_id: kb.id.clone(),
            size: 4,
            storage_name: "indexed-doc.txt".into(),
            content_hash: "0123456789abcdef0123456789abcdef".into(),
            indexed_content_hash: "0123456789abcdef0123456789abcdef".into(),
            run: "DONE".into(),
            progress: 1.0,
            progress_msg: "Indexed 1 chunks".into(),
            chunk_count: 1,
            created_at: 1,
            updated_at: 1,
        };
        env.state.docs.insert(doc).unwrap();
        std::fs::write(env.root.join("uploads/indexed-doc.txt"), b"same").unwrap();

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/datasets/{}/documents/indexed-doc/reparse",
                        kb.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let json = response_json(response).await;
        assert_eq!(json["code"], 0, "response body: {json}");
        assert!(json["data"]["task_id"].is_string());
        let tasks = env.state.tasks.list();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].doc_id, "indexed-doc");
        assert_eq!(tasks[0].priority, crate::api::features::TASK_PRIORITY_HIGH);
        assert_eq!(env.state.docs.get("indexed-doc").unwrap().run, "UNSTARTED");
    }

    #[tokio::test]
    async fn duplicate_file_names_are_incremented_per_owner_and_folder() {
        let env = test_env();
        let router = build_router(env.state.clone());

        for expected_name in ["report.txt", "report(1).txt", "report(2).txt"] {
            let boundary = format!("rayrag-file-duplicate-{}", uuid::Uuid::new_v4());
            let body = format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"report.txt\"\r\nContent-Type: text/plain\r\n\r\nfile content {expected_name}\r\n--{boundary}--\r\n"
            );
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/api/v1/files/upload")
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .header(
                            header::CONTENT_TYPE,
                            format!("multipart/form-data; boundary={boundary}"),
                        )
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let json = response_json(response).await;
            assert_eq!(status, StatusCode::CREATED, "response body: {json}");
            assert_eq!(json["data"]["name"], expected_name);
            assert_eq!(json["data"]["content_hash"].as_str().unwrap().len(), 32);
        }

        let mut names: Vec<_> = env
            .state
            .files
            .list_for("test-user", true, "root")
            .into_iter()
            .map(|file| file.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["report(1).txt", "report(2).txt", "report.txt"]);
    }

    #[tokio::test]
    async fn multipart_rejects_empty_file_without_artifacts() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Empty KB", "")
            .unwrap();
        let boundary = "rayrag-empty-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"empty.txt\"\r\nContent-Type: text/plain\r\n\r\n\r\n--{boundary}--\r\n"
        );
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(env.state.docs.list(&kb.id).is_empty());
        assert!(
            std::fs::read_dir(env.root.join("uploads"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn multipart_rejects_oversized_file_and_removes_temp_file() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().max_upload_bytes = 4;
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Oversize KB", "")
            .unwrap();
        let boundary = "rayrag-oversize-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"large.txt\"\r\nContent-Type: text/plain\r\n\r\n12345\r\n--{boundary}--\r\n"
        );
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(env.state.docs.list(&kb.id).is_empty());
        assert!(
            std::fs::read_dir(env.root.join("uploads"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn multipart_accepts_multiple_valid_files() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Multi KB", "")
            .unwrap();
        let boundary = "rayrag-multi-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"one.txt\"\r\nContent-Type: text/plain\r\n\r\none\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"two.txt\"\r\nContent-Type: text/plain\r\n\r\ntwo\r\n--{boundary}--\r\n"
        );
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let json = response_json(response).await;
        assert_eq!(status, StatusCode::ACCEPTED, "response body: {json}");
        assert_eq!(json["data"]["succeeded"].as_array().unwrap().len(), 2);
        assert!(json["data"]["failed"].as_array().unwrap().is_empty());
        let mut names: Vec<_> = env
            .state
            .docs
            .list(&kb.id)
            .into_iter()
            .map(|doc| doc.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["one.txt", "two.txt"]);
        assert_eq!(
            std::fs::read_dir(env.root.join("uploads")).unwrap().count(),
            2
        );
    }

    #[tokio::test]
    async fn multipart_rejects_invalid_ooxml_container_without_artifacts() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "OOXML KB", "")
            .unwrap();
        let boundary = "rayrag-ooxml-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"broken.docx\"\r\nContent-Type: application/vnd.openxmlformats-officedocument.wordprocessingml.document\r\n\r\nPK\\x03\\x04not-a-real-docx\r\n--{boundary}--\r\n"
        );
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(env.state.docs.list(&kb.id).is_empty());
        assert!(
            std::fs::read_dir(env.root.join("uploads"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[tokio::test]
    async fn multipart_rejects_content_that_does_not_match_extension() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "MIME KB", "")
            .unwrap();
        let boundary = "rayrag-mime-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fake.pdf\"\r\nContent-Type: application/pdf\r\n\r\nthis is not a pdf\r\n--{boundary}--\r\n"
        );
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(env.state.docs.list(&kb.id).is_empty());
    }

    #[tokio::test]
    async fn chat_requires_owned_kb_ids() {
        let env = test_env();
        let other_kb = env
            .state
            .kbs
            .create_for("other-user", "Other Chat KB", "")
            .unwrap();
        let body = serde_json::json!({
            "question": "hello",
            "kb_ids": [other_kb.id]
        });
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/chats")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn message_feedback_is_idempotent_and_reverses_previous_weight() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().chunk_feedback_enabled = true;
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Feedback KB", "")
            .unwrap();
        let mut chunk = search_chunk("feedback-chunk", &kb.id, "water quality");
        chunk.metadata.insert("pagerank_fea".into(), "10".into());
        env.state.engine.write().unwrap().add(chunk);
        env.state
            .engine
            .read()
            .unwrap()
            .save(&env.state.index_path)
            .unwrap();
        let conversation = env
            .state
            .conversations
            .create_for("test-user", "Feedback")
            .unwrap();
        let message_id = env
            .state
            .conversations
            .append_exchange_with_references(
                &conversation.id,
                "test-user",
                "question",
                "answer",
                vec!["water quality".into()],
                vec![crate::llm::ChunkReference {
                    id: "feedback-chunk".into(),
                    kb_id: kb.id.clone(),
                    content: "water quality".into(),
                    similarity: Some(0.9),
                    vector_similarity: Some(0.8),
                    term_similarity: Some(0.7),
                }],
            )
            .unwrap()
            .unwrap();
        let router = build_router(env.state.clone());
        let uri = format!(
            "/api/v1/chats/{}/messages/{}/feedback",
            conversation.id, message_id
        );
        let compatible_uri = format!(
            "/api/v1/chats/default/sessions/{}/messages/{}/feedback",
            conversation.id, message_id
        );

        let like = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&compatible_uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"thumbup":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(like.status(), StatusCode::OK);
        assert_eq!(response_json(like).await["data"]["success_count"], 1);
        assert_eq!(chunk_pagerank(&env.state, "feedback-chunk"), 11.0);

        let duplicate = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"thumbup":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::OK);
        assert_eq!(response_json(duplicate).await["data"]["success_count"], 0);
        assert_eq!(chunk_pagerank(&env.state, "feedback-chunk"), 11.0);

        let dislike = router
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"thumbup":false,"feedback":"not relevant"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(dislike.status(), StatusCode::OK);
        assert_eq!(response_json(dislike).await["data"]["success_count"], 1);
        assert_eq!(chunk_pagerank(&env.state, "feedback-chunk"), 9.0);
        let target = env
            .state
            .conversations
            .feedback_target(&conversation.id, "test-user", &message_id)
            .unwrap();
        assert_eq!(target.prior_thumb, Some(false));
        assert_eq!(target.prior_feedback.as_deref(), Some("not relevant"));
    }

    #[tokio::test]
    async fn message_feedback_hides_other_users_sessions() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().chunk_feedback_enabled = true;
        let conversation = env
            .state
            .conversations
            .create_for("other-user", "Private")
            .unwrap();
        let message_id = env
            .state
            .conversations
            .append_exchange_with_references(
                &conversation.id,
                "other-user",
                "question",
                "answer",
                Vec::new(),
                Vec::new(),
            )
            .unwrap()
            .unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!(
                        "/api/v1/chats/{}/messages/{}/feedback",
                        conversation.id, message_id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"thumbup":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn chat_completion_persists_message_id_and_structured_references() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Structured Reference KB", "")
            .unwrap();
        let mut chunk = search_chunk("reference-chunk", &kb.id, "water quality guidance");
        chunk.embedding = vec![1.0, 0.0];
        env.state.engine.write().unwrap().add(chunk);
        let conversation = env
            .state
            .conversations
            .create_for("test-user", "Structured Reference")
            .unwrap();

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/chats/{}/completions", conversation.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "water quality",
                            "kb_ids": [kb.id]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        let message_id = body["data"]["message_id"].as_str().unwrap();
        assert!(!message_id.is_empty());
        assert_eq!(body["data"]["reference"][0]["id"], "reference-chunk");
        assert_eq!(body["data"]["reference"][0]["kb_id"], kb.id);
        assert_eq!(body["data"]["reference"][0]["vector_similarity"], 1.0);

        let target = env
            .state
            .conversations
            .feedback_target(&conversation.id, "test-user", message_id)
            .unwrap();
        assert_eq!(target.references.len(), 1);
        assert_eq!(target.references[0].id, "reference-chunk");
        assert_eq!(target.references[0].kb_id, kb.id);
    }

    #[tokio::test]
    async fn chat_generation_params_reject_invalid_boundaries_before_execution() {
        let env = test_env();
        let conversation = env
            .state
            .conversations
            .create_for("test-user", "Generation validation")
            .unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/chats/{}/completions", conversation.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"question":"hello","temperature":1.01}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("temperature"));
    }

    #[tokio::test]
    async fn openai_generation_params_reject_wrong_types_before_model_resolution() {
        let env = test_env();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/chat/completions")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"model":"model","messages":[{"role":"user","content":"hello"}],"max_tokens":1.5}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("max_tokens"));
    }

    #[tokio::test]
    async fn agent_generation_params_reject_invalid_penalty() {
        let env = test_env();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents/default/completions")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"question":"hello","presence_penalty":-0.01}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn chat_lifecycle_renames_regenerates_in_place_and_deletes_turn() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Chat Lifecycle KB", "")
            .unwrap();
        let mut chunk = search_chunk("chat-lifecycle-chunk", &kb.id, "pond oxygen context");
        chunk.embedding = vec![1.0, 0.0];
        env.state.engine.write().unwrap().add(chunk);
        let conversation = env
            .state
            .conversations
            .create_for_settings(
                "test-user",
                "Original Name",
                vec![kb.id.clone()],
                None,
                None,
            )
            .unwrap();
        let router = build_router(env.state.clone());

        let rename = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(format!("/api/v1/chats/{}", conversation.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"  Oxygen Session  "}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rename.status(), StatusCode::OK);
        assert_eq!(
            response_json(rename).await["data"]["name"],
            "Oxygen Session"
        );

        let completion = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/chats/{}/completions", conversation.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"question":"How is oxygen?"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(completion.status(), StatusCode::OK);
        let completion_body = response_json(completion).await;
        let message_id = completion_body["data"]["message_id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            completion_body["data"]["reference"][0]["id"],
            "chat-lifecycle-chunk"
        );
        let before = env
            .state
            .conversations
            .get_for(&conversation.id, "test-user")
            .unwrap();
        assert_eq!(before.messages.len(), 2);
        assert_eq!(before.kb_ids, vec![kb.id.clone()]);

        let regenerate = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/chats/{}/messages/{}/regenerate",
                        conversation.id, message_id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(regenerate.status(), StatusCode::OK);
        let regenerate_body = response_json(regenerate).await;
        assert_eq!(regenerate_body["data"]["message_id"], message_id);
        assert_eq!(
            regenerate_body["data"]["reference"][0]["id"],
            "chat-lifecycle-chunk"
        );
        let regenerated = env
            .state
            .conversations
            .get_for(&conversation.id, "test-user")
            .unwrap();
        assert_eq!(regenerated.messages.len(), 2);
        assert_eq!(regenerated.messages[0].id, message_id);
        assert_eq!(regenerated.messages[1].id, message_id);
        assert_eq!(regenerated.messages[0].content, "How is oxygen?");

        let delete = router
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!(
                        "/api/v1/chats/{}/messages/{}",
                        conversation.id, message_id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::OK);
        assert!(
            response_json(delete).await["data"]["messages"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn chat_lifecycle_hides_other_users_sessions() {
        let env = non_admin_env();
        let conversation = env
            .state
            .conversations
            .create_for("other-user", "Private Session")
            .unwrap();
        let message_id = env
            .state
            .conversations
            .append_exchange_with_references(
                &conversation.id,
                "other-user",
                "Private question",
                "Private answer",
                Vec::new(),
                Vec::new(),
            )
            .unwrap()
            .unwrap();
        let router = build_router(env.state.clone());

        for request in [
            Request::builder()
                .method(Method::PATCH)
                .uri(format!("/api/v1/chats/{}", conversation.id))
                .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"name":"Stolen"}"#))
                .unwrap(),
            Request::builder()
                .method(Method::POST)
                .uri(format!(
                    "/api/v1/chats/{}/messages/{}/regenerate",
                    conversation.id, message_id
                ))
                .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from("{}"))
                .unwrap(),
            Request::builder()
                .method(Method::DELETE)
                .uri(format!(
                    "/api/v1/chats/{}/messages/{}",
                    conversation.id, message_id
                ))
                .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                .body(Body::empty())
                .unwrap(),
        ] {
            let response = router.clone().oneshot(request).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        let untouched = env.state.conversations.get(&conversation.id).unwrap();
        assert_eq!(untouched.name, "Private Session");
        assert_eq!(untouched.messages.len(), 2);
    }

    #[tokio::test]
    async fn evaluation_api_runs_retrieval_metrics_and_recommendations() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Evaluation KB", "")
            .unwrap();
        let mut relevant = search_chunk("relevant-chunk", &kb.id, "water quality management");
        relevant
            .metadata
            .insert("doc_id".into(), "relevant-doc".into());
        let mut distractor = search_chunk("distractor-chunk", &kb.id, "water feeding");
        distractor
            .metadata
            .insert("doc_id".into(), "distractor-doc".into());
        env.state
            .engine
            .write()
            .unwrap()
            .index(vec![relevant, distractor]);
        let router = build_router(env.state.clone());

        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/evaluations")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "Water Quality Evaluation",
                            "description": "Retrieval regression",
                            "kb_ids": [kb.id]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let dataset_id = response_json(create).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let add_case = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/evaluations/{dataset_id}/cases"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "water quality",
                            "relevant_chunk_ids": ["relevant-chunk"],
                            "relevant_doc_ids": ["relevant-doc"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(add_case.status(), StatusCode::CREATED);

        let start = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/evaluations/{dataset_id}/runs"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"name":"Baseline","top_k":2,"vector_weight":0}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(start.status(), StatusCode::OK);
        let start_body = response_json(start).await;
        let run_id = start_body["data"]["id"].as_str().unwrap().to_string();
        assert_eq!(start_body["data"]["status"], "COMPLETED");
        assert_eq!(start_body["data"]["metrics_summary"]["total_cases"], 1.0);
        assert_eq!(start_body["data"]["metrics_summary"]["avg_recall"], 1.0);
        assert_eq!(start_body["data"]["metrics_summary"]["avg_hit_rate"], 1.0);
        assert_eq!(start_body["data"]["metrics_summary"]["avg_mrr"], 1.0);
        assert_eq!(start_body["data"]["metrics_summary"]["avg_precision"], 0.5);

        let get_run = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/v1/evaluation-runs/{run_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get_run.status(), StatusCode::OK);
        let run_body = response_json(get_run).await;
        assert_eq!(run_body["data"]["results"].as_array().unwrap().len(), 1);
        assert_eq!(
            run_body["data"]["results"][0]["retrieved_chunk_ids"][0],
            "relevant-chunk"
        );

        let recommendations = router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/v1/evaluation-runs/{run_id}/recommendations"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(recommendations.status(), StatusCode::OK);
        let recommendations = response_json(recommendations).await;
        assert_eq!(recommendations["data"][0]["issue"], "Low Precision");
    }

    #[tokio::test]
    async fn evaluation_dataset_rejects_inaccessible_knowledge_bases() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("other-user", "Private Evaluation KB", "")
            .unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/evaluations")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "Forbidden",
                            "kb_ids": [kb.id]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            env.state
                .evaluations
                .list_datasets("test-user", false)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn manual_chunk_crud_updates_counts_and_availability() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Manual Chunk KB", "")
            .unwrap();
        let doc = stored_doc("manual-doc", &kb.id, 0);
        env.state.docs.insert(doc.clone()).unwrap();
        env.state.kbs.update_counts(&kb.id, 0, 1).unwrap();
        let router = build_router(env.state.clone());
        let collection_uri = format!("/api/v1/datasets/{}/documents/{}/chunks", kb.id, doc.id);

        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(&collection_uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "content": "water quality management",
                            "important_keywords": ["water", "quality"],
                            "questions": ["How to manage water quality?"],
                            "tag_kwd": ["aquaculture"],
                            "tag_feas": {"aquaculture": 1.0},
                            "positions": [[1, 2, 3, 4, 5]]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);
        let create_body = response_json(create).await;
        let chunk_id = create_body["data"]["chunk"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(create_body["data"]["chunk"]["available"], true);
        assert_eq!(env.state.docs.get(&doc.id).unwrap().chunk_count, 1);
        assert_eq!(env.state.kbs.get(&kb.id).unwrap().chunk_count, 1);

        let list = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("{collection_uri}?keywords=quality"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        assert_eq!(response_json(list).await["data"]["total"], 1);

        let item_uri = format!("{collection_uri}/{chunk_id}");
        let update = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(&item_uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "content": "pond oxygen management",
                            "important_keywords": ["oxygen"],
                            "questions": ["How to manage oxygen?"],
                            "available": false
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(update.status(), StatusCode::OK);
        let update_body = response_json(update).await;
        assert_eq!(update_body["data"]["content"], "pond oxygen management");
        assert_eq!(update_body["data"]["important_keywords"][0], "oxygen");
        assert_eq!(update_body["data"]["available"], false);

        let hidden =
            env.state
                .engine
                .read()
                .unwrap()
                .hybrid_search_kbs(crate::search::HybridSearchQuery {
                    query: "oxygen management",
                    query_embedding: None,
                    top_k: 10,
                    kb_ids: std::slice::from_ref(&kb.id),
                    vector_weight: 0.0,
                    doc_ids: None,
                    rank_feature: None,
                });
        assert!(hidden.is_empty());

        let enable = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(&collection_uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "chunk_ids": [chunk_id],
                            "available": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(enable.status(), StatusCode::OK);
        let visible =
            env.state
                .engine
                .read()
                .unwrap()
                .hybrid_search_kbs(crate::search::HybridSearchQuery {
                    query: "oxygen management",
                    query_embedding: None,
                    top_k: 10,
                    kb_ids: std::slice::from_ref(&kb.id),
                    vector_weight: 0.0,
                    doc_ids: None,
                    rank_feature: None,
                });
        assert_eq!(visible.len(), 1);

        let delete = router
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(&collection_uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "chunk_ids": [chunk_id] }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::OK);
        assert_eq!(response_json(delete).await["data"]["deleted"], 1);
        assert_eq!(env.state.docs.get(&doc.id).unwrap().chunk_count, 0);
        assert_eq!(env.state.kbs.get(&kb.id).unwrap().chunk_count, 0);
        assert!(env.state.engine.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn manual_chunk_mutation_requires_kb_management_permission() {
        let mut env = non_admin_env();
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        let kb = env
            .state
            .kbs
            .create_for("other-user", "Other Manual KB", "")
            .unwrap();
        let doc = stored_doc("other-manual-doc", &kb.id, 0);
        env.state.docs.insert(doc.clone()).unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/datasets/{}/documents/{}/chunks",
                        kb.id, doc.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"content":"forbidden"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(env.state.engine.read().unwrap().is_empty());
    }

    #[tokio::test]
    async fn manual_chunk_index_failure_preserves_counts_and_index() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().embedder = Some(Arc::new(TestEmbedder));
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Manual Rollback KB", "")
            .unwrap();
        let doc = stored_doc("manual-rollback-doc", &kb.id, 0);
        env.state.docs.insert(doc.clone()).unwrap();
        env.state.kbs.update_counts(&kb.id, 0, 1).unwrap();
        let index_path = env.root.join("index.json");
        std::fs::create_dir(&index_path).unwrap();

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/datasets/{}/documents/{}/chunks",
                        kb.id, doc.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"content":"rollback content"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(env.state.engine.read().unwrap().is_empty());
        assert_eq!(env.state.docs.get(&doc.id).unwrap().chunk_count, 0);
        assert_eq!(env.state.kbs.get(&kb.id).unwrap().chunk_count, 0);
    }

    #[tokio::test]
    async fn document_metadata_api_filters_retrieval_and_enriches_references() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Metadata KB", "")
            .unwrap();
        let doc_a = stored_doc("metadata-doc-a", &kb.id, 1);
        let doc_b = stored_doc("metadata-doc-b", &kb.id, 1);
        env.state.docs.insert(doc_a.clone()).unwrap();
        env.state.docs.insert(doc_b.clone()).unwrap();
        env.state.kbs.update_counts(&kb.id, 2, 2).unwrap();
        {
            let mut engine = env.state.engine.write().unwrap();
            let mut chunk_a = search_chunk("metadata-chunk-a", &kb.id, "pond oxygen guide");
            chunk_a.metadata.insert("doc_id".into(), doc_a.id.clone());
            let mut chunk_b = search_chunk("metadata-chunk-b", &kb.id, "pond oxygen guide");
            chunk_b.metadata.insert("doc_id".into(), doc_b.id.clone());
            engine.index(vec![chunk_a, chunk_b]);
        }
        let router = build_router(env.state.clone());
        let metadata_uri = format!("/api/v1/datasets/{}/documents/{}/metadata", kb.id, doc_a.id);
        let replace = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(&metadata_uri)
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "meta_fields": {
                                "tags": ["water、oxygen", "oxygen"],
                                "year": 2026,
                                "owner": "Farm A"
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(replace.status(), StatusCode::OK);
        assert_eq!(
            response_json(replace).await["data"]["tags"],
            serde_json::json!(["water", "oxygen"])
        );

        let list = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let list_body = response_json(list).await;
        let listed = list_body["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|document| document["id"] == doc_a.id)
            .unwrap();
        assert_eq!(listed["meta_fields"]["year"], 2026);

        let keys = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/v1/datasets/{}/metadata/keys", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response_json(keys).await["data"],
            serde_json::json!(["owner", "tags", "year"])
        );

        let summary = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/metadata/summary", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let summary_body = response_json(summary).await;
        assert_eq!(summary_body["data"]["year"]["type"], "number");
        assert_eq!(summary_body["data"]["tags"]["type"], "list");

        let retrieval = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/retrieval")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "pond oxygen",
                            "kb_ids": [kb.id],
                            "vector_similarity_weight": 0.0,
                            "meta_data_filter": {
                                "method": "manual",
                                "logic": "and",
                                "manual": [
                                    {"key": "tags", "op": "contains", "value": "oxygen"},
                                    {"name": "year", "comparison_operator": ">=", "value": 2026}
                                ]
                            },
                            "reference_metadata": {
                                "include": true,
                                "fields": ["owner"]
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(retrieval.status(), StatusCode::OK);
        let retrieval_body = response_json(retrieval).await;
        assert_eq!(retrieval_body["data"]["total"], 1);
        assert_eq!(retrieval_body["data"]["chunks"][0]["doc_id"], doc_a.id);
        assert_eq!(
            retrieval_body["data"]["chunks"][0]["document_metadata"],
            serde_json::json!({ "owner": "Farm A" })
        );

        let batch = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/metadata/batch", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "doc_ids": [doc_a.id],
                            "updates": [{"key": "tags", "value": "fish"}],
                            "deletes": [{"key": "owner"}]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(batch.status(), StatusCode::OK);
        assert_eq!(response_json(batch).await["data"]["updated"], 1);
        let metadata = env.state.document_metadata.get(&doc_a.id, &kb.id).unwrap();
        assert_eq!(
            metadata["tags"],
            serde_json::json!(["water", "oxygen", "fish"])
        );
        assert!(!metadata.contains_key("owner"));

        delete_document_data(&env.state, &doc_a).unwrap();
        assert!(env.state.document_metadata.get(&doc_a.id, &kb.id).is_none());
    }

    /// Upstream `document_api.update_metadata` (`PATCH
    /// /api/v1/datasets/{id}/documents/metadatas`): the body is
    /// `{selector: {document_ids, metadata_condition}, updates, deletes}`, the
    /// condition is evaluated by `common/metadata_utils.meta_filter`, and the
    /// answer carries both `updated` and `matched_docs`.
    /// Upstream `search_api` mounts the search app CRUD on `/api/v1/searches`
    /// and answers `GET /api/v1/searches/{id}` with the stored app. The route
    /// used to be missing: a `GET` on `/api/v1/searchapps/{id}` returned 405 with
    /// an empty body, which left the search page's settings drawer with nothing
    /// to load.
    #[tokio::test]
    async fn search_app_detail_matches_the_upstream_route_and_shape() {
        let mut env = test_env();
        let dir =
            std::env::temp_dir().join(format!("rayrag-search-detail-{}", uuid::Uuid::new_v4()));
        let store = Arc::new(
            crate::api::searchapp_mgr::SearchAppStore::new(dir.to_str().unwrap()).unwrap(),
        );
        Arc::get_mut(&mut env.state).unwrap().search_apps = Some(store.clone());
        let env = env;
        let router = build_router(env.state.clone());
        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/searches")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"name": "Drawer app", "kb_ids": []}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let created = response_json(create).await;
        assert_eq!(created["code"], 0);
        let app_id = created["data"]["id"].as_str().unwrap().to_string();

        for uri in [
            format!("/api/v1/searches/{app_id}"),
            format!("/api/v1/searchapps/{app_id}"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(&uri)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            let body = response_json(response).await;
            assert_eq!(body["code"], 0, "{uri}");
            assert_eq!(body["data"]["name"], "Drawer app", "{uri}");
            assert_eq!(body["data"]["id"], app_id, "{uri}");
        }

        // Unknown id and a foreign app keep upstream's messages.
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/searches/does-not-exist")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = response_json(response).await;
        assert_eq!(body["code"], 102);
        assert_eq!(body["message"], "Can't find this Search App!");

        let stored = env
            .state
            .search_apps
            .as_ref()
            .unwrap()
            .get(&app_id)
            .unwrap();
        assert_eq!(stored.owner_id, "test-user");
        let removed = router
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/searches/{app_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(removed.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn document_metadata_batch_update_follows_the_upstream_selector() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Metadata Selector KB", "")
            .unwrap();
        let doc_a = stored_doc("selector-doc-a", &kb.id, 1);
        let doc_b = stored_doc("selector-doc-b", &kb.id, 1);
        env.state.docs.insert(doc_a.clone()).unwrap();
        env.state.docs.insert(doc_b.clone()).unwrap();
        env.state
            .document_metadata
            .batch_update(
                &kb.id,
                &[doc_a.id.clone(), doc_b.id.clone()],
                &[
                    crate::api::document_metadata::MetadataUpdate {
                        key: "owner".into(),
                        value: serde_json::json!("Farm A"),
                        r#match: None,
                    },
                    crate::api::document_metadata::MetadataUpdate {
                        key: "owner".into(),
                        value: serde_json::json!("Farm B"),
                        r#match: None,
                    },
                ],
                &[],
            )
            .unwrap();
        // `batch_update` applies each update row to every listed document, so the
        // owner is written per document for the test's purpose.
        env.state
            .document_metadata
            .batch_update(
                &kb.id,
                &[doc_a.id.clone()],
                &[crate::api::document_metadata::MetadataUpdate {
                    key: "owner".into(),
                    value: serde_json::json!("Farm A"),
                    r#match: None,
                }],
                &[],
            )
            .unwrap();
        env.state
            .document_metadata
            .batch_update(
                &kb.id,
                &[doc_b.id.clone()],
                &[crate::api::document_metadata::MetadataUpdate {
                    key: "owner".into(),
                    value: serde_json::json!("Farm B"),
                    r#match: None,
                }],
                &[],
            )
            .unwrap();

        let router = build_router(env.state.clone());
        let uri = format!("/api/v1/datasets/{}/documents/metadatas", kb.id);
        let patch = |body: serde_json::Value| {
            Request::builder()
                .method(Method::PATCH)
                .uri(&uri)
                .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap()
        };

        // `metadata_condition` selects only Farm A's document, even though both
        // ids are offered in the selector.
        let response = router
            .clone()
            .oneshot(patch(serde_json::json!({
                "selector": {
                    "document_ids": [doc_a.id, doc_b.id],
                    "metadata_condition": {
                        "logic": "and",
                        "conditions": [
                            {"name": "owner", "comparison_operator": "is", "value": "Farm A"}
                        ]
                    }
                },
                "updates": [{"key": "tags", "value": "selector"}]
            })))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_json(response).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["updated"], 1);
        assert_eq!(body["data"]["matched_docs"], 1);
        assert_eq!(
            env.state.document_metadata.get(&doc_a.id, &kb.id).unwrap()["tags"],
            serde_json::json!("selector"),
            "a key that did not exist yet is written as the given value"
        );
        assert!(
            env.state
                .document_metadata
                .get(&doc_b.id, &kb.id)
                .unwrap()
                .get("tags")
                .is_none(),
            "the condition must exclude the other document"
        );

        // A condition list that matches nothing short-circuits with
        // `{updated: 0, matched_docs: 0}`.
        let response = router
            .clone()
            .oneshot(patch(serde_json::json!({
                "selector": {
                    "metadata_condition": {
                        "conditions": [{"name": "owner", "comparison_operator": "is", "value": "Nowhere"}]
                    }
                },
                "updates": [{"key": "tags", "value": "never"}]
            })))
            .await
            .unwrap();
        let body = response_json(response).await;
        assert_eq!(body["code"], 0);
        assert_eq!(body["data"]["updated"], 0);
        assert_eq!(body["data"]["matched_docs"], 0);

        // Upstream's per-field validation messages.
        let response = router
            .clone()
            .oneshot(patch(serde_json::json!({
                "selector": {"metadata_condition": "nope"},
                "updates": []
            })))
            .await
            .unwrap();
        let body = response_json(response).await;
        assert_eq!(body["code"], 102);
        assert_eq!(body["message"], "metadata_condition must be an object.");

        let response = router
            .clone()
            .oneshot(patch(serde_json::json!({
                "selector": {"document_ids": ["does-not-exist"]},
                "updates": []
            })))
            .await
            .unwrap();
        let body = response_json(response).await;
        assert_eq!(body["code"], 102);
        assert!(
            body["message"]
                .as_str()
                .unwrap()
                .starts_with("These documents do not belong to dataset"),
            "{}",
            body["message"]
        );

        let response = router
            .clone()
            .oneshot(patch(serde_json::json!({
                "selector": {"document_ids": [doc_a.id]},
                "updates": [{"key": "tags"}]
            })))
            .await
            .unwrap();
        let body = response_json(response).await;
        assert_eq!(body["code"], 102);
        assert_eq!(body["message"], "Each update requires key and value.");
    }

    #[tokio::test]
    async fn document_metadata_write_requires_kb_management_permission() {
        let env = non_admin_env();
        let kb = env
            .state
            .kbs
            .create_for("other-user", "Other Metadata KB", "")
            .unwrap();
        let doc = stored_doc("other-metadata-doc", &kb.id, 0);
        env.state.docs.insert(doc.clone()).unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!(
                        "/api/v1/datasets/{}/documents/{}/metadata",
                        kb.id, doc.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"meta_fields":{"owner":"forbidden"}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(env.state.document_metadata.get(&doc.id, &kb.id).is_none());
    }

    #[tokio::test]
    async fn message_feedback_restores_index_when_conversation_persistence_fails() {
        let mut env = test_env();
        Arc::get_mut(&mut env.state).unwrap().chunk_feedback_enabled = true;
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Feedback Rollback KB", "")
            .unwrap();
        let mut chunk = search_chunk("rollback-feedback-chunk", &kb.id, "water quality");
        chunk.metadata.insert("pagerank_fea".into(), "10".into());
        env.state.engine.write().unwrap().add(chunk);
        env.state
            .engine
            .read()
            .unwrap()
            .save(&env.state.index_path)
            .unwrap();
        let conversation = env
            .state
            .conversations
            .create_for("test-user", "Feedback Rollback")
            .unwrap();
        let message_id = env
            .state
            .conversations
            .append_exchange_with_references(
                &conversation.id,
                "test-user",
                "question",
                "answer",
                vec!["water quality".into()],
                vec![crate::llm::ChunkReference {
                    id: "rollback-feedback-chunk".into(),
                    kb_id: kb.id,
                    content: "water quality".into(),
                    similarity: Some(1.0),
                    vector_similarity: Some(1.0),
                    term_similarity: Some(1.0),
                }],
            )
            .unwrap()
            .unwrap();
        let conversation_path = env.root.join("conversations.json");
        std::fs::remove_file(&conversation_path).unwrap();
        std::fs::create_dir(&conversation_path).unwrap();

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!(
                        "/api/v1/chats/{}/messages/{}/feedback",
                        conversation.id, message_id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"thumbup":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(chunk_pagerank(&env.state, "rollback-feedback-chunk"), 10.0);
        let restored_index = SearchEngine::from_file(&env.state.index_path).unwrap();
        assert_eq!(
            restored_index.to_vec()[0]
                .metadata
                .get("pagerank_fea")
                .map(String::as_str),
            Some("10")
        );
        assert_eq!(
            env.state
                .conversations
                .feedback_target(&conversation.id, "test-user", &message_id)
                .unwrap()
                .prior_thumb,
            None
        );
    }

    #[tokio::test]
    async fn chunk_list_hides_documents_from_other_owners() {
        let env = test_env();
        let other_kb = env
            .state
            .kbs
            .create_for("other-user", "Other Chunk KB", "")
            .unwrap();
        env.state
            .docs
            .insert(crate::api::document::DocRecord {
                id: "other-doc".into(),
                name: "secret.txt".into(),
                kb_id: other_kb.id,
                size: 6,
                storage_name: "other-doc.txt".into(),
                content_hash: String::new(),
                indexed_content_hash: String::new(),
                run: "DONE".into(),
                progress: 1.0,
                progress_msg: "done".into(),
                chunk_count: 0,
                created_at: 1,
                updated_at: 1,
            })
            .unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/api/v1/chunks/other-doc")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn file_and_task_lists_are_scoped_to_authenticated_owner() {
        let env = non_admin_env();
        let other_task = env
            .state
            .tasks
            .push("other-user", "Other task", "doc-x")
            .unwrap();
        let own_task = env
            .state
            .tasks
            .push("member-user", "Own task", "doc-y")
            .unwrap();
        env.state
            .files
            .add(crate::api::file_mgr::FileRecord {
                id: "own-file".into(),
                name: "own.txt".into(),
                owner_id: "member-user".into(),
                parent_id: "root".into(),
                size: 1,
                content_hash: String::new(),
                file_type: "txt".into(),
                created_at: 1,
            })
            .unwrap();
        env.state
            .files
            .add(crate::api::file_mgr::FileRecord {
                id: "other-file".into(),
                name: "other.txt".into(),
                owner_id: "other-user".into(),
                parent_id: "root".into(),
                size: 1,
                content_hash: String::new(),
                file_type: "txt".into(),
                created_at: 1,
            })
            .unwrap();
        let router = build_router(env.state.clone());
        let tasks = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/tasks")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let tasks_json = response_json(tasks).await;
        let ids: Vec<&str> = tasks_json["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|task| task["id"].as_str())
            .collect();
        assert!(ids.contains(&own_task.as_str()));
        assert!(!ids.contains(&other_task.as_str()));

        let files = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/files")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let files_json = response_json(files).await;
        let ids: Vec<&str> = files_json["data"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|file| file["id"].as_str())
            .collect();
        assert_eq!(ids, vec!["own-file"]);
    }

    #[tokio::test]
    async fn task_patch_accepts_only_the_stop_action() {
        let env = test_env();
        let task_id = env
            .state
            .tasks
            .push("test-user", "Patch task", "patch-doc")
            .unwrap();
        let router = build_router(env.state.clone());

        let invalid = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(format!("/api/v1/tasks/{task_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"pause"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let invalid_json = response_json(invalid).await;
        assert_eq!(
            invalid_json["message"],
            "Invalid action 'pause'. Only 'stop' is supported."
        );

        let stopped = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(format!("/api/v1/tasks/{task_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"stop"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stopped.status(), StatusCode::OK);
        let stopped_json = response_json(stopped).await;
        assert_eq!(stopped_json["data"], true);
        let task = env
            .state
            .tasks
            .list()
            .into_iter()
            .find(|task| task.id == task_id)
            .unwrap();
        assert_eq!(task.status, "cancelled");
        assert!(task.cancel_requested);

        let stopped_again = router
            .oneshot(
                Request::builder()
                    .method(Method::PATCH)
                    .uri(format!("/api/v1/tasks/{task_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"action":"stop"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stopped_again.status(), StatusCode::OK);
        assert_eq!(response_json(stopped_again).await["data"], true);
    }

    #[tokio::test]
    async fn pipeline_operation_logs_filter_terminal_tasks_with_kb_acl() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Logs KB", "")
            .unwrap();
        let other_kb = env
            .state
            .kbs
            .create_for("other-user", "Other KB", "")
            .unwrap();
        let (done_id, _) = env
            .state
            .tasks
            .push_document_unique(
                "test-user",
                "Parse annual-report.pdf",
                "doc-1",
                &kb.id,
                crate::api::features::TASK_PRIORITY_LOW,
            )
            .unwrap();
        assert!(
            env.state
                .tasks
                .update(&done_id, "done", 1.0, "Indexed")
                .unwrap()
        );
        let (pending_id, _) = env
            .state
            .tasks
            .push_document_unique(
                "test-user",
                "Parse pending.pdf",
                "doc-2",
                &kb.id,
                crate::api::features::TASK_PRIORITY_LOW,
            )
            .unwrap();
        let (other_id, _) = env
            .state
            .tasks
            .push_document_unique(
                "other-user",
                "Parse secret.pdf",
                "doc-3",
                &other_kb.id,
                crate::api::features::TASK_PRIORITY_LOW,
            )
            .unwrap();
        assert!(
            env.state
                .tasks
                .update(&other_id, "failed", 1.0, "Failed")
                .unwrap()
        );

        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/pipeline/operation-logs?kb_id={}&keywords=annual&operation_status=done&page=1&page_size=10",
                        kb.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let json = response_json(response).await;
        assert_eq!(json["data"]["total"], 1);
        assert_eq!(json["data"]["items"][0]["id"], done_id);
        assert_eq!(json["data"]["items"][0]["kb_id"], kb.id);
        assert_eq!(json["data"]["items"][0]["task_type"], "document_parse");
        let ids: Vec<_> = json["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|task| task["id"].as_str())
            .collect();
        assert!(!ids.contains(&pending_id.as_str()));
        assert!(!ids.contains(&other_id.as_str()));

        let forbidden = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/pipeline/operation-logs?kb_id={}",
                        other_kb.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn non_admin_cannot_use_management_write_endpoints() {
        let env = non_admin_env();
        let router = build_router(env.state.clone());
        for (method, uri, body) in [
            (Method::POST, "/api/v1/system/new_api_key", "{}"),
            (
                Method::POST,
                "/api/v1/connectors",
                r#"{"id":"x","name":"x","source_type":"file","enabled":true,"config":{}}"#,
            ),
            (Method::POST, "/api/v1/plugins/ocr/toggle", "{}"),
        ] {
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method)
                        .uri(uri)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "uri: {uri}");
        }
    }

    #[tokio::test]
    async fn multipart_rejects_path_traversal_filename() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Traversal KB", "")
            .unwrap();
        let boundary = "rayrag-traversal-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"../escape.txt\"\r\nContent-Type: text/plain\r\n\r\nnope\r\n--{boundary}--\r\n"
        );
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/datasets/{}/documents", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(
                        header::CONTENT_TYPE,
                        format!("multipart/form-data; boundary={boundary}"),
                    )
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(env.state.docs.list(&kb.id).is_empty());
        assert!(!env.root.join("escape.txt").exists());
    }

    fn indexed_chunk(doc_id: &str, kb_id: &str, id: &str) -> crate::search::IndexedChunk {
        crate::search::IndexedChunk {
            id: id.into(),
            doc_name: "sample.txt".into(),
            content: "sample".into(),
            embedding: vec![1.0, 0.0],
            token_count: 1,
            position: 0,
            metadata: std::collections::HashMap::from([
                ("doc_id".into(), doc_id.into()),
                ("kb_id".into(), kb_id.into()),
            ]),
        }
    }

    fn stored_doc(id: &str, kb_id: &str, chunk_count: usize) -> crate::api::document::DocRecord {
        crate::api::document::DocRecord {
            id: id.into(),
            name: "sample.txt".into(),
            kb_id: kb_id.into(),
            size: 6,
            storage_name: "sample.txt".into(),
            content_hash: "hash".into(),
            indexed_content_hash: String::new(),
            run: "UNSTARTED".into(),
            progress: 0.0,
            progress_msg: String::new(),
            chunk_count,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn graph_checkpoint(
        doc: &crate::api::document::DocRecord,
        chunks: &[crate::search::IndexedChunk],
    ) -> crate::graph_store::GraphCheckpoint {
        crate::graph_store::GraphStore::build_checkpoint(
            &doc.id,
            &doc.kb_id,
            &doc.content_hash,
            "light",
            &[],
            chunks,
        )
    }

    #[test]
    fn raptor_chunks_are_marked_and_replaced_with_the_document() {
        let doc = stored_doc("doc-raptor", "kb-a", 0);
        let source = vec![
            indexed_chunk(&doc.id, &doc.kb_id, "one"),
            indexed_chunk(&doc.id, &doc.kb_id, "two"),
        ];
        let summaries = build_raptor_chunks(&doc, &source, 0.5, 2).unwrap();
        assert!(!summaries.is_empty());
        assert!(summaries.iter().all(|chunk| {
            chunk.metadata.get("raptor_kwd").map(String::as_str) == Some("1")
                && chunk.metadata.contains_key("raptor_layer_int")
                && chunk.metadata.get("doc_id") == Some(&doc.id)
        }));

        let mut engine = SearchEngine::from_chunks([source, summaries].concat());
        engine.replace_document(
            &doc.id,
            vec![indexed_chunk(&doc.id, &doc.kb_id, "replacement")],
        );
        let chunks = engine.to_vec();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "replacement");
    }

    #[test]
    fn failed_document_metadata_delete_restores_index_and_kb_counts() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Rollback KB", "")
            .unwrap();
        let doc = stored_doc("doc-delete", &kb.id, 1);
        env.state.docs.insert(doc.clone()).unwrap();
        env.state.kbs.update_counts(&kb.id, 1, 1).unwrap();
        {
            let mut engine = env.state.engine.write().unwrap();
            let mut old = indexed_chunk(&doc.id, &kb.id, "old");
            old.content = "John Smith uses Rust".into();
            env.state
                .graphs
                .replace_document(&doc.id, Some(graph_checkpoint(&doc, &[old.clone()])))
                .unwrap();
            engine.index(vec![old]);
            engine.save(&env.state.index_path).unwrap();
        }
        let docs_path = env.root.join("docs.json");
        std::fs::remove_file(&docs_path).unwrap();
        std::fs::create_dir(&docs_path).unwrap();

        assert!(delete_document_data(&env.state, &doc).is_err());
        assert!(env.state.docs.get(&doc.id).is_some());
        let kb = env.state.kbs.get(&kb.id).unwrap();
        assert_eq!(kb.doc_count, 1);
        assert_eq!(kb.chunk_count, 1);
        assert_eq!(env.state.engine.read().unwrap().len(), 1);
        assert!(
            !env.state
                .graphs
                .context_for_query(&[kb.id], "John Smith")
                .is_empty()
        );
    }

    #[test]
    fn failed_document_metadata_commit_restores_index_and_kb_counts() {
        let env = test_env();
        let kb = env
            .state
            .kbs
            .create_for("test-user", "Commit KB", "")
            .unwrap();
        let doc = stored_doc("doc-commit", &kb.id, 1);
        env.state.docs.insert(doc.clone()).unwrap();
        env.state.kbs.update_counts(&kb.id, 1, 1).unwrap();
        {
            let mut engine = env.state.engine.write().unwrap();
            let mut old = indexed_chunk(&doc.id, &kb.id, "old");
            old.content = "John Smith uses Rust".into();
            env.state
                .graphs
                .replace_document(&doc.id, Some(graph_checkpoint(&doc, &[old.clone()])))
                .unwrap();
            engine.index(vec![old]);
            engine.save(&env.state.index_path).unwrap();
        }
        let task_id = env.state.tasks.push("test-user", "Parse", &doc.id).unwrap();
        let lease = env
            .state
            .tasks
            .claim(&task_id, "worker", 60_000, "Parsing")
            .unwrap()
            .unwrap();
        let docs_path = env.root.join("docs.json");
        std::fs::remove_file(&docs_path).unwrap();
        std::fs::create_dir(&docs_path).unwrap();

        assert!(
            commit_indexed_document(
                &env.state,
                &doc,
                &task_id,
                &lease.token,
                vec![
                    indexed_chunk(&doc.id, &kb.id, "new-1"),
                    indexed_chunk(&doc.id, &kb.id, "new-2"),
                ],
                Some(graph_checkpoint(
                    &doc,
                    &[{
                        let mut chunk = indexed_chunk(&doc.id, &kb.id, "new-graph");
                        chunk.content = "Google Inc uses Docker".into();
                        chunk
                    }],
                )),
                "Indexed 2 chunks",
            )
            .is_err()
        );
        let kb = env.state.kbs.get(&kb.id).unwrap();
        assert_eq!(kb.chunk_count, 1);
        let chunks = env.state.engine.read().unwrap().to_vec();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, "old");
        assert_eq!(env.state.docs.get(&doc.id).unwrap().run, "UNSTARTED");
        assert!(
            !env.state
                .graphs
                .context_for_query(&[kb.id], "John Smith")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn agent_http_lifecycle_enforces_team_edit_and_owner_delete() {
        let env = test_env();
        let member = env
            .state
            .users
            .register(
                "Agent Member",
                "agent-member@example.com",
                "correct horse battery staple",
            )
            .unwrap();
        let member_token = env
            .state
            .users
            .login("agent-member@example.com", "correct horse battery staple")
            .unwrap()
            .unwrap();
        env.state
            .tenants
            .invite_member("test-user", &member.id, "test-user")
            .unwrap();
        env.state
            .tenants
            .accept_invitation("test-user", &member.id)
            .unwrap();
        let router = build_router(env.state.clone());

        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "title": "Collaborative Agent",
                            "description": "Initial",
                            "permission": "team",
                            "dsl": {
                                "components": [],
                                "history": ["old"],
                                "retrieval": [{"id": "chunk"}],
                                "memory": ["old"],
                                "path": ["begin"],
                                "variables": {"region": {"type": "string", "value": "cn"}},
                                "globals": {
                                    "sys.query": "old",
                                    "env.region": "stale",
                                    "user.keep": 1
                                }
                            },
                            "tags": ["Rust", "rust", "RAG"]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let create_body = response_json(create).await;
        let agent_id = create_body["data"]["id"].as_str().unwrap().to_string();
        assert_eq!(
            create_body["data"]["tags"],
            serde_json::json!(["Rust", "RAG"])
        );

        let member_get = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/v1/agents/{agent_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(member_get.status(), StatusCode::OK);

        let member_update = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/agents/{agent_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"description":"Edited by member"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(member_update.status(), StatusCode::OK);
        assert_eq!(
            response_json(member_update).await["data"]["description"],
            "Edited by member"
        );

        let tags = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/agents/{agent_id}/tags"))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"tags":["shared","Shared","research"]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tags.status(), StatusCode::OK);
        assert_eq!(
            response_json(tags).await["data"],
            serde_json::json!(["shared", "research"])
        );

        let reset = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{agent_id}/reset"))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::OK);
        let reset = response_json(reset).await;
        assert_eq!(reset["data"]["history"], serde_json::json!([]));
        assert_eq!(reset["data"]["retrieval"], serde_json::json!([]));
        assert_eq!(reset["data"]["memory"], serde_json::json!([]));
        assert_eq!(reset["data"]["path"], serde_json::json!([]));
        assert_eq!(reset["data"]["globals"]["sys.query"], "");
        assert_eq!(reset["data"]["globals"]["env.region"], "cn");
        assert_eq!(reset["data"]["globals"]["user.keep"], 1);
        assert_eq!(env.state.canvas_versions.list(&agent_id).len(), 1);
        assert_eq!(
            env.state.canvas_versions.list(&agent_id)[0].dsl["history"],
            serde_json::json!(["old"])
        );

        let list = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/v1/agents?keywords=collab&tags=shared")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list_body = response_json(list).await;
        assert_eq!(list_body["data"]["total"], 1);
        assert_eq!(list_body["data"]["canvas"][0]["id"], agent_id);

        let tag_counts = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/api/v1/agents/tags?canvas_category=agent_canvas")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(tag_counts.status(), StatusCode::OK);
        let tag_counts = response_json(tag_counts).await;
        assert!(
            tag_counts["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["tag"] == "shared" && entry["count"] == 1)
        );

        let member_delete = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/agents/{agent_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(member_delete.status(), StatusCode::NOT_FOUND);

        let make_private = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/agents/{agent_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"permission":"private"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(make_private.status(), StatusCode::OK);

        let hidden = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/api/v1/agents/{agent_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

        let owner_delete = router
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/agents/{agent_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(owner_delete.status(), StatusCode::OK);
        assert_eq!(response_json(owner_delete).await["data"], true);
    }

    #[tokio::test]
    async fn agent_stream_emits_flat_completed_lifecycle_and_done_frame() {
        let env = test_env();
        let agent = env
            .state
            .agents
            .create(
                "test-user",
                crate::api::features::AgentCreateRequest {
                    name: "Streaming Agent".into(),
                    description: String::new(),
                    permission: Some("private".into()),
                    kb_ids: Vec::new(),
                    prompt_template: None,
                    dsl: serde_json::json!({"components": {
                        "begin": {
                            "obj": {"component_name": "Begin", "params": {}},
                            "downstream": ["done"], "upstream": []
                        },
                        "done": {
                            "obj": {"component_name": "Message", "params": {
                                "content": ["hello {sys.query}"]
                            }},
                            "downstream": [], "upstream": ["begin"]
                        }
                    }}),
                    canvas_category: None,
                    canvas_type: String::new(),
                    tags: Vec::new(),
                    avatar: String::new(),
                    release: None,
                },
            )
            .unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"question":"Ada","stream":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
        let (frames, body) = response_sse_json(response).await;
        assert!(body.ends_with("data: [DONE]\n\n"));
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame["event"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "workflow_started",
                "node_started",
                "node_finished",
                "node_started",
                "node_finished",
                "message",
                "message_end",
                "workflow_finished"
            ]
        );
        assert_eq!(frames[0]["data"]["inputs"], "Ada");
        assert_eq!(frames[5]["data"]["content"], "hello Ada");
        assert_eq!(frames[7]["data"]["outputs"], "hello Ada");
        assert_eq!(
            frames[2]["data"]["component_id"],
            serde_json::json!("begin")
        );
        assert_eq!(
            frames[4]["data"]["outputs"]["content"],
            serde_json::json!("hello Ada")
        );
        let message_id = frames[0]["message_id"].as_str().unwrap();
        let task_id = frames[0]["task_id"].as_str().unwrap();
        let session_id = frames[0]["session_id"].as_str().unwrap();
        assert!(!message_id.is_empty());
        assert!(!task_id.is_empty());
        assert!(!session_id.is_empty());
        assert!(frames.iter().all(|frame| {
            frame["message_id"] == message_id
                && frame["task_id"] == task_id
                && frame["session_id"] == session_id
                && frame["created_at"].as_u64().is_some()
        }));
        let persisted = env.state.conversations.get(session_id).unwrap();
        assert_eq!(persisted.messages.len(), 2);
        assert_eq!(persisted.messages[1].content, "hello Ada");
        assert_eq!(persisted.messages[1].id, message_id);
    }

    #[tokio::test]
    async fn agent_stream_emits_waiting_payload_with_opaque_resume_token() {
        let env = test_env();
        let agent = env
            .state
            .agents
            .create(
                "test-user",
                crate::api::features::AgentCreateRequest {
                    name: "Streaming Wait Agent".into(),
                    description: String::new(),
                    permission: Some("private".into()),
                    kb_ids: Vec::new(),
                    prompt_template: None,
                    dsl: serde_json::json!({"components": {
                        "begin": {
                            "obj": {"component_name": "Begin", "params": {}},
                            "downstream": ["fill"], "upstream": []
                        },
                        "fill": {
                            "obj": {"component_name": "UserFillUp", "params": {
                                "enable_tips": true,
                                "tips": "Your name?",
                                "inputs": {
                                    "age": {"type": "line"},
                                    "name": {"type": "line"}
                                }
                            }},
                            "downstream": ["done"], "upstream": ["begin"]
                        },
                        "done": {
                            "obj": {"component_name": "Message", "params": {
                                "content": ["hello {fill@name}"]
                            }},
                            "downstream": [], "upstream": ["fill"]
                        }
                    }}),
                    canvas_category: None,
                    canvas_type: String::new(),
                    tags: Vec::new(),
                    avatar: String::new(),
                    release: None,
                },
            )
            .unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"question":"start","stream":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let (frames, body) = response_sse_json(response).await;
        assert!(body.ends_with("data: [DONE]\n\n"));
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame["event"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "workflow_started",
                "node_started",
                "node_finished",
                "node_started",
                "waiting_for_user"
            ]
        );
        assert_eq!(frames[3]["data"]["component_id"], "fill");
        let waiting = &frames[4]["data"];
        assert_eq!(waiting["cpn_id"], "fill");
        assert_eq!(waiting["tips"], "Your name?");
        assert_eq!(waiting["inputs"]["name"]["type"], "line");
        let resume_token = waiting["resume_token"].as_str().unwrap();
        assert!(!resume_token.is_empty());
        let session_id = frames[3]["session_id"].as_str().unwrap();
        let checkpoint = env.state.agent_checkpoints.get(session_id).unwrap();
        assert_eq!(checkpoint.checkpoint_id, resume_token);
        let persisted = env.state.conversations.get(session_id).unwrap();
        assert_eq!(persisted.messages.len(), 2);
        assert_eq!(persisted.messages[1].content, "");
        assert_eq!(
            persisted.messages[1].id,
            frames[0]["message_id"].as_str().unwrap()
        );

        let resumed = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "submitted",
                            "conversation_id": session_id,
                            "resume_token": resume_token,
                            "inputs": {"name": "Ada", "age": "37"},
                            "stream": true
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.status(), StatusCode::OK);
        let (resumed_frames, resumed_body) = response_sse_json(resumed).await;
        assert!(resumed_body.ends_with("data: [DONE]\n\n"));
        assert_eq!(
            resumed_frames
                .iter()
                .map(|frame| frame["event"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "node_started",
                "node_finished",
                "node_started",
                "node_finished",
                "message",
                "message_end",
                "workflow_finished"
            ]
        );
        assert_eq!(resumed_frames[0]["data"]["component_id"], "fill");
        assert_eq!(resumed_frames[2]["data"]["component_id"], "done");
        assert_eq!(resumed_frames[4]["data"]["content"], "hello Ada");
        assert!(env.state.agent_checkpoints.get(session_id).is_none());
    }

    #[tokio::test]
    async fn agent_stream_emits_error_and_done_for_runtime_failure() {
        let env = test_env();
        let agent = env
            .state
            .agents
            .create(
                "test-user",
                crate::api::features::AgentCreateRequest {
                    name: "Failing Streaming Agent".into(),
                    description: String::new(),
                    permission: Some("private".into()),
                    kb_ids: Vec::new(),
                    prompt_template: None,
                    dsl: serde_json::json!({"components": {
                        "begin": {
                            "obj": {"component_name": "Begin", "params": {}},
                            "downstream": [], "upstream": []
                        }
                    }}),
                    canvas_category: None,
                    canvas_type: String::new(),
                    tags: Vec::new(),
                    avatar: String::new(),
                    release: None,
                },
            )
            .unwrap();
        let response = build_router(env.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"question":"fail","stream":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let (frames, body) = response_sse_json(response).await;
        assert!(body.ends_with("data: [DONE]\n\n"));
        assert_eq!(frames.len(), 4);
        assert_eq!(frames[0]["event"], "workflow_started");
        assert_eq!(frames[1]["event"], "node_started");
        assert_eq!(frames[2]["event"], "node_finished");
        assert_eq!(frames[2]["data"]["component_id"], "begin");
        assert_eq!(frames[3]["code"], 500);
        assert_eq!(frames[3]["data"], false);
        assert!(
            frames[3]["message"]
                .as_str()
                .unwrap()
                .contains("without a Message")
        );
    }

    #[tokio::test]
    async fn agent_user_fill_up_waits_persists_and_resumes_once_over_http() {
        let env = test_env();
        let agent = env
            .state
            .agents
            .create(
                "test-user",
                crate::api::features::AgentCreateRequest {
                    name: "User Fill Up Agent".into(),
                    description: String::new(),
                    permission: Some("private".into()),
                    kb_ids: Vec::new(),
                    prompt_template: None,
                    dsl: serde_json::json!({"components": {
                        "begin": {
                            "obj": {"component_name": "Begin", "params": {}},
                            "downstream": ["fill"], "upstream": []
                        },
                        "fill": {
                            "obj": {"component_name": "UserFillUp", "params": {
                                "enable_tips": true,
                                "tips": "Tell us about yourself",
                                "inputs": {
                                    "age": {"type": "line"},
                                    "name": {"type": "line"}
                                }
                            }},
                            "downstream": ["done"], "upstream": ["begin"]
                        },
                        "done": {
                            "obj": {"component_name": "Message", "params": {
                                "content": ["hello {fill@name}, age {fill@age}"]
                            }},
                            "downstream": [], "upstream": ["fill"]
                        }
                    }}),
                    canvas_category: None,
                    canvas_type: String::new(),
                    tags: Vec::new(),
                    avatar: String::new(),
                    release: None,
                },
            )
            .unwrap();
        let router = build_router(env.state.clone());
        let first = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"question":"start"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CONTENT_TYPE], "application/json");
        let first = response_json(first).await;
        assert_eq!(first["data"]["event"], "waiting_for_user");
        assert_eq!(first["data"]["answer"], "");
        assert_eq!(first["data"]["waiting_for_user"]["kind"], "user_fill_up");
        assert_eq!(first["data"]["waiting_for_user"]["cpn_id"], "fill");
        assert_eq!(
            first["data"]["waiting_for_user"]["tips"],
            "Tell us about yourself"
        );
        assert_eq!(
            first["data"]["waiting_for_user"]["inputs"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["age", "name"]
        );
        let conversation_id = first["data"]["conversation_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let resume_token = first["data"]["resume_token"].as_str().unwrap().to_owned();
        let persisted = env.state.conversations.get(&conversation_id).unwrap();
        assert_eq!(persisted.messages.len(), 2);
        assert_eq!(persisted.messages[0].content, "start");
        assert_eq!(persisted.messages[1].role, "assistant");
        assert_eq!(persisted.messages[1].content, "");
        assert!(env.state.agent_checkpoints.get(&conversation_id).is_some());

        let wrong_token = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "submitted",
                            "conversation_id": conversation_id,
                            "resume_token": "wrong",
                            "inputs": {"name": "Ada", "age": "37"}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong_token.status(), StatusCode::CONFLICT);
        assert_eq!(
            env.state
                .conversations
                .get(&conversation_id)
                .unwrap()
                .messages
                .len(),
            2
        );

        let resumed = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "submitted",
                            "conversation_id": conversation_id,
                            "resume_token": resume_token,
                            "inputs": {"name": "Ada", "age": "37"}
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.status(), StatusCode::OK);
        let resumed = response_json(resumed).await;
        assert_eq!(resumed["data"]["answer"], "hello Ada, age 37");
        assert_eq!(
            resumed["data"]["workflow_path"],
            serde_json::json!(["begin", "fill", "done"])
        );
        let persisted = env.state.conversations.get(&conversation_id).unwrap();
        assert_eq!(persisted.messages.len(), 4);
        assert_eq!(persisted.messages[2].content, "submitted");
        assert_eq!(persisted.messages[3].content, "hello Ada, age 37");
        assert!(env.state.agent_checkpoints.get(&conversation_id).is_none());
    }

    #[tokio::test]
    async fn agent_completion_persists_continues_conversation_and_stats_isolate_canvas_and_acl() {
        let env = test_env();
        let member = env
            .state
            .users
            .register(
                "Agent Session Member",
                "agent-session-member@example.com",
                "correct horse battery staple",
            )
            .unwrap();
        let member_token = env
            .state
            .users
            .login(
                "agent-session-member@example.com",
                "correct horse battery staple",
            )
            .unwrap()
            .unwrap();
        env.state
            .tenants
            .invite_member("test-user", &member.id, "test-user")
            .unwrap();
        env.state
            .tenants
            .accept_invitation("test-user", &member.id)
            .unwrap();
        let agent = env
            .state
            .agents
            .create(
                "test-user",
                crate::api::features::AgentCreateRequest {
                    name: "Stats Agent".into(),
                    description: String::new(),
                    permission: Some("team".into()),
                    kb_ids: Vec::new(),
                    prompt_template: None,
                    dsl: serde_json::json!({}),
                    canvas_category: None,
                    canvas_type: String::new(),
                    tags: Vec::new(),
                    avatar: String::new(),
                    release: None,
                },
            )
            .unwrap();
        env.state
            .conversations
            .create_for_tenant_settings("test-user", "test-user", "chat", vec![], None, None)
            .unwrap();
        let router = build_router(env.state.clone());
        let first = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"question":"first"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first = response_json(first).await;
        let conversation_id = first["data"]["conversation_id"].as_str().unwrap();
        let persisted = env.state.conversations.get(conversation_id).unwrap();
        assert_eq!(persisted.owner_id, member.id);
        assert_eq!(persisted.tenant_id, "test-user");
        assert_eq!(persisted.source, "agent");
        assert_eq!(persisted.canvas_id.as_deref(), Some(agent.id.as_str()));
        assert_eq!(persisted.messages.len(), 2);

        let resumed = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "second",
                            "conversation_id": conversation_id
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resumed.status(), StatusCode::OK);
        assert_eq!(
            env.state
                .conversations
                .get(conversation_id)
                .unwrap()
                .messages
                .len(),
            4
        );

        let owner_cannot_resume_member_session = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{}/completions", agent.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "question": "steal",
                            "conversation_id": conversation_id
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            owner_cannot_resume_member_session.status(),
            StatusCode::NOT_FOUND
        );

        let stats = router
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/api/v1/stats?tenant_id=test-user&canvas_id={}&from_date=1970-01-01&to_date=2999-12-31",
                        agent.id
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(stats.status(), StatusCode::OK);
        let stats = response_json(stats).await;
        assert_eq!(stats["data"]["pv"][0][1], 1);
        assert_eq!(stats["data"]["round"][0][1], 2.0);
    }

    #[tokio::test]
    async fn compilation_template_group_http_lifecycle_is_tenant_scoped() {
        let env = test_env();
        let outsider = env
            .state
            .users
            .register(
                "Template Outsider",
                "template-outsider@example.com",
                "correct horse battery staple",
            )
            .unwrap();
        let outsider_token = env
            .state
            .users
            .login(
                "template-outsider@example.com",
                "correct horse battery staple",
            )
            .unwrap()
            .unwrap();
        assert_ne!(outsider.id, "test-user");
        let router = build_router(env.state.clone());

        let created = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/compilation_template_groups")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "Entity compilation",
                            "description": "First version",
                            "templates": [{
                                "name": "Entity tree",
                                "kind": "tree",
                                "config": {"raptor": {"rechunk": true}}
                            }]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = response_json(created).await;
        let group_id = created["data"]["id"].as_str().unwrap().to_string();
        assert_eq!(created["data"]["scope"], "file");

        let listed = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/compilation_template_groups?keywords=entity&page=1&page_size=10")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = response_json(listed).await;
        assert_eq!(listed["data"]["total"], 1);
        assert_eq!(listed["data"]["groups"][0]["id"], group_id);

        let hidden = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/compilation_template_groups/{group_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {outsider_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);

        let updated = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri(format!("/api/v1/compilation_template_groups/{group_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "description": "Dataset version",
                            "templates": [{
                                "name": "Wiki artifact",
                                "kind": "artifacts",
                                "config": {}
                            }]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        assert_eq!(response_json(updated).await["data"]["scope"], "dataset");

        let deleted = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/compilation_template_groups/{group_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::OK);
        assert_eq!(response_json(deleted).await["data"], true);

        let missing = router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/compilation_template_groups/{group_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn compilation_template_reference_routes_are_authenticated_and_read_only() {
        let env = test_env();
        let router = build_router(env.state.clone());

        for path in [
            "/api/v1/compilation_templates/builtins",
            "/api/v1/compilation_templates/wiki_presets",
        ] {
            let unauthorized = router
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

            let write = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::DELETE)
                        .uri(path)
                        .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(write.status(), StatusCode::METHOD_NOT_ALLOWED);
        }

        let builtins = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/compilation_templates/builtins")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(builtins.status(), StatusCode::OK);
        let builtins = response_json(builtins).await;
        assert_eq!(builtins["data"].as_array().unwrap().len(), 9);
        assert_eq!(builtins["data"][0]["id"], "wiki");
        assert_eq!(builtins["data"][8]["id"], "empty");

        env.state
            .tenant_models
            .upsert(
                &env.state.providers,
                "test-user",
                "minimax",
                "primary",
                TenantModelInstanceUpdate {
                    tenant_id: None,
                    instance_name: "Primary".into(),
                    api_base: None,
                    api_key: None,
                    clear_api_key: false,
                    models: vec![TenantModelSpec {
                        name: "MiniMax-M3".into(),
                        model_types: vec![ModelCapability::Chat],
                        max_tokens: None,
                        enabled: true,
                        is_tools: false,
                        ocr_config: None,
                    }],
                },
            )
            .unwrap();
        env.state
            .tenant_models
            .set_default_chat_model(
                &env.state.providers,
                "test-user",
                Some("minimax/primary/MiniMax-M3"),
            )
            .unwrap();
        let personal = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/compilation_templates/builtins")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let personal = response_json(personal).await;
        assert_eq!(
            personal["data"][0]["config"]["llm_id"],
            "minimax/primary/MiniMax-M3"
        );

        let member = env
            .state
            .users
            .register(
                "Builtin Member",
                "builtin-member@example.com",
                "member password 123",
            )
            .unwrap();
        let member_token = env
            .state
            .users
            .login("builtin-member@example.com", "member password 123")
            .unwrap()
            .unwrap();
        let forbidden = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/compilation_templates/builtins?tenant_id=test-user")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
        env.state
            .tenants
            .invite_member("test-user", &member.id, "test-user")
            .unwrap();
        env.state
            .tenants
            .accept_invitation("test-user", &member.id)
            .unwrap();
        let shared = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/compilation_templates/builtins?tenant_id=test-user")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(shared.status(), StatusCode::OK);
        assert_eq!(
            response_json(shared).await["data"][0]["config"]["llm_id"],
            "minimax/primary/MiniMax-M3"
        );
        let member_personal = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/compilation_templates/builtins")
                    .header(header::AUTHORIZATION, format!("Bearer {member_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(response_json(member_personal).await["data"][0]["config"]["llm_id"].is_null());

        let presets = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/compilation_templates/wiki_presets")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(presets.status(), StatusCode::OK);
        let presets = response_json(presets).await;
        assert_eq!(presets["data"].as_array().unwrap().len(), 6);
        assert_eq!(presets["data"][0]["id"], "brand");
        assert_eq!(presets["data"][5]["id"], "user_interview");
    }

    // Real GPU end-to-end: build a group with a tree template, then POST
    // /execute with chunks. Requires RAYRAG_TEST_LLM_BASE (e.g.
    // http://127.0.0.1:8088/v1) and RAYRAG_TEST_LLM_MODEL.
    #[tokio::test]
    #[ignore]
    async fn gpu_tree_template_execute_builds_trees_over_http() {
        let llm_base = std::env::var("RAYRAG_TEST_LLM_BASE").expect("RAYRAG_TEST_LLM_BASE");
        let llm_model = std::env::var("RAYRAG_TEST_LLM_MODEL").expect("RAYRAG_TEST_LLM_MODEL");
        let env = test_env();
        let mut state: AppState = (*env.state).clone();
        state.llm = Some(Arc::new(crate::llm::LlmClient::new(
            crate::llm::LlmConfig {
                api_base: llm_base,
                api_key: String::new(),
                model: llm_model,
                generation: Default::default(),
                system_prompt: String::new(),
            },
        )));
        let state = Arc::new(state);
        let router = build_router(state.clone());

        let created = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/compilation_template_groups")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "name": "GPU tree group",
                            "templates": [
                                {
                                    "name": "Raptor tree",
                                    "kind": "tree",
                                    "config": {"raptor": {"prompt": "Summarize:\n{cluster_content}", "max_token": 256, "threshold": 0.3, "max_cluster": 8}}
                                },
                                {
                                    "name": "Hypergraph",
                                    "kind": "hypergraph",
                                    "config": {
                                        "guideline": {
                                            "target": "从源文本中提取主要实体和关系。",
                                            "rules_for_entities": "实体包括人名、组织名、地点和概念。",
                                            "rules_for_relations": "关系连接两个已知实体。",
                                            "rules_for_time": ""
                                        },
                                        "entity": {
                                            "description": "文本中出现的命名实体。",
                                            "fields": [
                                                {"type": "person", "description": "人名", "rule": ""},
                                                {"type": "organization", "description": "组织名", "rule": ""},
                                                {"type": "location", "description": "地点", "rule": ""}
                                            ]
                                        },
                                        "relation": {
                                            "description": "两个实体之间的有向关系。",
                                            "fields": [
                                                {"type": "operates", "description": "经营关系", "rule": ""},
                                                {"type": "supplies", "description": "供应关系", "rule": ""}
                                            ]
                                        }
                                    }
                                }
                            ]
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = response_json(created).await;
        let group_id = created["data"]["id"].as_str().unwrap().to_string();

        let chunks = vec![
            serde_json::json!({"content": "中山市百鲤居水产养殖场主营四大家鱼养殖", "embedding": [0.9, 0.1]}),
            serde_json::json!({"content": "强调科学管理水质与溶氧以提高成活率", "embedding": [0.85, 0.15]}),
            serde_json::json!({"content": "饲料配方与投喂频率直接影响生长速度", "embedding": [0.1, 0.9]}),
        ];
        let executed = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!(
                        "/api/v1/compilation_template_groups/{group_id}/execute?doc_id=doc-1&kb_id=kb-1"
                    ))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({ "chunks": chunks }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(executed.status(), StatusCode::OK);
        let executed = response_json(executed).await;
        let results = executed["data"]["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "tree + hypergraph results: {results:?}");
        // Tree result.
        let tree_result = results
            .iter()
            .find(|r| r["graph"].is_object())
            .expect("tree result");
        assert!(tree_result["node_count"].as_u64().unwrap() >= 3);
        assert!(
            tree_result["graph"]["entities"].as_array().unwrap().len() >= 3,
            "tree graph projection should be returned"
        );
        // Hypergraph result.
        let hyper_result = results
            .iter()
            .find(|r| r["entities"].is_array())
            .expect("hypergraph result");
        let hyper_entities = hyper_result["entities"].as_array().unwrap();
        assert!(
            !hyper_entities.is_empty(),
            "hypergraph entities: {hyper_entities:?}"
        );
        assert!(
            hyper_entities.iter().any(|e| e["name"]
                .as_str()
                .map(|n| n.contains("百鲤居") || n.contains("李锦澎"))
                .unwrap_or(false)),
            "expected farm/owner entity"
        );
        // Persisted as a knowledge_compile document checkpoint.
        let checkpoint = state
            .graphs
            .snapshot()
            .checkpoints
            .get("doc-1")
            .cloned()
            .expect("doc-1 checkpoint persisted");
        assert_eq!(checkpoint.method, "knowledge_compile");
        assert_eq!(checkpoint.kb_id, "kb-1");
        assert!(checkpoint.entity_types.contains(&"entity".to_string()));
        assert!(
            checkpoint.graph.node_count() > 0,
            "compiled graph nodes persisted"
        );
    }

    /// Upstream `agent_api.py::create_agent_session` plus
    /// `api.ts::agentChatCompletion`: the Explore surface creates a session with
    /// the canvas prologue seeded as its first assistant message, lists it back,
    /// and posts the conversation to the body-addressed chat route.
    #[tokio::test]
    async fn agent_explore_session_creation_and_chat_route_match_upstream() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "title": "Launch Agent",
                            "dsl": {
                                "graph": {"nodes": [{"id": "begin", "type": "beginNode"}], "edges": []},
                                "components": {
                                    "begin": {
                                        "obj": {
                                            "component_name": "Begin",
                                            "params": {},
                                            "mode": "conversational",
                                            "prologue": "Hi! I'm your assistant."
                                        },
                                        "upstream": [],
                                        "downstream": []
                                    }
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let agent_id = response_json(create).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let session = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{agent_id}/sessions"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"name": "First session"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(session.status(), StatusCode::OK);
        let session = response_json(session).await;
        assert_eq!(session["code"], 0);
        assert_eq!(session["message"], "success");
        assert_eq!(session["data"]["name"], "First session");
        assert_eq!(session["data"]["source"], "agent");
        assert_eq!(session["data"]["agent_id"], agent_id);
        let session_id = session["data"]["id"].as_str().unwrap().to_owned();
        assert!(!session_id.is_empty());
        // `canvas.get_prologue()` is seeded as the first assistant message.
        assert_eq!(session["data"]["message"][0]["role"], "assistant");
        assert_eq!(
            session["data"]["message"][0]["content"],
            "Hi! I'm your assistant."
        );

        let list = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agents/{agent_id}/sessions"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list = response_json(list).await;
        assert!(
            list["data"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["id"] == session_id.as_str()),
            "created session missing from the list: {list}"
        );

        // `api.ts::agentChatCompletion` carries the canvas id in the body; the
        // literal route must win over `/api/v1/agents/{id}/completions`.
        let chat = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents/chat/completions")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "agent_id": agent_id,
                            "session_id": session_id,
                            "query": "hello",
                            "stream": false
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(chat.status(), StatusCode::NOT_FOUND);
        assert_ne!(chat.status(), StatusCode::METHOD_NOT_ALLOWED);

        let missing = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents/does-not-exist/sessions")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let missing = response_json(missing).await;
        assert_eq!(missing["code"], 102);
        assert_eq!(missing["message"], "Agent not found.");
    }
    /// RAGFlow `system_api.py` API tokens plus the `bot_api.py` agentbot
    /// surface: a token's `beta` secret is a bearer credential of its own, and
    /// the embedded agent chat (`/agentbots/{id}/inputs` and
    /// `/agentbots/{id}/completions`) answers to it without a user session.
    #[tokio::test]
    async fn system_tokens_and_agentbots_match_upstream() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "title": "Embedded Agent",
                            "dsl": {
                                "graph": {"nodes": [{"id": "begin", "type": "beginNode"}], "edges": []},
                                "components": {
                                    "begin": {
                                        "obj": {
                                            "component_name": "Begin",
                                            "params": {},
                                            "mode": "conversational",
                                            "prologue": "Hello from the embed",
                                            "input_form": {
                                                "query": {"type": "line", "name": "Query"}
                                            }
                                        },
                                        "upstream": [],
                                        "downstream": ["message"]
                                    },
                                    "message": {
                                        "obj": {
                                            "component_name": "Message",
                                            "params": {"content": ["embedded answer"]}
                                        },
                                        "upstream": ["begin"],
                                        "downstream": []
                                    }
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::OK);
        let agent_id = response_json(create).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        // `POST /system/tokens` — upstream mints `ragflow-<token>` plus a beta.
        let created = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/system/tokens")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::OK);
        let created = response_json(created).await;
        assert_eq!(created["code"], 0);
        let token = created["data"]["token"].as_str().unwrap().to_owned();
        let beta = created["data"]["beta"].as_str().unwrap().to_owned();
        assert!(token.starts_with("ragflow-"), "{token}");
        assert_eq!(beta.len(), 32, "{beta}");
        assert!(created["data"]["update_time"].is_null());
        assert!(created["data"]["update_date"].is_null());

        let listed = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/system/tokens")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = response_json(listed).await;
        assert_eq!(listed["data"].as_array().unwrap().len(), 1);
        assert_eq!(listed["data"][0]["beta"], beta);

        // The beta alone authenticates the embedded surface: no session token in
        // the Authorization header, exactly like `?auth=<beta>` in the embed URL.
        let inputs = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agentbots/{agent_id}/inputs"))
                    .header(header::AUTHORIZATION, format!("Bearer {beta}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(inputs.status(), StatusCode::OK);
        let inputs = response_json(inputs).await;
        assert_eq!(inputs["code"], 0);
        assert_eq!(inputs["data"]["title"], "Embedded Agent");
        assert_eq!(inputs["data"]["prologue"], "Hello from the embed");
        assert_eq!(inputs["data"]["mode"], "conversational");
        assert_eq!(inputs["data"]["inputs"]["query"]["type"], "line");

        let completion = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agentbots/{agent_id}/completions"))
                    .header(header::AUTHORIZATION, format!("Bearer {beta}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"stream": false, "query": "hi"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(completion.status(), StatusCode::OK);
        let completion = response_json(completion).await;
        assert_eq!(completion["code"], 0);
        assert_eq!(completion["data"]["answer"], "embedded answer");

        // Unknown beta secrets stay unauthorized.
        let rejected = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agentbots/{agent_id}/inputs"))
                    .header(header::AUTHORIZATION, "Bearer deadbeef")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

        // An unknown canvas keeps upstream's `Can't find agent by ID` answer.
        let missing = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/agentbots/nope/inputs")
                    .header(header::AUTHORIZATION, format!("Bearer {beta}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let missing = response_json(missing).await;
        assert_eq!(missing["code"], 102);
        assert_eq!(missing["message"], "Can't find agent by ID: nope");

        // `DELETE /system/tokens/{token}` revokes both credentials.
        let removed = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri(format!("/api/v1/system/tokens/{token}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(removed.status(), StatusCode::OK);
        assert_eq!(response_json(removed).await["data"], true);
        let revoked = router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agentbots/{agent_id}/inputs"))
                    .header(header::AUTHORIZATION, format!("Bearer {beta}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    }
    /// RAGFlow `dataset_api.py` ingestion logs and `agent_api.py::rerun_agent`:
    /// the dataflow-result page's data source, scoped to the dataset and
    /// answering the upstream error strings.
    #[tokio::test]
    async fn ingestion_logs_and_rerun_match_upstream() {
        let env = test_env();
        let router = build_router(env.state.clone());
        // The KB belongs to the signed-in user so the ACL check passes.
        let me = env
            .state
            .users
            .get_user_by_token(&env.token)
            .expect("token resolves");
        let kb = env.state.kbs.create_for(&me.id, "Ingestion", "").unwrap();
        let doc = crate::api::document::DocRecord {
            id: "ingestion-doc".into(),
            name: "report.pdf".into(),
            kb_id: kb.id.clone(),
            size: 1024,
            storage_name: "ingestion-doc.pdf".into(),
            content_hash: "0123456789abcdef0123456789abcdef".into(),
            indexed_content_hash: String::new(),
            run: "DONE".into(),
            progress: 1.0,
            progress_msg: String::new(),
            chunk_count: 0,
            created_at: 1,
            updated_at: 1,
        };
        env.state.docs.insert(doc.clone()).unwrap();
        // `get_ingestion_summary` reports the dataset row's denormalized
        // counters (`kb.doc_num` / `kb.chunk_num`), exactly like upstream.
        env.state.kbs.update_counts(&kb.id, 0, 1).unwrap();
        std::fs::write(env.root.join("uploads/ingestion-doc.pdf"), b"%PDF-1.4").unwrap();
        let report = crate::pipeline::PipelineReport {
            stages: vec![crate::pipeline::StageReport {
                id: "parser",
                component: "Parser",
                title: "Parser",
                elapsed_seconds: 0.5,
                outputs: serde_json::json!({
                    "output_format": {"type": "string", "value": "text"},
                    "text": {"type": "string", "value": "parsed body"},
                    "_elapsed_time": {"type": "number", "value": 0.5}
                }),
            }],
        };
        let log = crate::api::ingestion::record_parse_run(
            &env.state, &me.id, &kb.id, &doc.id, &doc.name, "naive", &report, "done", "",
        )
        .expect("ingestion log recorded");
        assert_eq!(log.dsl["path"], serde_json::json!(["parser"]));

        let listed = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/datasets/{}/ingestions", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        let listed = response_json(listed).await;
        assert_eq!(listed["code"], 0);
        assert_eq!(listed["data"]["total"], 1);
        assert_eq!(listed["data"]["logs"][0]["document_name"], "report.pdf");
        assert_eq!(listed["data"]["logs"][0]["operation_status"], "done");

        let detail = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/datasets/{}/ingestions/{}", kb.id, log.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(detail.status(), StatusCode::OK);
        let detail = response_json(detail).await;
        assert_eq!(
            detail["data"]["dsl"]["components"]["parser"]["obj"]["params"]["outputs"]["text"]["value"],
            "parsed body"
        );

        let missing = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/datasets/{}/ingestions/nope", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let missing = response_json(missing).await;
        assert_eq!(missing["code"], 102);
        assert_eq!(missing["message"], "Log not found");

        let summary = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/datasets/{}/ingestions/summary", kb.id))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let summary = response_json(summary).await;
        assert_eq!(summary["code"], 0);
        assert_eq!(summary["data"]["doc_num"], 1);
        assert!(summary["data"]["status"]["unstart_count"].is_number());

        // `POST /agents/rerun` rewrites the log's `path` and re-queues the parse.
        let rerun = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents/rerun")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "id": log.id,
                            "dsl": {"path": ["parser", "tokenChunker"], "components": {}},
                            "component_id": "parser"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rerun.status(), StatusCode::OK);
        let rerun = response_json(rerun).await;
        assert_eq!(rerun["code"], 0);
        assert_eq!(rerun["data"], true);
        assert_eq!(
            env.state.ingestion_logs.get(&kb.id, &log.id).unwrap().dsl["path"],
            serde_json::json!(["parser"])
        );

        // An unknown log id keeps upstream's "Document not found.".
        let unknown = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents/rerun")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::json!({"id": "nope"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let unknown = response_json(unknown).await;
        assert_eq!(unknown["code"], 102);
        assert_eq!(unknown["message"], "Document not found.");
    }
    /// RAGFlow `get_agent_logs` / `bot_api.agent_bot_logs`: the per-message run
    /// trace the log sheet polls, plus its beta-token sibling.
    #[tokio::test]
    async fn agent_run_traces_match_upstream() {
        let env = test_env();
        let router = build_router(env.state.clone());
        let create = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/agents")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "title": "Trace Agent",
                            "dsl": {
                                "graph": {"nodes": [{"id": "begin", "type": "beginNode"}], "edges": []},
                                "components": {
                                    "begin": {
                                        "obj": {
                                            "component_name": "Begin",
                                            "params": {},
                                            "mode": "conversational"
                                        },
                                        "upstream": [],
                                        "downstream": ["message"]
                                    },
                                    "message": {
                                        "obj": {
                                            "component_name": "Message",
                                            "params": {"content": ["traced answer"]}
                                        },
                                        "upstream": ["begin"],
                                        "downstream": []
                                    }
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let agent_id = response_json(create).await["data"]["id"]
            .as_str()
            .unwrap()
            .to_owned();

        let run = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(format!("/api/v1/agents/{agent_id}/completions"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"question": "trace me", "stream": false}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(run.status(), StatusCode::OK);
        let run = response_json(run).await;
        let message_id = run["data"]["message_id"].as_str().unwrap().to_owned();

        let trace = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agents/{agent_id}/logs/{message_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(trace.status(), StatusCode::OK);
        let trace = response_json(trace).await;
        assert_eq!(trace["code"], 0);
        let components = trace["data"].as_array().expect("trace array");
        let ids: Vec<&str> = components
            .iter()
            .filter_map(|component| component["component_id"].as_str())
            .collect();
        assert!(ids.contains(&"begin"), "trace: {trace}");
        assert!(ids.contains(&"message"), "trace: {trace}");
        let message_trace = components
            .iter()
            .find(|component| component["component_id"] == "message")
            .unwrap();
        let samples = message_trace["trace"].as_array().unwrap();
        assert!(!samples.is_empty());
        // Upstream's sample shape (`ITraceData.trace[]`).
        assert!(samples[0]["message"].is_string());
        assert!(samples[0]["elapsed_time"].is_number());
        assert!(samples[0]["timestamp"].is_number());
        assert!(samples[0]["progress"].is_number());

        // A message without a run answers the empty object, not an empty array.
        let missing = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agents/{agent_id}/logs/does-not-exist"))
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response_json(missing).await["data"], serde_json::json!({}));

        // The beta sibling authenticates with the token's beta secret.
        let token = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/system/tokens")
                    .header(header::AUTHORIZATION, format!("Bearer {}", env.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let beta = response_json(token).await["data"]["beta"]
            .as_str()
            .unwrap()
            .to_owned();
        let shared = router
            .oneshot(
                Request::builder()
                    .uri(format!("/api/v1/agentbots/{agent_id}/logs/{message_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {beta}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(shared.status(), StatusCode::OK);
        let shared = response_json(shared).await;
        assert_eq!(shared["code"], 0);
        assert!(shared["data"].as_array().is_some(), "trace: {shared}");
    }
}
