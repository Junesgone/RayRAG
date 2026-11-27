//! Connector / MCP / Bot / Channel / Plugin APIs.
//! Replaces RAGFlow's connector_api, mcp_api, bot_api, chat_channel_api, plugin_api.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::{
    Json,
    extract::rejection::JsonRejection,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::server::{AppState, AuthContext, api_error_code, code};

pub(crate) fn require_admin_response(auth: &AuthContext) -> Option<Response> {
    require_admin(auth)
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

// ── Connector (external data sources) ──────────────────────────

/// A stored connector instance.
///
/// The previous implementation answered the list from a hard-coded array of three invented
/// connectors and echoed `create` without storing anything, so a user could "add" a data source and
/// find it gone on reload. This is the real record: persisted, owner-scoped, and the only source the
/// list, detail, log, test and rebuild endpoints read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub source_type: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
    /// The outcome of the most recent `test`, so the list can show it without re-probing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_test: Option<serde_json::Value>,
}

fn default_enabled() -> bool {
    true
}

/// The body of a create request. The id, the owner and the timestamps are the server's to decide, so
/// they are not accepted from a client — and a body that omits nothing but those still parses.
#[derive(Debug, Deserialize, Default)]
pub struct ConnectorCreate {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub source_type: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub config: serde_json::Value,
    /// Accepted so a client that round-trips a whole record through create does not fail on it.
    #[serde(default)]
    pub id: String,
}

/// One line of a connector's history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorLogEntry {
    pub connector_id: String,
    pub at: u64,
    pub event: String,
    pub level: String,
    pub message: String,
}

#[derive(Serialize, Deserialize, Default)]
struct ConnectorFile {
    connectors: BTreeMap<String, Connector>,
    #[serde(default)]
    logs: Vec<ConnectorLogEntry>,
}

/// The connector instances and their history.
pub struct ConnectorStore {
    connectors: RwLock<BTreeMap<String, Connector>>,
    logs: RwLock<Vec<ConnectorLogEntry>>,
    path: String,
    save_lock: Mutex<()>,
}

/// How many log lines are kept per deployment, so an often-retried connector cannot grow without end.
const MAX_LOG_LINES: usize = 2_000;

