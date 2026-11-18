//! RayRAG Web Server — serves Rust SSR UI + REST API.
//! Startup: `rayrag serve --port 9380`

use axum::{
    Json, Router,
    body::Body,
    extract::{
        DefaultBodyLimit, Extension, FromRequest, Multipart, Path, Query, Request, State,
        multipart::Field,
    },
    http::{HeaderMap, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
    /// The compiled-artifact store (RAGFlow's doc store): one shared pool so the ported
    /// wiki / dataset-navigation / structure engines read and write exactly what the
    /// running server serves. Without it those engines had no data path at all.
    pub doc_store: Arc<crate::doc_store::DocStorePool>,
    pub document_metadata: Arc<crate::api::document_metadata::DocumentMetadataStore>,
    /// The registration whitelist shown at `/admin/whitelist` and enforced by `POST /api/v1/users`.
    pub whitelist: Arc<crate::registration_whitelist::WhitelistStore>,
    pub files: Arc<crate::api::file_mgr::FileStore>,
    pub workspace_commits: Arc<crate::api::workspaces::WorkspaceCommitStore>,
    pub attachments: Arc<crate::api::attachments::AttachmentStore>,
    pub metadata_config: Arc<crate::api::metadata_config::MetadataConfigStore>,
    pub artifacts: Arc<crate::api::artifacts::ArtifactStore>,
    pub skill_trees: Arc<crate::api::skill_tree::SkillTreeStore>,
    pub connectors: Arc<crate::api::connector::ConnectorStore>,
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

    // `/admin/whitelist` decides who may sign up. A disabled whitelist admits everyone, which is
    // the default; enabling it is what makes the list binding.
    if !state.whitelist.allows(email) {
        return ret_code_response(
            upstream::OPERATING_ERROR,
            "This email is not on the registration whitelist. Ask an administrator to add it at /admin/whitelist.",
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
    match register_account(&state, &nickname, email, password).await {
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
            if let Some(token) = token
                && let Ok(value) = axum::http::HeaderValue::from_str(&token)
            {
                response
                    .headers_mut()
                    .insert(axum::http::header::AUTHORIZATION, value);
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

pub(crate) fn file_physical_path(data_dir: &str, id: &str, name: &str) -> std::path::PathBuf {
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
            // The blob has to move with the record. Answering "renamed" while the bytes
            // stayed behind leaves a row the UI lists and nobody can ever open.
            if let Err(error) = std::fs::rename(&old_path, &new_path) {
                tracing::error!(%error, %file_id, "Failed to rename the file blob");
                return Json(serde_json::json!({
                    "code": 500,
                    "message": format!("Failed to rename file: {error}"),
                }))
                .into_response();
            }
        }
        let mut updated = record.clone();
        updated.name = name;
        if let Err(error) = state.files.remove(&file_id) {
            tracing::error!(%error, %file_id, "Failed to drop the old file record");
            return Json(serde_json::json!({
                "code": 500,
                "message": format!("Failed to update file record: {error}"),
            }))
            .into_response();
        }
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
        // A copy that did not happen must not be reported as a new file: the row would
        // be listed and downloadable-looking while its bytes do not exist.
        if let Err(error) = std::fs::copy(&source, &target) {
            tracing::error!(%error, %file_id, "Failed to copy the file blob");
            return Json(serde_json::json!({
                "code": 500,
                "message": format!("Failed to copy file: {error}"),
            }))
            .into_response();
        }
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
/// image parsing tries OCR and reports configuration guidance if extraction fails.
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

// ── Dataset REST handlers (RAGFlow v0.27.2 `dataset_api.py` face) ──

fn dataset_ok(data: Value) -> Response {
    Json(json!({ "code": 0, "data": data })).into_response()
}

fn dataset_ok_list(data: Value, total: usize) -> Response {
    Json(json!({ "code": 0, "data": data, "total_datasets": total })).into_response()
}

fn dataset_argument_error(message: impl AsRef<str>) -> Response {
    Json(json!({ "code": 101, "message": message.as_ref() })).into_response()
}

fn dataset_data_error(message: impl AsRef<str>) -> Response {
    Json(json!({ "code": 102, "message": message.as_ref() })).into_response()
}

fn compact_id(id: &str) -> String {
    crate::api::dataset_api::normalize_uuid(id).unwrap_or_else(|| id.trim().to_ascii_lowercase())
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(value) => {
                        output.push(value);
                        index += 3;
                    }
                    Err(_) => {
                        output.push(b'%');
                        index += 1;
                    }
                }
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&output).to_string()
}

fn query_pairs(raw: Option<&str>) -> Vec<(String, String)> {
    raw.unwrap_or("")
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((key, value)) => (percent_decode(key), percent_decode(value)),
            None => (percent_decode(part), String::new()),
        })
        .collect()
}

/// `validate_and_parse_json_request` for the endpoint bodies: content type,
/// syntax, payload type and pydantic-equivalent errors.
/// A data-pipeline choice that actually names one of the built-in ingestion
/// pipelines is accepted as direct-parser mode. The create dialog offers the
/// built-in pipelines in the Pipeline picker as a convenience; without this
/// normalization the 32-character pipeline-id rule rejects them.

/// RayRAG agent/canvas ids are UUIDs with dashes, while RAGFlow's dataset
/// `pipeline_id` column is a 32-character hexadecimal id. Normalize a dashed
/// UUID to the compact 32-character form before validation so the Pipeline
/// parse type can actually be saved (previously every dashed id was rejected as
/// too long and the configuration silently kept the old mode).
fn normalize_dashed_pipeline_id(payload: &mut serde_json::Map<String, Value>) {
    if payload.get("parse_type").and_then(Value::as_i64) != Some(2) {
        return;
    }
    let Some(Value::String(pipeline)) = payload.get("pipeline_id") else {
        return;
    };
    let trimmed = pipeline.trim();
    let compact: String = trimmed.chars().filter(|ch| *ch != '-').collect();
    if trimmed.len() != 36
        || compact.len() != 32
        || !compact.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return;
    }
    payload.insert(
        "pipeline_id".into(),
        serde_json::json!(compact.to_ascii_lowercase()),
    );
}
fn normalize_builtin_pipeline_request(payload: &mut serde_json::Map<String, Value>) {
    if payload.get("parse_type").and_then(Value::as_i64) != Some(2) {
        return;
    }
    let Some(Value::String(pipeline)) = payload.get("pipeline_id") else {
        return;
    };
    let method = if pipeline == "naive" {
        "general".to_string()
    } else {
        pipeline.clone()
    };
    if !crate::api::dataset_api::builtin_pipeline_chunk_method(&method) {
        return;
    }
    payload.insert("parse_type".into(), serde_json::json!(1));
    payload.insert("chunk_method".into(), serde_json::json!(method));
    payload.remove("pipeline_id");
}
fn parse_dataset_body(
    headers: &HeaderMap,
    body: &[u8],
) -> Result<serde_json::Map<String, Value>, String> {
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !content_type
        .to_ascii_lowercase()
        .contains("application/json")
    {
        return Err(format!(
            "Unsupported content type: Expected application/json, got {}",
            if content_type.is_empty() {
                "None"
            } else {
                content_type
            }
        ));
    }
    if body.is_empty() {
        return Ok(serde_json::Map::new());
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| {
        "Malformed JSON syntax: Missing commas/brackets or invalid encoding".to_string()
    })?;
    match value {
        Value::Object(map) => Ok(map),
        other => Err(format!(
            "Invalid request payload: expected object, got {}",
            match other {
                Value::Array(_) => "list",
                Value::String(_) => "str",
                Value::Number(_) => "int",
                Value::Bool(_) => "bool",
                _ => "NoneType",
            }
        )),
    }
}

fn dataset_user_meta(state: &AppState, owner_id: &str) -> (String, Value) {
    match state.users.get_user_by_id(owner_id) {
        Some(user) => (
            user.nickname,
            if user.avatar.trim().is_empty() {
                Value::Null
            } else {
                Value::String(user.avatar)
            },
        ),
        None => (String::new(), Value::Null),
    }
}

fn dataset_embedding_display_name(state: &AppState, owner_id: &str, selector: &str) -> String {
    if selector.trim().is_empty() || selector == "default" {
        return String::new();
    }
    if selector.contains('@') {
        return selector.to_string();
    }
    match state.tenant_models.resolve(
        &state.providers,
        owner_id,
        crate::api::tenant_models::ModelCapability::Embedding,
        Some(selector),
    ) {
        Ok(Some(model)) => model.ragflow_selector(),
        _ => selector.to_string(),
    }
}

/// The dataset REST face reports `embedding_model`/`embedding_model_name` in
/// RAGFlow's `<model>@<provider>` vocabulary, while RayRAG stores its own
/// tenant-model selector. Resolve it when possible, otherwise report the stored
/// identifier unchanged (never an empty placeholder).
fn dataset_wire_embedding(state: &AppState, kb: &crate::kb::KnowledgeBase) -> String {
    dataset_embedding_display_name(state, &kb.owner_id, &kb.embd_id)
}

/// Map a RayRAG embedding-selector failure onto upstream
/// `verify_embedding_availability`'s message when the identifier was an
/// upstream `<model>@<provider>` pair.
fn dataset_embedding_error(requested: Option<&str>, error: &str) -> String {
    if let Some(model) = requested.filter(|value| value.contains('@')) {
        let mut parts = model.splitn(2, '@');
        let _name = parts.next().unwrap_or("");
        let provider = parts.next().unwrap_or("");
        return format!("Provider {provider} not found for model {model}.");
    }
    error.to_string()
}

/// Sum of chunk tokens per dataset, from one index scan.
fn dataset_token_totals(state: &AppState) -> std::collections::HashMap<String, u64> {
    let mut totals: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for chunk in state.engine.read().unwrap().to_vec() {
        if let Some(kb_id) = chunk.metadata.get("kb_id") {
            *totals.entry(kb_id.clone()).or_default() += chunk.token_count as u64;
        }
    }
    totals
}

/// RayRAG's own web UI (and earlier clients) send the tenant-model selector
/// (`provider/instance/model`) or `default` where upstream expects
/// `<model>@<provider>`. Accept both: the original identifier is used for
/// resolution, while validation sees an upstream-shaped placeholder.
fn accept_rayrag_embedding_selector(
    payload: &mut serde_json::Map<String, Value>,
) -> Option<String> {
    if !payload.contains_key("embedding_model")
        && let Some(value) = payload.remove("embd_id")
    {
        payload.insert("embedding_model".into(), value);
    }
    let raw = payload.get("embedding_model")?.as_str()?.trim().to_string();
    if raw.is_empty()
        || raw.contains('@')
        || (raw.len() == 32 && raw.chars().all(|ch| ch.is_ascii_hexdigit()))
    {
        return None;
    }
    if raw == "default" || raw.contains('/') {
        payload.insert(
            "embedding_model".into(),
            Value::String(format!("{raw}@rayrag")),
        );
        return Some(raw);
    }
    None
}

fn unique_dataset_name(state: &AppState, owner_id: &str, desired: &str) -> String {
    let existing: Vec<String> = state
        .kbs
        .list()
        .into_iter()
        .filter(|kb| kb.owner_id == owner_id)
        .map(|kb| kb.name.to_lowercase())
        .collect();
    if !existing.contains(&desired.to_lowercase()) {
        return desired.to_string();
    }
    for counter in 1..1000 {
        let candidate = format!("{desired}({counter})");
        if !existing.contains(&candidate.to_lowercase()) {
            return candidate;
        }
    }
    desired.to_string()
}

fn merge_parser_config(
    current: &str,
    incoming: &Value,
    chunk_method: Option<&str>,
) -> Option<String> {
    let mut merged = serde_json::from_str::<Value>(current)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let model_marker = merged
        .get("parser_config_model")
        .and_then(Value::as_str)
        .is_some_and(|flag| flag == "1");
    if let Some(object) = incoming.as_object()
        && !object.is_empty()
    {
        // Upstream flattens parent_child before merging, and its deep_merge
        // keeps the existing nested parent_child when the request does not
        // supply one.
        let parent = object.get("parent_child").and_then(Value::as_object);
        let use_parent = parent
            .and_then(|parent| parent.get("use_parent_child"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        for (key, value) in object {
            if key == "parent_child" {
                continue;
            }
            merged.insert(key.clone(), value.clone());
        }
        if use_parent {
            if let Some(parent) = parent {
                merged.insert("parent_child".into(), Value::Object(parent.clone()));
                if let Some(children) = parent.get("children_delimiter") {
                    merged.insert("children_delimiter".into(), children.clone());
                }
            }
            merged.insert("enable_children".into(), Value::Bool(true));
        } else {
            merged.insert("children_delimiter".into(), Value::String(String::new()));
            merged.insert("enable_children".into(), Value::Bool(false));
        }
    }
    if let Some(method) = chunk_method {
        merged.insert("chunk_method".into(), Value::String(method.to_string()));
    }
    if model_marker {
        merged.insert("parser_config_model".into(), Value::String("1".into()));
    }
    serde_json::to_string(&Value::Object(merged)).ok()
}

async fn list_datasets(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let pairs = query_pairs(raw.as_deref());
    let params = match crate::api::dataset_api::parse_dataset_list_query(&pairs) {
        Ok(params) => params,
        Err(issues) => {
            return dataset_argument_error(crate::api::dataset_api::format_issues(&issues));
        }
    };
    let accessible =
        state
            .kbs
            .list_accessible(&auth.user_id, auth.is_admin, |tenant_id, user_id| {
                state.tenants.is_member(tenant_id, user_id)
            });
    if params.filter_only {
        let mut owners: Vec<(String, usize)> = Vec::new();
        for kb in &accessible {
            match owners.iter_mut().find(|(owner, _)| owner == &kb.owner_id) {
                Some((_, count)) => *count += 1,
                None => owners.push((kb.owner_id.clone(), 1)),
            }
        }
        let total: usize = owners.iter().map(|(_, count)| count).sum();
        let owner_entries: Vec<Value> = owners
            .iter()
            .map(|(owner, count)| {
                let (label, _) = dataset_user_meta(&state, owner);
                json!({ "count": count, "id": owner, "label": label })
            })
            .collect();
        return Json(json!({
            "code": 0,
            "data": { "filter": { "owner": owner_entries }, "total": total }
        }))
        .into_response();
    }
    let mut selected = accessible;
    if let Some(id) = params.id.as_deref() {
        selected.retain(|kb| compact_id(&kb.id) == id);
        if selected.is_empty() {
            return dataset_data_error(format!(
                "User '{}' lacks permission for dataset '{}'",
                auth.user_id, id
            ));
        }
    }
    if !params.ids.is_empty() {
        selected.retain(|kb| params.ids.contains(&compact_id(&kb.id)));
        if selected.is_empty() {
            return dataset_ok_list(json!([]), 0);
        }
    }
    if let Some(name) = params.name.as_deref() {
        selected.retain(|kb| kb.name == name);
        if selected.is_empty() {
            return dataset_data_error(format!(
                "User '{}' lacks permission for dataset '{}'",
                auth.user_id, name
            ));
        }
    }
    if let Some(parser_id) = params.parser_id.as_deref() {
        selected.retain(|kb| {
            crate::api::dataset_api::canonical_chunk_method(
                &crate::api::dataset_api::dataset_chunk_method(kb),
            ) == crate::api::dataset_api::canonical_chunk_method(&parser_id)
        });
    }
    if !params.keywords.trim().is_empty() {
        let needle = params.keywords.trim().to_lowercase();
        selected.retain(|kb| kb.name.to_lowercase().contains(&needle));
    }
    if !params.owner_ids.is_empty() {
        let owners: Vec<String> = params
            .owner_ids
            .iter()
            .flat_map(|owner| [compact_id(owner), owner.trim().to_ascii_lowercase()])
            .collect();
        selected.retain(|kb| {
            owners.contains(&compact_id(&kb.owner_id))
                || owners.contains(&kb.owner_id.trim().to_ascii_lowercase())
        });
    }
    selected.sort_by(|left, right| {
        let ordering = if params.orderby == "update_time" {
            left.updated_at.cmp(&right.updated_at)
        } else {
            left.created_at.cmp(&right.created_at)
        };
        if params.desc {
            ordering.reverse()
        } else {
            ordering
        }
    });
    let total = selected.len();
    let offset = params
        .page
        .saturating_sub(1)
        .saturating_mul(params.page_size);
    let page: Vec<_> = selected
        .into_iter()
        .skip(offset)
        .take(params.page_size)
        .collect();
    let tokens = dataset_token_totals(&state);
    let mut entries = Vec::with_capacity(page.len());
    for kb in page {
        let mut entry =
            crate::api::dataset_api::dataset_entry(&kb, tokens.get(&kb.id).copied().unwrap_or(0));
        let (nickname, tenant_avatar) = dataset_user_meta(&state, &kb.owner_id);
        entry["nickname"] = Value::String(nickname);
        entry["tenant_avatar"] = tenant_avatar;
        let wire_embedding = dataset_wire_embedding(&state, &kb);
        entry["embedding_model"] = Value::String(wire_embedding.clone());
        entry["embedding_model_name"] = Value::String(wire_embedding);
        if params.include_parsing_status {
            let runs = state.docs.list(&kb.id).into_iter().map(|doc| doc.run);
            let counts = crate::api::dataset_api::parsing_counts(runs);
            if let (Some(entry), Some(counts)) = (entry.as_object_mut(), counts.as_object()) {
                for (key, value) in counts {
                    entry.insert(key.clone(), value.clone());
                }
            }
        }
        entries.push(entry);
    }
    dataset_ok_list(Value::Array(entries), total)
}

async fn get_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
) -> Response {
    let accessible =
        state
            .kbs
            .list_accessible(&auth.user_id, auth.is_admin, |tenant_id, user_id| {
                state.tenants.is_member(tenant_id, user_id)
            });
    let Some(kb) = accessible
        .into_iter()
        .find(|kb| kb.id == dataset_id || compact_id(&kb.id) == compact_id(&dataset_id))
    else {
        return dataset_data_error(format!(
            "User '{}' lacks permission for dataset '{}'",
            auth.user_id, dataset_id
        ));
    };
    let token_num = state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .iter()
        .filter(|chunk| chunk.metadata.get("kb_id").map(String::as_str) == Some(kb.id.as_str()))
        .map(|chunk| chunk.token_count as u64)
        .sum();
    let size: u64 = state
        .docs
        .list(&kb.id)
        .iter()
        .map(|doc| doc.size as u64)
        .sum();
    let mut entry = crate::api::dataset_api::dataset_entry(&kb, token_num);
    entry["embedding_model"] = Value::String(dataset_wire_embedding(&state, &kb));
    entry["size"] = json!(size);
    entry["connectors"] = dataset_connectors_wire(&state, &kb.connectors);
    dataset_ok(entry)
}

async fn create_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let mut payload = match parse_dataset_body(&headers, &body) {
        Ok(payload) => payload,
        Err(message) => return dataset_argument_error(message),
    };
    normalize_builtin_pipeline_request(&mut payload);
    normalize_dashed_pipeline_id(&mut payload);
    let rayrag_embedding = accept_rayrag_embedding_selector(&mut payload);
    let request = match crate::api::dataset_api::validate_dataset_body(&payload, false) {
        Ok(request) => request,
        Err(issues) => {
            return dataset_argument_error(crate::api::dataset_api::format_issues(&issues));
        }
    };
    let requested_embedding = rayrag_embedding
        .as_deref()
        .or(request.embedding_model.as_deref());
    let embd_id =
        match validate_tenant_embedding_selector(&state, &auth.user_id, requested_embedding) {
            Ok(selector) => selector,
            Err(error) => {
                return dataset_data_error(dataset_embedding_error(
                    requested_embedding,
                    &error.to_string(),
                ));
            }
        };
    let name = unique_dataset_name(&state, &auth.user_id, request.name.as_deref().unwrap_or(""));
    let chunk_method = request
        .chunk_method
        .clone()
        .unwrap_or_else(|| "naive".to_string());
    // Upstream stores the service-side naive defaults when parser_config is
    // omitted, and a ParserConfig-model-normalized object when it is supplied;
    // the marker lets the REST face reproduce both shapes.
    let parser_config_from_request = request.parser_config.is_some();
    let mut parser_config = request.parser_config.clone().unwrap_or_else(|| {
        json!({
            "table_context_size": 0,
            "image_context_size": 0,
            "layout_recognize": "DeepDOC",
            "chunk_token_num": 512,
            "delimiter": "\n",
            "auto_keywords": 0,
            "auto_questions": 0,
            "html4excel": false,
            "topn_tags": 3,
            "parent_child": { "use_parent_child": false, "children_delimiter": "\n" },
            "children_delimiter": "",
            "llm_id": "",
        })
    });
    if let Some(object) = parser_config.as_object_mut() {
        object.insert("chunk_method".into(), Value::String(chunk_method.clone()));
        if parser_config_from_request {
            object.insert("parser_config_model".into(), Value::String("1".into()));
        }
        if let Some(parse_type) = request.parse_type {
            object.insert("parse_type".into(), Value::String(parse_type.to_string()));
        }
    }
    if request.auto_metadata_config.is_some()
        && let Some(object) = parser_config.as_object_mut()
    {
        // Upstream create_dataset maps auto_metadata_config into the parser config
        // through the legacy field list: with the current AutoMetadataConfig model the
        // list is empty, and enable_metadata defaults to true.
        object.insert("metadata".into(), json!([]));
        object.insert("enable_metadata".into(), json!(true));
    }
    let parser_config_json = parser_config.to_string();
    let language = request
        .language
        .clone()
        .unwrap_or_else(crate::kb::default_language);
    match state.kbs.create_for_with_parser_config_and_language(
        &auth.user_id,
        &name,
        request.description.as_deref().unwrap_or(""),
        &request.permission,
        &embd_id,
        &parser_config_json,
        &language,
    ) {
        Ok(kb) => {
            let kb = if let Some(avatar) = request.avatar.as_deref() {
                state
                    .kbs
                    .update_identity(&kb.id, None, None, None, Some(avatar))
                    .ok()
                    .flatten()
                    .unwrap_or(kb)
            } else {
                kb
            };
            let kb = if request.parse_type.is_some() {
                match request.pipeline_id.clone() {
                    Some(pipeline_id) => state
                        .kbs
                        .set_pipeline_id(&kb.id, Some(pipeline_id))
                        .ok()
                        .flatten()
                        .unwrap_or(kb),
                    None => kb,
                }
            } else {
                kb
            };
            let mut entry = crate::api::dataset_api::dataset_entry(&kb, 0);
            entry["embedding_model"] = Value::String(dataset_wire_embedding(&state, &kb));
            dataset_ok(entry)
        }
        Err(error) => dataset_data_error(error.to_string()),
    }
}

async fn update_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let mut payload = match parse_dataset_body(&headers, &body) {
        Ok(payload) => payload,
        Err(message) => return dataset_argument_error(message),
    };
    normalize_builtin_pipeline_request(&mut payload);
    normalize_dashed_pipeline_id(&mut payload);
    let rayrag_embedding = accept_rayrag_embedding_selector(&mut payload);
    let request = match crate::api::dataset_api::validate_dataset_body(&payload, true) {
        Ok(request) => request,
        Err(issues) => {
            return dataset_argument_error(crate::api::dataset_api::format_issues(&issues));
        }
    };
    let Some(current) = state.kbs.get(&dataset_id).or_else(|| {
        state
            .kbs
            .list()
            .into_iter()
            .find(|kb| compact_id(&kb.id) == compact_id(&dataset_id))
    }) else {
        return dataset_data_error(format!(
            "User '{}' lacks permission for dataset '{}'",
            auth.user_id, dataset_id
        ));
    };
    if !kb_manageable(&state, &current.id, &auth) {
        return dataset_data_error(format!(
            "User '{}' lacks permission for dataset '{}'",
            auth.user_id, dataset_id
        ));
    }
    if request.fields_set.is_empty() {
        return dataset_data_error("no properties were modified");
    }
    if let Some(name) = request.name.as_deref() {
        if !name.eq_ignore_ascii_case(&current.name) {
            let taken = state.kbs.list().into_iter().any(|kb| {
                kb.id != current.id
                    && kb.owner_id == current.owner_id
                    && kb.name.eq_ignore_ascii_case(name)
            });
            if taken {
                return dataset_data_error(format!("Dataset name '{name}' already exists"));
            }
        }
    }
    let requested_embedding = rayrag_embedding
        .as_deref()
        .or(request.embedding_model.as_deref());
    let embd_id = match requested_embedding {
        Some(selector) if !selector.is_empty() && selector != current.embd_id => {
            match validate_tenant_embedding_selector(&state, &current.owner_id, Some(selector)) {
                Ok(selector) => Some(selector),
                Err(error) => {
                    return dataset_data_error(dataset_embedding_error(
                        Some(selector),
                        &error.to_string(),
                    ));
                }
            }
        }
        _ => None,
    };
    if let Err(error) = state.kbs.update_identity(
        &current.id,
        request.name.as_deref(),
        request.description.as_deref(),
        None,
        request.avatar.as_deref(),
    ) {
        return dataset_data_error(error.to_string());
    }
    if let Some(language) = request.language.as_deref() {
        if let Err(error) = state.kbs.set_language(&current.id, language) {
            return dataset_data_error(error.to_string());
        }
    }
    let mut parser_config_json = None;
    if request.parser_config.is_some() {
        let incoming = request.parser_config.clone().unwrap_or_else(|| json!({}));
        parser_config_json = merge_parser_config(
            &current.parser_config,
            &incoming,
            request.chunk_method.as_deref(),
        );
    } else if let Some(method) = request.chunk_method.as_deref() {
        // Upstream `get_parser_config(chunk_method, None)` replaces the parser
        // config when only the chunk method changes.
        if method != crate::api::dataset_api::dataset_chunk_method(&current) {
            parser_config_json =
                Some(crate::api::dataset_api::parser_config_for_method(method).to_string());
        }
    }
    if request.parser_config.is_none() && request.auto_metadata_config.is_some() {
        // update_dataset maps auto_metadata_config into parser_config before the
        // parent-child flatten/deep-merge: metadata is empty and enable_metadata true.
        let incoming = json!({ "metadata": [], "enable_metadata": true });
        parser_config_json = merge_parser_config(
            &current.parser_config,
            &incoming,
            request.chunk_method.as_deref(),
        );
    }
    if request.provided("pipeline_id") {
        if let Err(error) = state
            .kbs
            .set_pipeline_id(&current.id, request.pipeline_id.clone())
        {
            return dataset_data_error(error.to_string());
        }
    } else if request.chunk_method.is_some()
        && current
            .pipeline_id
            .as_deref()
            .is_some_and(|pipeline| !pipeline.trim().is_empty())
    {
        if let Err(error) = state.kbs.set_pipeline_id(&current.id, None) {
            return dataset_data_error(error.to_string());
        }
    }
    match state.kbs.update_config(
        &current.id,
        Some(&request.permission),
        embd_id.as_deref(),
        parser_config_json.as_deref(),
        None,
    ) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return dataset_data_error(format!(
                "User '{}' lacks permission for dataset '{}'",
                auth.user_id, dataset_id
            ));
        }
        Err(error) => return dataset_data_error(error.to_string()),
    }
    if request.provided("pagerank") {
        let pagerank = u32::try_from(request.pagerank.max(0)).unwrap_or(0);
        if let Err(error) = state.kbs.set_pagerank(&current.id, pagerank) {
            return dataset_data_error(error.to_string());
        }
    }
    // Linked data sources (the `connectors` column). A PUT without the field
    // keeps the current links on purpose: RayRAG pages save the dataset by
    // re-sending only the fields they edited, and RAGFlow’s clear-on-omit
    // semantics would silently unlink every data source whenever another page
    // saves the dataset name. An explicit array (including an empty one)
    // replaces the links.
    if let Some(items) = request.connectors.as_deref() {
        let old_ids: Vec<&str> = current
            .connectors
            .iter()
            .map(|link| link.id.as_str())
            .collect();
        let links = normalize_connector_links(items);
        let newly_linked: Vec<String> = links
            .iter()
            .filter(|link| !old_ids.iter().any(|id| *id == link.id.as_str()))
            .map(|link| link.id.clone())
            .collect();
        if let Err(error) = state.kbs.set_connectors(&current.id, links) {
            return dataset_data_error(error.to_string());
        }
        // Upstream `Connector2KbService.link_connectors` runs the newly linked
        // connector immediately: linking a data source is what syncs it into
        // the dataset. The request does not wait for that sync.
        if state.data_sources.is_some() {
            for connector_id in newly_linked {
                crate::api::data_source_mgr::schedule_link_sync(
                    state.clone(),
                    auth.clone(),
                    connector_id,
                    current.id.clone(),
                );
            }
        }
    }
    let Some(kb) = state.kbs.get(&current.id) else {
        return dataset_data_error("Dataset updated failed");
    };
    let token_num = dataset_token_totals(&state)
        .get(&kb.id)
        .copied()
        .unwrap_or(0);
    let mut entry = crate::api::dataset_api::dataset_entry(&kb, token_num);
    entry["embedding_model"] = Value::String(dataset_wire_embedding(&state, &kb));
    entry["connectors"] = dataset_connectors_wire(&state, &kb.connectors);
    dataset_ok(entry)
}

