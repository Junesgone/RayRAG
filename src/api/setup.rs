//! First-login setup: the visual counterpart of the environment file.
//!
//! RayRAG is configured entirely through environment variables (the same names
//! RAGFlow uses), which is right for operators and unfriendly for everyone else. A
//! fresh deployment therefore offers a guided page: it lists the settings that
//! matter, shows what this host would recommend, writes the chosen values back to
//! the environment file the process actually read, and applies the ones that can be
//! applied without a restart.
//!
//! Two rules shape the design:
//!
//! * **Say what happens next.** Every field is marked `live` (read on each call, so
//!   it takes effect immediately) or not (read at startup, so it needs a restart).
//!   A wizard that silently pretends a restart is unnecessary is worse than no
//!   wizard.
//! * **Never lie about the secret.** Values are echoed back for review only when the
//!   caller may see them; secrets are reported as "set" or "not set" instead of
//!   being sent to the browser.

use crate::api::common::{ApiError, ApiErrorKind};
use crate::host_resources::HostResources;
use crate::server::{AppState, AuthContext};
use axum::{
    Json,
    extract::{Extension, State},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

/// Environment file the wizard writes to.
///
/// The process remembers the file it loaded at startup (`RAYRAG_ENV_FILE`, the
/// working directory's `.env`, or the project's) so a change lands where the next
/// boot will read it. A deployment that started with no file at all gets one beside
/// its state, which is the directory the application already owns.
static ENV_FILE: OnceLock<PathBuf> = OnceLock::new();

/// Record the environment file chosen at startup.
pub fn remember_env_file(path: PathBuf) {
    let _ = ENV_FILE.set(path);
}

/// The environment file to write, given the state directory (`static_dir/..`).
pub fn env_file_path(state: &AppState) -> PathBuf {
    if let Some(path) = ENV_FILE.get() {
        return path.clone();
    }
    if let Ok(explicit) = std::env::var("RAYRAG_ENV_FILE")
        && !explicit.trim().is_empty()
    {
        return PathBuf::from(explicit);
    }
    Path::new(&state.static_dir).join("../.env")
}

/// Which kind of input a field wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Secret,
    Number,
    Select,
}

/// One configurable setting.
#[derive(Debug, Clone, Serialize)]
pub struct SetupField {
    /// Environment variable name — the same name RAGFlow documents.
    pub key: &'static str,
    pub label_en: &'static str,
    pub label_zh: &'static str,
    pub group: &'static str,
    pub kind: FieldKind,
    pub help_en: &'static str,
    pub help_zh: &'static str,
    /// Current effective value (secrets are replaced by a marker).
    pub value: String,
    /// True when the value came from the real environment rather than the file.
    pub from_environment: bool,
    /// True when the process reads this on every call, so the change applies at once.
    pub live: bool,
    /// Choices for [`FieldKind::Select`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<&'static str>,
    /// Host-derived suggestion, when there is one (e.g. worker count).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recommended: Option<String>,
}

/// Placeholder sent to the browser instead of a secret.
pub const SECRET_PLACEHOLDER: &str = "********";