impl ConnectorStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        let file: ConnectorFile = if std::path::Path::new(path).exists() {
            let data = std::fs::read_to_string(path)?;
            serde_json::from_str(&data).map_err(|error| {
                anyhow::anyhow!("Failed to parse connector store '{path}': {error}")
            })?
        } else {
            ConnectorFile::default()
        };
        Ok(Self {
            connectors: RwLock::new(file.connectors),
            logs: RwLock::new(file.logs),
            path: path.to_string(),
            save_lock: Mutex::new(()),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            connectors: RwLock::new(BTreeMap::new()),
            logs: RwLock::new(Vec::new()),
            path: String::new(),
            save_lock: Mutex::new(()),
        }
    }

    fn persist(&self) -> anyhow::Result<()> {
        if self.path.is_empty() {
            return Ok(());
        }
        let _guard = self.save_lock.lock().unwrap();
        let file = ConnectorFile {
            connectors: self.connectors.read().unwrap().clone(),
            logs: self.logs.read().unwrap().clone(),
        };
        crate::persistence::save_json(std::path::Path::new(&self.path), &file)
    }

    /// Insert or replace a connector.
    pub fn put(&self, connector: Connector) -> anyhow::Result<()> {
        self.connectors
            .write()
            .unwrap()
            .insert(connector.id.clone(), connector);
        self.persist()
    }

    pub fn get(&self, id: &str) -> Option<Connector> {
        self.connectors.read().unwrap().get(id).cloned()
    }

    /// Every connector the caller may see: an administrator sees all, anyone else their own.
    pub fn list(&self, user_id: &str, is_admin: bool) -> Vec<Connector> {
        self.connectors
            .read()
            .unwrap()
            .values()
            .filter(|connector| is_admin || connector.owner_id == user_id)
            .cloned()
            .collect()
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<bool> {
        let removed = self.connectors.write().unwrap().remove(id).is_some();
        if removed {
            self.logs
                .write()
                .unwrap()
                .retain(|line| line.connector_id != id);
            self.persist()?;
        }
        Ok(removed)
    }

    /// A connector the caller may read, or `None`.
    pub fn get_for(&self, id: &str, user_id: &str, is_admin: bool) -> Option<Connector> {
        self.get(id)
            .filter(|connector| is_admin || connector.owner_id == user_id)
    }

    pub fn log(
        &self,
        connector_id: &str,
        event: &str,
        level: &str,
        message: &str,
    ) -> anyhow::Result<()> {
        let mut logs = self.logs.write().unwrap();
        logs.push(ConnectorLogEntry {
            connector_id: connector_id.to_string(),
            at: crate::api::utils::datetime::now_ms(),
            event: event.to_string(),
            level: level.to_string(),
            message: message.to_string(),
        });
        if logs.len() > MAX_LOG_LINES {
            let drop_to = logs.len() - MAX_LOG_LINES;
            logs.drain(0..drop_to);
        }
        drop(logs);
        self.persist()
    }

    /// This connector's history, newest first.
    pub fn logs_for(&self, connector_id: &str, limit: usize) -> Vec<ConnectorLogEntry> {
        let mut lines: Vec<ConnectorLogEntry> = self
            .logs
            .read()
            .unwrap()
            .iter()
            .filter(|line| line.connector_id == connector_id)
            .cloned()
            .collect();
        lines.reverse();
        lines.truncate(limit.clamp(1, 500));
        lines
    }

    pub fn log_count(&self, connector_id: &str) -> usize {
        self.logs
            .read()
            .unwrap()
            .iter()
            .filter(|line| line.connector_id == connector_id)
            .count()
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ConnectorLogQuery {
    #[serde(default)]
    pub limit: Option<usize>,
}

/// The connector types this deployment can actually build, named as the registry names them.
pub(crate) fn known_source_types() -> Vec<String> {
    crate::connectors::ConnectorRegistry::kinds()
        .into_iter()
        .map(|info| info.kind.to_string())
        .collect()
}

fn data_error(message: &str) -> Response {
    api_error_code(StatusCode::OK, code::INVALID_OR_MISSING_DATA, message)
}

fn not_found(message: &str) -> Response {
    api_error_code(
        StatusCode::NOT_FOUND,
        code::INVALID_OR_MISSING_DATA,
        message,
    )
}

/// Build the connector's `SourceOptions` from its stored configuration.
fn source_options(config: &serde_json::Value) -> crate::data_source::SourceOptions {
    let text = |key: &str| {
        config
            .get(key)
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    };
    let mut options = crate::data_source::SourceOptions::default();
    // The registry reads a source's location from `url` and a sub-path from `target` (`LocalConnector`
    // uses the url as its root directory). `path` is accepted as the friendly alias for `url`, because
    // that is what a person types for a local directory, and "root directory URL is required" for a
    // connector whose path was given is exactly the kind of confusing refusal worth avoiding.
    options.url = text("url");
    if options.url.is_empty() {
        options.url = text("path");
    }
    options.token = if config.get("token").is_some() {
        text("token")
    } else {
        text("api_key")
    };
    options.target = if config.get("target").is_some() {
        text("target")
    } else {
        text("subdir")
    };
    if let Some(max_items) = config.get("max_items").and_then(|value| value.as_u64()) {
        options.max_items = max_items as usize;
    }
    options
}

/// `GET /api/v1/connectors`.
pub async fn list_connectors(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    let connectors = state.connectors.list(&auth.user_id, auth.is_admin);
    Json(serde_json::json!({
        "code": 0,
        "data": connectors,
        "total": connectors.len(),
        "message": "success",
    }))
    .into_response()
}

/// `POST /api/v1/connectors`.
pub async fn create_connector(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    body: Result<Json<ConnectorCreate>, JsonRejection>,
) -> Response {
    // Creating a connector is a management write, as it was before this family was made real.
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    // axum's own rejection is plain text, which no client of this API can read as a business code, so
    // a malformed body is reported the way every other refusal here is.
    let Json(body) = match body {
        Ok(body) => body,
        Err(rejection) => {
            return api_error_code(
                StatusCode::BAD_REQUEST,
                code::INVALID_ARGUMENT,
                &format!("Invalid request body: {rejection}"),
            );
        }
    };
    let mut body = Connector {
        id: body.id,
        name: body.name,
        source_type: body.source_type,
        enabled: body.enabled,
        config: body.config,
        owner_id: String::new(),
        created_at: 0,
        updated_at: 0,
        last_test: None,
    };
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return api_error_code(
            StatusCode::BAD_REQUEST,
            code::INVALID_ARGUMENT,
            "Connector name is required.",
        );
    }
    if name.chars().count() > 128 {
        return api_error_code(
            StatusCode::BAD_REQUEST,
            code::INVALID_ARGUMENT,
            "Connector name must be at most 128 characters.",
        );
    }
    let kind = body.source_type.trim().to_string();
    let known = known_source_types();
    if !known.iter().any(|candidate| candidate == &kind) {
        return api_error_code(
            StatusCode::BAD_REQUEST,
            code::INVALID_ARGUMENT,
            &format!(
                "Unknown connector type '{kind}'. Known types: {}.",
                known.join(", ")
            ),
        );
    }
    let now = crate::api::utils::datetime::now_ms();
    body.id = if body.id.trim().is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        body.id
    };
    body.name = name;
    body.source_type = kind;
    body.owner_id = auth.user_id.clone();
    body.created_at = now;
    body.updated_at = now;
    body.last_test = None;
    let id = body.id.clone();
    if let Err(error) = state.connectors.put(body.clone()) {
        return api_error_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        );
    }
    if let Err(error) = state.connectors.log(
        &id,
        "created",
        "info",
        &format!("Connector '{}' created", body.name),
    ) {
        tracing::warn!(%error, "Failed to persist the connector creation log");
    }
    Json(serde_json::json!({ "code": 0, "data": body, "message": "success" })).into_response()
}

