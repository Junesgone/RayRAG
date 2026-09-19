//! Connector / MCP / Bot / Channel / Plugin APIs.
//! Replaces RAGFlow's connector_api, mcp_api, bot_api, chat_channel_api, plugin_api.

use axum::{
    Json,
    extract::{Extension, Path},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

use crate::server::AuthContext;

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

#[derive(Serialize, Deserialize)]
pub struct Connector {
    pub id: String,
    pub name: String,
    pub source_type: String,
    pub enabled: bool,
    pub config: serde_json::Value,
}

/// GET /api/v1/connectors
pub async fn list_connectors() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": [
            {"id":"local","name":"Local Files","source_type":"file","enabled":true},
            {"id":"web","name":"Web Crawler","source_type":"web","enabled":false},
            {"id":"database","name":"Database","source_type":"database","enabled":false},
        ]
    }))
}

/// POST /api/v1/connectors
pub async fn create_connector(
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<Connector>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    Json(serde_json::json!({"code":0,"data":body})).into_response()
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

/// GET /api/v1/mcp/tools — MCP tools listing
pub async fn mcp_tools() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": McpServer {
            name: "RayRAG MCP".into(),
            version: "0.1.0".into(),
            tools: vec![
                McpTool {
                    name: "search_knowledge".into(),
                    description: "Search RayRAG knowledge base with vector similarity".into(),
                    parameters: serde_json::json!({"type":"object","properties":{"query":{"type":"string"},"top_k":{"type":"integer","default":10}}}),
                },
                McpTool {
                    name: "list_datasets".into(),
                    description: "List available knowledge bases".into(),
                    parameters: serde_json::json!({"type":"object"}),
                },
            ],
        }
    }))
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

/// GET /api/v1/bots — list bot integrations
pub async fn list_bots() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": [
            {"id":"slack-local","platform":"slack","name":"Slack Bot","enabled":false,"webhook_url":null},
            {"id":"discord-local","platform":"discord","name":"Discord Bot","enabled":false,"webhook_url":null},
        ]
    }))
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

/// GET /api/v1/channels — list chat channels
pub async fn list_channels() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": [
            {"id":"feishu","name":"Feishu","channel_type":"feishu","enabled":true,"config":{}},
            {"id":"webchat","name":"Web Chat","channel_type":"web","enabled":true,"config":{}},
        ]
    }))
}

// ── Plugin System ──────────────────────────────────────────────

#[derive(Serialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub enabled: bool,
}

/// GET /api/v1/plugins — list plugins
pub async fn list_plugins() -> impl IntoResponse {
    Json(serde_json::json!({
        "code": 0,
        "data": [
            {"id":"ocr","name":"PaddleOCR","version":"1.0","enabled":true},
            {"id":"rerank","name":"mxbai-rerank","version":"1.0","enabled":true},
        ]
    }))
}

/// POST /api/v1/plugins/{id}/toggle — toggle plugin
pub async fn toggle_plugin(
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    if let Some(response) = require_admin(&auth) {
        return response;
    }
    Json(serde_json::json!({"code":0,"message":format!("Plugin {} toggled",id)})).into_response()
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