/// Every field the page offers, in display order.
fn field_specs() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    FieldKind,
    &'static str,
    &'static str,
    bool,
    Vec<&'static str>,
)> {
    vec![
        (
            "EMBED_API_BASE",
            "Embedding endpoint",
            "嵌入模型地址",
            "models",
            FieldKind::Text,
            "OpenAI-compatible base URL, e.g. http://127.0.0.1:8888/v1",
            "OpenAI 兼容地址，例如 http://127.0.0.1:8888/v1",
            false,
            Vec::new(),
        ),
        (
            "EMBED_API_KEY",
            "Embedding API key",
            "嵌入模型密钥",
            "models",
            FieldKind::Secret,
            "Sent as the bearer token to the embedding endpoint",
            "作为 Bearer 令牌发送给嵌入服务",
            false,
            Vec::new(),
        ),
        (
            "EMBED_MODEL",
            "Embedding model",
            "嵌入模型名称",
            "models",
            FieldKind::Text,
            "Model name the endpoint expects",
            "服务端识别的模型名称",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_EMBEDDING_DIMENSION",
            "Embedding dimension",
            "向量维度",
            "models",
            FieldKind::Number,
            "Must match the model (384 for Qwen3-Embedding-4B)",
            "必须与模型一致（Qwen3-Embedding-4B 为 384）",
            false,
            Vec::new(),
        ),
        (
            "LLM_API_BASE",
            "Chat model endpoint",
            "大语言模型地址",
            "models",
            FieldKind::Text,
            "OpenAI-compatible base URL for chat completions",
            "OpenAI 兼容的对话补全地址",
            false,
            Vec::new(),
        ),
        (
            "LLM_API_KEY",
            "Chat model API key",
            "大语言模型密钥",
            "models",
            FieldKind::Secret,
            "Bearer token for the chat endpoint",
            "对话服务的 Bearer 令牌",
            false,
            Vec::new(),
        ),
        (
            "LLM_MODEL",
            "Chat model",
            "大语言模型名称",
            "models",
            FieldKind::Text,
            "Model name used for answers, keywords and summaries",
            "用于问答、关键词与摘要的模型名称",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_VECTOR_BACKEND",
            "Vector backend",
            "向量后端",
            "storage",
            FieldKind::Select,
            "zvec is the native store; json keeps a portable index only",
            "zvec 为原生向量库；json 仅保留可移植索引",
            false,
            vec!["zvec", "json"],
        ),
        (
            "RAYRAG_ZVEC_DIR",
            "Vector data directory",
            "向量数据目录",
            "storage",
            FieldKind::Text,
            "Where native collections live (zvec backend)",
            "原生集合的存放目录（zvec 后端）",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_POSTGRES_URL",
            "PostgreSQL URL",
            "PostgreSQL 连接串",
            "storage",
            FieldKind::Text,
            "Metadata store; PostgreSQL 18.4 is the deployment baseline",
            "元数据存储；部署基线为 PostgreSQL 18.4",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_MAX_CONCURRENT_TASKS",
            "Concurrent documents",
            "并发入库文档数",
            "resources",
            FieldKind::Number,
            "How many documents are parsed at once; empty means derive from this host",
            "同时解析的文档数；留空表示按本机资源自动决定",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_CMD_TIMEOUT",
            "Command timeout (s)",
            "命令超时（秒）",
            "resources",
            FieldKind::Number,
            "Outer bound for every external command, 7200 (2 h) maximum",
            "所有外部命令的超时上限，最大 7200（2 小时）",
            true,
            Vec::new(),
        ),
        (
            "RAYRAG_MODEL_TIMEOUT",
            "Model call timeout (s)",
            "模型调用超时（秒）",
            "resources",
            FieldKind::Number,
            "Embedding, vision and OCR calls give up after this many seconds",
            "嵌入、视觉与 OCR 调用超过该秒数即放弃",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_HTTP_BODY_LIMIT_BYTES",
            "API response limit (bytes)",
            "API 响应体上限（字节）",
            "resources",
            FieldKind::Number,
            "Largest response body buffered from an external service",
            "单次缓存的外部服务响应体上限",
            true,
            Vec::new(),
        ),
        (
            "RAYRAG_CONNECTOR_BODY_LIMIT_BYTES",
            "Download limit (bytes)",
            "下载上限（字节）",
            "resources",
            FieldKind::Number,
            "Largest document downloaded from a connector (S3, WebDAV, HTTP, RSS)",
            "连接器（S3/WebDAV/HTTP/RSS）单次下载的文档上限",
            true,
            Vec::new(),
        ),
        (
            "RAYRAG_PDF_STREAM_LIMIT_BYTES",
            "PDF stream limit (bytes)",
            "PDF 流解压上限（字节）",
            "resources",
            FieldKind::Number,
            "Largest decompressed PDF stream; guards against expansion bombs",
            "单个 PDF 流解压后的上限，用于防御解压炸弹",
            true,
            Vec::new(),
        ),
        (
            "RAYRAG_ZVEC_MAX_BUFFER_BYTES",
            "Vector write buffer (bytes)",
            "向量写缓冲（字节）",
            "resources",
            FieldKind::Number,
            "Ceiling for one collection's native write buffer",
            "单个集合的原生写缓冲上限",
            true,
            Vec::new(),
        ),
        (
            "RAYRAG_OCR_PROVIDER",
            "OCR provider",
            "OCR 提供方",
            "models",
            FieldKind::Select,
            "proxy talks to a PaddleOCR-compatible service; none disables OCR",
            "proxy 调用 PaddleOCR 兼容服务；none 表示关闭 OCR",
            true,
            vec!["none", "proxy", "paddleocr"],
        ),
        (
            "RAYRAG_OCR_BASE_URL",
            "OCR endpoint",
            "OCR 服务地址",
            "models",
            FieldKind::Text,
            "Base URL of the OCR service (proxy provider)",
            "OCR 服务地址（proxy 提供方）",
            true,
            Vec::new(),
        ),
    ]
}