/// `GET /api/v1/connectors/{connector_id}`.
pub async fn get_connector(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(connector_id): Path<String>,
) -> Response {
    match state
        .connectors
        .get_for(&connector_id, &auth.user_id, auth.is_admin)
    {
        Some(connector) => Json(serde_json::json!({
            "code": 0,
            "data": connector,
            "message": "success",
        }))
        .into_response(),
        None => not_found("Connector not found."),
    }
}

/// `PATCH /api/v1/connectors/{connector_id}`.
pub async fn patch_connector(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(connector_id): Path<String>,
    Json(body): Json<ConnectorPatch>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(mut connector) = state
        .connectors
        .get_for(&connector_id, &auth.user_id, auth.is_admin)
    else {
        return not_found("Connector not found.");
    };
    if let Some(name) = body.name.as_deref() {
        let name = name.trim();
        if name.is_empty() {
            return api_error_code(
                StatusCode::BAD_REQUEST,
                code::INVALID_ARGUMENT,
                "Connector name is required.",
            );
        }
        connector.name = name.to_string();
    }
    if let Some(enabled) = body.enabled {
        connector.enabled = enabled;
    }
    if let Some(config) = body.config {
        connector.config = config;
    }
    connector.updated_at = crate::api::utils::datetime::now_ms();
    if let Err(error) = state.connectors.put(connector.clone()) {
        return api_error_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        );
    }
    if let Err(error) = state
        .connectors
        .log(&connector_id, "updated", "info", "Connector updated")
    {
        tracing::warn!(%error, "Failed to persist the connector update log");
    }
    Json(serde_json::json!({ "code": 0, "data": connector, "message": "success" })).into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct ConnectorPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
}

/// `DELETE /api/v1/connectors/{connector_id}`.
pub async fn delete_connector(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(connector_id): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    if state
        .connectors
        .get_for(&connector_id, &auth.user_id, auth.is_admin)
        .is_none()
    {
        return not_found("Connector not found.");
    }
    match state.connectors.delete(&connector_id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true, "message": "success" }))
            .into_response(),
        Ok(false) => not_found("Connector not found."),
        Err(error) => api_error_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        ),
    }
}

/// `GET /api/v1/connectors/{connector_id}/logs`.
pub async fn connector_logs(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(connector_id): Path<String>,
    Query(query): Query<ConnectorLogQuery>,
) -> Response {
    if state
        .connectors
        .get_for(&connector_id, &auth.user_id, auth.is_admin)
        .is_none()
    {
        return not_found("Connector not found.");
    }
    let limit = query.limit.unwrap_or(50);
    let logs = state.connectors.logs_for(&connector_id, limit);
    Json(serde_json::json!({
        "code": 0,
        "data": { "logs": logs, "total": state.connectors.log_count(&connector_id) },
        "message": "success",
    }))
    .into_response()
}

