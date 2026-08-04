//! RayRAG Web Server — serves Rust SSR UI + REST API.
//! Startup: `rayrag serve --port 9380`

use axum::{
    Json, Router,
    extract::{
        DefaultBodyLimit, Extension, Multipart, Path, Query, Request, State, multipart::Field,
    },
    http::{Method, StatusCode},
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
    pub providers: Arc<crate::api::features::ProviderStore>,
    pub tenant_models: Arc<crate::api::tenant_models::TenantModelStore>,
    pub memories: Arc<crate::api::features::MemoryStore>,
    pub tasks: Arc<crate::api::features::TaskQueue>,
    pub agents: Arc<crate::api::features::AgentStore>,
    pub agent_runs: Arc<crate::api::features::AgentRunRegistry>,
    pub agent_checkpoints: Arc<crate::agent_checkpoint::AgentCheckpointStore>,
    pub canvas_versions: Arc<crate::api::features::CanvasVersionStore>,
    pub compilation_templates: Arc<crate::api::compilation_templates::CompilationTemplateStore>,
    pub evaluations: Arc<crate::api::evaluation::EvaluationStore>,
    pub log_levels: Arc<crate::logging::LogLevelManager>,
    pub chunk_feedback_enabled: bool,
    pub chunk_feedback_weighting: crate::chunk_feedback::FeedbackWeighting,
    pub(crate) document_commit_lock: Arc<std::sync::Mutex<()>>,
}

// ── Auth ────────────────────────────────────────────────────────

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