/// Read a key the way the process sees it: the real environment first, then the
/// environment file (a deployment may have edited the file without restarting).
fn effective_value(key: &str, file_entries: &[(String, String)]) -> (String, bool) {
    if let Ok(value) = std::env::var(key)
        && !value.is_empty()
    {
        return (value, true);
    }
    file_entries
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| (value.clone(), false))
        .unwrap_or_default()
}

/// Build the field list with current values and host recommendations.
pub fn setup_fields(state: &AppState, reveal_secrets: bool) -> Vec<SetupField> {
    let entries = read_env_file(&env_file_path(state)).unwrap_or_default();
    let host = HostResources::detect();
    let mut fields = Vec::new();
    for (key, label_en, label_zh, group, kind, help_en, help_zh, live, options) in field_specs() {
        let (mut value, from_environment) = effective_value(key, &entries);
        if kind == FieldKind::Secret && !value.is_empty() && !reveal_secrets {
            value = SECRET_PLACEHOLDER.to_string();
        }
        let recommended = match key {
            "RAYRAG_MAX_CONCURRENT_TASKS" => Some(host.document_task_limit().to_string()),
            "RAYRAG_CMD_TIMEOUT" => Some(crate::common::cmd_timeout::DEFAULT_SECS.to_string()),
            "RAYRAG_MODEL_TIMEOUT" => {
                Some(crate::common::cmd_timeout::DEFAULT_MODEL_SECS.to_string())
            }
            "RAYRAG_HTTP_BODY_LIMIT_BYTES"
            | "RAYRAG_CONNECTOR_BODY_LIMIT_BYTES"
            | "RAYRAG_PDF_STREAM_LIMIT_BYTES"
            | "RAYRAG_ZVEC_MAX_BUFFER_BYTES" => None,
            _ => None,
        };
        fields.push(SetupField {
            key,
            label_en,
            label_zh,
            group,
            kind,
            help_en,
            help_zh,
            value,
            from_environment,
            live,
            options,
            recommended,
        });
    }
    fields
}

/// Whether a deployment still needs the guided setup.
///
/// A deployment that already names a model endpoint (or was explicitly marked as
/// configured) is left alone: the page stays reachable at `/setup`, but nobody is
/// pushed through it.
pub fn needs_setup(state: &AppState) -> bool {
    let entries = read_env_file(&env_file_path(state)).unwrap_or_default();
    let (marked, _) = effective_value("RAYRAG_SETUP_COMPLETED", &entries);
    if marked == "1" {
        return false;
    }
    let (embed, _) = effective_value("EMBED_API_BASE", &entries);
    let (llm, _) = effective_value("LLM_API_BASE", &entries);
    embed.trim().is_empty() && llm.trim().is_empty()
}

/// The answer the home page needs, without an `AppState` in hand.
///
/// `index` renders before the request reaches the API layer, so the probe resolves the
/// environment file the same way [`env_file_path`] does, minus the state directory
/// (the process remembers the file it loaded at startup).
pub struct SetupProbe {
    pub needs_setup: bool,
}

impl SetupProbe {
    pub fn current() -> Self {
        let path = ENV_FILE
            .get()
            .cloned()
            .or_else(|| {
                std::env::var("RAYRAG_ENV_FILE")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from(".env"));
        let entries = read_env_file(&path).unwrap_or_default();
        let (marked, _) = effective_value("RAYRAG_SETUP_COMPLETED", &entries);
        if marked == "1" {
            return Self { needs_setup: false };
        }
        let (embed, _) = effective_value("EMBED_API_BASE", &entries);
        let (llm, _) = effective_value("LLM_API_BASE", &entries);
        Self {
            needs_setup: embed.trim().is_empty() && llm.trim().is_empty(),
        }
    }
}

/// Read `KEY=value` pairs from an environment file, ignoring comments.
pub fn read_env_file(path: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return Ok(Vec::new());
    };
    let mut entries = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        entries.push((
            key.trim().to_string(),
            value.trim().trim_matches(['"', '\'']).to_string(),
        ));
    }
    Ok(entries)
}