/// `POST /api/v1/connectors/{connector_id}/test`.
///
/// This really builds the connector and lists the source, so "test passed" means the remote side
/// answered. A type with no builder says so instead of reporting success.
pub async fn test_connector(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(connector_id): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(mut connector) = state
        .connectors
        .get_for(&connector_id, &auth.user_id, auth.is_admin)
    else {
        return not_found("Connector not found.");
    };
    let started = std::time::Instant::now();
    let result = probe_connector(&connector).await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let outcome = match result {
        Ok(files) => serde_json::json!({
            "ok": true,
            "files": files,
            "elapsed_ms": elapsed_ms,
            "message": format!("Connected and listed {files} file(s)."),
        }),
        Err(error) => serde_json::json!({
            "ok": false,
            "files": 0,
            "elapsed_ms": elapsed_ms,
            "message": error.to_string(),
        }),
    };
    connector.last_test = Some(outcome.clone());
    connector.updated_at = crate::api::utils::datetime::now_ms();
    if let Err(error) = state.connectors.put(connector.clone()) {
        tracing::warn!(%error, "Failed to persist the connector test result");
    }
    let (event, level, message) = match outcome["ok"].as_bool().unwrap_or(false) {
        true => (
            "test",
            "info",
            outcome["message"].as_str().unwrap_or_default().to_string(),
        ),
        false => (
            "test",
            "error",
            outcome["message"].as_str().unwrap_or_default().to_string(),
        ),
    };
    if let Err(error) = state.connectors.log(&connector_id, event, level, &message) {
        tracing::warn!(%error, "Failed to persist the connector test log");
    }
    Json(serde_json::json!({ "code": 0, "data": outcome, "message": "success" })).into_response()
}

/// Build the connector and list the source, under the deployment's command timeout.
async fn probe_connector(connector: &Connector) -> anyhow::Result<usize> {
    let kind = connector.source_type.as_str();
    let options = source_options(&connector.config);
    let mut connector_impl = crate::connectors::ConnectorRegistry::create(kind, options)
        .map_err(|error| anyhow::anyhow!("Cannot build a '{kind}' connector: {error}"))?;
    let timeout = crate::common::cmd_timeout::duration();
    tokio::time::timeout(timeout, async {
        connector_impl.load_credentials().await?;
        connector_impl.list_files().await
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "The source did not answer within {} seconds.",
            timeout.as_secs()
        )
    })?
    .map(|files| files.len())
}

/// `POST /api/v1/connectors/{connector_id}/rebuild`.
///
/// It reports what the source currently offers rather than claiming that documents were re-indexed:
/// importing them is the parse pipeline's job, and saying "rebuilt 12 documents" when nothing was
/// written is exactly the kind of silent lie this project keeps eliminating.
pub async fn rebuild_connector(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(connector_id): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    let Some(connector) = state
        .connectors
        .get_for(&connector_id, &auth.user_id, auth.is_admin)
    else {
        return not_found("Connector not found.");
    };
    if !connector.enabled {
        return data_error("This connector is disabled; enable it before rebuilding.");
    }
    let started = std::time::Instant::now();
    match probe_connector(&connector).await {
        Ok(files) => {
            let elapsed_ms = started.elapsed().as_millis() as u64;
            let message = format!(
                "Scanned {files} file(s) in {elapsed_ms} ms. Importing them is the parse pipeline's job, so no document was written by this call."
            );
            if let Err(error) = state
                .connectors
                .log(&connector_id, "rebuild", "info", &message)
            {
                tracing::warn!(%error, "Failed to persist the connector rebuild log");
            }
            Json(serde_json::json!({
                "code": 0,
                "data": { "scanned": files, "imported": 0, "elapsed_ms": elapsed_ms },
                "message": message,
            }))
            .into_response()
        }
        Err(error) => {
            let message = format!("Rebuild failed: {error}");
            if let Err(log_error) =
                state
                    .connectors
                    .log(&connector_id, "rebuild", "error", &message)
            {
                tracing::warn!(%log_error, "Failed to persist the connector rebuild log");
            }
            data_error(&message)
        }
    }
}