/// Normalise the `connectors` array of a dataset PUT into stored links.
///
/// The Configuration form posts the wire rows it rendered; only the linked id
/// and the auto_parse switch are stored (the name/source/status are projected
/// from the data-source store on read). Non-objects are ignored, the id must be
/// a non-empty string, the first appearance of an id wins, and only the literal
/// string "0" disables auto-parse (anything else means "1", the upstream column
/// default).
fn normalize_connector_links(items: &[Value]) -> Vec<crate::kb::ConnectorLink> {
    let mut links: Vec<crate::kb::ConnectorLink> = Vec::new();
    for item in items {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(id) = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if links.iter().any(|link| link.id == id) {
            continue;
        }
        let auto_parse = match object.get("auto_parse").and_then(Value::as_str) {
            Some("0") => "0",
            _ => "1",
        };
        links.push(crate::kb::ConnectorLink {
            id: id.to_string(),
            auto_parse: auto_parse.to_string(),
        });
    }
    links
}

/// Project the stored links onto the wire rows the dataset pages read:
/// {id, name, source, auto_parse, status}.
///
/// name / source / status come from the data-source store, mirroring upstream
/// Connector2Kb JOIN Connector. A link whose source record is gone is skipped
/// instead of being rendered with an empty name, which is the same inner-join
/// semantics the upstream API has.
fn dataset_connectors_wire(state: &AppState, links: &[crate::kb::ConnectorLink]) -> Value {
    let mut entries = Vec::with_capacity(links.len());
    for link in links {
        let Some(record) = state
            .data_sources
            .as_ref()
            .and_then(|store| store.get(&link.id))
        else {
            continue;
        };
        let status = if record.status.is_empty() {
            "unstart".to_string()
        } else {
            record.status.clone()
        };
        entries.push(json!({
            "id": link.id,
            "name": record.name,
            "source": record.kind,
            "auto_parse": link.auto_parse,
            "status": status,
        }));
    }
    Value::Array(entries)
}

/// Removes one dataset and everything hanging off it (documents, graph assets).
/// Shared by `DELETE /api/v1/datasets/{id}` and the upstream collection route.
fn delete_dataset_data(state: &Arc<AppState>, dataset_id: &str) -> crate::Result<bool> {
    let document_ids: Vec<String> = state
        .docs
        .list(dataset_id)
        .into_iter()
        .map(|doc| doc.id)
        .collect();
    for doc in state.docs.list(dataset_id) {
        delete_document_data(state, &doc)?;
    }
    // The metadata schema and its per-document overrides go with the dataset; leaving them behind
    // would resurrect a schema if a new dataset were ever given the same id.
    if let Err(error) = state
        .metadata_config
        .drop_dataset(dataset_id, &document_ids)
    {
        tracing::warn!(%error, dataset_id, "Failed to persist metadata configuration cleanup");
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
        return dataset_data_error(format!(
            "User '{}' lacks permission for dataset '{}'",
            auth.user_id, dataset_id
        ));
    }
    match delete_dataset_data(&state, &dataset_id) {
        Ok(true) => dataset_ok(json!({ "success_count": 1 })),
        Ok(false) => dataset_data_error(format!(
            "User '{}' lacks permission for dataset '{}'",
            auth.user_id, dataset_id
        )),
        Err(error) => dataset_data_error(format!("Internal server error: {error}")),
    }
}