/// Write `updates` into the environment file, preserving everything else.
///
/// Existing lines keep their position (and their comment neighbors); new keys are
/// appended under a header. The write is atomic, so a crash cannot leave a
/// half-written file that the next boot would read.
pub fn write_env_file(path: &Path, updates: &[(String, String)]) -> anyhow::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut pending: Vec<(String, String)> = updates.to_vec();

    for line in lines.iter_mut() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, _)) = trimmed.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if let Some(index) = pending.iter().position(|(name, _)| name == key) {
            let (name, value) = pending.remove(index);
            *line = format!("{name}={value}");
        }
    }

    if !pending.is_empty() {
        if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("# Written by the RayRAG first-login setup page".to_string());
        for (name, value) in pending {
            lines.push(format!("{name}={value}"));
        }
    }

    let mut content = lines.join("\n");
    content.push('\n');
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::persistence::atomic_write(path, content.as_bytes())?;
    Ok(())
}

/// Which values this process can adopt without a restart.
///
/// The list mirrors how each setting is read. The size limits are consulted on every
/// call, and the OCR client is rebuilt per parse, so those are live. Everything else
/// is decided once: the embedding and chat clients are constructed at startup (and
/// keep the timeout they were built with), the vector backend, dimension, data
/// directory, database URL and worker count are all startup decisions. Claiming more
/// than that would be a promise the process does not keep.
pub fn applies_live(key: &str) -> bool {
    matches!(
        key,
        // Read on every call by the reader that enforces them.
        "RAYRAG_CMD_TIMEOUT"
            | "RAYRAG_HTTP_BODY_LIMIT_BYTES"
            | "RAYRAG_CONNECTOR_BODY_LIMIT_BYTES"
            | "RAYRAG_PDF_STREAM_LIMIT_BYTES"
            | "RAYRAG_ZVEC_MAX_BUFFER_BYTES"
            // The OCR client is rebuilt for every parse.
            | "RAYRAG_OCR_PROVIDER"
            | "RAYRAG_OCR_BASE_URL"
    )
}

/// Apply the live-adoptable values to the current process.
fn apply_live(key: &str, value: &str) {
    if !applies_live(key) {
        return;
    }
    // SAFETY: the setup endpoint is serialized by the caller and the values are
    // plain strings; Rust's environment access is otherwise unsafe only because
    // other threads could be reading concurrently, which `set_var` handles for
    // owned values.
    unsafe {
        std::env::set_var(key, value);
    }
}

/// `GET /api/v1/setup/status` — does this deployment still need the guided page?
pub async fn setup_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let host = HostResources::detect();
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "needs_setup": needs_setup(&state),
            "env_file": env_file_path(&state).display().to_string(),
            "host": {
                "cpus": host.cpus,
                "available_memory_bytes": host.available_memory_bytes,
                "document_tasks": host.document_task_limit(),
                // The page prints this line directly, so the status route carries it
                // too (it used to render "undefined" before the options call landed).
                "summary": host.summary(),
            },
            "version": crate::build_info::VERSION,
        }
    }))
}

/// `GET /api/v1/setup/options` — the fields, their current values and this host's
/// recommendations. Requires a session: the page is part of the authenticated UI.
pub async fn setup_options(
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
) -> impl IntoResponse {
    let fields = setup_fields(&state, false);
    let host = HostResources::detect();
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "fields": fields,
            "env_file": env_file_path(&state).display().to_string(),
            "host": {
                "cpus": host.cpus,
                "available_memory_bytes": host.available_memory_bytes,
                "document_tasks": host.document_task_limit(),
                "summary": host.summary(),
            },
        }
    }))
}

#[derive(serde::Deserialize)]
pub struct SetupUpdate {
    pub key: String,
    pub value: String,
}

#[derive(serde::Deserialize)]
pub struct SetupCompleteRequest {
    pub updates: Vec<SetupUpdate>,
    /// Leave the guided page reachable but stop treating this deployment as new.
    #[serde(default)]
    pub mark_completed: bool,
}