// ── MCP Server ─────────────────────────────────────────────────

#[derive(Serialize)]
pub struct McpServer {
    pub name: String,
    pub version: String,
    pub tools: Vec<McpTool>,
}

#[derive(Serialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// GET /api/v1/mcp/tools — the MCP tool listing, built from the registry.
///
/// This used to answer with two invented tools (`search_knowledge`, `list_datasets`) that no code could
/// execute, while `mcp_server` held the real catalogue. Both this listing and
/// `GET /api/v1/plugin/tools` now read that one registry.
pub async fn mcp_tools(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    let tools = crate::api::plugin_tools::catalogue(&state, &auth);
    Json(serde_json::json!({
        "code": 0,
        "data": McpServer {
            name: "RayRAG MCP".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            tools: tools
                .into_iter()
                .map(|tool| McpTool {
                    name: tool.function.name,
                    description: tool.function.description,
                    parameters: tool.function.parameters,
                })
                .collect(),
        },
    }))
    .into_response()
}

// ── Bot / Slack Integration ────────────────────────────────────

#[derive(Serialize)]
pub struct BotChannel {
    pub id: String,
    pub platform: String,
    pub name: String,
    pub enabled: bool,
    pub webhook_url: Option<String>,
}

/// The channel map inside the configuration: `channels_config_from_env()` wraps its entries in a
/// `channels` key (the shape the runtime consumes), so reading the top level would report one channel
/// literally called `channels` — which is what the first version of this endpoint did.
pub(crate) fn channel_entries(config: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    let map = config
        .get("channels")
        .and_then(|value| value.as_object())
        .or_else(|| config.as_object());
    map.map(|channels| {
        channels
            .iter()
            .map(|(id, value)| (id.clone(), value.clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// Channel rows for the API, from the configuration as it really is.
pub(crate) fn channel_rows_from_config(config: &serde_json::Value) -> Vec<serde_json::Value> {
    channel_entries(config)
        .into_iter()
        .map(|(id, value)| {
            let enabled = value
                .get("enabled")
                .and_then(|flag| flag.as_bool())
                .unwrap_or(true);
            serde_json::json!({
                "id": id,
                "name": channel_display_name(&id),
                "channel_type": id,
                "enabled": enabled,
                "config": value,
            })
        })
        .collect()
}

/// Bot rows: a configured channel that carries an outbound `callback_url`.
pub(crate) fn bot_rows_from_config(config: &serde_json::Value) -> Vec<serde_json::Value> {
    channel_entries(config)
        .into_iter()
        .filter_map(|(id, value)| {
            let callback = value.get("callback_url").and_then(|url| url.as_str())?;
            Some(serde_json::json!({
                "id": format!("{id}-local"),
                "platform": id,
                "name": format!("{} Bot", channel_display_name(&id)),
                "enabled": value.get("enabled").and_then(|flag| flag.as_bool()).unwrap_or(true),
                "webhook_url": callback,
            }))
        })
        .collect()
}

/// `GET /api/v1/bots` — the bots wired to this deployment.
///
/// It used to answer with two invented rows (`slack-local`, `discord-local`). A bot here is a channel
/// that carries an outbound callback, which is what makes it a bot rather than a plain channel — so the
/// list is derived from the same configuration and cannot disagree with it.
pub async fn list_bots() -> Response {
    let rows = bot_rows_from_config(&crate::channels::channels_config_from_env());
    Json(serde_json::json!({
        "code": 0,
        "data": rows,
        "total": rows.len(),
        "note": "A bot is a configured channel with an outbound callback_url.",
    }))
    .into_response()
}

// ── Chat Channel Management ────────────────────────────────────

#[derive(Serialize)]
pub struct ChatChannel {
    pub id: String,
    pub name: String,
    pub channel_type: String,
    pub enabled: bool,
    pub config: serde_json::Value,
}

/// `GET /api/v1/channels` — the chat channels this deployment is configured with.
///
/// It used to answer with two invented rows (`feishu`, `webchat`) whatever the deployment had. The
/// source is now `channels_config_from_env()`, the same configuration the channel runtime reads, so a
/// channel appears here exactly when a message sent to it would be handled.
pub async fn list_channels() -> Response {
    let rows = channel_rows_from_config(&crate::channels::channels_config_from_env());
    Json(serde_json::json!({
        "code": 0,
        "data": rows,
        "total": rows.len(),
        // A channel that is not configured cannot appear, so an empty list has a plain explanation
        // rather than looking like a broken page.
        "note": "Channels come from RAYRAG_CHANNELS_CONFIG or the per-channel environment variables.",
    }))
    .into_response()
}

/// A display name for a channel id, capitalised rather than invented.
fn channel_display_name(id: &str) -> String {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
        None => id.to_string(),
    }
}

// ── Plugin System ──────────────────────────────────────────────

#[derive(Serialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
}

// ── MCP SSE Server surface (mcp/server/server.py) ─────────────────────────

/// GET /api/v1/mcp/server — capability info of the embedded MCP SSE server:
/// protocol version, transports and endpoints.
pub async fn mcp_server_info() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": {
            "name": crate::mcp_server::SERVER_NAME,
            "version": crate::mcp_server::SERVER_VERSION,
            "protocolVersion": crate::mcp_client::PROTOCOL_VERSION,
            "transports": ["sse", "streamable-http"],
            "endpoints": {
                "sse": "/sse",
                "messages": "/messages/",
            },
        }
    }))
}