/// `DELETE /api/v1/datasets` — upstream `dataset_api.delete` +
/// `dataset_api_service.delete_datasets`.
async fn delete_datasets(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let payload = match parse_dataset_body(&headers, &body) {
        Ok(payload) => payload,
        Err(message) => return dataset_argument_error(message),
    };
    let mut issues: Vec<crate::api::dataset_api::ValidationIssue> = Vec::new();
    for key in payload.keys() {
        if key != "ids" && key != "delete_all" {
            issues.push(crate::api::dataset_api::ValidationIssue::new(
                key,
                "Extra inputs are not permitted",
                crate::api::dataset_api::display_value(&payload[key]),
            ));
        }
    }
    let mut raw_ids: Vec<String> = Vec::new();
    match payload.get("ids") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                match item.as_str() {
                    Some(text) => raw_ids.push(text.to_string()),
                    None => issues.push(crate::api::dataset_api::ValidationIssue::new(
                        format!("ids.{index}"),
                        "Input should be a valid string",
                        crate::api::dataset_api::display_value(item),
                    )),
                }
            }
        }
        Some(other) => issues.push(crate::api::dataset_api::ValidationIssue::new(
            "ids",
            "Input should be a valid list",
            crate::api::dataset_api::display_value(other),
        )),
    }
    let delete_all = match payload.get("delete_all") {
        None => false,
        Some(Value::Bool(flag)) => *flag,
        Some(value) => {
            issues.push(crate::api::dataset_api::ValidationIssue::new(
                "delete_all",
                "Input should be a valid boolean",
                crate::api::dataset_api::display_value(value),
            ));
            false
        }
    };
    let mut ids: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut duplicates: Vec<String> = Vec::new();
    for raw in &raw_ids {
        match crate::api::dataset_api::normalize_uuid(raw) {
            Some(id) => {
                if seen.contains(&id) && !duplicates.contains(&id) {
                    duplicates.push(id.clone());
                }
                seen.push(id.clone());
                ids.push(id);
            }
            None => issues.push(crate::api::dataset_api::ValidationIssue::new(
                "ids",
                "Invalid UUID format",
                format!("['{raw}']"),
            )),
        }
    }
    if !duplicates.is_empty() {
        issues.push(crate::api::dataset_api::ValidationIssue::new(
            "ids",
            format!("Duplicate ids: '{}'", duplicates.join(", ")),
            format!("{:?}", raw_ids),
        ));
    }
    if !issues.is_empty() {
        return dataset_argument_error(crate::api::dataset_api::format_issues(&issues));
    }
    let accessible =
        state
            .kbs
            .list_accessible(&auth.user_id, auth.is_admin, |tenant_id, user_id| {
                state.tenants.is_member(tenant_id, user_id)
            });
    if ids.is_empty() && !delete_all {
        return dataset_ok(json!({ "success_count": 0 }));
    }
    let target_ids: Vec<String> = if delete_all {
        accessible.iter().map(|kb| kb.id.clone()).collect()
    } else {
        ids
    };
    let mut missing: Vec<String> = Vec::new();
    let mut resolved: Vec<String> = Vec::new();
    for id in &target_ids {
        match accessible
            .iter()
            .find(|kb| compact_id(&kb.id) == *id || kb.id == *id)
        {
            Some(kb) => resolved.push(kb.id.clone()),
            None => missing.push(id.clone()),
        }
    }
    if !missing.is_empty() {
        return dataset_data_error(format!(
            "User '{}' lacks permission for datasets: '{}'",
            auth.user_id,
            missing.join(", ")
        ));
    }
    let mut success_count = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for id in &resolved {
        match delete_dataset_data(&state, id) {
            Ok(true) => success_count += 1,
            Ok(false) => errors.push(format!("Delete dataset error for {id}")),
            Err(error) => errors.push(format!("Delete dataset error for {id}: {error}")),
        }
    }
    if errors.is_empty() {
        return dataset_ok(json!({ "success_count": success_count }));
    }
    let message = format!(
        "Successfully deleted {success_count} datasets, {} failed. Details: {}...",
        errors.len(),
        errors.join("; ").chars().take(128).collect::<String>()
    );
    if success_count == 0 {
        return dataset_data_error(message);
    }
    dataset_ok(
        json!({ "success_count": success_count, "errors": errors.into_iter().take(5).collect::<Vec<_>>() }),
    )
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
        "wav" => head.len() >= 12 && starts_with(b"RIFF") && &head[8..12] == b"WAVE",
        "mp3" => starts_with(b"ID3")
            || (head.len() >= 2 && head[0] == 0xff && head[1] & 0xe6 == 0xe2),
        "ogg" | "opus" => starts_with(b"OggS"),
        "flac" => starts_with(b"fLaC"),
        "m4a" => head.len() >= 12 && &head[4..8] == b"ftyp"
            && matches!(&head[8..12], b"M4A " | b"M4B " | b"isom" | b"iso2" | b"mp41" | b"mp42"),
        "aac" => head.len() >= 2 && head[0] == 0xff && head[1] & 0xf6 == 0xf0,
        "wma" => starts_with(&[
            0x30, 0x26, 0xb2, 0x75, 0x8e, 0x66, 0xcf, 0x11,
            0xa6, 0xd9, 0x00, 0xaa, 0x00, 0x62, 0xce, 0x6c,
        ]),
        "webm" => starts_with(b"\x1a\x45\xdf\xa3"),
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
        | "toml" | "log" | "eml" => validate_utf8_text(path),
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
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let conversion = match extension.to_ascii_lowercase().as_str() {
            "doc" => {
                "Open the file in Word, WPS or LibreOffice and save as DOCX (.docx), then upload again. 旧版 Word 请另存为 DOCX 后重新上传。"
            }
            "xls" => {
                "Open the file in Excel, WPS or LibreOffice and save as XLSX (.xlsx), then upload again. 旧版 Excel 请另存为 XLSX 后重新上传。"
            }
            "ppt" => {
                "Open the file in PowerPoint, WPS or LibreOffice and save as PPTX (.pptx), then upload again. 旧版演示文稿请另存为 PPTX 后重新上传。"
            }
            _ => {
                "Convert the file to a supported format such as PDF, DOCX, XLSX, PPTX or TXT, then upload again. 请转换为支持的文档格式后重新上传。"
            }
        };
        anyhow::bail!("Unsupported file type: {name}. {conversion}");
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

/// Upstream `sanitize_path` for the optional multipart `parent_path`: backslashes
/// become slashes, leading/trailing separators are dropped, `.`/`..` segments are
/// removed, and every character outside `[A-Za-z0-9_-/]` is stripped.
fn sanitize_parent_path(raw: Option<&str>) -> Option<String> {
    let raw = raw?.trim();
    if raw.is_empty() {
        return None;
    }
    let normalized = raw.replace('\\', "/");
    let normalized = normalized.trim_matches('/');
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .collect();
    if parts.is_empty() {
        return None;
    }
    let sanitized: String = parts
        .join("/")
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '/'))
        .collect();
    (!sanitized.is_empty()).then_some(sanitized)
}

/// Upstream `_upload_local_documents` only forwards `table_column_mode` and
/// `table_column_roles` from the multipart `parser_config`; malformed JSON or a
/// non-object is silently ignored.
fn parser_config_override(raw: &str) -> Option<serde_json::Value> {
    let parsed: serde_json::Value = serde_json::from_str(raw).ok()?;
    let object = parsed.as_object()?;
    let mut filtered = serde_json::Map::new();
    for key in ["table_column_mode", "table_column_roles"] {
        if let Some(value) = object.get(key) {
            filtered.insert(key.to_string(), value.clone());
        }
    }
    (!filtered.is_empty()).then_some(serde_json::Value::Object(filtered))
}

/// Upstream stores the KB parser config merged with the request override on the
/// document. `None` keeps the historical "follow the dataset face" behavior.
fn merged_parser_config(
    state: &AppState,
    dataset_id: &str,
    override_config: &Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    let override_config = override_config.as_ref()?;
    let mut merged = state
        .kbs
        .get(dataset_id)
        .and_then(|kb| serde_json::from_str::<serde_json::Value>(&kb.parser_config).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    if let Some(object) = override_config.as_object() {
        for (key, value) in object {
            merged.insert(key.clone(), value.clone());
        }
    }
    Some(serde_json::Value::Object(merged))
}

/// Move a streamed upload under the sanitized `parent_path` folder. The stored
/// name becomes `<parent>/<id>.<ext>`, which is also what download/delete use,
/// so the response location and the blob on disk agree.
async fn move_upload_into_parent(
    upload: &mut PersistedUpload,
    parent_path: &str,
    upload_dir: &FsPath,
) -> anyhow::Result<()> {
    let directory = upload_dir.join(parent_path);
    tokio::fs::create_dir_all(&directory).await?;
    let target = directory.join(&upload.storage_name);
    if target.exists() {
        anyhow::bail!("Upload destination already exists");
    }
    tokio::fs::rename(&upload.path, &target).await?;
    upload.path = target;
    upload.storage_name = format!("{parent_path}/{}", upload.storage_name);
    Ok(())
}

/// `POST /api/v1/datasets/{id}/documents` — dispatches the three upstream upload
/// types. `empty` takes a JSON body, `local` keeps the multipart path, and `web`
/// renders the URL to PDF with headless Chromium before ingesting it.
async fn upload_document_route(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    request: Request,
) -> Response {
    let upload_type = query
        .get("type")
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "local".to_string());
    let return_raw_files = query
        .get("return_raw_files")
        .map(|value| value.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    match upload_type.as_str() {
        "empty" => upload_empty_document(state, auth, dataset_id, request).await,
        "local" => match Multipart::from_request(request, &state).await {
            Ok(multipart) => {
                upload_document(
                    State(state),
                    Extension(auth),
                    Path(dataset_id),
                    multipart,
                    return_raw_files,
                )
                .await
            }
            Err(error) => api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        },
        "web" => upload_web_document(state, auth, dataset_id, request).await,
        _ => api_error_result(
            StatusCode::OK,
            code::INVALID_ARGUMENT,
            "`type` must be one of \"local\", \"web\", or \"empty\".",
        ),
    }
}

/// `POST /api/v1/datasets/{id}/documents?type=web` — render an http(s) page to
/// PDF with headless Chromium, then ingest it like a local upload. The URL is
/// checked with the same SSRF guard as the crawler before Chromium sees it.
async fn upload_web_document(
    state: Arc<AppState>,
    auth: AuthContext,
    dataset_id: String,
    request: Request,
) -> Response {
    if state.kbs.get(&dataset_id).is_none() {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            &format!("Can't find the dataset with ID {dataset_id}!"),
        );
    }
    if !kb_manageable(&state, &dataset_id, &auth) {
        return api_error_result(StatusCode::OK, code::UNAUTHENTICATED, "no authorization");
    }
    let mut multipart = match Multipart::from_request(request, &state).await {
        Ok(multipart) => multipart,
        Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
    };
    let mut name = String::new();
    let mut url = String::new();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        };
        let field_name = field.name().map(str::to_string);
        match field_name.as_deref() {
            Some("name") => {
                if let Ok(value) = field.text().await {
                    name = value.trim().to_string();
                }
            }
            Some("url") => {
                if let Ok(value) = field.text().await {
                    url = value.trim().to_string();
                }
            }
            _ => {}
        }
    }
    if name.is_empty() {
        return api_error_result(StatusCode::OK, code::INVALID_ARGUMENT, "Lack of \"name\"");
    }
    if url.is_empty() {
        return api_error_result(StatusCode::OK, code::INVALID_ARGUMENT, "Lack of \"url\"");
    }
    if name.len() > 255 {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_ARGUMENT,
            "File name must be 255 bytes or less.",
        );
    }
    if url.len() > 4096 {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_ARGUMENT,
            "The URL format is invalid",
        );
    }
    let checked_url = url.clone();
    let safe = tokio::task::spawn_blocking(move || {
        crate::common::ssrf_guard::assert_url_is_safe(&checked_url)
    })
    .await;
    match safe {
        Ok(Ok(_)) => {}
        _ => {
            return api_error_result(
                StatusCode::OK,
                code::INVALID_ARGUMENT,
                "The URL format is invalid",
            );
        }
    }
    let pdf = match render_url_to_pdf(&url).await {
        Ok(pdf) => pdf,
        Err(error) => {
            return api_error_result(
                StatusCode::OK,
                code::SERVER_ERROR,
                &format!("html2pdf failed: {error}"),
            );
        }
    };
    let upload_dir = FsPath::new(&state.static_dir).join("../uploads");
    if let Err(error) = tokio::fs::create_dir_all(&upload_dir).await {
        return api_error_result(
            StatusCode::OK,
            code::SERVER_ERROR,
            &format!("Cannot create upload directory: {error}"),
        );
    }
    let doc_id = uuid::Uuid::new_v4().to_string();
    let storage_name = format!("{doc_id}.pdf");
    let path = upload_dir.join(&storage_name);
    if let Err(error) = tokio::fs::write(&path, &pdf).await {
        return api_error_result(
            StatusCode::OK,
            code::SERVER_ERROR,
            &format!("Cannot store the generated PDF: {error}"),
        );
    }
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    hasher.update(&pdf);
    let upload = PersistedUpload {
        name: format!("{name}.pdf"),
        storage_name,
        path,
        size: pdf.len(),
        content_hash: format!("{:032x}", hasher.digest128()),
        cleanup_on_drop: true,
    };
    match register_document_upload(state, &auth.user_id, &dataset_id, doc_id, upload) {
        Ok(data) => (
            StatusCode::OK,
            Json(serde_json::json!({ "code": 0, "data": data })),
        )
            .into_response(),
        Err(error) => api_error_result(StatusCode::OK, code::SERVER_ERROR, &error.to_string()),
    }
}

async fn render_url_to_pdf(url: &str) -> anyhow::Result<Vec<u8>> {
    let binary = std::env::var("RAYRAG_CHROMIUM_BIN").unwrap_or_else(|_| "chromium".to_string());
    let stamp = uuid::Uuid::new_v4();
    let pdf_path = std::env::temp_dir().join(format!("rayrag-web-{stamp}.pdf"));
    let profile_dir = std::env::temp_dir().join(format!("rayrag-chrome-{stamp}"));
    let mut command = tokio::process::Command::new(binary);
    command
        .arg("--headless=new")
        .arg("--no-sandbox")
        .arg("--disable-gpu")
        .arg("--disable-dev-shm-usage")
        .arg("--no-first-run")
        .arg("--no-pdf-header-footer")
        .arg("--virtual-time-budget=15000")
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg(format!("--print-to-pdf={}", pdf_path.display()))
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let output =
        match tokio::time::timeout(std::time::Duration::from_secs(45), command.output()).await {
            Ok(output) => output?,
            Err(_) => anyhow::bail!("Chromium timed out while rendering the page"),
        };
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if !output.status.success() {
        anyhow::bail!(
            "Chromium exited with {}: {}",
            output.status,
            stderr.chars().take(400).collect::<String>()
        );
    }
    let bytes = tokio::fs::read(&pdf_path).await;
    tokio::fs::remove_file(&pdf_path).await.ok();
    tokio::fs::remove_dir_all(&profile_dir).await.ok();
    let bytes = bytes?;
    if !bytes.starts_with(b"%PDF") {
        anyhow::bail!(
            "Chromium did not produce a PDF{}",
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.chars().take(400).collect::<String>())
            }
        );
    }
    Ok(bytes)
}

async fn upload_empty_document(
    state: Arc<AppState>,
    auth: AuthContext,
    dataset_id: String,
    request: Request,
) -> Response {
    if state.kbs.get(&dataset_id).is_none() {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            &format!("Can't find the dataset with ID {dataset_id}!"),
        );
    }
    if !kb_manageable(&state, &dataset_id, &auth) {
        return api_error_result(StatusCode::OK, code::UNAUTHENTICATED, "no authorization");
    }
    let body = match axum::body::to_bytes(
        request.into_body(),
        state.max_upload_bytes.max(64 * 1024),
    )
    .await
    {
        Ok(body) => body,
        Err(error) => {
            return api_error_result(
                StatusCode::OK,
                code::INVALID_ARGUMENT,
                &format!("Malformed request body: {error}"),
            );
        }
    };
    let payload: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_result(
                StatusCode::OK,
                code::INVALID_ARGUMENT,
                &format!("Malformed JSON body: {error}"),
            );
        }
    };
    let name = payload
        .get("name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_ARGUMENT,
            "File name can't be empty.",
        );
    }
    if name.len() > 255 {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_ARGUMENT,
            "File name must be 255 bytes or less.",
        );
    }
    if state
        .docs
        .list(&dataset_id)
        .iter()
        .any(|doc| doc.name == name)
    {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            "Duplicated document name in the same dataset.",
        );
    }
    let now = unix_ms();
    let doc = crate::api::document::DocRecord {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        kb_id: dataset_id.clone(),
        size: 0,
        storage_name: String::new(),
        content_hash: String::new(),
        indexed_content_hash: String::new(),
        run: "UNSTARTED".into(),
        progress: 0.0,
        progress_msg: String::new(),
        chunk_count: 0,
        created_at: now,
        updated_at: now,
        status: crate::api::document::enabled_status(),
        doc_type: Some("virtual".into()),
        ..Default::default()
    };
    let doc = match state.docs.insert_unique(doc) {
        Ok(doc) => doc,
        Err(error) => {
            return api_error_result(StatusCode::OK, code::OPERATION_ERROR, &error.to_string());
        }
    };
    if let Err(error) = state.kbs.update_counts(&dataset_id, 0, 1) {
        let _ = state.docs.delete(&doc.id);
        return api_error_result(StatusCode::OK, code::OPERATION_ERROR, &error.to_string());
    }
    let data = crate::api::document::empty_document_entry(&state, &doc);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "code": 0, "data": data })),
    )
        .into_response()
}