/// `POST /api/v1/setup/complete` — persist the chosen values and adopt the live ones.
pub async fn setup_complete(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<SetupCompleteRequest>,
) -> Response {
    if !auth.is_admin {
        return ApiError::new(
            ApiErrorKind::NotAdmin,
            "Only administrators may change deployment settings",
        )
        .into_response();
    }
    let allowed: Vec<&'static str> = field_specs().into_iter().map(|spec| spec.0).collect();
    let mut updates: Vec<(String, String)> = Vec::new();
    for update in &request.updates {
        let key = update.key.trim();
        if !allowed.contains(&key) {
            // An unknown key would be written into the environment file and never
            // read: refuse it instead of pretending it was applied.
            return ApiError::new(ApiErrorKind::Admin, format!("Unsupported setting '{key}'"))
                .into_response();
        }
        let value = update.value.trim();
        // The page round-trips secrets as a placeholder; an unchanged placeholder
        // means "keep the stored value".
        if value == SECRET_PLACEHOLDER {
            continue;
        }
        if let Err(error) = validate(key, value) {
            return ApiError::new(ApiErrorKind::Admin, error.to_string()).into_response();
        }
        updates.push((key.to_string(), value.to_string()));
    }
    if request.mark_completed {
        updates.push(("RAYRAG_SETUP_COMPLETED".to_string(), "1".to_string()));
    }

    let path = env_file_path(&state);
    if let Err(error) = write_env_file(&path, &updates) {
        return ApiError::new(
            ApiErrorKind::Admin,
            format!("Could not write {}: {error}", path.display()),
        )
        .into_response();
    }
    let mut applied_live = Vec::new();
    let mut needs_restart = Vec::new();
    for (key, value) in &updates {
        if applies_live(key) {
            apply_live(key, value);
            applied_live.push(key.clone());
        } else {
            needs_restart.push(key.clone());
        }
    }
    tracing::info!(
        env_file = %path.display(),
        live = applied_live.len(),
        restart = needs_restart.len(),
        "Deployment settings updated from the setup page"
    );
    Json(serde_json::json!({
        "code": 0,
        "message": "Saved",
        "data": {
            "env_file": path.display().to_string(),
            "applied_live": applied_live,
            "needs_restart": needs_restart,
        }
    }))
    .into_response()
}

/// Reject values the process could not honour, with the reason.
fn validate(key: &str, value: &str) -> anyhow::Result<()> {
    let numeric = matches!(
        key,
        "RAYRAG_MAX_CONCURRENT_TASKS"
            | "RAYRAG_CMD_TIMEOUT"
            | "RAYRAG_MODEL_TIMEOUT"
            | "RAYRAG_HTTP_BODY_LIMIT_BYTES"
            | "RAYRAG_CONNECTOR_BODY_LIMIT_BYTES"
            | "RAYRAG_PDF_STREAM_LIMIT_BYTES"
            | "RAYRAG_ZVEC_MAX_BUFFER_BYTES"
            | "RAYRAG_EMBEDDING_DIMENSION"
    );
    if value.is_empty() {
        // Empty means "leave it to the default", which is always valid.
        return Ok(());
    }
    if numeric {
        let parsed: i64 = value
            .parse()
            .map_err(|_| anyhow::anyhow!("'{key}' must be a number, got '{value}'"))?;
        if parsed <= 0 {
            anyhow::bail!("'{key}' must be greater than zero");
        }
        if key == "RAYRAG_CMD_TIMEOUT" && parsed > 7200 {
            anyhow::bail!("'RAYRAG_CMD_TIMEOUT' is capped at 7200 seconds (2 hours)");
        }
    }
    if key == "RAYRAG_VECTOR_BACKEND" && !matches!(value, "zvec" | "json") {
        anyhow::bail!("'RAYRAG_VECTOR_BACKEND' must be 'zvec' or 'json'");
    }
    if key.ends_with("_BASE_URL") || key.ends_with("_API_BASE") {
        if !value.starts_with("http://") && !value.starts_with("https://") {
            anyhow::bail!("'{key}' must start with http:// or https://");
        }
    }
    Ok(())
}