/// GET /api/v1/mcp/tools/full — the full MCP tool registry served by the
/// SSE server (includes `ragflow_retrieval` from mcp/server/server.py).
pub async fn mcp_tools_full() -> impl IntoResponse {
    let registry = crate::mcp_server::ToolRegistry::default();
    Json(serde_json::json!({
        "code": 0,
        "data": McpServer {
            name: crate::mcp_server::SERVER_NAME.into(),
            version: crate::mcp_server::SERVER_VERSION.into(),
            tools: registry
                .list()
                .iter()
                .map(|tool| McpTool {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.input_schema.clone(),
                })
                .collect::<Vec<_>>(),
        }
    }))
}

#[cfg(test)]
mod channel_config_tests {
    use super::*;

    #[test]
    fn both_configuration_shapes_are_read_and_the_wrapper_is_not_a_channel() {
        // The shape `channels_config_from_env()` produces.
        let wrapped = serde_json::json!({
            "channels": {
                "feishu": {"enabled": true, "app_id": "a"},
                "webhook": {"enabled": false, "callback_url": "https://example.com/hook"}
            }
        });
        assert_eq!(channel_entries(&wrapped).len(), 2);
        let rows = channel_rows_from_config(&wrapped);
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter().all(|row| row["id"] != "channels"),
            "the wrapper key must not be reported as a channel: {rows:?}"
        );
        assert!(
            rows.iter()
                .any(|row| row["id"] == "feishu" && row["enabled"] == true)
        );
        assert!(
            rows.iter()
                .any(|row| row["id"] == "webhook" && row["enabled"] == false)
        );

        // The unwrapped shape, which RAYRAG_CHANNELS_CONFIG may be given as.
        let plain = serde_json::json!({"feishu": {"enabled": true}});
        let rows = channel_rows_from_config(&plain);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "feishu");

        // Nothing configured means nothing claimed.
        assert!(channel_rows_from_config(&serde_json::json!({})).is_empty());
        assert!(channel_rows_from_config(&serde_json::json!({"channels": {}})).is_empty());
    }

    #[test]
    fn a_bot_is_a_channel_with_a_callback() {
        let config = serde_json::json!({
            "channels": {
                "webhook": {"callback_url": "https://example.com/hook", "enabled": true},
                "feishu": {"app_id": "a", "app_secret": "b"}
            }
        });
        let bots = bot_rows_from_config(&config);
        assert_eq!(
            bots.len(),
            1,
            "only the callback channel is a bot: {bots:?}"
        );
        assert_eq!(bots[0]["platform"], "webhook");
        assert_eq!(bots[0]["webhook_url"], "https://example.com/hook");
        assert_eq!(bots[0]["id"], "webhook-local");
    }

    #[test]
    fn a_display_name_is_capitalised_not_invented() {
        assert_eq!(channel_display_name("feishu"), "Feishu");
        assert_eq!(channel_display_name("webhook"), "Webhook");
        assert_eq!(channel_display_name(""), "");
    }
}

#[cfg(test)]
mod connector_store_tests {
    use super::*;