async fn upload_document(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
    mut multipart: Multipart,
    return_raw_files: bool,
) -> Response {
    if state.kbs.get(&dataset_id).is_none() {
        // Upstream: `Can't find the dataset with ID …!` (DATA_ERROR), delivered on HTTP 200.
        return api_error_result(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            &format!("Can't find the dataset with ID {dataset_id}!"),
        );
    }
    if !kb_manageable(&state, &dataset_id, &auth) {
        // Upstream: `no authorization` (AUTHENTICATION_ERROR), also on HTTP 200.
        return api_error_result(StatusCode::OK, code::UNAUTHENTICATED, "no authorization");
    }
    let upload_dir = FsPath::new(&state.static_dir).join("../uploads");
    let mut succeeded = Vec::new();
    let mut failed = Vec::new();
    let mut file_count = 0_usize;
    let mut pending: Vec<(String, String, PersistedUpload)> = Vec::new();
    let mut parent_path: Option<String> = None;
    let mut parser_config_request: Option<serde_json::Value> = None;
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => return api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        };
        match field.name() {
            // Both text fields are read before registration: multipart clients are
            // free to put `file` before or after them, and upstream's form parser
            // exposes every field regardless of order.
            Some("parent_path") => {
                if let Ok(value) = field.text().await {
                    parent_path = sanitize_parent_path(Some(&value));
                }
                continue;
            }
            Some("parser_config") => {
                if let Ok(value) = field.text().await {
                    parser_config_request = parser_config_override(&value);
                }
                continue;
            }
            Some("file") => {}
            _ => continue,
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
            Ok(upload) => pending.push((doc_id, original_name, upload)),
            Err(error) => failed.push(serde_json::json!({
                "name": original_name,
                "message": error.to_string()
            })),
        }
    }
    if file_count == 0 {
        // Upstream: `No file part!` (ARGUMENT_ERROR) on HTTP 200.
        return api_error_result(StatusCode::OK, code::INVALID_ARGUMENT, "No file part!");
    }
    let parser_config = merged_parser_config(&state, &dataset_id, &parser_config_request);
    for (doc_id, original_name, mut upload) in pending {
        if let Some(parent) = parent_path.as_deref() {
            if let Err(error) = move_upload_into_parent(&mut upload, parent, &upload_dir).await {
                failed.push(serde_json::json!({
                    "name": original_name,
                    "message": error.to_string()
                }));
                continue;
            }
        }
        let doc_id_for_raw = doc_id.clone();
        match register_document_upload_with_options(
            state.clone(),
            &auth.user_id,
            &dataset_id,
            doc_id,
            upload,
            parser_config.clone(),
            parent_path.clone(),
        ) {
            Ok(data) => {
                if return_raw_files {
                    let raw = state
                        .docs
                        .get(&doc_id_for_raw)
                        .map(|doc| crate::api::document::raw_document_entry(&state, &doc))
                        .unwrap_or(data);
                    succeeded.push(raw);
                } else {
                    succeeded.push(data);
                }
            }
            Err(error) => failed.push(serde_json::json!({
                "name": original_name,
                "message": error.to_string()
            })),
        }
    }
    if failed.is_empty() {
        // Upstream `_upload_local_documents`: `get_result(data=doc_data)` — HTTP 200, the
        // mapped documents as an **array**, and no `message` key on success.
        let mut data = serde_json::Value::Array(succeeded);
        if !return_raw_files {
            crate::api::compat::enrich_documents(&mut data);
        }
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "code": 0, "data": data })),
        )
            .into_response();
    }
    // Upstream joins the failures as `"{filename}: {message}"` lines and answers with
    // `SERVER_ERROR` — HTTP 200 and business code 500.
    let message = failed
        .iter()
        .map(|failure| {
            format!(
                "{}: {}",
                failure["name"].as_str().unwrap_or("uploaded"),
                failure["message"].as_str().unwrap_or("Upload failed")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if succeeded.is_empty() {
        return api_error_result(StatusCode::OK, code::SERVER_ERROR, &message);
    }
    // A partial success keeps upstream's mixed envelope: code 500, the failure lines in
    // `message`, and the documents that did land in `data`.
    let mut data = serde_json::Value::Array(succeeded);
    if !return_raw_files {
        crate::api::compat::enrich_documents(&mut data);
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "code": code::SERVER_ERROR, "message": message, "data": data })),
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
    register_document_upload_with_options(state, owner_id, dataset_id, doc_id, upload, None, None)
}

pub(crate) fn register_document_upload_with_options(
    state: Arc<AppState>,
    owner_id: &str,
    dataset_id: &str,
    doc_id: String,
    upload: PersistedUpload,
    parser_config: Option<serde_json::Value>,
    parent_path: Option<String>,
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
        status: crate::api::document::enabled_status(),
        parser_config,
        parent_path,
        ..Default::default()
    };
    let doc = state.docs.insert_unique(doc)?;
    if let Err(error) = state.kbs.update_counts(dataset_id, 0, 1) {
        // The upload is being undone: report the primary failure, but say it if the
        // compensating writes fail too, otherwise an orphan row/blob stays behind.
        if let Err(cleanup) = state.docs.delete(&doc_id) {
            tracing::error!(%cleanup, %doc_id, "Failed to remove the document row after a failed upload");
        }
        if let Err(cleanup) = std::fs::remove_file(&path) {
            tracing::error!(%cleanup, path = %path.display(), "Failed to remove the uploaded blob after a failed upload");
        }
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
            if let Err(cleanup) = state.docs.delete(&doc_id) {
                tracing::error!(%cleanup, %doc_id, "Failed to remove the document row after a failed queue");
            }
            if let Err(cleanup) = state.kbs.update_counts(dataset_id, 0, -1) {
                tracing::error!(%cleanup, %dataset_id, "Failed to roll back the KB document count");
            }
            if let Err(cleanup) = std::fs::remove_file(&path) {
                tracing::error!(%cleanup, path = %path.display(), "Failed to remove the uploaded blob after a failed queue");
            }
            return Err(error);
        }
    };
    upload.commit();
    tracing::info!(%task_id, %doc_id, "document uploaded");
    let tokens = {
        let mut ids = std::collections::HashSet::new();
        ids.insert(doc.id.clone());
        state
            .engine
            .read()
            .map(|engine| engine.token_counts_for(&doc.kb_id, &ids))
            .unwrap_or_default()
            .remove(&doc.id)
            .unwrap_or(0)
    };
    Ok(crate::api::document::doc_list_entry(&state, &doc, tokens))
}

/// RAGFlow's business codes — the API guide's "Response body codes" table. The HTTP status and
/// the body code are separate signals, and callers are told to check both: a `400` can carry
/// `101` (bad argument), `102` (missing/invalid data) or `103` (operation error), a `401` carries
/// `109`, a `403` carries `108`, and a duplicate is `409` in both places.
pub(crate) mod code {
    pub const SUCCESS: i32 = 0;
    pub const INVALID_ARGUMENT: i32 = 101;
    pub const INVALID_OR_MISSING_DATA: i32 = 102;
    pub const OPERATION_ERROR: i32 = 103;
    /// RAGFlow's `RetCode.NOT_EFFECTIVE`: the request worked but did not take effect.
    pub const NOT_EFFECTIVE: i32 = 10;
    pub const CONNECTION_ERROR: i32 = 105;
    pub const STILL_RUNNING: i32 = 106;
    pub const NO_PERMISSION: i32 = 108;
    pub const UNAUTHENTICATED: i32 = 109;
    /// RAGFlow's `RetCode.SERVER_ERROR`: the upload family reports a server-side failure in
    /// the body while the transport stays HTTP 200.
    pub const SERVER_ERROR: i32 = 500;
}

/// An error whose HTTP status and business code are chosen separately, so a caller that only
/// reads the status and a caller that only reads the body code agree on what happened.
pub(crate) fn api_error_code(status: StatusCode, business_code: i32, message: &str) -> Response {
    (
        status,
        Json(ApiResponse {
            code: business_code,
            message: message.into(),
            data: None,
        }),
    )
        .into_response()
}

/// The `get_error_data_result` shape: `{code, message}` and **no** `data` key — upstream's
/// helper drops `None` fields from the dict it builds, so a client must not find the key at
/// all. The document family's business errors all take this form (probes `up_both`,
/// `up_none`, `img_fake`, `del_nobody`, `parse_nobody`, …), unlike `api_error_code`, which
/// serialises `data: null` for callers whose counterpart uses `get_json_result`.
pub(crate) fn api_error_result(status: StatusCode, business_code: i32, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "code": business_code, "message": message })),
    )
        .into_response()
}

/// `101` — the request was understood but an argument is wrong or absent.
pub(crate) fn invalid_argument(message: &str) -> Response {
    api_error_code(StatusCode::BAD_REQUEST, code::INVALID_ARGUMENT, message)
}

/// `102` — the thing being addressed does not exist, or is not owned by the caller. Upstream uses
/// this for "You don't own the dataset …" and for missing rows.
pub(crate) fn invalid_or_missing(message: &str) -> Response {
    api_error_code(
        StatusCode::BAD_REQUEST,
        code::INVALID_OR_MISSING_DATA,
        message,
    )
}

/// `108` — the caller may not perform this operation.
pub(crate) fn no_permission(message: &str) -> Response {
    api_error_code(StatusCode::FORBIDDEN, code::NO_PERMISSION, message)
}

/// `109` — not authenticated (or the token expired).
pub(crate) fn unauthenticated(message: &str) -> Response {
    api_error_code(StatusCode::UNAUTHORIZED, code::UNAUTHENTICATED, message)
}

/// `409` — the request conflicts with existing state, e.g. a duplicate name.
pub(crate) fn conflict(message: &str) -> Response {
    api_error_code(StatusCode::CONFLICT, 409, message)
}