/// Register the environment file this process loaded, so the page writes back to it.
pub fn remember_loaded_env_file() {
    let candidates = [
        std::env::var("RAYRAG_ENV_FILE").ok(),
        Some(".env".to_string()),
        Some(format!("{}/.env", env!("CARGO_MANIFEST_DIR"))),
    ];
    if let Some(path) = candidates
        .into_iter()
        .flatten()
        .find(|path| Path::new(path).is_file())
    {
        remember_env_file(PathBuf::from(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_writes_preserve_comments_and_update_in_place() {
        let dir = std::env::temp_dir().join(format!("rayrag-setup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(
            &path,
            "# deployment notes\nEMBED_API_BASE=http://old:8888/v1\nUNRELATED=keep-me\n",
        )
        .unwrap();

        write_env_file(
            &path,
            &[
                ("EMBED_API_BASE".into(), "http://new:8888/v1".into()),
                ("LLM_API_KEY".into(), "sk-123".into()),
            ],
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        // The comment and the untouched key survive, the existing key is updated in
        // place, and the new key is appended.
        assert!(content.contains("# deployment notes"), "{content}");
        assert!(content.contains("UNRELATED=keep-me"), "{content}");
        assert!(
            content.contains("EMBED_API_BASE=http://new:8888/v1"),
            "{content}"
        );
        assert!(!content.contains("http://old"), "{content}");
        assert!(content.contains("LLM_API_KEY=sk-123"), "{content}");

        // Reading it back gives the same pairs the loader would see.
        let entries = read_env_file(&path).unwrap();
        assert!(
            entries
                .iter()
                .any(|(k, v)| k == "EMBED_API_BASE" && v == "http://new:8888/v1")
        );
        assert!(
            entries
                .iter()
                .any(|(k, v)| k == "UNRELATED" && v == "keep-me")
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn validation_rejects_values_the_process_cannot_honour() {
        assert!(validate("RAYRAG_MODEL_TIMEOUT", "120").is_ok());
        assert!(
            validate("RAYRAG_MODEL_TIMEOUT", "").is_ok(),
            "empty means default"
        );
        assert!(validate("RAYRAG_MODEL_TIMEOUT", "soon").is_err());
        assert!(validate("RAYRAG_MODEL_TIMEOUT", "0").is_err());
        assert!(validate("RAYRAG_CMD_TIMEOUT", "7201").is_err());
        assert!(validate("RAYRAG_VECTOR_BACKEND", "mysql").is_err());
        assert!(validate("RAYRAG_VECTOR_BACKEND", "zvec").is_ok());
        // A base URL without a scheme would fail at the first request: say so now.
        assert!(validate("EMBED_API_BASE", "127.0.0.1:8888").is_err());
        assert!(validate("EMBED_API_BASE", "http://127.0.0.1:8888/v1").is_ok());
    }

    #[test]
    fn live_settings_are_exactly_the_ones_read_per_call() {
        // The distinction the page shows must match how the process reads them: a
        // value advertised as live that only takes effect after a restart is a lie.
        for key in [
            "RAYRAG_CMD_TIMEOUT",
            "RAYRAG_HTTP_BODY_LIMIT_BYTES",
            "RAYRAG_CONNECTOR_BODY_LIMIT_BYTES",
            "RAYRAG_PDF_STREAM_LIMIT_BYTES",
            "RAYRAG_ZVEC_MAX_BUFFER_BYTES",
            "RAYRAG_OCR_PROVIDER",
            "RAYRAG_OCR_BASE_URL",
        ] {
            assert!(applies_live(key), "{key} is read per call and must be live");
        }
        for key in [
            // The embedding and chat clients are constructed at startup and keep the
            // timeout they were built with, so the page must not promise otherwise.
            "RAYRAG_MODEL_TIMEOUT",
            "EMBED_API_BASE",
            "LLM_API_KEY",
            "RAYRAG_MAX_CONCURRENT_TASKS",
            "RAYRAG_VECTOR_BACKEND",
            "RAYRAG_EMBEDDING_DIMENSION",
            "RAYRAG_POSTGRES_URL",
        ] {
            assert!(!applies_live(key), "{key} is decided at startup");
        }
        // Every offered field must have an explicit answer.
        for spec in field_specs() {
            let key = spec.0;
            let live = spec.7;
            assert_eq!(
                live,
                applies_live(key),
                "{key}: the page's badge and the process disagree"
            );
        }
    }
}