    fn sample(id: &str, owner: &str) -> Connector {
        Connector {
            id: id.into(),
            name: "files".into(),
            source_type: "local".into(),
            enabled: true,
            config: serde_json::json!({"path": "/tmp"}),
            owner_id: owner.into(),
            created_at: 1,
            updated_at: 1,
            last_test: None,
        }
    }

    #[test]
    fn the_list_is_scoped_to_the_owner_unless_the_caller_administers_the_deployment() {
        let store = ConnectorStore::in_memory();
        store.put(sample("a", "user-1")).unwrap();
        store.put(sample("b", "user-2")).unwrap();
        assert_eq!(store.list("user-1", false).len(), 1);
        assert_eq!(store.list("user-1", false)[0].id, "a");
        assert_eq!(store.list("user-1", true).len(), 2);
        // A connector another user owns is not readable, and reads as absent rather than forbidden,
        // which is what an unknown id reads as too.
        assert!(store.get_for("b", "user-1", false).is_none());
        assert!(store.get_for("b", "user-1", true).is_some());
        assert!(store.get_for("missing", "user-1", true).is_none());
    }

    #[test]
    fn deleting_a_connector_takes_its_history_with_it() {
        let store = ConnectorStore::in_memory();
        store.put(sample("a", "user-1")).unwrap();
        store.log("a", "created", "info", "created").unwrap();
        store.log("a", "test", "error", "refused").unwrap();
        assert_eq!(store.log_count("a"), 2);
        // Newest first, so the most recent failure is the first thing a user sees.
        assert_eq!(store.logs_for("a", 10)[0].message, "refused");
        assert!(store.delete("a").unwrap());
        assert_eq!(store.log_count("a"), 0);
        assert!(!store.delete("a").unwrap());
    }

    #[test]
    fn the_log_is_bounded_so_a_retried_connector_cannot_grow_without_end() {
        let store = ConnectorStore::in_memory();
        store.put(sample("a", "user-1")).unwrap();
        for index in 0..(MAX_LOG_LINES + 25) {
            store
                .log("a", "test", "info", &format!("line {index}"))
                .unwrap();
        }
        assert_eq!(store.log_count("a"), MAX_LOG_LINES);
        // The oldest lines are the ones dropped.
        assert_eq!(
            store.logs_for("a", 5)[0].message,
            format!("line {}", MAX_LOG_LINES + 24)
        );
        // One response is clamped to 500 lines even though 2000 are kept, so a caller cannot pull the
        // whole history into memory by asking for it.
        assert_eq!(store.logs_for("a", 5_000).len(), 500);
        assert_eq!(
            store.logs_for("a", 5_000).last().unwrap().message,
            format!("line {}", MAX_LOG_LINES - 500 + 25)
        );
    }

    #[test]
    fn the_known_types_come_from_the_registry_rather_than_a_second_list() {
        let known = known_source_types();
        assert!(known.iter().any(|kind| kind == "local"), "{known:?}");
        assert!(known.iter().any(|kind| kind == "webdav"), "{known:?}");
        // A type nobody implements is not offered, so validation cannot accept one.
        assert!(
            !known.iter().any(|kind| kind == "not-a-real-source"),
            "{known:?}"
        );
    }

    #[test]
    fn configuration_is_read_under_both_the_upstream_and_the_local_key_names() {
        // `path` is the friendly alias for the source location, and the registry reads it from `url`.
        let options =
            source_options(&serde_json::json!({"path": "/srv/data", "api_key": "secret"}));
        assert_eq!(options.url, "/srv/data");
        assert_eq!(options.token, "secret");
        assert!(options.target.is_empty());
        let options =
            source_options(&serde_json::json!({"token": "t2", "target": "sub", "subdir": "x"}));
        assert_eq!(options.token, "t2");
        assert_eq!(options.target, "sub");
        // An explicit url wins over the alias, so a caller that sets both is not second-guessed.
        let options = source_options(&serde_json::json!({"url": "/explicit", "path": "/alias"}));
        assert_eq!(options.url, "/explicit");
        // Absent keys are empty rather than guessed.
        let options = source_options(&serde_json::json!({}));
        assert!(options.target.is_empty() && options.token.is_empty() && options.url.is_empty());
    }
}