pub(crate) fn api_error(status: StatusCode, message: &str) -> Response {
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
    // Dataset bound to a data pipeline (parse_type=2): the Go backend runs the
    // user canvas. RayRAG has no dataflow canvas engine, so refuse explicitly
    // instead of silently running the built-in parser and reporting success.
    if let Some(pipeline) = state
        .kbs
        .get(&doc.kb_id)
        .and_then(|kb| kb.pipeline_id)
        .filter(|pipeline| !pipeline.trim().is_empty())
    {
        anyhow::bail!(
            "Dataset is bound to data pipeline '{pipeline}'; data pipeline execution is not available yet — switch the dataset to a built-in pipeline."
        );
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
        if let Err(cancel_error) = state.tasks.request_cancel(&task_id) {
            tracing::error!(%cancel_error, %task_id, "Failed to cancel the queued task after a status write failure");
        }
        return Err(error);
    }
    // Hand the task to the async TaskExecutor (RAGFlow task_executor port):
    // the executor's worker pool runs `process_document` off the request path.
    // The TaskQueue lease makes this execution exclusive with the legacy
    // dispatcher, so queueing here is idempotent with `dispatch_pending_document_tasks`.
    state
        .task_executor
        .submit(crate::task_executor::ExecTask {
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
        })
        .map_err(|error| {
            // Answering "queued for parsing" while the executor is shut down leaves the
            // document UNSTARTED for ever with nothing to explain why.
            anyhow::anyhow!("Failed to queue the parse task: {error}")
        })?;
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
        if let Err(error) =
            state
                .docs
                .update_status(&doc.id, "CANCELLED", 1.0, "Processing cancelled")
        {
            tracing::error!(%error, doc_id = %doc.id, "Failed to persist cancelled document status");
        }
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
        // Include setup failures in the same durable log as extraction failures.
        let parsed = async {
            let config: crate::ParserConfig = serde_json::from_str(&kb.parser_config)
                .map_err(|error| anyhow::anyhow!(
                    "Invalid parser configuration: {error}. Open dataset Configuration, correct the parser settings, and parse again."
                ))?;
            let embedder = kb_embedder_for(&state, std::slice::from_ref(&doc.kb_id))
                .map_err(|error| anyhow::anyhow!(
                    "{error}. Configure an embedding model in model providers, check its API URL, API key and model name, then select it in dataset Configuration and parse again."
                ))?;
            // Attach the tenant image2text model when available; OCR may recover.
            let mut pipeline_builder =
                crate::pipeline::Pipeline::new(config.clone()).with_shared_embedder(embedder);
            match kb_vision_for(&state, std::slice::from_ref(&doc.kb_id)) {
                Ok(Some(vision)) => pipeline_builder = pipeline_builder.with_vision_client(vision),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%error, "Vision model resolution failed; image parsing continues with OCR fallback");
                    pipeline_builder = pipeline_builder.with_vision_config_error(format!(
                        "Cannot resolve the tenant image-to-text model: {error}. Open model providers, enable the provider and select a valid image-to-text model"
                    ));
                }
            }
            let pipeline = pipeline_builder.with_document_parsers_from_env()
                .map_err(|error| anyhow::anyhow!(
                    "{error}. Check the OCR/layout/ASR client provider, API URL, model and credentials in the deployment environment, then parse again."
                ))?;
            let path_str = path
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Invalid upload path"))?;
            let (chunks, report) = pipeline.process_with_report(path_str).await?;
            Ok::<_, anyhow::Error>((config, chunks, report))
        }.await;
        let (config, chunks, report) = match parsed {
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
        let raptor_enabled = config.raptor_enabled;
        let raptor_depth = config.raptor_depth;
        let raptor_threshold = config.raptor_threshold;
        let graphrag_enabled = config.graphrag_enabled;
        let graphrag_method = config.graphrag_method.clone();
        let graphrag_entity_types = config.graphrag_entity_types.clone();
        let indexed_result = async {
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
            ).map_err(|error| anyhow::anyhow!(
                "Failed to commit document index: {error:#}. Check index storage permissions, free disk space and the configured database/vector backend connection, then parse again."
            ))?;
            Ok::<usize, anyhow::Error>(new_count)
        }
        .await;
        // Extraction success is not ingestion success until the index is committed.
        let (status, message) = match &indexed_result {
            Ok(_) => ("done", String::new()),
            Err(error) => ("failed", error.to_string()),
        };
        if let Err(error) = crate::api::ingestion::record_parse_run(
            &state,
            &kb.owner_id,
            &doc.kb_id,
            &doc.id,
            &doc.name,
            &kb_parser_id(&kb),
            &report,
            status,
            &message,
        ) {
            tracing::warn!(%error, "Failed to record ingestion log");
        }
        indexed_result
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
            // A cancellation that fails to persist leaves the task claimed and the
            // document parsing for ever, with nothing in the log to explain it.
            if let Err(error) =
                state
                    .tasks
                    .update_claimed(&task_id, &lease.token, "cancelled", 1.0, message)
            {
                tracing::error!(%error, %task_id, "Failed to persist task cancellation");
            }
            if let Err(error) = state.docs.update_status(&doc.id, "CANCELLED", 1.0, message) {
                tracing::error!(%error, doc_id = %doc.id, "Failed to persist cancelled document status");
            }
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
            if let Err(error) = state
                .docs
                .update_status(&doc.id, "UNSTARTED", 0.0, &message)
            {
                // The retry is queued, but the document keeps showing FAILED: the two
                // views disagree and nothing says why.
                tracing::error!(%error, doc_id = %doc.id, "Failed to reset document status for the retry");
            }
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
    let previous_doc = {
        let mut engine = state.engine.write().unwrap();
        let previous = engine.chunks_for_document(&doc.id);
        engine.replace_document(&doc.id, indexed);
        let current = engine.chunks_for_document(&doc.id);
        persist_document_change(state, &mut engine, &doc.id, &doc.kb_id, &previous, &current)?;
        previous
    };

    if let Err(error) = state.graphs.replace_document(&doc.id, graph_checkpoint) {
        restore_document_index(
            state,
            &doc.id,
            &doc.kb_id,
            &previous_doc,
            "Graph checkpoint update failed",
        );
        return Err(error.context("Failed to update GraphRAG artifacts after index commit"));
    }

    if let Err(error) = state.kbs.update_counts(&doc.kb_id, count_delta, 0) {
        if let Err(rollback) = state.graphs.replace_snapshot(previous_graphs) {
            tracing::error!(%rollback, doc_id = %doc.id, "Failed to roll back the GraphRAG snapshot");
        }
        restore_document_index(
            state,
            &doc.id,
            &doc.kb_id,
            &previous_doc,
            "KB count update failed",
        );
        return Err(error.context("Failed to update KB counts after index commit"));
    }
    match state
        .docs
        .mark_indexed(&doc.id, &doc.content_hash, new_count, message)
    {
        Ok(true) => {}
        Ok(false) => {
            if let Err(rollback) = state.graphs.replace_snapshot(previous_graphs.clone()) {
                tracing::error!(%rollback, doc_id = %doc.id, "Failed to roll back the GraphRAG snapshot");
            }
            if let Err(rollback) = state.kbs.update_counts(&doc.kb_id, -count_delta, 0) {
                tracing::error!(%rollback, kb_id = %doc.kb_id, "Failed to roll back the chunk count");
            }
            restore_document_index(
                state,
                &doc.id,
                &doc.kb_id,
                &previous_doc,
                "Document metadata disappeared during commit",
            );
            anyhow::bail!("Document metadata disappeared during index commit");
        }
        Err(error) => {
            if let Err(rollback) = state.graphs.replace_snapshot(previous_graphs) {
                tracing::error!(%rollback, doc_id = %doc.id, "Failed to roll back the GraphRAG snapshot");
            }
            if let Err(rollback_error) = state.kbs.update_counts(&doc.kb_id, -count_delta, 0) {
                tracing::error!(%rollback_error, doc_id = %doc.id, "Failed to roll back KB counts");
            }
            restore_document_index(
                state,
                &doc.id,
                &doc.kb_id,
                &previous_doc,
                "Document metadata update failed",
            );
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

fn restore_document_index(
    state: &AppState,
    doc_id: &str,
    kb_id: &str,
    previous: &[crate::search::IndexedChunk],
    reason: &str,
) {
    let mut engine = state.engine.write().unwrap();
    crate::store::rollback_document_index(
        &mut engine,
        &state.index_path,
        &state.vector_mirror,
        doc_id,
        kb_id,
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

/// Persist one document's change without cloning the whole search index.
///
/// The JSON index is still saved in full, but the mirror update and the rollback
/// snapshot only touch the document's chunks. A commit on a large knowledge base
/// no longer allocates a second copy of every embedding in the corpus.
pub(crate) fn persist_document_change(
    state: &AppState,
    engine: &mut crate::search::SearchEngine,
    doc_id: &str,
    kb_id: &str,
    previous: &[crate::search::IndexedChunk],
    current: &[crate::search::IndexedChunk],
) -> anyhow::Result<()> {
    crate::store::search_mapping::validate_replacement_snapshot(engine.chunks())?;
    if let Err(error) = state
        .vector_mirror
        .replace_document_chunks(kb_id, previous, current)
    {
        if let Err(rollback_error) = state
            .vector_mirror
            .replace_document_chunks(kb_id, current, previous)
        {
            tracing::error!(%rollback_error, doc_id = %doc_id, "Failed to roll back zvec document mirror");
        }
        engine.replace_document(doc_id, previous.to_vec());
        return Err(error.context("Failed to commit document to online vector mirror"));
    }
    if let Err(error) = engine.save(&state.index_path) {
        if let Err(rollback_error) = state
            .vector_mirror
            .replace_document_chunks(kb_id, current, previous)
        {
            tracing::error!(%rollback_error, doc_id = %doc_id, "Failed to roll back zvec document mirror");
        }
        engine.replace_document(doc_id, previous.to_vec());
        return Err(error.context("Failed to persist document change to JSON index"));
    }
    Ok(())
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
            if let Err(error) = state.tasks.update_claimed(
                &task_id,
                &lease.token,
                "cancelled",
                1.0,
                "Processing cancelled",
            ) {
                tracing::error!(%error, %task_id, "Failed to persist Memory task cancellation");
            }
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
            // Every one of these writes decides a task's fate; a silent failure leaves it
            // pending/claimed for ever and the file is never parsed.
            if let Err(error) = state.tasks.update(&task.id, status, 1.0, &message) {
                tracing::error!(%error, task_id = %task.id, "Failed to persist recovered task outcome");
            }
            drop(active_guard);
            drop(permit);
            continue;
        }
        let Some(doc) = state.docs.get(&task.doc_id) else {
            if let Err(error) = state.tasks.update(
                &task.id,
                "failed",
                1.0,
                "Cannot recover task because document metadata is missing",
            ) {
                tracing::error!(%error, task_id = %task.id, "Failed to persist task rejection (document metadata missing)");
            }
            drop(active_guard);
            drop(permit);
            continue;
        };
        let path = document_storage_path(&state, &doc);
        if !path.is_file() {
            if let Err(error) = state.tasks.update(
                &task.id,
                "failed",
                1.0,
                "Cannot recover task because uploaded file is missing",
            ) {
                tracing::error!(%error, task_id = %task.id, "Failed to persist task rejection (uploaded file missing)");
            }
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
    let gpus = crate::host_resources::gpu_summary(&crate::host_resources::detect_gpus());
    tracing::info!(
        host = %resources.summary(),
        gpus = %gpus,
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
    let (removed_chunks, previous_doc) = {
        let mut engine = state.engine.write().unwrap();
        let previous = engine.chunks_for_document(&doc.id);
        let removed = engine.remove_document(&doc.id);
        persist_document_change(state, &mut engine, &doc.id, &doc.kb_id, &previous, &[])?;
        (removed, previous)
    };
    if let Err(error) = state.graphs.replace_document(&doc.id, None) {
        restore_document_index(
            state,
            &doc.id,
            &doc.kb_id,
            &previous_doc,
            "Graph checkpoint deletion failed",
        );
        return Err(error.context("Failed to delete GraphRAG document checkpoint"));
    }
    if let Err(error) = state
        .kbs
        .update_counts(&doc.kb_id, -(removed_chunks as isize), -1)
    {
        if let Err(rollback) = state.graphs.replace_snapshot(previous_graphs) {
            tracing::error!(%rollback, doc_id = %doc.id, "Failed to roll back the GraphRAG snapshot");
        }
        restore_document_index(
            state,
            &doc.id,
            &doc.kb_id,
            &previous_doc,
            "KB count update failed during document deletion",
        );
        return Err(error.context("Failed to update KB counts during document deletion"));
    }
    let previous_document_metadata = state.document_metadata.get(&doc.id, &doc.kb_id);
    if let Err(error) = state.document_metadata.delete(&doc.id, &doc.kb_id) {
        if let Err(rollback) = state.graphs.replace_snapshot(previous_graphs) {
            tracing::error!(%rollback, doc_id = %doc.id, "Failed to roll back the GraphRAG snapshot");
        }
        if let Err(rollback) = state
            .kbs
            .update_counts(&doc.kb_id, removed_chunks as isize, 1)
        {
            tracing::error!(%rollback, kb_id = %doc.kb_id, "Failed to roll back the chunk count");
        }
        restore_document_index(
            state,
            &doc.id,
            &doc.kb_id,
            &previous_doc,
            "Document metadata store deletion failed",
        );
        return Err(error.context("Failed to delete document-level metadata"));
    }
    match state.docs.delete(&doc.id) {
        Ok(true) => {}
        Ok(false) => {
            if let Some(metadata) = previous_document_metadata.clone()
                && let Err(rollback) = state
                    .document_metadata
                    .replace(&doc.id, &doc.kb_id, metadata)
            {
                tracing::error!(%rollback, doc_id = %doc.id, "Failed to restore document-level metadata");
            }
            if let Err(rollback) = state.graphs.replace_snapshot(previous_graphs.clone()) {
                tracing::error!(%rollback, doc_id = %doc.id, "Failed to roll back the GraphRAG snapshot");
            }
            if let Err(rollback) = state
                .kbs
                .update_counts(&doc.kb_id, removed_chunks as isize, 1)
            {
                tracing::error!(%rollback, kb_id = %doc.kb_id, "Failed to roll back the chunk count");
            }
            restore_document_index(
                state,
                &doc.id,
                &doc.kb_id,
                &previous_doc,
                "Document metadata was already missing",
            );
            anyhow::bail!("Document metadata was already deleted");
        }
        Err(error) => {
            if let Some(metadata) = previous_document_metadata
                && let Err(rollback) = state
                    .document_metadata
                    .replace(&doc.id, &doc.kb_id, metadata)
            {
                tracing::error!(%rollback, doc_id = %doc.id, "Failed to restore document-level metadata");
            }
            if let Err(rollback) = state.graphs.replace_snapshot(previous_graphs) {
                tracing::error!(%rollback, doc_id = %doc.id, "Failed to roll back the GraphRAG snapshot");
            }
            if let Err(rollback_error) =
                state
                    .kbs
                    .update_counts(&doc.kb_id, removed_chunks as isize, 1)
            {
                tracing::error!(%rollback_error, doc_id = %doc.id, "Failed to roll back KB counts");
            }
            restore_document_index(
                state,
                &doc.id,
                &doc.kb_id,
                &previous_doc,
                "Document metadata deletion failed",
            );
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

/// `GET /api/v1/datasets/{id}/graph` — upstream `dataset_api.get_knowledge_graph`.
///
/// Returns the graph this dataset has already built (the same artifacts
/// `POST /graphrag/build` writes and `/graphrag/query` reads), shaped as upstream's
/// `{graph: {nodes, edges}, mind_map: {}}`. A dataset with no graph yet answers with an empty
/// graph and a `built: false` marker instead of a 404, so the UI can say "not built yet"
/// rather than treating it as an error.
/// `POST /api/v1/datasets/{id}/index` — upstream `dataset_api.rebuild_dataset_index`.
///
/// Re-mirrors the dataset's chunks into the vector index. RayRAG already performed exactly this
/// inside its internal repair sweep; the endpoint reports what the sweep found per dataset
/// (`chunks`, `repaired`) instead of a bare success.
async fn rebuild_dataset_index(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(dataset_id): Path<String>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("You don't own the dataset {dataset_id}."),
        );
    }
    let backend = if state.vector_mirror.is_enabled() {
        "online"
    } else {
        "disabled"
    };
    let chunks = state.engine.read().unwrap().chunks_for_kb(&dataset_id);
    if chunks.is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "data": {
                "dataset_id": dataset_id,
                "backend": backend,
                "chunks": 0,
                "repaired": false,
                "message": "No parsed chunks in this dataset; nothing to index."
            }
        }))
        .into_response();
    }
    let count = chunks.len();
    match state.vector_mirror.repair_owner(&dataset_id, &chunks) {
        Ok(repaired) => Json(serde_json::json!({
            "code": 0,
            "data": { "dataset_id": dataset_id, "backend": backend, "chunks": count, "repaired": repaired }
        }))
        .into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("rebuilding the index failed: {error}"),
        ),
    }
}

/// `GET /api/v1/datasets/{id}/index` — what the dataset's vector index currently holds.
async fn dataset_index_status(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(dataset_id): Path<String>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("You don't own the dataset {dataset_id}."),
        );
    }
    let chunks = state.engine.read().unwrap().chunks_for_kb(&dataset_id);
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "dataset_id": dataset_id,
            "index_type": "vector",
            "enabled": state.vector_mirror.is_enabled(),
            "chunks": chunks.len()
        }
    }))
    .into_response()
}

/// `DELETE /api/v1/datasets/{id}/index` — drop the dataset's vector index (no index name in the
/// path; upstream's variant of `DELETE /datasets/{id}/{index_type}`).
async fn delete_named_dataset_index(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path(dataset_id): Path<String>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("You don't own the dataset {dataset_id}."),
        );
    }
    let removed = state
        .engine
        .read()
        .unwrap()
        .chunks_for_kb(&dataset_id)
        .len();
    match state.vector_mirror.repair_owner(&dataset_id, &[]) {
        Ok(_) => Json(serde_json::json!({
            "code": 0,
            "data": { "dataset_id": dataset_id, "index_type": "vector", "removed_vectors": removed }
        }))
        .into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("dropping the index failed: {error}"),
        ),
    }
}

/// `DELETE /api/v1/datasets/{id}/{index_type}` — upstream drops one named index for a dataset.
///
/// RayRAG keeps a single vector index per dataset, so only the vector index type is meaningful;
/// any other type is refused with the list of what it does support rather than silently
/// answering "deleted".
async fn delete_dataset_index(
    State(state): State<Arc<AppState>>,
    axum::extract::Extension(auth): axum::extract::Extension<AuthContext>,
    Path((dataset_id, index_type)): Path<(String, String)>,
) -> Response {
    if !kb_accessible(&state, &dataset_id, &auth) {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("You don't own the dataset {dataset_id}."),
        );
    }
    if index_type != "vector" {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!(
                "RayRAG keeps one vector index per dataset, so '{index_type}' cannot be dropped. Supported: vector."
            ),
        );
    }
    let chunks = state.engine.read().unwrap().chunks_for_kb(&dataset_id);
    let removed = chunks.len();
    match state.vector_mirror.repair_owner(&dataset_id, &[]) {
        Ok(_) => Json(serde_json::json!({
            "code": 0,
            "data": { "dataset_id": dataset_id, "index_type": index_type, "removed_vectors": removed }
        }))
        .into_response(),
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("dropping the index failed: {error}"),
        ),
    }
}

async fn dataset_graph(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    Query(query): Query<DatasetGraphQuery>,
) -> Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return api_error(
            StatusCode::BAD_REQUEST,
            &format!("You don't own the dataset {kb_id}."),
        );
    }
    let max_nodes = query.max_nodes.unwrap_or(256).clamp(1, 2048);
    let snapshot = state.graphs.snapshot();
    let Some(graph) = snapshot.kb_graphs.get(&kb_id) else {
        return Json(serde_json::json!({
            "code": 0,
            "data": {
                "graph": { "nodes": [], "edges": [], "total_nodes": 0, "returned_nodes": 0, "returned_edges": 0 },
                "mind_map": {},
                "built": false,
            }
        }))
        .into_response();
    };
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "graph": graph.to_upstream_graph(max_nodes),
            "mind_map": {},
            "built": true,
        }
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct DatasetGraphQuery {
    #[serde(default)]
    max_nodes: Option<usize>,
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

/// Publish the first registered account after database credentials synchronize.
async fn register_account(
    state: &Arc<AppState>,
    nickname: &str,
    email: &str,
    password: &str,
) -> anyhow::Result<crate::auth::User> {
    let users = state.users.clone();
    let nickname = nickname.to_owned();
    let email = email.to_owned();
    let password = password.to_owned();
    let env_path = crate::api::setup::env_file_path(state);
    let static_dir = state.static_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        users.register_with(&nickname, &email, &password, |password| {
            crate::persistence::sync_postgres_admin_password(password, &static_dir, &env_path)?;
            Ok(())
        })
    })
    .await
    .map_err(|error| anyhow::anyhow!("account registration task failed: {error}"))?;
    if let Err(error) = &result {
        tracing::error!(%error, "Account registration failed");
    }
    result
}

pub(crate) async fn update_account_password(
    state: &Arc<AppState>,
    user_id: &str,
    current: Option<&str>,
    new_password: &str,
) -> anyhow::Result<bool> {
    let users = state.users.clone();
    let user_id = user_id.to_owned();
    let current = current.map(str::to_owned);
    let new_password = new_password.to_owned();
    let env_path = crate::api::setup::env_file_path(state);
    let static_dir = state.static_dir.clone();
    let result = tokio::task::spawn_blocking(move || {
        users.update_password_with(&user_id, current.as_deref(), &new_password, |password| {
            if crate::persistence::sync_postgres_admin_password(password, &static_dir, &env_path)?
                .is_some()
            {
                tracing::info!(
                    "PostgreSQL administrator password synchronized during account update"
                );
            }
            Ok(())
        })
    })
    .await
    .map_err(|error| anyhow::anyhow!("account password update task failed: {error}"))?;
    if let Err(error) = &result {
        tracing::error!(%error, "Account password update failed");
    }
    result
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
    match update_account_password(&state, &auth.user_id, Some(current), new_password).await {
        Ok(true) => Json(serde_json::json!({ "code": 0, "message": "ok" })).into_response(),
        Ok(false) => {
            Json(serde_json::json!({ "code": 400, "message": "User not found" })).into_response()
        }
        Err(error) => {
            Json(serde_json::json!({ "code": 400, "message": error.to_string() })).into_response()
        }
    }
}

