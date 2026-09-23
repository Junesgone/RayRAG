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

/// The directory that looks like a RayRAG checkout/deployment root.
///
/// The guided page writes the deployment's configuration file, and the place a person
/// looks for that file is the project root — the directory with `docker-compose.yml`,
/// `Cargo.toml` and `.env.example` in it — not a path derived from the state volume.
/// The search starts at the working directory (which is the project root both when the
/// binary is started from a checkout and in the image, whose `WORKDIR` is `/app`) and
/// then at the executable's directory, walking up a few levels.
pub fn project_root() -> Option<PathBuf> {
    const MARKERS: [&str; 4] = [
        "docker-compose.yml",
        "compose.yaml",
        "Cargo.toml",
        ".env.example",
    ];
    let starts: Vec<PathBuf> = [
        std::env::current_dir().ok(),
        std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf)),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
    ]
    .into_iter()
    .flatten()
    .collect();
    for start in starts {
        let mut candidate = Some(start.as_path());
        for _ in 0..4 {
            let Some(directory) = candidate else { break };
            if MARKERS
                .iter()
                .any(|marker| directory.join(marker).is_file())
            {
                return Some(directory.to_path_buf());
            }
            candidate = directory.parent();
        }
    }
    None
}

/// The environment file the wizard writes, and the one the next boot reads.
///
/// Order: an explicit `RAYRAG_ENV_FILE`, then the file this process loaded at startup,
/// then the project root's `.env` (created there when the deployment has none yet),
/// then the state directory's sibling as the last resort. Writing beside the state
/// volume was the old default; it works, but nobody finds it there.
pub fn env_file_path(state: &AppState) -> PathBuf {
    env_file_path_for(&state.static_dir)
}

/// [`env_file_path`] without an `AppState`, so the choice is testable on its own.
pub fn env_file_path_for(static_dir: &str) -> PathBuf {
    resolve_env_file(
        ENV_FILE.get().map(PathBuf::as_path),
        std::env::var("RAYRAG_ENV_FILE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .as_deref(),
        project_root().as_deref(),
        static_dir,
    )
}

/// The rule behind [`env_file_path`], separated from the process-wide state so it can be
/// tested without depending on whether another test already set the file.
pub fn resolve_env_file(
    remembered: Option<&Path>,
    explicit: Option<&str>,
    project_root: Option<&Path>,
    static_dir: &str,
) -> PathBuf {
    if let Some(path) = remembered {
        return path.to_path_buf();
    }
    if let Some(path) = explicit {
        return PathBuf::from(path);
    }
    if let Some(root) = project_root {
        let candidate = root.join(".env");
        // A project root that already has the file wins; otherwise the state directory
        // keeps its file if one exists, and only a deployment with neither gets a new
        // file at the project root.
        if candidate.is_file() {
            return candidate;
        }
        let state_file = lexical_normalise(&Path::new(static_dir).join("../.env"));
        if state_file.is_file() {
            return state_file;
        }
        return candidate;
    }
    lexical_normalise(&Path::new(static_dir).join("../.env"))
}

/// Resolve `..` components textually, so a receipt reads `/app/web/.env` instead of
/// `/app/web/static/../.env`. No filesystem calls: a symlinked deployment keeps the path
/// it was configured with.
fn lexical_normalise(path: &Path) -> PathBuf {
    let mut parts: Vec<std::ffi::OsString> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                if parts.last().is_some_and(|last| last != "..") {
                    parts.pop();
                } else {
                    parts.push(component.as_os_str().to_os_string());
                }
            }
            std::path::Component::CurDir => {}
            other => parts.push(other.as_os_str().to_os_string()),
        }
    }
    let mut normalised = PathBuf::new();
    for part in parts {
        normalised.push(part);
    }
    normalised
}

