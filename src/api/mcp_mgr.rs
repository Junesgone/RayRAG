//! Tenant-scoped MCP server management.
//!
//! Mirrors the fixed RAGFlow v0.26.4 `mcp_api.py` wire contract:
//! `GET/POST /api/v1/mcp/servers`, `GET/PUT/DELETE /api/v1/mcp/servers/{id}`,
//! `POST /api/v1/mcp/servers/import` and `POST /api/v1/mcp/servers/{id}/test`.
//! Create/update/import connect to the server with the bundled streamable-HTTP
//! MCP client (`initialize` + `tools/list`) and cache the returned tool map in
//! `variables.tools`, failing the save with the upstream message when the
//! endpoint is unreachable. URLs are SSRF-checked with the same global-only
//! address policy as `assert_url_is_safe`; `ALLOW_ANY_HOST=1` restores access
//! to LAN/loopback servers for mainland self-hosted deployments.

use crate::api::joint_services::{McpServerRecord, mcp_server_exists, mcp_server_get_servers};
use crate::server::{AppState, AuthContext};
use anyhow::{Context, bail};
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

const VALID_MCP_SERVER_TYPES: [&str; 2] = ["sse", "streamable-http"];
const MCP_PROBE_SECONDS: u64 = 10;

pub struct McpServerStore {
    rows: RwLock<Vec<McpServerRecord>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl McpServerStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = PathBuf::from(path);
        crate::persistence::restore_if_missing(&path)?;
        let rows =
            crate::persistence::load_json::<Vec<McpServerRecord>>(&path)?.unwrap_or_default();
        Ok(Self {
            rows: RwLock::new(rows),
            path: Some(path),
            save_lock: Mutex::new(()),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            rows: RwLock::new(Vec::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn list(&self, tenant_id: &str) -> Vec<McpServerRecord> {
        self.rows
            .read()
            .unwrap()
            .iter()
            .filter(|row| row.tenant_id == tenant_id)
            .cloned()
            .collect()
    }

    pub fn get(&self, tenant_id: &str, id: &str) -> Option<McpServerRecord> {
        self.rows
            .read()
            .unwrap()
            .iter()
            .find(|row| row.tenant_id == tenant_id && row.id == id)
            .cloned()
    }

    pub fn insert(&self, row: McpServerRecord) -> anyhow::Result<()> {
        {
            let mut rows = self.rows.write().unwrap();
            rows.retain(|existing| !(existing.tenant_id == row.tenant_id && existing.id == row.id));
            rows.push(row);
        }
        self.persist()
    }

    pub fn update(&self, tenant_id: &str, id: &str, row: McpServerRecord) -> anyhow::Result<bool> {
        let changed = {
            let mut rows = self.rows.write().unwrap();
            match rows
                .iter_mut()
                .find(|existing| existing.tenant_id == tenant_id && existing.id == id)
            {
                Some(slot) => {
                    *slot = row;
                    true
                }
                None => false,
            }
        };
        if changed {
            self.persist()?;
        }
        Ok(changed)
    }

    pub fn delete(&self, tenant_id: &str, id: &str) -> anyhow::Result<bool> {
        let removed = {
            let mut rows = self.rows.write().unwrap();
            let before = rows.len();
            rows.retain(|row| !(row.tenant_id == tenant_id && row.id == id));
            rows.len() != before
        };
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    fn persist(&self) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let _guard = self.save_lock.lock().unwrap();
        let rows: Vec<_> = self.rows.read().unwrap().clone();
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(&rows)?)
            .with_context(|| format!("Failed to persist MCP servers '{}':", path.display()))
    }
}

#[derive(Debug, Deserialize)]
pub struct McpListQuery {
    #[serde(default)]
    pub keywords: Option<String>,
    #[serde(default)]
    pub page: Option<usize>,
    #[serde(default)]
    pub page_size: Option<usize>,
    #[serde(default)]
    pub orderby: Option<String>,
    #[serde(default)]
    pub desc: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct McpGetQuery {
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct McpUpsertRequest {
    pub name: String,
    pub url: String,
    pub server_type: String,
    #[serde(default)]
    pub authorization_token: Option<String>,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    #[serde(default)]
    pub variables: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct McpImportRequest {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct McpTestRequest {
    pub url: String,
    pub server_type: String,
    #[serde(default)]
    pub headers: Option<serde_json::Value>,
    #[serde(default)]
    pub variables: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct McpImportResult {
    server: String,
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn mcp_error(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 102, "message": message })),
    )
        .into_response()
}

fn allow_any_host() -> bool {
    std::env::var("ALLOW_ANY_HOST")
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

/// Upstream `assert_url_is_safe`: http/https scheme, present host, and every
/// resolved address globally routable unless `ALLOW_ANY_HOST` is enabled.
fn validate_mcp_url(url: &str) -> anyhow::Result<()> {
    validate_mcp_url_with_bypass(url, allow_any_host())
}

fn validate_mcp_url_with_bypass(url: &str, allow_private: bool) -> anyhow::Result<()> {
    let parsed = url::Url::parse(url.trim()).context("Invalid MCP url.")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("Invalid MCP url.");
    }
    let host = parsed.host_str().context("Invalid MCP url.")?.to_string();
    if allow_private {
        return Ok(());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if crate::connectors::is_private_ip(ip) {
            bail!("Invalid MCP url.");
        }
        return Ok(());
    }
    use std::net::ToSocketAddrs;
    let addrs: Vec<std::net::SocketAddr> = (host.as_str(), 80)
        .to_socket_addrs()
        .context("Invalid MCP url.")?
        .collect();
    if let Some(bad) = addrs
        .iter()
        .find(|addr| crate::connectors::is_private_ip(addr.ip()))
    {
        let _ = bad;
        bail!("Invalid MCP url.");
    }
    Ok(())
}

fn validate_mcp_name(name: &str) -> anyhow::Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        });
    if !valid {
        bail!(
            "It must be 1-64 characters long and can only contain letters, numbers, hyphens, and underscores."
        );
    }
    Ok(())
}

fn validate_server_type(server_type: &str) -> anyhow::Result<()> {
    if !VALID_MCP_SERVER_TYPES.contains(&server_type) {
        bail!("Unsupported MCP server type.");
    }
    Ok(())
}

/// Connect with the streamable-HTTP MCP client and pull the tool catalogue.
/// `authorization_token` is attached as `Authorization: Bearer <token>`.
pub async fn fetch_mcp_tools(
    url: &str,
    authorization_token: Option<&str>,
) -> anyhow::Result<Vec<crate::mcp_client::McpTool>> {
    let mut headers = BTreeMap::new();
    if let Some(token) = authorization_token.filter(|token| !token.trim().is_empty()) {
        headers.insert(
            "Authorization".to_string(),
            format!("Bearer {}", token.trim()),
        );
    }
    let client = crate::mcp_client::McpClient::new(crate::mcp_client::McpTransport::Http {
        url: url.trim().to_string(),
        headers,
    });
    let _init = tokio::time::timeout(Duration::from_secs(MCP_PROBE_SECONDS), client.initialize())
        .await
        .context("MCP initialize timed out")??;
    let tools = tokio::time::timeout(Duration::from_secs(MCP_PROBE_SECONDS), client.list_tools())
        .await
        .context("MCP tools/list timed out")??;
    Ok(tools)
}

fn tools_object(
    tools: &[crate::mcp_client::McpTool],
) -> serde_json::Map<String, serde_json::Value> {
    tools
        .iter()
        .filter_map(|tool| {
            serde_json::to_value(tool)
                .ok()
                .map(|value| (tool.name.clone(), value))
        })
        .collect()
}

fn export_payload(rows: &[McpServerRecord]) -> serde_json::Value {
    let servers: serde_json::Map<_, _> = rows
        .iter()
        .map(|row| {
            let tools = row
                .variables
                .get("tools")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let token = row
                .variables
                .get("authorization_token")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            (
                row.name.clone(),
                serde_json::json!({
                    "type": row.server_type,
                    "url": row.url,
                    "name": row.name,
                    "authorization_token": token,
                    "tools": tools,
                }),
            )
        })
        .collect();
    serde_json::json!({ "mcpServers": servers })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// GET `/api/v1/mcp/servers` — filter, order and paginate the tenant list.
pub async fn list_mcp_servers(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<McpListQuery>,
) -> Response {
    let rows = state.mcp_servers.list(&auth.user_id);
    let keywords = query
        .keywords
        .as_deref()
        .filter(|value| !value.trim().is_empty());
    let orderby = query.orderby.as_deref().unwrap_or("create_time");
    let desc = !matches!(query.desc.as_deref(), Some("false"));
    let filtered = mcp_server_get_servers(
        &rows,
        &auth.user_id,
        None,
        keywords,
        orderby,
        desc,
        None,
        None,
    );
    let total = filtered.len();
    let page = query.page.unwrap_or(0);
    let page_size = query.page_size.unwrap_or(0);
    let page_rows = if page > 0 && page_size > 0 {
        let start = (page - 1) * page_size;
        filtered.into_iter().skip(start).take(page_size).collect()
    } else {
        filtered
    };
    Json(serde_json::json!({
        "code": 0,
        "data": { "mcp_servers": page_rows, "total": total }
    }))
    .into_response()
}

/// GET `/api/v1/mcp/servers/{id}` — detail, or `?mode=download` export.
pub async fn get_mcp_server(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<McpGetQuery>,
) -> Response {
    let Some(row) = state.mcp_servers.get(&auth.user_id, &id) else {
        return mcp_error(&format!(
            "Cannot find MCP server {id} for user {}",
            auth.user_id
        ));
    };
    if query.mode.as_deref() == Some("download") {
        return Json(serde_json::json!({ "code": 0, "data": export_payload(&[row]) }))
            .into_response();
    }
    Json(serde_json::json!({ "code": 0, "data": row })).into_response()
}

/// POST `/api/v1/mcp/servers` — validate, probe and create one server.
pub async fn create_mcp_server(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<McpUpsertRequest>,
) -> Response {
    if let Err(error) = validate_server_type(&request.server_type) {
        return mcp_error(&error.to_string());
    }
    if let Err(error) = validate_mcp_name(&request.name) {
        return mcp_error(&error.to_string());
    }
    if mcp_server_exists(
        &state.mcp_servers.list(&auth.user_id),
        &request.name,
        &auth.user_id,
    ) {
        return mcp_error("Duplicated MCP server name.");
    }
    if let Err(error) = validate_mcp_url(&request.url) {
        return mcp_error(&error.to_string());
    }
    let authorization_token = request
        .variables
        .as_ref()
        .and_then(|variables| variables.get("authorization_token"))
        .and_then(serde_json::Value::as_str)
        .or(request.authorization_token.as_deref())
        .map(str::to_string)
        .unwrap_or_default();
    let tools = match fetch_mcp_tools(&request.url, Some(&authorization_token)).await {
        Ok(tools) => tools,
        Err(error) => return mcp_error(&format!("Failed to connect to MCP server: {error}")),
    };
    let mut variables = request.variables.unwrap_or_else(|| serde_json::json!({}));
    variables["authorization_token"] = serde_json::json!(authorization_token);
    variables["tools"] = serde_json::json!(tools_object(&tools));
    let now = now_ms();
    let row = McpServerRecord {
        id: uuid::Uuid::new_v4().to_string(),
        name: request.name,
        server_type: request.server_type,
        url: request.url,
        description: String::new(),
        variables,
        tenant_id: auth.user_id.clone(),
        create_time: now,
        update_time: now,
    };
    match state.mcp_servers.insert(row.clone()) {
        Ok(()) => Json(serde_json::json!({ "code": 0, "data": row })).into_response(),
        Err(error) => mcp_error(&error.to_string()),
    }
}

/// PUT `/api/v1/mcp/servers/{id}` — validate, probe and update one server.
pub async fn update_mcp_server(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(request): Json<McpUpsertRequest>,
) -> Response {
    let Some(existing) = state.mcp_servers.get(&auth.user_id, &id) else {
        return mcp_error(&format!(
            "Cannot find MCP server {id} for user {}",
            auth.user_id
        ));
    };
    let name = if request.name.is_empty() {
        existing.name.clone()
    } else {
        request.name.clone()
    };
    let server_type = if request.server_type.is_empty() {
        existing.server_type.clone()
    } else {
        request.server_type.clone()
    };
    let url = if request.url.is_empty() {
        existing.url.clone()
    } else {
        request.url.clone()
    };
    if let Err(error) = validate_server_type(&server_type) {
        return mcp_error(&error.to_string());
    }
    if let Err(error) = validate_mcp_name(&name) {
        return mcp_error(&error.to_string());
    }
    if name != existing.name
        && mcp_server_exists(&state.mcp_servers.list(&auth.user_id), &name, &auth.user_id)
    {
        return mcp_error("Duplicated MCP server name.");
    }
    if let Err(error) = validate_mcp_url(&url) {
        return mcp_error(&error.to_string());
    }
    let authorization_token = request
        .variables
        .as_ref()
        .and_then(|variables| variables.get("authorization_token"))
        .and_then(serde_json::Value::as_str)
        .or(request.authorization_token.as_deref())
        .map(str::to_string)
        .unwrap_or_default();
    let tools = match fetch_mcp_tools(&url, Some(&authorization_token)).await {
        Ok(tools) => tools,
        Err(error) => return mcp_error(&format!("Failed to connect to MCP server: {error}")),
    };
    let mut variables = request.variables.unwrap_or_else(|| serde_json::json!({}));
    variables["authorization_token"] = serde_json::json!(authorization_token);
    variables["tools"] = serde_json::json!(tools_object(&tools));
    let row = McpServerRecord {
        id: id.clone(),
        name,
        server_type,
        url,
        description: existing.description,
        variables,
        tenant_id: auth.user_id.clone(),
        create_time: existing.create_time,
        update_time: now_ms(),
    };
    match state.mcp_servers.update(&auth.user_id, &id, row.clone()) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": row })).into_response(),
        Ok(false) => mcp_error(&format!(
            "Cannot find MCP server {id} for user {}",
            auth.user_id
        )),
        Err(error) => mcp_error(&error.to_string()),
    }
}

/// DELETE `/api/v1/mcp/servers/{id}`.
pub async fn delete_mcp_server(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    match state.mcp_servers.delete(&auth.user_id, &id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => mcp_error(&format!(
            "Cannot find MCP server {id} for user {}",
            auth.user_id
        )),
        Err(error) => mcp_error(&error.to_string()),
    }
}

/// POST `/api/v1/mcp/servers/import` — create many servers from an exported
/// `{mcpServers:{name:{type,url,authorization_token}}}` payload, renaming
/// duplicates with an `_N` suffix like upstream.
pub async fn import_mcp_servers(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<McpImportRequest>,
) -> Response {
    let mut results = Vec::new();
    for (server_name, config) in &request.mcp_servers {
        let object = config.as_object();
        let kind = object
            .and_then(|config| config.get("type"))
            .and_then(serde_json::Value::as_str);
        let url = object
            .and_then(|config| config.get("url"))
            .and_then(serde_json::Value::as_str);
        let (Some(kind), Some(url)) = (kind, url) else {
            results.push(McpImportResult {
                server: server_name.clone(),
                success: false,
                action: None,
                id: None,
                new_name: None,
                message: Some("Missing required fields (type or url)".into()),
            });
            continue;
        };
        if let Err(error) = validate_server_type(kind) {
            results.push(McpImportResult {
                server: server_name.clone(),
                success: false,
                action: None,
                id: None,
                new_name: None,
                message: Some(error.to_string()),
            });
            continue;
        }
        if let Err(error) = validate_mcp_name(server_name) {
            results.push(McpImportResult {
                server: server_name.clone(),
                success: false,
                action: None,
                id: None,
                new_name: None,
                message: Some(error.to_string()),
            });
            continue;
        }
        if let Err(error) = validate_mcp_url(url) {
            results.push(McpImportResult {
                server: server_name.clone(),
                success: false,
                action: None,
                id: None,
                new_name: None,
                message: Some(error.to_string()),
            });
            continue;
        }
        let token = object
            .and_then(|config| config.get("authorization_token"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let existing = state.mcp_servers.list(&auth.user_id);
        let mut new_name = server_name.clone();
        let mut counter = 0;
        while mcp_server_exists(&existing, &new_name, &auth.user_id) {
            new_name = format!("{server_name}_{counter}");
            counter += 1;
        }
        let tools = match fetch_mcp_tools(url, Some(&token)).await {
            Ok(tools) => tools,
            Err(error) => {
                results.push(McpImportResult {
                    server: server_name.clone(),
                    success: false,
                    action: None,
                    id: None,
                    new_name: None,
                    message: Some(error.to_string()),
                });
                continue;
            }
        };
        let mut variables = serde_json::json!({ "authorization_token": token });
        variables["tools"] = serde_json::json!(tools_object(&tools));
        let now = now_ms();
        let id = uuid::Uuid::new_v4().to_string();
        let row = McpServerRecord {
            id: id.clone(),
            name: new_name.clone(),
            server_type: kind.to_string(),
            url: url.to_string(),
            description: String::new(),
            variables,
            tenant_id: auth.user_id.clone(),
            create_time: now,
            update_time: now,
        };
        match state.mcp_servers.insert(row) {
            Ok(()) => results.push(McpImportResult {
                server: server_name.clone(),
                success: true,
                action: Some("created".into()),
                id: Some(id),
                new_name: Some(new_name.clone()),
                message: (new_name != *server_name).then(|| {
                    format!("Renamed from '{server_name}' to '{new_name}' avoid duplication")
                }),
            }),
            Err(error) => results.push(McpImportResult {
                server: server_name.clone(),
                success: false,
                action: None,
                id: None,
                new_name: None,
                message: Some(error.to_string()),
            }),
        }
    }
    Json(serde_json::json!({ "code": 0, "data": { "results": results } })).into_response()
}

/// POST `/api/v1/mcp/servers/{id}/test` — probe a candidate server without
/// persisting it, returning the tool catalogue with `enabled=true`.
pub async fn test_mcp_server(
    State(_state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
    Path(_id): Path<String>,
    Json(request): Json<McpTestRequest>,
) -> Response {
    if let Err(error) = validate_server_type(&request.server_type) {
        return mcp_error(&error.to_string());
    }
    if let Err(error) = validate_mcp_url(&request.url) {
        return mcp_error(&error.to_string());
    }
    let token = request
        .variables
        .as_ref()
        .and_then(|variables| variables.get("authorization_token"))
        .and_then(serde_json::Value::as_str);
    match fetch_mcp_tools(&request.url, token).await {
        Ok(tools) => {
            let listed: Vec<serde_json::Value> = tools
                .iter()
                .map(|tool| {
                    let mut value =
                        serde_json::to_value(tool).unwrap_or_else(|_| serde_json::json!({}));
                    if let Some(object) = value.as_object_mut() {
                        object.insert("enabled".to_string(), serde_json::json!(true));
                    }
                    value
                })
                .collect();
            Json(serde_json::json!({ "code": 0, "data": listed })).into_response()
        }
        Err(error) => mcp_error(&format!("Test MCP error: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_is_tenant_scoped_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp.json").to_string_lossy().into_owned();
        let store = McpServerStore::new(&path).unwrap();
        let row = McpServerRecord {
            id: "mcp-1".into(),
            name: "search-server".into(),
            server_type: "streamable-http".into(),
            url: "https://mcp.example".into(),
            description: String::new(),
            variables: serde_json::json!({ "tools": {} }),
            tenant_id: "tenant-a".into(),
            create_time: 1,
            update_time: 1,
        };
        store.insert(row).unwrap();
        assert_eq!(store.list("tenant-a").len(), 1);
        assert!(store.list("tenant-b").is_empty());
        let reopened = McpServerStore::new(&path).unwrap();
        assert!(reopened.get("tenant-a", "mcp-1").is_some());
        assert!(reopened.get("tenant-b", "mcp-1").is_none());
        assert!(reopened.delete("tenant-a", "mcp-1").unwrap());
        assert!(
            McpServerStore::new(&path)
                .unwrap()
                .list("tenant-a")
                .is_empty()
        );
    }

    #[test]
    fn name_and_type_validation_match_the_fixed_contract() {
        assert!(validate_mcp_name("my-server_1").is_ok());
        assert!(validate_mcp_name("").is_err());
        assert!(validate_mcp_name("bad name").is_err());
        assert!(validate_mcp_name(&"x".repeat(65)).is_err());
        assert!(validate_server_type("sse").is_ok());
        assert!(validate_server_type("streamable-http").is_ok());
        assert!(validate_server_type("stdio").is_err());
    }

    #[test]
    fn url_guard_accepts_public_and_optional_lan_bypass() {
        assert!(validate_mcp_url_with_bypass("http://127.0.0.1:8080/mcp", true).is_ok());
        assert!(validate_mcp_url("ftp://mcp.example.com").is_err());
        assert!(validate_mcp_url("http://127.0.0.1:8080/mcp").is_err());
        assert!(validate_mcp_url("http://192.168.1.10/mcp").is_err());
    }

    #[tokio::test]
    async fn fetch_mcp_tools_initializes_and_lists_the_catalogue() {
        let app = axum::Router::new().route(
            "/mcp",
            axum::routing::post(|Json(body): Json<serde_json::Value>| async move {
                let id = body.get("id").cloned().unwrap_or(serde_json::json!(0));
                let method = body
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let result = match method {
                    "initialize" => serde_json::json!({
                        "protocolVersion": "2025-03-26",
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "audit-mcp", "version": "1.0.0" }
                    }),
                    "tools/list" => serde_json::json!({
                        "tools": [{
                            "name": "audit_tool",
                            "description": "Audit tool",
                            "inputSchema": { "type": "object" }
                        }]
                    }),
                    _ => serde_json::json!({}),
                };
                Json(serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let tools = fetch_mcp_tools(&format!("http://{address}/mcp"), Some("audit-token"))
            .await
            .unwrap();
        server.abort();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "audit_tool");
        assert_eq!(tools[0].description, "Audit tool");
    }
}