/// POST /api/v1/auth/logout — upstream `user_api.log_out`.
///
/// Upstream rewrites the stored `access_token` to `INVALID_<hex>` and saves the user
/// row, so the token cannot be reused. RayRAG's store keeps session tokens in a map;
/// removing the presented one has the same observable effect (the credential stops
/// working) and answers the same envelope.
async fn logout(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    state.users.logout(&auth.token);
    Json(serde_json::json!({ "code": 0, "data": true })).into_response()
}

/// The last resort for a path no route matched.
///
/// An unrouted `/api/...` path used to be answered with the **application shell HTML**.
/// A client then called `response.json()` on `<!DOCTYPE html>`, which surfaced in the UI as
/// `Unexpected token '<', "<!DOCTYPE "...` — and because the shell boots the SPA, the user
/// was thrown back to the login page with no indication that the request 404'd. An API path
/// now answers JSON with the status that matches it, so a wrong URL is visible instead of
/// masquerading as a session problem. Page paths keep the friendly HTML 404.
async fn unrouted_path(req: axum::http::Request<axum::body::Body>) -> axum::response::Response {
    let path = req.uri().path().to_string();
    if path.starts_with("/api/") {
        tracing::warn!(%path, "unrouted API path answered with JSON 404");
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "code": 404,
                "message": format!("No such API endpoint: {path}"),
                "data": null
            })),
        )
            .into_response();
    }
    crate::web::not_found_page(req).await.into_response()
}

/// `GET /api/v1/agents/prompts` — upstream `agent_api.prompts`.
///
/// The canvas shows the exact guidance its planner, reflector and citation steps run with.
/// All four strings are the constants RayRAG already carries (ported verbatim from
/// `rag/prompts/generator.py`), so the page and the runtime cannot disagree.
async fn agent_prompts(
    State(_state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
) -> Response {
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "task_analysis": format!(
                "{}\n\n{}",
                crate::prompts::RAGFLOW_ANALYZE_TASK_SYSTEM_PROMPT,
                crate::prompts::RAGFLOW_ANALYZE_TASK_USER_PROMPT
            ),
            "plan_generation": crate::prompts::RAGFLOW_NEXT_STEP_PROMPT,
            "reflection": crate::prompts::RAGFLOW_REFLECT_PROMPT,
            "citation_guidelines": crate::prompts::RAGFLOW_CITATION_PROMPT,
        }
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
struct TestDbConnectionRequest {
    db_type: String,
    database: String,
    username: String,
    host: String,
    port: serde_json::Value,
    password: String,
}

/// `POST /api/v1/agents/test_db_connection` — upstream `agent_api.test_db_connection`.
///
/// The canvas' database component asks the server to verify a connection before saving it.
/// Two rules are enforced here rather than trusted to the caller:
///
/// * the host goes through `assert_host_is_safe` (the raw host/port form of the SSRF guard),
///   so a component cannot be pointed at an internal service;
/// * **the check is a real connect-and-authenticate for PostgreSQL**, because a port probe
///   would report success for reachable-but-wrong credentials — exactly the silent error the
///   canvas must not show. Types without a client in this build answer an explicit
///   unsupported message instead of a guess.
async fn test_db_connection(
    State(_state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let request: TestDbConnectionRequest = match serde_json::from_value(body) {
        Ok(request) => request,
        Err(error) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                &format!(
                    "db_type, database, username, host, port and password are required ({error})"
                ),
            );
        }
    };
    let port = match &request.port {
        serde_json::Value::Number(number) => number.as_u64().unwrap_or(0) as u16,
        serde_json::Value::String(text) => text.trim().parse::<u16>().unwrap_or(0),
        _ => 0,
    };
    if port == 0 {
        return api_error(StatusCode::BAD_REQUEST, "port must be a positive integer");
    }
    // SSRF guard: a rejected host is a data error naming the reason, never a silent skip.
    // An operator who really needs to reach a database on the local network (the common
    // case on a NAS, where the database *is* local) opts in with
    // `RAYRAG_ALLOW_PRIVATE_DB_HOSTS=1`; the default stays closed, and the message says so
    // instead of leaving the user with a bare rejection.
    let allow_private = matches!(
        std::env::var("RAYRAG_ALLOW_PRIVATE_DB_HOSTS")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    );
    if !allow_private
        && let Err(error) = crate::common::ssrf_guard::assert_host_is_safe(&request.host)
    {
        tracing::warn!(host = %request.host, %error, "rejected test_db_connection host");
        return Json(serde_json::json!({
            "code": 102,
            "message": format!(
                "{error} Set RAYRAG_ALLOW_PRIVATE_DB_HOSTS=1 to allow database connections \
                 to private addresses."
            )
        }))
        .into_response();
    }
    let db_type = request.db_type.trim().to_ascii_lowercase();
    if db_type != "postgresql" && db_type != "postgres" {
        return Json(serde_json::json!({
            "code": 102,
            "message": format!(
                "db_type '{}' has no client in this build; only PostgreSQL connections can be verified",
                request.db_type
            )
        }))
        .into_response();
    }
    let dsn = format!(
        "host={} port={} user={} password={} dbname={} connect_timeout=10",
        request.host, port, request.username, request.password, request.database
    );
    // The driver is synchronous, so the attempt runs on a blocking thread (the same rule
    // the doc store obeys).
    let outcome = tokio::task::spawn_blocking(move || {
        postgres::Client::connect(&dsn, postgres::NoTls).map(|_| ())
    })
    .await;
    match outcome {
        Ok(Ok(())) => Json(serde_json::json!({
            "code": 0,
            "data": { "success": true, "message": "Connection succeeded." }
        }))
        .into_response(),
        Ok(Err(error)) => {
            // The password must never travel back in an error string.
            let message = error.to_string().replace(&request.password, "***");
            Json(serde_json::json!({
                "code": 102,
                "message": format!("Connection failed: {message}")
            }))
            .into_response()
        }
        Err(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("connection task failed: {error}"),
        ),
    }
}

#[derive(serde::Deserialize)]
struct AgentDownloadQuery {
    #[serde(default)]
    id: Option<String>,
}

/// `GET /api/v1/agents/download?id=` — upstream `agent_api.download_agent_file`.
///
/// One stored file's bytes, addressed by id rather than by path: the canvas downloads the
/// DSL it exported through the same file store the rest of the product uses.
async fn agent_download(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<AgentDownloadQuery>,
) -> Response {
    let Some(id) = query
        .id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return api_error(StatusCode::BAD_REQUEST, "id is required");
    };
    crate::api::file_mgr::serve_file_bytes(&state, &auth, id).await
}

/// `GET /api/v1/searchbots/detail` — upstream `bot_api.detail_share_embedded`.
///
/// The embedded search widget reads the app it was minted from: the beta token in the URL
/// identifies the tenant, `search_id` names the app. Upstream requires the app to belong to
/// that tenant, which is the ownership check performed here.
async fn searchbot_detail(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<SearchBotDetailQuery>,
) -> Response {
    let Some(search_id) = query
        .search_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return api_error(StatusCode::BAD_REQUEST, "search_id is required");
    };
    let Some(app) = state
        .search_apps
        .as_ref()
        .and_then(|store| store.get(search_id))
    else {
        return Json(serde_json::json!({
            "code": 102,
            "message": "Can't find this Search App!"
        }))
        .into_response();
    };
    if app.owner_id != auth.user_id && !auth.is_admin {
        return Json(serde_json::json!({
            "code": 102,
            "message": "Has no permission for this operation."
        }))
        .into_response();
    }
    Json(serde_json::json!({ "code": 0, "data": app })).into_response()
}

#[derive(serde::Deserialize)]
struct SearchBotDetailQuery {
    #[serde(default)]
    search_id: Option<String>,
}

/// `POST /api/v1/searchbots/retrieval_test` — upstream `bot_api.retrieval_test_embedded`.
///
/// Upstream's embedded retrieval test names its scope with the *singular* `kb_id`; the
/// shared retrieval handler reads `kb_ids`/`dataset_ids`, so the wrapper translates the
/// body and then defers to it. One retrieval implementation, three entry points.
async fn searchbot_retrieval_test(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(mut body): Json<serde_json::Value>,
) -> Response {
    if let Some(object) = body.as_object_mut() {
        if !object.contains_key("kb_ids")
            && let Some(kb_id) = object.get("kb_id").cloned()
        {
            match kb_id {
                serde_json::Value::String(id) => {
                    object.insert("kb_ids".into(), serde_json::json!([id]));
                }
                serde_json::Value::Array(ids) => {
                    object.insert("kb_ids".into(), serde_json::Value::Array(ids));
                }
                _ => {}
            }
        }
        // Upstream maps `size` onto `page_size` for the embedded client.
        if !object.contains_key("page_size")
            && let Some(size) = object.get("size").cloned()
        {
            object.insert("page_size".into(), size);
        }
    }
    if body
        .get("kb_ids")
        .and_then(|value| value.as_array())
        .is_none_or(|ids| ids.is_empty())
    {
        return api_error(StatusCode::BAD_REQUEST, "Please specify dataset firstly.");
    }
    retrieval(State(state), Extension(auth), Json(body)).await
}

/// `POST /api/v1/searchbots/mindmap` — upstream `bot_api.mindmap`.
///
/// The body is the same `{question, kb_ids}` shape the chat mind-map endpoint takes, so
/// the embedded widget and the chat page share one implementation.
async fn searchbot_mindmap(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    // The chat endpoint's request type carries the question only; `kb_ids` is accepted and
    // ignored here rather than rejected, so the embedded client's body passes unchanged.
    let Some(question) = body.get("question").and_then(|value| value.as_str()) else {
        return api_error(StatusCode::BAD_REQUEST, "question is required");
    };
    crate::api::features::chat_mindmap(
        State(state),
        Extension(auth),
        Json(crate::api::features::MindMapRequest {
            question: question.to_string(),
        }),
    )
    .await
}