/// Which kind of input a field wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Text,
    Secret,
    Number,
    Select,
    /// Shown but not editable here: the value belongs to `docker-compose.yml` or the
    /// shell that starts the process, so the page offers a copy-paste line instead of
    /// an input that would write a file nothing reads.
    Readonly,
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
    /// Choices for [`FieldKind::Select`]. A value the table does not list (a
    /// hand-edited `True`, say) is appended, so the page renders what is really set
    /// instead of silently falling back to the first option.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
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
            "RERANK_API_BASE",
            "Rerank endpoint (optional)",
            "重排模型地址（可选）",
            "models",
            FieldKind::Text,
            "Cohere-compatible /v1/rerank endpoint; leave empty to rank by keyword and vector score",
            "Cohere 兼容的 /v1/rerank 地址；留空则按关键词与向量得分排序",
            false,
            Vec::new(),
        ),
        (
            "RERANK_API_KEY",
            "Rerank API key",
            "重排模型密钥",
            "models",
            FieldKind::Secret,
            "Bearer token for the rerank endpoint",
            "重排服务的 Bearer 令牌",
            false,
            Vec::new(),
        ),
        (
            "RERANK_MODEL",
            "Rerank model",
            "重排模型名称",
            "models",
            FieldKind::Text,
            "Sent as 'model' with every rerank request; endpoints serving one model can leave it empty",
            "随每次重排请求作为 model 发送；只提供一个模型的服务可以留空",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_ASR_API_BASE",
            "Speech-to-text endpoint (optional)",
            "语音转写地址（可选）",
            "models",
            FieldKind::Text,
            "OpenAI-compatible /audio/transcriptions endpoint for audio documents",
            "音频文档使用的 OpenAI 兼容 /audio/transcriptions 地址",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_ASR_API_KEY",
            "Speech-to-text API key",
            "语音转写密钥",
            "models",
            FieldKind::Secret,
            "Bearer token for the speech-to-text endpoint",
            "语音转写服务的 Bearer 令牌",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_ASR_MODEL",
            "Speech-to-text model",
            "语音转写模型名称",
            "models",
            FieldKind::Text,
            "Defaults to whisper-1",
            "默认 whisper-1",
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
        (
            "SEARXNG_URL",
            "Search engine (SearXNG)",
            "搜索引擎（SearXNG）",
            "search",
            FieldKind::Text,
            "Self-hosted SearXNG base URL used by the agent's web search; the domestic-network answer to Google",
            "Agent 联网搜索使用的自建 SearXNG 地址；面向中国大陆网络的 Google 替代",
            true,
            Vec::new(),
        ),
        (
            "TAVILY_API_KEY",
            "Tavily API key",
            "Tavily 密钥",
            "search",
            FieldKind::Secret,
            "Optional Tavily key for the agent's search and extract tools",
            "可选的 Tavily 密钥，供 Agent 的搜索与正文提取工具使用",
            true,
            Vec::new(),
        ),
        (
            "BOCHA_API_KEY",
            "Bocha API key",
            "博查密钥",
            "search",
            FieldKind::Secret,
            "Optional Bocha key (Chinese search API) for the agent's web search",
            "可选的博查（国内搜索 API）密钥，供 Agent 联网搜索使用",
            true,
            Vec::new(),
        ),
        (
            "REGISTER_ENABLED",
            "Allow sign-up",
            "允许注册",
            "access",
            FieldKind::Select,
            "Upstream settings.REGISTER_ENABLED; 1 keeps registration open (the default for a new deployment)",
            "对齐上游 settings.REGISTER_ENABLED；1 表示开放注册（新部署的默认值）",
            false,
            vec!["1", "0"],
        ),
        (
            "DISABLE_PASSWORD_LOGIN",
            "Disable password login",
            "禁用密码登录",
            "access",
            FieldKind::Select,
            "true leaves only the configured OAuth channels able to sign in (1/yes are also accepted)",
            "true 表示仅允许已配置的 OAuth 渠道登录（1/yes 亦被接受）",
            false,
            vec!["false", "true"],
        ),
        (
            "RAYRAG_CORS_ORIGIN",
            "Allowed browser origin",
            "允许的浏览器来源",
            "access",
            FieldKind::Text,
            "Origin allowed to call the API from a browser (empty means same-origin only)",
            "允许浏览器跨域调用 API 的来源（留空表示仅同源）",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_MAX_UPLOAD_BYTES",
            "Upload limit (bytes)",
            "上传体积上限（字节）",
            "access",
            FieldKind::Number,
            "Largest single upload accepted",
            "单次上传的最大字节数",
            false,
            Vec::new(),
        ),
        (
            "RAYRAG_MEMORY_LIMIT",
            "Container memory limit",
            "容器内存上限",
            "resources",
            FieldKind::Readonly,
            "docker-compose.yml sets mem_limit from ${RAYRAG_MEMORY_LIMIT:-6g}; copy the line next to your compose file and recreate the container",
            "docker-compose.yml 用 ${RAYRAG_MEMORY_LIMIT:-6g} 设置 mem_limit；把下面这行放到 compose 文件旁并重建容器",
            false,
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
            "RAYRAG_MEMORY_LIMIT" => {
                Some(crate::common::cmd_timeout::DEFAULT_MEMORY_LIMIT.to_string())
            }
            "RAYRAG_HTTP_BODY_LIMIT_BYTES"
            | "RAYRAG_CONNECTOR_BODY_LIMIT_BYTES"
            | "RAYRAG_PDF_STREAM_LIMIT_BYTES"
            | "RAYRAG_ZVEC_MAX_BUFFER_BYTES" => None,
            _ => None,
        };
        let mut options: Vec<String> = options.into_iter().map(str::to_string).collect();
        if kind == FieldKind::Select
            && !value.is_empty()
            && value != SECRET_PLACEHOLDER
            && !options.iter().any(|option| option == &value)
        {
            // Never rewrite a setting the operator typed by hand just because the table
            // did not anticipate its spelling.
            options.push(value.clone());
        }
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
    if let Err(error) = crate::persistence::atomic_write(path, content.as_bytes()) {
        // A bind-mounted file cannot be replaced by `rename` — the mount point is busy
        // ("Device or resource busy"), which is exactly how docker-compose mounts the
        // project's `.env` so the setup page writes the file the operator edits. The new
        // content is written beside the file first, then copied over it in place, so a
        // failure still leaves a complete copy on disk.
        let temporary = path.with_extension("env.new");
        let staged = std::fs::write(&temporary, content.as_bytes()).is_ok();
        if staged && std::fs::write(path, content.as_bytes()).is_ok() {
            let _ = std::fs::remove_file(&temporary);
            tracing::info!(
                path = %path.display(),
                "Wrote the environment file in place (the target is a bind mount)"
            );
            return Ok(());
        }
        return Err(error);
    }
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
            // The agent tools read these from the environment at call time, so the
            // value written here is the one the next search uses.
            | "SEARXNG_URL"
            | "TAVILY_API_KEY"
            | "BOCHA_API_KEY"
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
        if is_readonly(key) {
            return ApiError::new(
                ApiErrorKind::Admin,
                format!("'{key}' belongs to the container definition and cannot be set here"),
            )
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
    // What the operator still has to do. Configuring endpoints in the file is only
    // half of a working deployment: the models themselves are added (and verified)
    // on the model-provider page, so the receipt points there instead of ending the
    // flow on a saved file.
    let entries = read_env_file(&path).unwrap_or_default();
    let (embed, _) = effective_value("EMBED_API_BASE", &entries);
    let (llm, _) = effective_value("LLM_API_BASE", &entries);
    let needs_model_provider = embed.trim().is_empty() && llm.trim().is_empty();
    Json(serde_json::json!({
        "code": 0,
        "message": "Saved",
        "data": {
            "env_file": path.display().to_string(),
            "applied_live": applied_live,
            "needs_restart": needs_restart,
            "next": {
                "needs_model_provider": needs_model_provider,
                "url": "/user-setting/model",
            },
        }
    }))
    .into_response()
}

/// The choices of a select field, from the same table the page renders.
pub fn select_options(key: &str) -> Option<Vec<&'static str>> {
    field_specs()
        .into_iter()
        .find(|spec| spec.0 == key)
        .filter(|spec| spec.4 == FieldKind::Select)
        .map(|spec| spec.8)
}