async fn register(
    State(state): State<Arc<AppState>>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    if !std::env::var("RAYRAG_ALLOW_REGISTRATION")
        .map(|value| value.eq_ignore_ascii_case("true") || value == "1")
        .unwrap_or(false)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiResponse {
                code: 403,
                message: "Registration is disabled".into(),
                data: None,
            }),
        )
            .into_response();
    }

    let nick = b.get("nickname").and_then(|v| v.as_str()).unwrap_or("user");
    let email = b.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let pass = b.get("password").and_then(|v| v.as_str()).unwrap_or("");
    match state.users.register(nick, email, pass) {
        Ok(_) => (
            StatusCode::CREATED,
            Json(ApiResponse {
                code: 0,
                message: "ok".into(),
                data: None,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse {
                code: 400,
                message: e.to_string(),
                data: None,
            }),
        )
            .into_response(),
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Clone)]
pub struct AuthContext {
    pub user_id: String,
    pub is_admin: bool,
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
    let permission = body
        .get("permission")
        .and_then(|value| value.as_str())
        .unwrap_or("private");
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
    match state
        .kbs
        .create_for_with_config(&auth.user_id, name, description, permission, &embd_id)
    {
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
    let permission = body.get("permission").and_then(|value| value.as_str());
    let requested_embedding = body
        .get("embd_id")
        .or_else(|| body.get("embedding_model"))
        .and_then(|value| value.as_str());
    // 解析 chunk/解析器配置 → parser_config JSON
    let mut config = serde_json::Map::new();
    for key in [
        "chunk_method",
        "chunk_size",
        "overlap",
        "delimiter",
        "parser_type",
        "raptor_enabled",
        "raptor_depth",
        "graphrag_enabled",
        "entity_types",
        "layout_recognize",
        "ocr_enabled",
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
    if permission.is_none() && requested_embedding.is_none() && parser_config.is_none() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "permission or embedding_model is required",
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
    match state
        .kbs
        .update_config(&dataset_id, permission, embd_id.as_deref(), parser_config.as_deref())
    {
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

async fn delete_dataset(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
) -> Response {
    if !kb_manageable(&state, &dataset_id, &auth) {
        return api_error(StatusCode::NOT_FOUND, "Knowledge base not found");
    }
    for doc in state.docs.list(&dataset_id) {
        if let Err(error) = delete_document_data(&state, &doc) {
            return api_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
        }
    }
    match state.kbs.delete(&dataset_id) {
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

pub(crate) struct PersistedUpload {
    pub(crate) name: String,
    pub(crate) storage_name: String,
    pub(crate) path: PathBuf,
    pub(crate) size: usize,
    pub(crate) content_hash: String,
    cleanup_on_drop: bool,
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

fn register_document_upload(
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
    Ok(task_id)
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
        let pipeline = crate::pipeline::Pipeline::new(config)
            .with_shared_embedder(embedder)
            .with_document_parsers_from_env()?;
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid upload path"))?;
        let chunks = pipeline.process(path_str).await?;
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
        persist_index_change(state, &mut engine, previous.clone())?;
        previous
    };

    if let Err(error) = state.graphs.replace_document(&doc.id, graph_checkpoint) {
        restore_index(state, previous_index, "Graph checkpoint update failed");
        return Err(error.context("Failed to update GraphRAG artifacts after index commit"));
    }

    if let Err(error) = state.kbs.update_counts(&doc.kb_id, count_delta, 0) {
        state.graphs.replace_snapshot(previous_graphs).ok();
        restore_index(state, previous_index, "KB count update failed");
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
                previous_index,
                "Document metadata disappeared during commit",
            );
            anyhow::bail!("Document metadata disappeared during index commit");
        }
        Err(error) => {
            state.graphs.replace_snapshot(previous_graphs).ok();
            if let Err(rollback_error) = state.kbs.update_counts(&doc.kb_id, -count_delta, 0) {
                tracing::error!(%rollback_error, doc_id = %doc.id, "Failed to roll back KB counts");
            }
            restore_index(state, previous_index, "Document metadata update failed");
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

fn restore_index(state: &AppState, previous: Vec<crate::search::IndexedChunk>, reason: &str) {
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
    previous: Vec<crate::search::IndexedChunk>,
) -> anyhow::Result<()> {
    crate::store::persist_online_index(engine, &state.index_path, &state.vector_mirror, previous)
        .map_err(|error| anyhow::anyhow!("Failed to commit online search index: {error}"))
}

fn dispatch_pending_document_tasks(state: Arc<AppState>, task_slots: Arc<tokio::sync::Semaphore>) {
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

fn max_concurrent_document_tasks() -> usize {
    std::env::var("RAYRAG_MAX_CONCURRENT_TASKS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_CONCURRENT_DOCUMENT_TASKS)
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
        persist_index_change(state, &mut engine, previous.clone())?;
        (removed, previous)
    };
    if let Err(error) = state.graphs.replace_document(&doc.id, None) {
        restore_index(state, previous_index, "Graph checkpoint deletion failed");
        return Err(error.context("Failed to delete GraphRAG document checkpoint"));
    }
    if let Err(error) = state
        .kbs
        .update_counts(&doc.kb_id, -(removed_chunks as isize), -1)
    {
        state.graphs.replace_snapshot(previous_graphs).ok();
        restore_index(
            state,
            previous_index,
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
            previous_index,
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
                previous_index,
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
            restore_index(state, previous_index, "Document metadata deletion failed");
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

// ── Search ─────────────────────────────────────────────────────




async fn admin_delete_user(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(user_id): axum::extract::Path<String>,
) -> Response {
    if !auth.is_admin {
        return Json(serde_json::json!({ "code": 403, "message": "Admin required" })).into_response();
    }
    match state.users.delete_user(&user_id, &auth.user_id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => Json(serde_json::json!({ "code": 404, "message": "User not found" })).into_response(),
        Err(error) => Json(serde_json::json!({ "code": 400, "message": error.to_string() })).into_response(),
    }
}

async fn admin_reset_password(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(user_id): axum::extract::Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if !auth.is_admin {
        return Json(serde_json::json!({ "code": 403, "message": "Admin required" })).into_response();
    }
    let new_password = body.get("new_password").and_then(|v| v.as_str()).unwrap_or("");
    match state.users.reset_password(&user_id, new_password) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => Json(serde_json::json!({ "code": 404, "message": "User not found" })).into_response(),
        Err(error) => Json(serde_json::json!({ "code": 400, "message": error.to_string() })).into_response(),
    }
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
        .filter(|chunk| chunk.metadata.get("kb_id").map(String::as_str) == Some(dataset_id.as_str()))
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
        by_doc.entry(doc_id).or_default().push(chunk.content.clone());
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
    let edges = names
        .iter()
        .map(|name| graph.degree(name))
        .sum::<usize>()
        / 2;
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
    let current = body.get("current_password").and_then(|v| v.as_str()).unwrap_or("");
    let new_password = body.get("new_password").and_then(|v| v.as_str()).unwrap_or("");
    match state.users.change_password(&auth.user_id, current, new_password) {
        Ok(()) => Json(serde_json::json!({ "code": 0, "message": "ok" })).into_response(),
        Err(error) => Json(serde_json::json!({ "code": 400, "message": error.to_string() })).into_response(),
    }
}


async fn clear_conversation(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(conversation_id): axum::extract::Path<String>,
) -> Response {
    if state.conversations.get_for(&conversation_id, &auth.user_id).is_none() {
        return Json(serde_json::json!({ "code": 404, "message": "Conversation not found" })).into_response();
    }
    match state.conversations.clear_messages(&conversation_id) {
        Ok(_) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Err(error) => Json(serde_json::json!({ "code": 500, "message": error.to_string() })).into_response(),
    }
}

async fn export_conversation(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(conversation_id): axum::extract::Path<String>,
) -> Response {
    let Some(conversation) = state.conversations.get_for(&conversation_id, &auth.user_id) else {
        return Json(serde_json::json!({ "code": 404, "message": "Conversation not found" })).into_response();
    };
    let mut markdown = format!("# {}\n\n", conversation.name);
    for message in &conversation.messages {
        let role = if message.role == "user" { "**User**" } else { "**Assistant**" };
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

async fn delete_conversation(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(conversation_id): axum::extract::Path<String>,
) -> Response {
    match state.conversations.delete_for(&conversation_id, &auth.user_id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "message": "ok" })).into_response(),
        Ok(false) => Json(serde_json::json!({ "code": 404, "message": "Conversation not found" })).into_response(),
        Err(error) => Json(serde_json::json!({ "code": 500, "message": error.to_string() })).into_response(),
    }
}

async fn retrieval(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(b): Json<serde_json::Value>,
) -> Response {
    let q = b.get("question").and_then(|v| v.as_str()).unwrap_or("");
    let top_k = b.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let page = b.get("page").and_then(|v| v.as_u64()).unwrap_or(1).max(1) as usize;
    let rerank_id = b
        .get("rerank_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let rerank = b.get("rerank").and_then(|v| v.as_bool()).unwrap_or(false) || rerank_id.is_some();
    let vector_weight = b
        .get("vector_similarity_weight")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.3)
        .clamp(0.0, 1.0) as f32;
    let threshold = b
        .get("similarity_threshold")
        .and_then(|value| value.as_f64())
        .unwrap_or(0.2)
        .clamp(0.0, 1.0) as f32;
    let kb_ids: Vec<String> = b
        .get("kb_ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let doc_ids: Option<Vec<String>> =
        b.get("doc_ids")
            .and_then(|value| value.as_array())
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            });
    let filtered_doc_ids = match crate::api::document_metadata::resolve_metadata_doc_ids(
        &state,
        &kb_ids,
        doc_ids.as_deref(),
        b.get("meta_data_filter"),
    ) {
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
    let msgs = vec![crate::llm::ChatMessage::new("user", q)];
    match llm.chat(&msgs).await {
        Ok(a) => Json(ApiResponse {
            code: 0,
            message: "ok".into(),
            data: Some(serde_json::json!({"answer":a,"reference":[]})),
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

fn public_api_path(path: &str) -> bool {
    matches!(
        path,
        "/api/v1/auth/login"
            | "/api/v1/user/register"
            | "/api/v1/system/healthz"
            | "/api/v1/version"
    )
}

async fn require_api_auth(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if request.method() == Method::OPTIONS
        || !request.uri().path().starts_with("/api/v1/")
        || public_api_path(request.uri().path())
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

    if let Some(user_id) = token.as_deref().and_then(|token| state.users.validate_token(token)) {
        let is_admin = state.users.is_admin(&user_id);
        let mut request = request;
        request
            .extensions_mut()
            .insert(AuthContext { user_id, is_admin });
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
    headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match token.and_then(|token| state.users.get_user_by_token(token)) {
        Some(user) => Json(ApiResponse {
            code: 0,
            message: "ok".into(),
            data: Some(serde_json::json!({
                "id": user.id,
                "nickname": user.nickname,
                "email": user.email,
                "role": user.role,
                "created_at": user.created_at,
            })),
        }),
        None => Json(ApiResponse {
            code: 401,
            message: "Unauthorized".into(),
            data: None,
        }),
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
        .route("/search", get(crate::web::search_page))
        .route("/providers", get(crate::web::providers_page))
        .route("/skills", get(crate::web::skills_page))
        .route("/documents/{kb_id}/{doc_id}", get(crate::web::document_page))
        .route("/settings", get(crate::web::settings_page))
        .route("/api-docs", get(crate::web::api_docs))
        .route("/status", get(crate::web::status_page))
        .route("/chat", get(crate::web::chat_page))
        .route("/chat/{id}", get(crate::web::chat_detail_page))
        .route("/dataset/{id}", get(crate::web::dataset_page))
        .route("/files", get(crate::web::files_page))

        .route("/agents", get(crate::web::agents_page))
        .route("/agents/{agent_id}", get(crate::web::agent_detail_page))
        .route("/memories", get(crate::web::memories_page))
        .route("/admin", get(crate::web::admin_page))
        .route("/login", get(crate::web::login_page))
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
                    data: Some(serde_json::json!({"version":env!("CARGO_PKG_VERSION")})),
                })
            }),
        )
        .route("/api/v1/auth/login", post(login))
        .route("/api/v1/user/register", post(register))
        .route("/api/v1/user/info", get(user_info))
        .route("/api/v1/components", get(component_catalog))
        .route("/api/v1/datasets", get(list_datasets).post(create_dataset))
        .route(
            "/api/v1/datasets/{id}",
            put(update_dataset).delete(delete_dataset),
        )
        .route(
            "/api/v1/datasets/{id}/documents",
            post(upload_document).get(crate::api::document::list_docs),
        )
        .route(
            "/api/v1/datasets/{id}/documents/{did}",
            get(crate::api::document::get_doc).delete(crate::api::document::delete_doc),
        )
        .route(
            "/api/v1/datasets/{id}/documents/{did}/reparse",
            post(crate::api::document::reparse_doc),
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
        .route("/api/v1/conversations/{conversation_id}", axum::routing::delete(delete_conversation))
        .route(
            "/api/v1/conversations/{conversation_id}/clear",
            axum::routing::post(clear_conversation),
        )
        .route(
            "/api/v1/conversations/{conversation_id}/export",
            axum::routing::get(export_conversation),
        )
        .route("/api/v1/user/password", axum::routing::patch(change_password))
        .route(
            "/api/v1/datasets/{dataset_id}/graphrag/build",
            axum::routing::post(build_graphrag),
        )
        .route(
            "/api/v1/admin/users/{user_id}",
            axum::routing::delete(admin_delete_user),
        )
        .route(
            "/api/v1/admin/users/{user_id}/password",
            axum::routing::patch(admin_reset_password),
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
            "/api/v1/files/{file_id}",
            axum::routing::delete(crate::api::file_mgr::delete_file),
        )
        .route(
            "/api/v1/files/{file_id}/download",
            axum::routing::get(crate::api::file_mgr::download_file),
        )
        .route(
            "/api/v1/files/upload",
            post(crate::api::file_mgr::upload_file),
        )
        .route(
            "/api/v1/chat/completions",
            post(crate::api::openai_proxy::chat_completions),
        )
        .route("/api/v1/models", get(crate::api::openai_proxy::list_models))
        .route(
            "/api/v1/providers",
            get(crate::api::features::list_providers),
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
        .route(
            "/api/v1/agents/{id}/versions",
            get(crate::api::features::list_agent_versions),
        )
        .route(
            "/api/v1/agents/{id}/versions/{version_id}",
            get(crate::api::features::get_agent_version),
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
            axum::routing::post(
                crate::api::compilation_templates::execute_group_templates,
            ),
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
            "/api/v1/memories/{memory_id}",
            get(crate::api::features::get_memory_config)
                .put(crate::api::features::update_memory)
                .delete(crate::api::features::delete_memory),
        )
        .route(
            "/api/v1/memories/{memory_id}/config",
            get(crate::api::features::get_memory_config),
        )
        .route("/api/v1/users", get(crate::api::system::list_users))
        .route("/api/v1/users/{id}", put(crate::api::system::update_user))
        .route(
            "/api/v1/users/invite",
            post(crate::api::system::invite_user),
        )
        .route("/api/v1/tenant", get(crate::api::system::get_tenant))
        .route("/api/v1/tenants", get(crate::api::system::list_tenants))
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
        .route("/api/v1/bots", get(crate::api::connector::list_bots))
        .route(
            "/api/v1/channels",
            get(crate::api::connector::list_channels),
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
        .route("/api/v1/system/healthz", get(crate::api::system::healthz))
        .route(
            "/api/v1/system/new_api_key",
            post(crate::api::system::new_api_key),
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
            "/api/v1/langfuse",
            get(crate::api::system::get_langfuse).post(crate::api::system::update_langfuse),
        )
        .fallback_service(ServeDir::new(sd).precompressed_gzip().precompressed_br().not_found_service(axum::routing::get(crate::web::not_found_page)))
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
        let current = engine.read().unwrap().to_vec();
        vector_mirror.reconcile_snapshot(&current)?;
    }
    let embedder: Option<SharedEmbedder> = match (
        std::env::var("EMBED_API_BASE").ok(),
        std::env::var("EMBED_API_KEY").ok(),
        std::env::var("EMBED_MODEL").ok(),
    ) {
        (Some(api_base), Some(api_key), Some(model))
            if !api_base.is_empty() && !model.is_empty() =>
        {
            tracing::info!(model = %model, api_base = %api_base, "OpenAI-compatible embedder enabled");
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
        providers,
        tenant_models,
        memories: Arc::new(crate::api::features::MemoryStore::new(&memories_path)?),
        tasks: Arc::new(crate::api::features::TaskQueue::new(&t)?),
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
        document_commit_lock: Arc::new(std::sync::Mutex::new(())),
    });

    start_document_task_dispatcher(state.clone());
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!(
        "RayRAG v{} on http://0.0.0.0:{}",
        env!("CARGO_PKG_VERSION"),
        port
    );
    axum::serve(listener, router).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
            providers: Arc::new(crate::api::features::ProviderStore::in_memory()),
            tenant_models: Arc::new(crate::api::tenant_models::TenantModelStore::in_memory()),
            memories: Arc::new(crate::api::features::MemoryStore::in_memory()),
            tasks: Arc::new(crate::api::features::TaskQueue::in_memory()),
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
            document_commit_lock: Arc::new(std::sync::Mutex::new(())),
        });
        TestEnv { root, state, token }
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
            providers: Arc::new(crate::api::features::ProviderStore::in_memory()),
            tenant_models: Arc::new(crate::api::tenant_models::TenantModelStore::in_memory()),
            memories: Arc::new(crate::api::features::MemoryStore::in_memory()),
            tasks: Arc::new(crate::api::features::TaskQueue::in_memory()),
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
            document_commit_lock: Arc::new(std::sync::Mutex::new(())),
        });
        TestEnv { root, state, token }
    }

    async fn response_json(response: Response) -> serde_json::Value {
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
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
                    .uri("/api/v1/providers/minimax/models")
                    .header(header::AUTHORIZATION, format!("Bearer {}", member.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

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
        assert_eq!(model_types, vec!["chat", "embedding"]);

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
        assert_eq!(env.state.kbs.get(&kb.id).unwrap().permission, "private");

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

    #[tokio::test]
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
                    .uri("/api/v1/agents/tags?canvas_category=agent")
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
        state.llm = Some(Arc::new(crate::llm::LlmClient::new(crate::llm::LlmConfig {
            api_base: llm_base,
            api_key: String::new(),
            model: llm_model,
            generation: Default::default(),
            system_prompt: String::new(),
        })));
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
        assert!(!hyper_entities.is_empty(), "hypergraph entities: {hyper_entities:?}");
        assert!(
            hyper_entities
                .iter()
                .any(|e| e["name"].as_str().map(|n| n.contains("百鲤居") || n.contains("李锦澎")).unwrap_or(false)),
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
        assert!(checkpoint.graph.node_count() > 0, "compiled graph nodes persisted");
    }
}