/// POST /api/v1/datasets/{dataset_id}/search — upstream `chunk_api.retrieval_test`.
///
/// Upstream registers the *same* handler on `/retrieval`, `/datasets/search` and
/// `/datasets/<dataset_id>/search`; the path-scoped variant only differs in that the id
/// from the path becomes `dataset_ids`. Keeping the shared handler means the scoped call
/// cannot drift from the plain one.
async fn dataset_scoped_retrieval(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(dataset_id): axum::extract::Path<String>,
    Json(mut body): Json<serde_json::Value>,
) -> Response {
    if let Some(object) = body.as_object_mut() {
        object.insert("dataset_ids".into(), serde_json::json!([dataset_id]));
        object.remove("kb_id");
    }
    retrieval(State(state), Extension(auth), Json(body)).await
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
    // Upstream `chunk_api.retrieval_test` requires `dataset_ids`; RayRAG's own clients
    // use `kb_ids`, and the share/embed links send the singular `kb_id`. All three are
    // read here, because a client written against RAGFlow would otherwise be told that
    // no knowledge base is accessible.
    let kb_ids: Vec<String> = b
        .get("kb_ids")
        .or_else(|| b.get("dataset_ids"))
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
    // Upstream's name is `document_ids` (it copies `doc_ids` onto it on the way in);
    // both are read so either client selects the same documents.
    let doc_ids: Option<Vec<String>> = b
        .get("doc_ids")
        .or_else(|| b.get("document_ids"))
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
    // Upstream `retrieval_service.py` keeps only documents whose `status` is "1": a
    // document the operator disabled must stop appearing in answers, for every client,
    // not just in the browser that flipped the switch. Only restrict the query when a
    // disabled document is actually involved, so the common path is unchanged.
    let filtered_doc_ids = {
        let disabled: Vec<String> = state
            .docs
            .list_all()
            .into_iter()
            .filter(|doc| doc.status != "1" && kb_ids.contains(&doc.kb_id))
            .map(|doc| doc.id)
            .collect();
        if disabled.is_empty() {
            filtered_doc_ids
        } else {
            match filtered_doc_ids {
                Some(ids) => Some(
                    ids.into_iter()
                        .filter(|id| !disabled.contains(id))
                        .collect::<Vec<_>>(),
                ),
                None => {
                    // The engine takes an allow-list, so it is built from the
                    // documents that are still enabled in the queried datasets.
                    let enabled: Vec<String> = state
                        .docs
                        .list_all()
                        .into_iter()
                        .filter(|doc| kb_ids.contains(&doc.kb_id) && doc.status == "1")
                        .map(|doc| doc.id)
                        .collect();
                    Some(enabled)
                }
            }
        }
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
            {
                let mut chunk = serde_json::json!({
                    "score": result.score,
                    "vector_similarity": result.vector_score,
                    "term_similarity": result.term_score,
                    "content": result.chunk.content,
                    "doc_name": result.chunk.doc_name,
                    "chunk_id": result.chunk.id,
                    "doc_id": doc_id,
                    "kb_id": kb_id,
                    "similarity": result.score,
                    "highlight": highlight.then(|| highlight_content(&result.chunk.content, q)),
                    "document_metadata": document_metadata.flatten(),
                });
                // Upstream's citation block: `id`, `document_id`, `dataset_id`, `positions`,
                // `image_id`, `content_ltks`, `url`, `doc_type` and the keyword lists. RayRAG's own
                // names stay beside them so nothing that already read this response breaks.
                if let Some(object) = chunk.as_object_mut() {
                    for (key, value) in crate::api::chunk_citation::citation_fields(
                        &result.chunk.id,
                        &doc_id,
                        &result.chunk.doc_name,
                        &kb_id,
                        &result.chunk.content,
                        &result.chunk.metadata,
                    ) {
                        object.entry(key).or_insert(value);
                    }
                }
                chunk
            }
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
    const PATTERNS: [&str; 22] = [
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
        // Upstream mounts both on the unauthenticated group (`system_api.py` has no
        // `@login_required`): the login page and the Dify probe call them before a token.
        "/api/v1/system/ping",
        "/api/v1/language",
        "/api/v1/dify/retrieval/health",
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
        // The password-reset flow runs before anyone has a token — that is the whole point of it.
        // Upstream mounts `user_api.forget_get_captcha`, `forget_send_otp` and `reset_password` on the
        // unauthenticated group; RayRAG had them behind the bearer middleware, so a user who had
        // forgotten their password could not reach the endpoints meant for exactly that situation.
        "/api/v1/auth/password/forgot/captcha",
        "/api/v1/auth/password/forgot/otp",
        "/api/v1/auth/password/forgot/otp/verify",
        "/api/v1/auth/password/reset",
        // Upstream v0.27.2 inbound agent webhook: its security model is the
        // Begin component's webhook config, not the RayRAG bearer token.
        "/api/v1/agents/{agent_id}/webhook",
        // v0.27.2 Go backend mounts the built-in pipeline catalog on the
        // unauthenticated group (internal/router/router.go apiNoAuth): it is
        // public static data the dataset form loads before any user action.
        "/api/v1/pipelines",
        "/api/v1/pipelines/{id}",
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
        .route("/search/{search_id}", get(crate::web::search_page_with_id))
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
        .route(
            "/compilation-templates/edit-next",
            get(crate::web::compilation_template_edit_new_page),
        )
        .route(
            "/compilation-templates/edit-next/{id}",
            get(crate::web::compilation_template_edit_page),
        )
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
        // Upstream `routes.tsx` renders the search experience at `/search/:id`;
        // the settings form lives at RayRAG's own `/searches/:id` route.
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
        // Upstream `routes.tsx` composes these three paths with a literal double
        // slash (string concatenation), so the routes themselves carry `//`.
        // Registering the literal spellings keeps deep links byte-compatible with
        // RAGFlow instead of answering 404.
        .route(
            "/dataset//files/{id}",
            get(crate::web::dataset_double_slash_files),
        )
        .route(
            "/memory//memory-message/{memory_id}",
            get(crate::web::memory_message_page),
        )
        .route(
            "/memory//memory-setting/{memory_id}",
            get(crate::web::memory_setting_page),
        )
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
        .route("/memory", get(crate::web::memory_shell_page))
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
        .route("/admin/whitelist", get(crate::web::admin_whitelist_page))
        .route("/admin/roles", get(crate::web::admin_roles_page))
        .route("/admin/monitoring", get(crate::web::admin_monitoring_page))
        .route(
            "/admin/sandbox-settings",
            get(crate::web::admin_sandbox_settings_page),
        )
        // RAGFlow-style aliases that had no route and answered 404. `/datasets/new` is the
        // create dialog on the datasets page, and `/knowledge` is the older name for that list.
        .route(
            "/datasets/new",
            get(|| async { axum::response::Redirect::permanent("/datasets") }),
        )
        .route(
            "/knowledge",
            get(|| async { axum::response::Redirect::permanent("/datasets") }),
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
        // v0.27.2 Go backend built-in pipeline catalog (public static data,
        // apiNoAuth in the Go router): list + single-template DSL.
        .route(
            "/api/v1/pipelines",
            get(crate::api::pipelines::list_pipelines),
        )
        .route(
            "/api/v1/pipelines/{id}",
            get(crate::api::pipelines::get_pipeline),
        )
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
            post(upload_document_route)
                .get(crate::api::document::list_docs)
                // Upstream `document_api.py::delete_documents`: the bulk bar deletes a
                // selection in one request instead of one request per file.
                .delete(crate::api::document::delete_docs_bulk),
        )
        .route(
            // Upstream `document_api.py::batch_update_document_status`.
            "/api/v1/datasets/{id}/documents/batch-update-status",
            post(crate::api::document::batch_update_document_status),
        )
        .route(
            // Upstream `document_api.py::ingest`: parse or cancel a whole selection.
            "/api/v1/documents/ingest",
            post(crate::api::document::ingest_documents),
        )
        // Upstream `document_api.py::metadata_batch_update`: the same body as the PATCH
        // `/documents/metadatas` route, mounted on the path its client calls.
        .route(
            "/api/v1/datasets/{dataset_id}/metadata/update",
            post(crate::api::document_metadata::update_document_metadatas),
        )
        // Upstream `dataset_api.py` artifact surface. `HEAD` is the sidebar probe
        // (`has_any_wiki`), `GET` lists pages, `DELETE` clears them.
        .route(
            "/api/v1/datasets/{dataset_id}/artifacts",
            get(crate::api::dataset_artifacts::list_artifacts)
                .head(crate::api::dataset_artifacts::has_any_wiki)
                .delete(crate::api::dataset_artifacts::clear_artifacts),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/artifacts/compile",
            post(crate::api::dataset_artifacts::compile_artifacts),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/artifacts/topics",
            get(crate::api::dataset_artifacts::list_artifact_topics),
        )
        // Upstream declares these three as literal routes rather than one parameterised path.
        .route(
            "/api/v1/datasets/{dataset_id}/artifacts/structure",
            get(crate::api::dataset_artifacts::list_structure_artifacts)
                .delete(crate::api::dataset_artifacts::delete_structure_artifacts),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/artifacts/graph",
            get(crate::api::dataset_artifacts::list_graph_artifacts),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/artifacts/alteration",
            get(crate::api::dataset_artifacts::list_alteration_artifacts),
        )
        // Upstream also exposes the listing by path: `/artifacts/{page_type}`.
        .route(
            "/api/v1/datasets/{dataset_id}/artifacts/{page_type}",
            get(crate::api::dataset_artifacts::list_artifacts_by_type),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/artifacts/{page_type}/{slug}",
            get(crate::api::dataset_artifacts::get_artifact_page)
                // Upstream `dataset_api.update_wiki_page`: save an edited page.
                .put(crate::api::dataset_artifacts::update_artifact_page),
        )
        // Upstream `dataset_api.py` navigation surface over the ported nav engine.
        .route(
            "/api/v1/datasets/{dataset_id}/navigation",
            get(crate::api::dataset_navigation::list_nav)
                // Upstream `dataset_api.generate_dataset_nav`.
                .post(crate::api::dataset_navigation::generate_navigation)
                .delete(crate::api::dataset_navigation::delete_nav),
        )
        // Added by RayRAG: probe every configured model endpoint, because "the dataset has no
        // entities" and "the chat endpoint is down" look identical from a dataset page.
        .route(
            "/api/v1/models/endpoints/check",
            get(crate::api::tenant_models::check_model_endpoints),
        )
        // Upstream `dataset_api.py` index maintenance: `POST /datasets/{id}/index` rebuilds the
        // dataset's vectors and `DELETE /datasets/{id}/{index_type}` drops them. RayRAG already
        // owned both operations internally (`vector_mirror`); this exposes them where upstream
        // has them instead of leaving them reachable only from the internal repair sweep.
        .route(
            "/api/v1/datasets/{dataset_id}/index",
            post(rebuild_dataset_index)
                .get(dataset_index_status)
                .delete(delete_named_dataset_index),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/{index_type}",
            axum::routing::delete(delete_dataset_index),
        )
        // Upstream `user_api.py` password reset flow (`forgot/otp`, `otp/verify`, `reset`).
        .route(
            // Upstream registers this as POST; its own docstring says GET, and a browser fetching an
            // image sends GET, so both verbs answer the same handler.
            "/api/v1/auth/password/forgot/captcha",
            get(crate::api::password_flow::forgot_captcha)
                .post(crate::api::password_flow::forgot_captcha),
        )
        .route(
            "/api/v1/auth/password/forgot/otp",
            post(crate::api::password_flow::request_otp),
        )
        .route(
            "/api/v1/auth/password/forgot/otp/verify",
            post(crate::api::password_flow::verify_otp),
        )
        .route(
            "/api/v1/auth/password/reset",
            post(crate::api::password_flow::reset_password),
        )
        // ── Deprecated v0.24.0 aliases (upstream's backward-compatibility layer) ─────────────
        // The API guide's "Deprecated API Aliases" table lists these; they stay available so an
        // older integration keeps working, and each one is the *same handler* as its replacement
        // rather than a second implementation that could drift.
        .route(
            "/api/v1/file/upload",
            axum::routing::post(crate::api::file_mgr::create_or_upload),
        )
        .route(
            "/api/v1/file/create",
            axum::routing::post(crate::api::file_mgr::create_or_upload),
        )
        .route("/api/v1/file/list", get(crate::api::file_mgr::list_files))
        .route(
            "/api/v1/file/root_folder",
            get(crate::api::file_mgr::list_files),
        )
        .route(
            "/api/v1/file/parent_folder",
            get(crate::api::file_mgr::parent_folder_by_query),
        )
        .route(
            "/api/v1/file/all_parent_folder",
            get(crate::api::file_mgr::file_ancestors_by_query),
        )
        .route(
            "/api/v1/file/rm",
            axum::routing::post(crate::api::file_mgr::delete_files),
        )
        .route(
            "/api/v1/file/rename",
            axum::routing::post(crate::api::file_mgr::legacy_file_rename),
        )
        .route(
            "/api/v1/file/mv",
            axum::routing::post(crate::api::file_mgr::move_files),
        )
        .route(
            "/api/v1/file/get/{file_id}",
            get(crate::api::file_mgr::get_file),
        )
        // The deprecated OpenAI-shaped chat routes: same handlers as the replacements.
        .route(
            "/api/v1/chats_openai/{chat_id}/chat/completions",
            post(crate::api::openai_proxy::openai_rag_chat_completions),
        )
        // Upstream serves this one outside `/api` as well.
        .route("/v1/system/healthz", get(crate::api::system::healthz))
        // Upstream v0.27.2 `agent_api.py` webhook surface: an inbound agent
        // trigger whose contract lives in the Begin component's params
        // (mode Webhook). Every HTTP method is routed to the same handler.
        .route(
            "/api/v1/agents/{agent_id}/webhook",
            axum::routing::any(crate::api::agent_webhook::webhook_trigger),
        )
        .route(
            "/api/v1/agents/{agent_id}/webhook/logs",
            get(crate::api::agent_webhook::webhook_trace),
        )
        .route(
            "/api/v1/agents/{agent_id}/webhook/test",
            axum::routing::any(crate::api::agent_webhook::webhook_test),
        )
        // Upstream `user_api.py::tenant_info`: the tenant's per-capability default models
        // under RAGFlow's own column names.
        .route(
            "/api/v1/users/me/models",
            get(crate::api::tenant_models::me_models)
                .patch(crate::api::tenant_models::patch_me_models),
        )
        // Upstream `agent_api.py` canvas utilities: the prompt set the UI displays and the
        // id-addressed file download the canvas uses.
        .route("/api/v1/agents/prompts", get(agent_prompts))
        .route("/api/v1/agents/download", get(agent_download))
        .route(
            "/api/v1/agents/test_db_connection",
            post(test_db_connection),
        )
        // Upstream `bot_api.py` embedded search-bot surface (beta-token clients).
        .route("/api/v1/searchbots/detail", get(searchbot_detail))
        .route(
            "/api/v1/searchbots/retrieval_test",
            post(searchbot_retrieval_test),
        )
        .route("/api/v1/searchbots/mindmap", post(searchbot_mindmap))
        .route(
            // The literal `search` segment must stay ahead of `{name}`.
            "/api/v1/datasets/{dataset_id}/navigation/search",
            get(crate::api::dataset_navigation::search_nav),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/navigation/{name}/children",
            get(crate::api::dataset_navigation::list_nav_children),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/navigation/{name}",
            axum::routing::delete(crate::api::dataset_navigation::delete_nav_node),
        )
        // Upstream `dataset_api.py::get_flattened_metadata` / `aggregate_tags`.
        .route(
            "/api/v1/datasets/metadata/flattened",
            get(crate::api::document_metadata::flattened_metadata),
        )
        .route(
            "/api/v1/datasets/tags/aggregation",
            get(crate::api::dataset_tags::aggregate_tags),
        )
        // Upstream `document_api.py::parse_documents` / `stop_parse_documents`: the
        // dataset-scoped selection endpoints behind the file list's toolbar.
        .route(
            "/api/v1/datasets/{dataset_id}/documents/parse",
            post(crate::api::document::parse_documents),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/documents/stop",
            post(crate::api::document::stop_parse_documents),
        )
        // `chunk_api.py` mounts its own `parse` / `stop_parsing` handlers on the
        // chunks path — different envelopes from document_api's `/documents/parse`
        // / `/documents/stop` (bare `{"code":0}`, pipeline rejection, invalid-state
        // `DOC_STOP_PARSING_INVALID_STATE` data payload).
        .route(
            "/api/v1/datasets/{dataset_id}/chunks",
            post(crate::api::document::chunk_parse_documents)
                .delete(crate::api::document::chunk_stop_parsing),
        )
        // Upstream v0.27.2 `document_api.download`: `GET /datasets/{id}/documents/{did}`
        // streams the file bytes (`send_file(as_attachment=True)`), it is not a JSON
        // metadata route. Document settings come from the list endpoint with `?id=`.
        .route(
            "/api/v1/datasets/{id}/documents/{did}",
            get(crate::api::document::download_doc)
                .delete(crate::api::document::delete_doc)
                .patch(crate::api::document::update_document),
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
            "/api/v1/documents/{doc_id}/preview/docx",
            get(crate::api::document::preview_docx),
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
            // Upstream `document_api.py::metadata_summary` is a GET that reads
            // `doc_ids` from the query string; the POST form is RayRAG's own. Both live
            // on one route because axum requires a single parameter name per position.
            "/api/v1/datasets/{id}/metadata/summary",
            get(crate::api::document_metadata::metadata_summary_get)
                .post(crate::api::document_metadata::metadata_summary),
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
        // Upstream mounts `retrieval_test` on all three paths (`chunk_api.py`).
        .route("/api/v1/datasets/search", post(retrieval))
        .route(
            "/api/v1/datasets/{dataset_id}/search",
            post(dataset_scoped_retrieval),
        )
        .route(
            "/api/v1/conversations",
            axum::routing::post(crate::api::features::create_conversation)
                // The list's "delete selected", "delete all" (ids/delete_all/chat_id bodies) and
                // legacy single delete, all of sessions.
                .delete(crate::api::features::bulk_delete_conversations),
        )
        .route(
            "/api/v1/conversations/{conversation_id}/completions",
            axum::routing::post(crate::api::features::complete_conversation),
        )
        .route(
            "/api/v1/conversations/{conversation_id}",
            axum::routing::get(crate::api::features::get_conversation)
                .patch(crate::api::features::update_conversation)
                .delete(crate::api::features::delete_conversation),
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
            "/api/v1/datasets/{dataset_id}/graph",
            axum::routing::get(dataset_graph),
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
            "/api/v1/admin/whitelist",
            get(crate::api::admin_whitelist::get_whitelist)
                .post(crate::api::admin_whitelist::add_entry)
                .put(crate::api::admin_whitelist::set_enabled)
                .delete(crate::api::admin_whitelist::remove_entries),
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
        // The embedded search bot: upstream answers this one with a Server-Sent Events stream in its own
        // envelope, which is why it is a separate handler from `/search`.
        .route("/api/v1/searchbots/ask", post(crate::api::searchbot::ask))
        .route(
            "/api/v1/dify/retrieval",
            // Upstream registers GET (probe) and POST (retrieval) on this path.
            get(crate::api::search::dify_retrieval_probe).post(crate::api::search::dify_retrieval),
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
            get(crate::api::features::list_chats)
                .post(crate::api::features::create_chat_assistant)
                // The guide's `DELETE /api/v1/chats` deletes chat assistants.
                .delete(crate::api::features::bulk_delete_chat_assistants),
        )
        .route(
            "/api/v1/chats/{id}",
            // The guide's single-item chat verbs are chat *assistants*; sessions have their own
            // endpoint below, which is where these handlers used to live.
            get(crate::api::features::get_chat)
                .put(crate::api::features::replace_chat_assistant)
                .patch(crate::api::features::patch_chat_assistant)
                .delete(crate::api::features::delete_chat),
        )
        .route(
            // The guide's path parameter here is the chat assistant; the session-scoped
            // completion this used to be lives at the conversations endpoint below.
            "/api/v1/chats/{id}/completions",
            post(crate::api::features::complete_chat_assistant),
        )
        .route(
            "/api/v1/chats/{chat_id}/sessions/{session_id}/messages/{message_id}/feedback",
            put(crate::chunk_feedback::update_message_feedback),
        )
        .route(
            "/api/v1/conversations/{session_id}/messages/{message_id}/feedback",
            put(crate::chunk_feedback::update_session_message_feedback),
        )
        .route(
            "/api/v1/conversations/{session_id}/messages/{message_id}",
            post(crate::api::features::regenerate_chat_message)
                .delete(crate::api::features::delete_chat_message),
        )
        .route(
            "/api/v1/conversations/{session_id}/messages/{message_id}/regenerate",
            post(crate::api::features::regenerate_chat_message),
        )
        // Upstream `chat_api` mounts the session collection and one session per chat app.
        .route(
            "/api/v1/chats/{chat_id}/sessions",
            get(crate::api::features::list_chat_sessions)
                .post(crate::api::features::create_chat_session)
                .delete(crate::api::features::delete_chat_sessions),
        )
        .route(
            "/api/v1/chats/{chat_id}/sessions/{session_id}",
            get(crate::api::features::get_chat_session)
                .patch(crate::api::features::update_chat_session),
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
        .route(
            "/api/v1/files",
            get(crate::api::file_mgr::list_files)
                // Upstream `file_api.create_or_upload`: multipart uploads, a JSON body
                // creates a folder — one path for both faces.
                .post(crate::api::file_mgr::create_or_upload)
                // Upstream `file_api.delete`: bulk delete by `ids`.
                .delete(crate::api::file_mgr::delete_files),
        )
        .route(
            // Upstream `file_api.move`: re-parent and optionally rename.
            "/api/v1/files/move",
            axum::routing::post(crate::api::file_mgr::move_files),
        )
        .route(
            "/api/v1/files/folder",
            axum::routing::post(crate::api::file_mgr::create_folder),
        )
        // Upstream `file_api.parent_folder` / `ancestors`: the file manager's breadcrumb.
        .route(
            "/api/v1/files/{file_id}/parent",
            get(crate::api::file_mgr::parent_folder),
        )
        .route(
            "/api/v1/files/{file_id}/ancestors",
            get(crate::api::file_mgr::file_ancestors),
        )
        // The workspace commit family (upstream `file_commit_api.py`): snapshots of a file
        // workspace, their files, the diff between two, the uncommitted changes, the tree at a
        // commit, one file's content at a commit, and a file's version history.
        .route(
            "/api/v1/workspaces/{workspace_id}/commits",
            get(crate::api::workspaces::list_commits).post(crate::api::workspaces::create_commit),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/commits/diff",
            get(crate::api::workspaces::diff_commits),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/changes",
            get(crate::api::workspaces::uncommitted_changes),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/commits/{commit_id}",
            get(crate::api::workspaces::get_commit),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/commits/{commit_id}/files",
            get(crate::api::workspaces::list_commit_files),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/commits/{commit_id}/tree",
            get(crate::api::workspaces::commit_tree),
        )
        .route(
            "/api/v1/workspaces/{workspace_id}/commits/{commit_id}/files/{file_id}/content",
            get(crate::api::workspaces::commit_file_content),
        )
        // The same family under the dataset prefix: RayRAG's dataset scope is the knowledge base
        // itself, which is how upstream writes artifact commits (`folder_id = kb_id`).
        .route(
            "/api/v1/datasets/{dataset_id}/commits",
            get(crate::api::workspaces::list_dataset_commits)
                .post(crate::api::workspaces::create_dataset_commit),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/commits/diff",
            get(crate::api::workspaces::diff_dataset_commits),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/changes",
            get(crate::api::workspaces::dataset_uncommitted_changes),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/commits/{commit_id}",
            get(crate::api::workspaces::get_dataset_commit),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/commits/{commit_id}/files",
            get(crate::api::workspaces::list_dataset_commit_files),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/commits/{commit_id}/tree",
            get(crate::api::workspaces::dataset_commit_tree),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/commits/{commit_id}/files/{file_id}/content",
            get(crate::api::workspaces::dataset_commit_file_content),
        )
        .route(
            "/api/v1/workspace-files/{file_id}/versions",
            get(crate::api::workspaces::file_versions),
        )
        // Document assets (upstream `document_api.py`): the thumbnail map a document list points at,
        // and the image endpoint that map's urls resolve to.
        .route(
            "/api/v1/thumbnails",
            get(crate::api::doc_assets::list_thumbnails),
        )
        // Auto-metadata configuration (upstream `dataset_api.py` + `document_api.py`): the field
        // schema a dataset defines, and the per-document override.
        // Audio (`chat_api.py`): text to speech and speech to text through the OpenAI-compatible
        // clients in `audio.rs`.
        // `POST /datasets/{id}/embedding/check`: do the stored vectors still agree with the model?
        .route(
            "/api/v1/datasets/{dataset_id}/embedding/check",
            axum::routing::post(crate::api::embedding_check::check_dataset_embedding),
        )
        .route(
            "/api/v1/chat/audio/speech",
            axum::routing::post(crate::api::audio_api::speech),
        )
        .route(
            "/api/v1/chat/audio/transcription",
            axum::routing::post(crate::api::audio_api::transcription),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/metadata/config",
            get(crate::api::metadata_config::get_dataset_metadata_config)
                .put(crate::api::metadata_config::update_dataset_metadata_config),
        )
        .route(
            "/api/v1/datasets/{dataset_id}/documents/{document_id}/metadata/config",
            axum::routing::put(crate::api::metadata_config::update_document_metadata_config),
        )
        // The document-level structure graph. The assembly helpers existed for a while with no caller;
        // this is the route that uses them, so a compiled graph can actually be read back.
        .route(
            "/api/v1/datasets/{dataset_id}/documents/{document_id}/structure/graph",
            get(crate::api::structure_graph::document_structure_graph)
                .delete(crate::api::structure_graph::delete_document_structure_graph),
        )
        // The sandbox artifacts a code execution produced. Upstream serves one by filename from its
        // artifact bucket; RayRAG stores what its executor collected and serves it the same way.
        .route(
            "/api/v1/documents/artifact/{filename}",
            get(crate::api::artifacts::get_artifact).delete(crate::api::artifacts::delete_artifact),
        )
        .route(
            "/api/v1/documents/artifacts",
            get(crate::api::artifacts::list_artifacts),
        )
        // The compiled skill tree. Upstream reads and deletes it through these five verbs; the compile
        // route is RayRAG's producer, because a button needs an endpoint (see the registry).
        .route(
            "/api/v1/datasets/{kb_id}/skills",
            get(crate::api::skill_tree::get_skills)
                .head(crate::api::skill_tree::head_skills)
                .delete(crate::api::skill_tree::delete_skills),
        )
        .route(
            "/api/v1/datasets/{kb_id}/skills/compile",
            post(crate::api::skill_tree::compile_skills),
        )
        .route(
            "/api/v1/datasets/{kb_id}/skills/{*skill_kwd}",
            get(crate::api::skill_tree::get_skill_node)
                .delete(crate::api::skill_tree::delete_skill_node),
        )
        .route(
            "/api/v1/documents/images/{image_id}",
            get(crate::api::doc_assets::document_image),
        )
        // Runtime attachments (upstream `document_api.py::upload_info` and
        // `agent_api.py::_stream_agent_attachment`): create one from a file or a url, stream it back
        // for download or preview, and the canvas-scoped upload.
        .route(
            "/api/v1/documents/upload",
            axum::routing::post(crate::api::attachments::upload_document),
        )
        .route(
            "/api/v1/agents/attachments/{attachment_id}/download",
            get(crate::api::attachments::download_attachment),
        )
        .route(
            "/api/v1/agents/attachments/{attachment_id}/preview",
            get(crate::api::attachments::preview_attachment),
        )
        .route(
            "/api/v1/agents/{agent_id}/upload",
            axum::routing::post(crate::api::attachments::upload_agent_file),
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
            // The guide's conversation endpoint: three modes and `{code,message,data:{answer,…}}`
            // frames. OpenAI clients keep their own shapes at `/v1/chat/completions`.
            "/api/v1/chat/completions",
            post(crate::api::rag_chat::chat_completions),
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
        // The public model catalogue that fills in capabilities and pricing for a
        // custom provider/model (`crate::model_catalog`). Authenticated like the
        // provider pickers it serves; only the refresh is admin-only.
        .route(
            "/api/v1/model-catalog/status",
            get(crate::api::model_catalog::catalog_status),
        )
        .route(
            "/api/v1/model-catalog/providers",
            get(crate::api::model_catalog::catalog_providers),
        )
        .route(
            "/api/v1/model-catalog/lookup",
            get(crate::api::model_catalog::catalog_lookup),
        )
        .route(
            "/api/v1/model-catalog/search",
            get(crate::api::model_catalog::catalog_search),
        )
        .route(
            "/api/v1/model-catalog/refresh",
            post(crate::api::model_catalog::catalog_refresh),
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
            // Upstream `provider_api.update_provider_instance` updates the instance
            // addressed by name in the path.
            get(crate::api::tenant_models::show_ragflow_provider_instance)
                .put(crate::api::tenant_models::update_ragflow_provider_instance),
        )
        .route(
            "/api/v1/providers/{provider_name}/instances/{instance_name}/models",
            get(crate::api::tenant_models::list_ragflow_instance_models)
                .post(crate::api::tenant_models::add_ragflow_instance_model)
                .put(crate::api::tenant_models::edit_ragflow_instance_models)
                // Upstream `provider_api.delete_models_from_instance`.
                .delete(crate::api::tenant_models::delete_ragflow_instance_model),
        )
        .route(
            "/api/v1/providers/{provider_name}/instances/{instance_name}/models/{model_name}",
            // Upstream registers PATCH (model row status) and POST (`chat_to_model`, a
            // direct chat through that model) on this path.
            axum::routing::patch(crate::api::tenant_models::update_ragflow_instance_model_status)
                .post(crate::api::tenant_models::chat_to_provider_model),
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
                .post(crate::api::agent_log::create_agent_session)
                // Upstream `agent_api.delete_agent_session`: `ids` selection or
                // `delete_all` for the agent's whole history.
                .delete(crate::api::agent_log::delete_agent_sessions),
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
        // ── Upstream spellings (RAGFlow `api/apps/restful_apis/compilation_*_api.py`).
        // RayRAG's own routes above use underscores; a client written against RAGFlow
        // calls these hyphenated paths, so both spellings must answer.
        .route(
            "/api/v1/compilation-template-groups",
            get(crate::api::compilation_templates::list_groups)
                .post(crate::api::compilation_templates::create_group),
        )
        .route(
            "/api/v1/compilation-template-groups/{id}",
            get(crate::api::compilation_templates::get_group)
                .put(crate::api::compilation_templates::update_group)
                .delete(crate::api::compilation_templates::delete_group),
        )
        .route(
            "/api/v1/compilation-template-groups/{id}/execute",
            axum::routing::post(crate::api::compilation_templates::execute_group_templates),
        )
        .route(
            "/api/v1/compilation-templates/builtins",
            get(crate::api::compilation_templates::list_builtin_templates),
        )
        .route(
            "/api/v1/compilation-templates/wiki-presets",
            get(crate::api::compilation_templates::list_wiki_presets),
        )
        .route("/api/v1/stats", get(crate::api::features::get_stats))
        // Upstream `stats_api.stats` is mounted at `/system/stats`; RayRAG's own path
        // above answers the same payload (pv/uv/speed/tokens/round/thumb_up).
        .route("/api/v1/system/stats", get(crate::api::features::get_stats))
        // Upstream `system_api.ping` (unauthenticated, literal `pong`).
        .route("/api/v1/system/ping", get(crate::api::system::ping))
        // Upstream `system_api.gaussdb_status` / `oceanbase_status`: RayRAG's document
        // engine is PostgreSQL, so both answer the upstream "not in use" envelopes
        // instead of 404 (a monitoring client reads the status field, not the code).
        .route(
            "/api/v1/system/gaussdb/status",
            get(crate::api::system::gaussdb_status),
        )
        .route(
            "/api/v1/system/oceanbase/status",
            get(crate::api::system::oceanbase_status),
        )
        // Upstream `system_api.language` (unauthenticated front-end compatibility shim).
        .route("/api/v1/language", get(crate::api::system::language))
        // Upstream `user_api.log_out`.
        .route("/api/v1/auth/logout", axum::routing::post(logout))
        // Upstream `dify_retrieval_api.retrieval_health_check` (unauthenticated).
        .route(
            "/api/v1/dify/retrieval/health",
            get(crate::api::search::dify_retrieval_health),
        )
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
            // Upstream `user_api.py` registers GET (profile) and PATCH (settings) on the
            // same path; a client that reads the profile with GET must not get a 405.
            axum::routing::get(user_info).patch(crate::api::system::update_me),
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
        // One connector instance: its detail, its history, a real probe, and a rebuild that reports
        // what the source offers instead of claiming documents were written.
        .route(
            "/api/v1/connectors/{connector_id}",
            get(crate::api::connector::get_connector)
                .patch(crate::api::connector::patch_connector)
                .delete(crate::api::connector::delete_connector),
        )
        .route(
            "/api/v1/connectors/{connector_id}/logs",
            get(crate::api::connector::connector_logs),
        )
        .route(
            "/api/v1/connectors/{connector_id}/test",
            axum::routing::post(crate::api::connector::test_connector),
        )
        .route(
            "/api/v1/connectors/{connector_id}/rebuild",
            axum::routing::post(crate::api::connector::rebuild_connector),
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
        .route(
            "/api/v1/plugins",
            get(crate::api::plugin_tools::list_plugins),
        )
        .route(
            "/api/v1/plugin/tools",
            get(crate::api::plugin_tools::plugin_tools),
        )
        .route(
            "/api/v1/plugins/{id}/toggle",
            post(crate::api::plugin_tools::toggle_plugin),
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
            // The page polls this while the operator reads the form: an edit to `.env`
            // made outside the wizard shows up instead of leaving stale values on screen.
            "/api/v1/setup/watch",
            get(crate::api::setup::setup_watch),
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
            "/api/v1/llm/aimlapi/authorize/start",
            post(crate::api::aimlapi::aimlapi_authorize_start),
        )
        .route(
            "/api/v1/llm/aimlapi/authorize/poll",
            post(crate::api::aimlapi::aimlapi_authorize_poll),
        )
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
        // Upstream registers both spellings on the same handler
        // (`search_api.py`: `/searches/<search_id>/completion` and `.../completions`).
        .route(
            "/api/v1/searches/{search_id}/completion",
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
            // The guide's replacement endpoint, and the deprecated alias that points at it. Both
            // resolve to the same handler, so they cannot drift apart.
            post(crate::api::file_link::link_files_to_datasets),
        )
        .route(
            "/api/v1/files/link-to-datasets",
            post(crate::api::file_link::link_files_to_datasets),
        )
        .route(
            "/api/v1/file/convert",
            post(crate::api::file_link::link_files_to_datasets),
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
                .not_found_service(axum::routing::get(unrouted_path)),
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
    // A password rotation writes a state-volume credential file; it wins over an
    // unchanged compose environment value on the next start.
    crate::persistence::apply_persisted_postgres_url(static_dir)?;
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
    let whitelist_path = std::env::var("RAYRAG_WHITELIST_FILE")
        .unwrap_or_else(|_| format!("{static_dir}/../registration-whitelist.json"));
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
    let workspace_commits = Arc::new(crate::api::workspaces::WorkspaceCommitStore::new(
        &format!("{}/../uploads/", static_dir),
    )?);
    let attachments = Arc::new(crate::api::attachments::AttachmentStore::new(&format!(
        "{}/../uploads/attachments",
        static_dir
    ))?);
    let metadata_config = Arc::new(crate::api::metadata_config::MetadataConfigStore::new(
        &format!("{}/../uploads/metadata_config.json", static_dir),
    )?);
    let connectors = Arc::new(crate::api::connector::ConnectorStore::new(&format!(
        "{}/../uploads/connectors.json",
        static_dir
    ))?);
    // Sandbox artifacts: the executor writes through `ArtifactStore::shared()`, so the API holds that
    // very instance. Two instances over one file would leave the route reading a stale copy.
    crate::api::artifacts::ArtifactStore::set_default_path(format!(
        "{static_dir}/../uploads/artifacts.json"
    ));
    let artifacts = crate::api::artifacts::ArtifactStore::shared();
    let skill_trees = Arc::new(crate::api::skill_tree::SkillTreeStore::new(&format!(
        "{static_dir}/../uploads/skill_trees.json"
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

    // No baked-in provider and no baked-in model: RayRAG talks to a chat endpoint only when the
    // operator asked for one. Defaulting to a third party (and to a specific model) meant a fresh
    // install could send requests to a service the user never chose, and the startup log would name
    // a model that was not their setting.
    // The endpoint and the model name must come from the operator; the key stays optional because
    // a model server on the local network (llama.cpp, Ollama, vLLM) often needs no key at all.
    let llm = match (
        std::env::var("LLM_API_BASE").ok(),
        std::env::var("LLM_MODEL").ok(),
    ) {
        (Some(api_base), Some(model))
            if !api_base.trim().is_empty() && !model.trim().is_empty() =>
        {
            let api_key = std::env::var("LLM_API_KEY").unwrap_or_default();
            tracing::info!(
                model = %model,
                source = "LLM_MODEL",
                "Chat model configured from your settings"
            );
            Some(Arc::new(LlmClient::new(LlmConfig {
                api_base,
                api_key,
                model,
                ..LlmConfig::default()
            })))
        }
        _ => {
            tracing::info!(
                "Chat model not configured yet: set LLM_API_BASE, LLM_API_KEY and LLM_MODEL in your .env, or finish the first-run guide in the web UI (Models step). Running without a chat model."
            );
            None
        }
    };

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
            tracing::info!(
                model = %model,
                source = "EMBED_MODEL",
                "Embedding model configured from your settings"
            );
            tracing::debug!(api_base = %api_base, "Embedding endpoint");
            Some(Arc::new(crate::embed::openai_compatible_embedder(
                &api_base, &api_key, &model,
            )))
        }
        _ => {
            tracing::info!(
                "Embedding model not configured yet: set EMBED_API_BASE, EMBED_API_KEY and EMBED_MODEL in your .env, or finish the first-run guide in the web UI (Models step). Running without an embedding model."
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
            // RAGFlow-compatible name; a reranker that needs to be told which model
            // to run (TEI with several loaded, vLLM, Xinference) reads it here.
            model: std::env::var("RERANK_MODEL")
                .ok()
                .filter(|model| !model.trim().is_empty()),
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
        doc_store: Arc::new(crate::doc_store::DocStorePool::from_env()),
        graphs: Arc::new(crate::graph_store::GraphStore::new(&g)?),
        engine,
        vector_mirror,
        index_path: i,
        model_path: m,
        max_upload_bytes: max_upload_bytes(),
        docs,
        workspace_commits,
        attachments,
        metadata_config,
        artifacts,
        skill_trees,
        connectors,
        document_metadata,
        whitelist: Arc::new(crate::registration_whitelist::WhitelistStore::new(
            &whitelist_path,
        )?),
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
    let no_users_yet = state.users.list_users().is_empty();
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

    // No account is pre-created: the first registration becomes the administrator and the
    // PostgreSQL administrator role's password follows that account. Say so at startup; when
    // the operator explicitly configured the legacy seed account, warn if it is weak.
    if no_users_yet {
        tracing::info!(
            "no account exists yet; the first registration becomes the RayRAG administrator"
        );
    } else if let Some(value) = std::env::var("RAYRAG_ADMIN_PASSWORD").ok() {
        let value = value.trim().to_ascii_lowercase();
        let weak = value.starts_with("replace-with")
            || value == "changeme"
            || value == "password"
            || value == "admin"
            || value.len() < 12;
        if weak {
            tracing::warn!(
                "RAYRAG_ADMIN_PASSWORD is a weak placeholder; change the password after signing in"
            );
        }
    }

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
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown)
    .await?;
    Ok(())
}