/// Fields the page shows but must never write: their value belongs to the container
/// definition, and accepting the write would store a setting nothing reads.
pub fn is_readonly(key: &str) -> bool {
    field_specs()
        .into_iter()
        .any(|spec| spec.0 == key && spec.4 == FieldKind::Readonly)
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
    if let Some(options) = select_options(key)
        && !options.contains(&value)
    {
        // A hand-edited file may spell a boolean switch the way the parser reads it
        // (`DISABLE_PASSWORD_LOGIN=True`), and refusing to save the page's own current
        // value would make the wizard unusable on such a deployment.
        let normalised = value.to_ascii_lowercase();
        let boolean_switch = matches!(key, "DISABLE_PASSWORD_LOGIN" | "REGISTER_ENABLED")
            && matches!(
                normalised.as_str(),
                "true" | "false" | "1" | "0" | "yes" | "no"
            );
        if !boolean_switch {
            anyhow::bail!("'{key}' must be one of {}", options.join(", "));
        }
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
    let project_file = project_root().map(|root| root.join(".env"));
    let candidates = [
        std::env::var("RAYRAG_ENV_FILE")
            .ok()
            .filter(|path| !path.trim().is_empty()),
        // The project root's file first: it is the one the operator edits and the one
        // `docker compose` interpolates.
        project_file.as_ref().map(|path| path.display().to_string()),
        Some(".env".to_string()),
    ];
    if let Some(path) = candidates
        .into_iter()
        .flatten()
        .find(|path| Path::new(path).is_file())
    {
        remember_env_file(PathBuf::from(path));
    }
    // A deployment with no file yet still gets one at the project root, so the page's
    // receipt points somewhere the user can find.
    if ENV_FILE.get().is_none()
        && let Some(path) = project_file
    {
        remember_env_file(path);
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
    fn the_table_offers_the_switches_a_deployment_actually_reads() {
        let keys: Vec<&str> = field_specs().into_iter().map(|spec| spec.0).collect();
        for key in [
            // Reranking: documented in the README long before it was offered here, and
            // `RERANK_MODEL` was silently ignored until this round.
            "RERANK_API_BASE",
            "RERANK_API_KEY",
            "RERANK_MODEL",
            "RAYRAG_ASR_API_BASE",
            "RAYRAG_ASR_MODEL",
            // Domestic-network search: the reason the agent's web search works without
            // reaching Google.
            "SEARXNG_URL",
            "TAVILY_API_KEY",
            "BOCHA_API_KEY",
            // Sign-up and hard limits.
            "REGISTER_ENABLED",
            "DISABLE_PASSWORD_LOGIN",
            "RAYRAG_CORS_ORIGIN",
            "RAYRAG_MAX_UPLOAD_BYTES",
            // Shown but owned by the container definition.
            "RAYRAG_MEMORY_LIMIT",
        ] {
            assert!(
                keys.contains(&key),
                "{key} is missing from the guided setup"
            );
        }
        let groups: Vec<&str> = field_specs().into_iter().map(|spec| spec.3).collect();
        for group in ["models", "storage", "resources", "search", "access"] {
            assert!(groups.contains(&group), "group {group} is missing");
        }
        // A select without options would render an empty dropdown that cannot be saved.
        for spec in field_specs() {
            if spec.4 == FieldKind::Select {
                assert!(!spec.8.is_empty(), "{} has no choices", spec.0);
            }
        }
        assert_eq!(select_options("REGISTER_ENABLED"), Some(vec!["1", "0"]));
        assert_eq!(select_options("EMBED_API_BASE"), None);
        assert!(validate("REGISTER_ENABLED", "1").is_ok());
        assert!(validate("REGISTER_ENABLED", "maybe").is_err());
        assert!(validate("DISABLE_PASSWORD_LOGIN", "2").is_err());
        // The shipped deployment spells this switch `false` (docker-compose.yml), so
        // the page must offer that spelling and accept it back unchanged. The bug this
        // guards: the select listed 0/1 only, the current value rendered as unselected,
        // and saving wrote a different setting than the operator saw.
        assert_eq!(
            select_options("DISABLE_PASSWORD_LOGIN"),
            Some(vec!["false", "true"])
        );
        assert!(validate("DISABLE_PASSWORD_LOGIN", "false").is_ok());
        assert!(validate("DISABLE_PASSWORD_LOGIN", "true").is_ok());
        // A hand-edited spelling the parser honours still round-trips.
        assert!(validate("DISABLE_PASSWORD_LOGIN", "True").is_ok());
        assert!(validate("DISABLE_PASSWORD_LOGIN", "yes").is_ok());
    }

    /// A bind-mounted file cannot be replaced by `rename`: the wizard must still be
    /// able to save into the project's `.env` that docker-compose mounts.
    #[test]
    fn a_file_that_cannot_be_replaced_is_written_in_place() {
        let dir = std::env::temp_dir().join(format!("rayrag-setup-bind-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".env");
        std::fs::write(&path, "EMBED_API_BASE=http://old:8888/v1\n").unwrap();
        // Stand in for "rename over the target fails" by making the target's directory
        // read-only: the staged copy lands in the same directory, so a plain atomic
        // write fails while the in-place path (opening the existing file) still works.
        let mut permissions = std::fs::metadata(&dir).unwrap().permissions();
        let original_mode = std::os::unix::fs::PermissionsExt::mode(&permissions);
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o500);
        std::fs::set_permissions(&dir, permissions).unwrap();
        let outcome = write_env_file(
            &path,
            &[(
                "EMBED_API_BASE".to_string(),
                "http://new:8888/v1".to_string(),
            )],
        );
        let mut restore = std::fs::metadata(&dir).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut restore, original_mode);
        std::fs::set_permissions(&dir, restore).unwrap();
        // The write either succeeded (the fallback ran) or reported the reason; what must
        // never happen is a silently truncated file.
        let content = std::fs::read_to_string(&path).unwrap();
        if outcome.is_ok() {
            assert!(content.contains("http://new:8888/v1"), "{content}");
        } else {
            assert!(content.contains("http://old:8888/v1"), "{content}");
        }
        assert!(
            content.ends_with('\n'),
            "the file must stay complete: {content:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_environment_file_defaults_to_the_project_root() {
        // The wizard's receipt must point at a file the operator can find: the project
        // root (the directory with `docker-compose.yml`/`Cargo.toml`), not a path beside
        // the state volume.
        let root = project_root().expect("the test runs from the project root");
        assert!(
            root.join("Cargo.toml").is_file(),
            "the detected root must be the crate root: {}",
            root.display()
        );
        let state_dir =
            std::env::temp_dir().join(format!("rayrag-setup-state-{}", uuid::Uuid::new_v4()));
        let static_dir = state_dir.join("static");
        std::fs::create_dir_all(&static_dir).unwrap();
        let static_dir = static_dir.to_str().unwrap();

        // A project root that has no `.env` yet, which is what a fresh clone looks like
        // (the real repository here has one, and it must win — see case 6).
        let empty_root = state_dir.join("project");
        std::fs::create_dir_all(&empty_root).unwrap();
        // 1. A fresh deployment gets its file at the project root, where the operator
        //    looks for it (and where `docker compose` reads it).
        assert_eq!(
            resolve_env_file(None, None, Some(&empty_root), static_dir),
            empty_root.join(".env")
        );
        // 2. An explicit RAYRAG_ENV_FILE wins over everything.
        assert_eq!(
            resolve_env_file(None, Some("/etc/rayrag/.env"), Some(&root), static_dir),
            PathBuf::from("/etc/rayrag/.env")
        );
        // 3. The file this process actually loaded wins over the guess: the wizard must
        //    write the file the next boot reads.
        assert_eq!(
            resolve_env_file(
                Some(Path::new("/srv/rayrag/env")),
                None,
                Some(&root),
                static_dir
            ),
            PathBuf::from("/srv/rayrag/env")
        );
        // 4. A deployment that already keeps its file beside the state directory keeps
        //    it: the wizard must not start writing a second file.
        std::fs::write(state_dir.join(".env"), "EMBED_API_BASE=\n").unwrap();
        assert_eq!(
            resolve_env_file(None, None, Some(&empty_root), static_dir),
            state_dir.join(".env")
        );
        // 6. A project root that already has the file is preferred over the state
        //    directory: that is the file `docker compose` interpolates.
        assert_eq!(
            resolve_env_file(None, None, Some(&root), static_dir),
            root.join(".env")
        );
        // 5. Without a project root the old state-relative default still applies.
        assert_eq!(
            resolve_env_file(None, None, None, static_dir),
            state_dir.join(".env")
        );
        std::fs::remove_dir_all(&state_dir).ok();
    }

    #[test]
    fn a_select_renders_a_value_the_table_did_not_list() {
        // `setup_fields` needs an `AppState`, so the same rule is exercised on the
        // helper it uses: the option list gains the current value instead of dropping it.
        let mut options: Vec<String> = vec!["false".to_string(), "true".to_string()];
        let value = "True".to_string();
        if !options.iter().any(|option| option == &value) {
            options.push(value.clone());
        }
        assert_eq!(options.last().map(String::as_str), Some("True"));
        assert!(
            options.contains(&"false".to_string()),
            "the listed spellings stay available"
        );
    }

    #[test]
    fn the_container_memory_limit_is_shown_but_never_written() {
        // The page must not promise a setting it cannot deliver: `mem_limit` comes
        // from docker-compose.yml, and a write into the environment file would be a
        // line nothing reads. The endpoint refuses it; the page renders the line.
        assert!(is_readonly("RAYRAG_MEMORY_LIMIT"));
        assert!(!is_readonly("RAYRAG_MAX_CONCURRENT_TASKS"));
        assert!(!applies_live("RAYRAG_MEMORY_LIMIT"));
        let spec = field_specs()
            .into_iter()
            .find(|spec| spec.0 == "RAYRAG_MEMORY_LIMIT")
            .expect("the memory limit is offered");
        assert_eq!(spec.4, FieldKind::Readonly);
        assert!(
            spec.5.contains("docker-compose.yml"),
            "the help must say where the value belongs: {}",
            spec.5
        );
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
