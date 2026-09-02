//! MCP (Model Context Protocol) SSE server — Rust port of RAGFlow
//! `mcp/server/server.py`.
//!
//! RAGFlow's MCP server exposes a single `ragflow_retrieval` tool over the
//! MCP protocol using the official Python SDK (`Server`, `SseServerTransport`,
//! `StreamableHTTPSessionManager`). This module is a dependency-free Rust
//! port of that server side:
//!
//! - [`ToolRegistry`] — the tool catalogue (server.py `@app.list_tools`):
//!   `ragflow_retrieval` with the exact `inputSchema` and dataset-aware
//!   description, plus RayRAG's `search_knowledge` / `list_datasets`.
//! - [`McpServerCore`] — JSON-RPC 2.0 protocol handling (server.py
//!   `initialize` / `tools/list` / `tools/call` / notifications), pure and
//!   unit-testable without sockets.
//! - [`build_mcp_router`] — the SSE transport (server.py
//!   `SseServerTransport("/messages/")`): `GET /sse` opens an event stream
//!   and emits the `endpoint` event, `POST /messages/?session_id=…` accepts
//!   JSON-RPC messages and pushes `message` events back on the stream.
//! - [`extract_token_from_headers`] / [`resolve_api_key`] — server.py
//!   `_extract_token_from_headers` + `with_api_key` (self-host vs host mode).
//!
//! The wire protocol types are shared with the client half
//! ([`crate::mcp_client`]), so the client and server speak the exact same
//! framing.

use axum::{
    Json, Router,
    body::{Body, Bytes, to_bytes},
    extract::Query,
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    convert::Infallible,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};
use tokio::sync::mpsc;

use crate::mcp_client::{JsonRpcError, McpContent, McpTool, PROTOCOL_VERSION, method};

/// Server identity reported in the `initialize` handshake
/// (`Server("ragflow-mcp-server")`).
pub const SERVER_NAME: &str = "rayrag-mcp-server";
pub const SERVER_VERSION: &str = "0.1.0";

/// server.py defaults for the standalone launcher.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:9380";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 9382;
/// server.py `AUTH_TOKEN_STATE_KEY`.
pub const AUTH_TOKEN_STATE_KEY: &str = "ragflow_auth_token";

/// JSON-RPC 2.0 error codes (the MCP SDK maps its exceptions onto these).
pub mod error_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// A JSON-RPC 2.0 response the server can serialize (the client-side
/// [`crate::mcp_client::JsonRpcResponse`] is deserialize-only, so the server
/// carries its own Serialize mirror).
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponseBody {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponseBody {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ── Launch mode + configuration (server.py click options / env) ───────────

/// server.py `LaunchMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// `--mode=self-host`: one tenant, `HOST_API_KEY` is always used.
    SelfHost,
    /// `--mode=host`: multi-tenant; every request must carry an
    /// `Authorization: Bearer` / `x-api-key` header.
    Host,
}

impl LaunchMode {
    pub fn from_str(value: &str) -> Self {
        if value.eq_ignore_ascii_case("host") {
            Self::Host
        } else {
            Self::SelfHost
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SelfHost => "self-host",
            Self::Host => "host",
        }
    }
}

/// `mcp/server/server.py` configuration surface (`RAGFLOW_MCP_*` env vars).
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub base_url: String,
    pub host: String,
    pub port: u16,
    pub mode: LaunchMode,
    pub host_api_key: String,
    pub sse_enabled: bool,
    pub streamable_http_enabled: bool,
    pub json_response: bool,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.into(),
            host: DEFAULT_HOST.into(),
            port: DEFAULT_PORT,
            mode: LaunchMode::SelfHost,
            host_api_key: String::new(),
            sse_enabled: true,
            streamable_http_enabled: true,
            json_response: true,
        }
    }
}

impl McpServerConfig {
    /// `parse_bool_flag`: `1/true/yes/on` (case-insensitive) → true.
    pub fn parse_bool_flag(value: &str, default: bool) -> bool {
        let value = value.trim().to_ascii_lowercase();
        if matches!(value.as_str(), "1" | "true" | "yes" | "on") {
            true
        } else if matches!(value.as_str(), "0" | "false" | "no" | "off") {
            false
        } else {
            default
        }
    }

    /// Load from `RAGFLOW_MCP_*` environment variables, mirroring `main()`.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Ok(value) = std::env::var("RAGFLOW_MCP_BASE_URL") {
            config.base_url = value;
        }
        if let Ok(value) = std::env::var("RAGFLOW_MCP_HOST") {
            config.host = value;
        }
        if let Ok(value) = std::env::var("RAGFLOW_MCP_PORT") {
            config.port = value.parse().unwrap_or(DEFAULT_PORT);
        }
        if let Ok(value) = std::env::var("RAGFLOW_MCP_LAUNCH_MODE") {
            config.mode = LaunchMode::from_str(&value);
        }
        if let Ok(value) = std::env::var("RAGFLOW_MCP_HOST_API_KEY") {
            config.host_api_key = value;
        }
        config.sse_enabled = Self::parse_bool_flag(
            &std::env::var("RAGFLOW_MCP_TRANSPORT_SSE_ENABLED").unwrap_or_default(),
            true,
        );
        config.streamable_http_enabled = Self::parse_bool_flag(
            &std::env::var("RAGFLOW_MCP_TRANSPORT_STREAMABLE_ENABLED").unwrap_or_default(),
            true,
        );
        config.json_response = Self::parse_bool_flag(
            &std::env::var("RAGFLOW_MCP_JSON_RESPONSE").unwrap_or_default(),
            true,
        );
        // server.py: json-response is ignored when streamable HTTP is off.
        if !config.streamable_http_enabled {
            config.json_response = false;
        }
        config
    }
}

// ── Retrieval backend (server.py `RAGFlowConnector`) ──────────────────────

/// One accessible dataset row (`connector.list_datasets` → `{"id", "description"}`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatasetInfo {
    pub id: String,
    #[serde(default)]
    pub description: String,
}

/// `call_tool` arguments for the retrieval tools, mirroring the
/// `inputSchema` of `ragflow_retrieval`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalRequest {
    #[serde(default)]
    pub dataset_ids: Vec<String>,
    #[serde(default)]
    pub document_ids: Vec<String>,
    #[serde(default)]
    pub question: String,
    #[serde(default = "default_page")]
    pub page: usize,
    #[serde(default = "default_page_size")]
    pub page_size: usize,
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,
    #[serde(default = "default_vector_weight")]
    pub vector_similarity_weight: f64,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub keyword: bool,
    #[serde(default)]
    pub rerank_id: Option<String>,
    #[serde(default)]
    pub force_refresh: bool,
}

fn default_page() -> usize {
    1
}
fn default_page_size() -> usize {
    10
}
fn default_similarity_threshold() -> f64 {
    0.2
}
fn default_vector_weight() -> f64 {
    0.3
}
fn default_top_k() -> usize {
    1024
}

impl RetrievalRequest {
    /// Parse `tools/call` arguments with the same defaults as server.py
    /// `call_tool` (`arguments.get("page", 1)`, `page_size` 10, …).
    pub fn from_arguments(arguments: &Value) -> Self {
        let strings = |key: &str| -> Vec<String> {
            arguments
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        };
        Self {
            dataset_ids: strings("dataset_ids"),
            document_ids: strings("document_ids"),
            question: arguments
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            page: arguments
                .get("page")
                .and_then(Value::as_u64)
                .map(|v| v as usize)
                .unwrap_or(1),
            page_size: arguments
                .get("page_size")
                .and_then(Value::as_u64)
                .map(|v| v as usize)
                .unwrap_or(10),
            similarity_threshold: arguments
                .get("similarity_threshold")
                .and_then(Value::as_f64)
                .unwrap_or(0.2),
            vector_similarity_weight: arguments
                .get("vector_similarity_weight")
                .and_then(Value::as_f64)
                .unwrap_or(0.3),
            top_k: arguments
                .get("top_k")
                .and_then(Value::as_u64)
                .map(|v| v as usize)
                .unwrap_or(1024),
            keyword: arguments
                .get("keyword")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            rerank_id: arguments
                .get("rerank_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            force_refresh: arguments
                .get("force_refresh")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        }
    }
}

/// A hit the in-memory backend can return.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievedChunk {
    pub dataset_id: String,
    pub document_id: String,
    pub document_keyword: String,
    pub content: String,
    pub score: f64,
}

/// Backend behind the `ragflow_retrieval` / `search_knowledge` / `list_datasets`
/// tools. The production deployment swaps this for the live search engine;
/// the default [`InMemoryRetrievalBackend`] keeps the protocol testable.
pub trait RetrievalBackend: Send + Sync {
    /// `connector.list_datasets` — accessible datasets for tool descriptions.
    fn list_datasets(&self) -> Vec<DatasetInfo>;
    /// `connector.retrieval` — returns the structured retrieval response
    /// (chunks + pagination + query_info), or an error message.
    fn retrieval(&self, request: &RetrievalRequest) -> Result<Value, String>;
}

/// Default backend: filter + score-sort + paginate an in-memory chunk list,
/// and build the RAGFlow-shaped retrieval response.
#[derive(Debug, Clone, Default)]
pub struct InMemoryRetrievalBackend {
    datasets: Vec<DatasetInfo>,
    chunks: Vec<RetrievedChunk>,
}

impl InMemoryRetrievalBackend {
    pub fn new(datasets: Vec<DatasetInfo>, chunks: Vec<RetrievedChunk>) -> Self {
        Self { datasets, chunks }
    }

    pub fn empty() -> Self {
        Self::default()
    }
}

impl RetrievalBackend for InMemoryRetrievalBackend {
    fn list_datasets(&self) -> Vec<DatasetInfo> {
        self.datasets.clone()
    }

    fn retrieval(&self, request: &RetrievalRequest) -> Result<Value, String> {
        let mut hits: Vec<&RetrievedChunk> = self
            .chunks
            .iter()
            .filter(|chunk| {
                request.dataset_ids.is_empty() || request.dataset_ids.contains(&chunk.dataset_id)
            })
            .filter(|chunk| {
                request.document_ids.is_empty() || request.document_ids.contains(&chunk.document_id)
            })
            .filter(|chunk| chunk.score >= request.similarity_threshold)
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let total = hits.len();
        let start = request.page.saturating_sub(1) * request.page_size;
        let page_hits: Vec<&RetrievedChunk> = hits
            .into_iter()
            .skip(start)
            .take(request.page_size)
            .collect();

        let dataset_names: HashMap<&str, &str> = self
            .datasets
            .iter()
            .map(|dataset| (dataset.id.as_str(), dataset.description.as_str()))
            .collect();

        // `_map_chunk_fields`: preserve raw fields + dataset_name /
        // document_name / per-chunk document_metadata.
        let chunks: Vec<Value> = page_hits
            .iter()
            .enumerate()
            .map(|(index, chunk)| {
                json!({
                    "id": format!("{}_{index}", chunk.dataset_id),
                    "dataset_id": chunk.dataset_id,
                    "document_id": chunk.document_id,
                    "document_keyword": chunk.document_keyword,
                    "content": chunk.content,
                    "score": chunk.score,
                    "dataset_name": dataset_names
                        .get(chunk.dataset_id.as_str())
                        .copied()
                        .unwrap_or("Unknown"),
                    "document_name": chunk.document_keyword,
                    "document_metadata": {
                        "document_id": chunk.document_id,
                        "name": chunk.document_keyword,
                        "dataset_id": chunk.dataset_id,
                    },
                })
            })
            .collect();

        let total_pages = if total == 0 {
            0
        } else {
            total.div_ceil(request.page_size)
        };
        Ok(json!({
            "chunks": chunks,
            "pagination": {
                "page": request.page,
                "page_size": request.page_size,
                "total_chunks": total,
                "total_pages": total_pages,
            },
            "query_info": {
                "question": request.question,
                "similarity_threshold": request.similarity_threshold,
                "vector_weight": request.vector_similarity_weight,
                "keyword_search": request.keyword,
                "dataset_count": if request.dataset_ids.is_empty() {
                    self.datasets.len()
                } else {
                    request.dataset_ids.len()
                },
            },
        }))
    }
}

// ── Tool registry (server.py `@app.list_tools`) ───────────────────────────

/// The MCP tool catalogue. [`McpTool`] is the shared client/server wire type.
#[derive(Debug, Clone, Default)]
pub struct ToolRegistry {
    tools: Vec<McpTool>,
}

impl ToolRegistry {
    /// The `ragflow_retrieval` tool with server.py's exact description and
    /// `inputSchema`; `dataset_description` is the newline-delimited dataset
    /// listing appended to the description.
    pub fn ragflow_retrieval_tool(dataset_description: &str) -> McpTool {
        McpTool {
            name: "ragflow_retrieval".into(),
            description: "Retrieve relevant chunks from the RAGFlow retrieve interface based on the question. You can optionally specify dataset_ids to search only specific datasets, or omit dataset_ids entirely to search across ALL available datasets. You can also optionally specify document_ids to search within specific documents. When dataset_ids is not provided or is empty, the system will automatically search across all available datasets. Below is the list of all available datasets, including their descriptions and IDs:\n".to_string()
                + dataset_description,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "dataset_ids": {"type": "array", "items": {"type": "string"}, "description": "Optional array of dataset IDs to search. If not provided or empty, all datasets will be searched."},
                    "document_ids": {"type": "array", "items": {"type": "string"}, "description": "Optional array of document IDs to search within."},
                    "question": {"type": "string", "description": "The question or query to search for."},
                    "page": {"type": "integer", "description": "Page number for pagination", "default": 1, "minimum": 1},
                    "page_size": {"type": "integer", "description": "Number of results to return per page (default: 10, max recommended: 50 to avoid token limits)", "default": 10, "minimum": 1, "maximum": 100},
                    "similarity_threshold": {"type": "number", "description": "Minimum similarity threshold for results", "default": 0.2, "minimum": 0.0, "maximum": 1.0},
                    "vector_similarity_weight": {"type": "number", "description": "Weight for vector similarity vs term similarity", "default": 0.3, "minimum": 0.0, "maximum": 1.0},
                    "keyword": {"type": "boolean", "description": "Enable keyword-based search", "default": false},
                    "top_k": {"type": "integer", "description": "Maximum results to consider before ranking", "default": 1024, "minimum": 1, "maximum": 1024},
                    "rerank_id": {"type": "string", "description": "Optional reranking model identifier"},
                    "force_refresh": {"type": "boolean", "description": "Set to true only if fresh dataset and document metadata is explicitly required. Otherwise, cached metadata is used (default: false).", "default": false},
                },
                "required": ["question"],
            }),
        }
    }

    /// RayRAG's `search_knowledge` tool (api/connector.rs catalogue).
    pub fn search_knowledge_tool() -> McpTool {
        McpTool {
            name: "search_knowledge".into(),
            description: "Search RayRAG knowledge base with vector similarity".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "The question or query to search for."},
                    "top_k": {"type": "integer", "description": "Maximum results to consider before ranking", "default": 10, "minimum": 1},
                },
                "required": ["query"],
            }),
        }
    }

    /// RayRAG's `list_datasets` tool (api/connector.rs catalogue).
    pub fn list_datasets_tool() -> McpTool {
        McpTool {
            name: "list_datasets".into(),
            description: "List available knowledge bases".into(),
            input_schema: json!({ "type": "object" }),
        }
    }

    pub fn empty() -> Self {
        Self { tools: Vec::new() }
    }

    pub fn with_tools(tools: Vec<McpTool>) -> Self {
        Self { tools }
    }

    /// Default catalogue: the server.py `ragflow_retrieval` tool plus
    /// RayRAG's connector tools.
    pub fn default() -> Self {
        Self::with_tools(vec![
            Self::ragflow_retrieval_tool(""),
            Self::search_knowledge_tool(),
            Self::list_datasets_tool(),
        ])
    }

    pub fn list(&self) -> &[McpTool] {
        &self.tools
    }

    pub fn get(&self, name: &str) -> Option<&McpTool> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// Rebuild the `ragflow_retrieval` description with a live dataset
    /// listing (server.py `list_tools` calls `connector.list_datasets`).
    pub fn enrich_datasets(&self, datasets: &[DatasetInfo]) -> Self {
        let description = datasets
            .iter()
            .map(|dataset| {
                json!({ "description": dataset.description, "id": dataset.id }).to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        Self {
            tools: self
                .tools
                .iter()
                .map(|tool| {
                    if tool.name == "ragflow_retrieval" {
                        Self::ragflow_retrieval_tool(&description)
                    } else {
                        tool.clone()
                    }
                })
                .collect(),
        }
    }
}

// ── Protocol core (server.py `@app.list_tools` / `@app.call_tool`) ────────

/// The server-side protocol engine: registry + retrieval backend + JSON-RPC
/// dispatch. Pure — no I/O — so every protocol path is unit-testable.
pub struct McpServerCore {
    registry: ToolRegistry,
    backend: Arc<dyn RetrievalBackend>,
}

impl McpServerCore {
    pub fn new(registry: ToolRegistry, backend: Arc<dyn RetrievalBackend>) -> Self {
        Self { registry, backend }
    }

    pub fn with_backend(backend: Arc<dyn RetrievalBackend>) -> Self {
        Self::new(ToolRegistry::default(), backend)
    }

    pub fn default() -> Self {
        Self::with_backend(Arc::new(InMemoryRetrievalBackend::empty()))
    }

    /// The tool catalogue with live dataset descriptions (server.py
    /// `list_tools`).
    pub fn tools(&self) -> Vec<McpTool> {
        self.registry
            .enrich_datasets(&self.backend.list_datasets())
            .list()
            .to_vec()
    }

    /// Dispatch one raw JSON-RPC message. Returns `None` for notifications
    /// (no `id`), which the protocol acknowledges silently.
    pub fn handle_jsonrpc(&self, message: &Value) -> Option<JsonRpcResponseBody> {
        if !message.is_object() {
            return Some(JsonRpcResponseBody::err(
                Some(Value::Null),
                error_code::INVALID_REQUEST,
                "Invalid Request",
            ));
        }
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(JsonRpcResponseBody::err(
                message.get("id").cloned(),
                error_code::INVALID_REQUEST,
                "Invalid Request",
            ));
        }
        let id = message.get("id").cloned();
        let Some(method_name) = message.get("method").and_then(Value::as_str) else {
            return Some(JsonRpcResponseBody::err(
                id,
                error_code::INVALID_REQUEST,
                "Invalid Request",
            ));
        };
        // JSON-RPC notifications carry no id (or null) and get no response.
        let is_notification = id.is_none() || id.as_ref() == Some(&Value::Null);
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method_name {
            method::INITIALIZE => {
                if is_notification {
                    None
                } else {
                    Some(self.handle_initialize(id, &params))
                }
            }
            // `notifications/initialized` — acknowledged by the client SDK,
            // never answered.
            "notifications/initialized" => None,
            "ping" => {
                if is_notification {
                    None
                } else {
                    Some(JsonRpcResponseBody::ok(id, json!({})))
                }
            }
            method::TOOLS_LIST => {
                if is_notification {
                    None
                } else {
                    Some(JsonRpcResponseBody::ok(
                        id,
                        json!({ "tools": self.tools() }),
                    ))
                }
            }
            method::TOOLS_CALL => {
                if is_notification {
                    None
                } else {
                    Some(self.handle_tools_call(id, &params))
                }
            }
            other => Some(JsonRpcResponseBody::err(
                id,
                error_code::METHOD_NOT_FOUND,
                format!("Method not found: {other}"),
            )),
        }
    }

    /// `initialize` — negotiate the protocol version and report capabilities.
    fn handle_initialize(&self, id: Option<Value>, params: &Value) -> JsonRpcResponseBody {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(PROTOCOL_VERSION);
        let protocol = if matches!(requested, "2024-11-05" | "2025-03-26" | "2025-06-18") {
            requested
        } else {
            PROTOCOL_VERSION
        };
        JsonRpcResponseBody::ok(
            id,
            json!({
                "protocolVersion": protocol,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
            }),
        )
    }

    /// `tools/call` — dispatch to the retrieval backend; unknown tools raise
    /// server.py's `ValueError(f"Tool not found: {name}")` as
    /// `INVALID_PARAMS`.
    fn handle_tools_call(&self, id: Option<Value>, params: &Value) -> JsonRpcResponseBody {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return JsonRpcResponseBody::err(
                id,
                error_code::INVALID_PARAMS,
                "Invalid params: missing tool name",
            );
        };
        let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

        match name {
            "ragflow_retrieval" | "search_knowledge" => {
                let mut request = RetrievalRequest::from_arguments(&arguments);
                // `search_knowledge` speaks `query` instead of `question`.
                if request.question.is_empty() {
                    request.question = arguments
                        .get("query")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                }
                if request.question.trim().is_empty() {
                    return JsonRpcResponseBody::err(
                        id,
                        error_code::INVALID_PARAMS,
                        "Invalid params: question is required",
                    );
                }
                match self.backend.retrieval(&request) {
                    Ok(text) => JsonRpcResponseBody::ok(
                        id,
                        json!({
                            "content": [McpContent::Text { text: text.to_string() }],
                            "isError": false,
                        }),
                    ),
                    Err(message) => {
                        JsonRpcResponseBody::err(id, error_code::INTERNAL_ERROR, message)
                    }
                }
            }
            "list_datasets" => {
                let lines: Vec<String> = self
                    .backend
                    .list_datasets()
                    .iter()
                    .map(|dataset| {
                        json!({ "description": dataset.description, "id": dataset.id }).to_string()
                    })
                    .collect();
                JsonRpcResponseBody::ok(
                    id,
                    json!({
                        "content": [McpContent::Text { text: lines.join("\n") }],
                        "isError": false,
                    }),
                )
            }
            other => JsonRpcResponseBody::err(
                id,
                error_code::INVALID_PARAMS,
                format!("Tool not found: {other}"),
            ),
        }
    }
}

// ── Auth (server.py `_extract_token_from_headers` / `with_api_key`) ───────

/// Extract the API key from request headers, mirroring server.py's
/// `_extract_token_from_headers`: `Authorization: Bearer …` (any casing)
/// first, then the `api_key` / `x-api-key` variants.
pub fn extract_token_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers.get("authorization") {
        let text = auth.to_str().unwrap_or("").trim();
        if text.to_ascii_lowercase().starts_with("bearer ") {
            let token = text[7..].trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    for key in ["api_key", "x-api-key"] {
        if let Some(value) = headers.get(key) {
            let text = value.to_str().unwrap_or("").trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

/// server.py `with_api_key`: self-host mode always uses `HOST_API_KEY`;
/// host mode requires a per-request header token (401 otherwise).
pub fn resolve_api_key(config: &McpServerConfig, headers: &HeaderMap) -> Result<String, Response> {
    if config.mode == LaunchMode::SelfHost {
        return Ok(config.host_api_key.clone());
    }
    match extract_token_from_headers(headers) {
        Some(token) => Ok(token),
        None => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "Missing or invalid authorization header" })),
        )
            .into_response()),
    }
}

// ── SSE framing helpers (server.py `SseServerTransport`) ──────────────────

/// One SSE frame: `event: <event>\ndata: <data>\n\n`.
pub fn format_sse_event(event: &str, data: &str) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

/// The initial `endpoint` event that tells the client where to POST
/// JSON-RPC messages.
pub fn sse_endpoint_event(session_id: &str) -> String {
    format_sse_event("endpoint", &format!("/messages/?session_id={session_id}"))
}

/// A `message` event carrying one JSON-RPC response payload.
pub fn sse_message_event(payload: &str) -> String {
    format_sse_event("message", payload)
}

/// Parse one SSE frame back into `(event, data)`; `None` for frames without
/// a data line.
pub fn parse_sse_frame(frame: &str) -> Option<(String, String)> {
    let mut event: Option<String> = None;
    let mut data: Option<String> = None;
    for line in frame.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data = Some(value.trim().to_string());
        }
    }
    Some((event?, data?))
}

// ── SSE transport state + router ──────────────────────────────────────────

/// Shared state for the SSE transport: the protocol core, the config and the
/// live session map (session id → response channel).
#[derive(Clone)]
pub struct McpSseState {
    pub core: Arc<McpServerCore>,
    pub config: McpServerConfig,
    sessions: Arc<Mutex<HashMap<String, mpsc::Sender<Event>>>>,
    next_session_id: Arc<AtomicU64>,
}

impl McpSseState {
    pub fn new(core: Arc<McpServerCore>, config: McpServerConfig) -> Self {
        Self {
            core,
            config,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_session_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// A state with the default core (in-memory backend) and config — handy
    /// for mounting the router standalone.
    pub fn default() -> Self {
        Self::new(
            Arc::new(McpServerCore::default()),
            McpServerConfig::default(),
        )
    }

    fn register_session(&self, tx: mpsc::Sender<Event>) -> String {
        let session_id = format!(
            "sse-{}",
            self.next_session_id.fetch_add(1, Ordering::Relaxed)
        );
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .insert(session_id.clone(), tx);
        session_id
    }

    fn unregister_session(&self, session_id: &str) {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .remove(session_id);
    }

    /// Push one `message` event onto the session's stream. Returns `false`
    /// when the session is unknown or its client disconnected.
    async fn publish(&self, session_id: &str, event: Event) -> bool {
        let sender = self
            .sessions
            .lock()
            .expect("session map lock poisoned")
            .get(session_id)
            .cloned();
        match sender {
            Some(sender) => sender.send(event).await.is_ok(),
            None => false,
        }
    }
}

/// The `GET /sse` stream: emits the `endpoint` event first, then every
/// `message` event pushed by `POST /messages/`. Unregisters the session when
/// the client disconnects.
struct SessionStream {
    rx: mpsc::Receiver<Event>,
    state: McpSseState,
    session_id: String,
    first: Option<Event>,
}

impl Unpin for SessionStream {}

impl Stream for SessionStream {
    type Item = Result<Event, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(first) = self.first.take() {
            return Poll::Ready(Some(Ok(first)));
        }
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(event)) => Poll::Ready(Some(Ok(event))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for SessionStream {
    fn drop(&mut self) {
        self.state.unregister_session(&self.session_id);
    }
}

/// `GET /sse` — open the event stream (server.py `handle_sse`).
pub async fn sse_handler(state: Arc<McpSseState>, headers: HeaderMap) -> Response {
    if let Err(response) = resolve_api_key(&state.config, &headers) {
        return response;
    }
    let (tx, rx) = mpsc::channel::<Event>(64);
    let session_id = state.register_session(tx);
    let stream = SessionStream {
        rx,
        state: state.as_ref().clone(),
        session_id: session_id.clone(),
        first: Some(
            Event::default()
                .event("endpoint")
                .data(format!("/messages/?session_id={session_id}")),
        ),
    };
    Sse::new(stream).into_response()
}

/// `POST /messages/?session_id=…` — accept one JSON-RPC message, dispatch it
/// through the protocol core and push any response onto the session's SSE
/// stream. Returns 202 Accepted per the MCP SSE spec (server.py
/// `sse.handle_post_message`).
pub async fn post_message(
    state: Arc<McpSseState>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    body: Body,
) -> Response {
    if let Err(response) = resolve_api_key(&state.config, &headers) {
        return response;
    }
    let Some(session_id) = params.get("session_id") else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing session_id" })),
        )
            .into_response();
    };
    let bytes: Bytes = match to_bytes(body, 1 << 20).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Request body too large" })),
            )
                .into_response();
        }
    };
    let message: Value = match serde_json::from_slice(&bytes) {
        Ok(message) => message,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": error_code::PARSE_ERROR, "message": "Parse error" },
                })),
            )
                .into_response();
        }
    };

    if let Some(response) = state.core.handle_jsonrpc(&message) {
        let payload = serde_json::to_string(&response).unwrap_or_default();
        let event = Event::default().event("message").data(payload);
        if !state.publish(session_id, event).await {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "Unknown session" })),
            )
                .into_response();
        }
    }
    StatusCode::ACCEPTED.into_response()
}

/// Build the standalone MCP SSE router: `GET /sse` + `POST /messages/`.
/// Merge it into the main application with `Router::merge` when the MCP
/// server should share the HTTP listener.
/// Mounts the MCP SSE transport as a stateless router (`Router<()>`),
/// so it can be `nest`ed under any parent state (e.g. `Router<Arc<AppState>>`).
pub fn build_mcp_router(state: McpSseState) -> Router<()> {
    let state = Arc::new(state);
    Router::new()
        .route(
            "/sse",
            get({
                let state = state.clone();
                move |headers: HeaderMap| sse_handler(state.clone(), headers)
            }),
        )
        .route(
            "/messages/",
            post({
                let state = state.clone();
                move |headers: HeaderMap, params: Query<HashMap<String, String>>, body: Body| {
                    post_message(state.clone(), headers, params, body)
                }
            }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn header(value: &str) -> HeaderValue {
        value.parse().expect("valid header value")
    }

    #[test]
    fn initialize_tools_list_and_ping_protocol_handshake() {
        let core = McpServerCore::default();

        // initialize → protocol version + capabilities + serverInfo.
        let request = json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                        "clientInfo": { "name": "test-client", "version": "1.0" } }
        });
        let response = core.handle_jsonrpc(&request).expect("initialize answered");
        assert_eq!(response.id, Some(json!(1)));
        let result = response.result.expect("initialize result");
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(result["capabilities"]["tools"], json!({}));

        // tools/list → the registry includes ragflow_retrieval with schema.
        let response = core
            .handle_jsonrpc(&json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }))
            .expect("tools/list answered");
        let tools = response.result.unwrap()["tools"]
            .as_array()
            .unwrap()
            .clone();
        assert!(tools.len() >= 3);
        let retrieval = tools
            .iter()
            .find(|tool| tool["name"] == "ragflow_retrieval")
            .expect("ragflow_retrieval registered");
        assert_eq!(retrieval["inputSchema"]["required"][0], "question");
        assert!(
            retrieval["description"]
                .as_str()
                .unwrap()
                .contains("Below is the list of all available datasets")
        );

        // ping → empty result.
        let response = core
            .handle_jsonrpc(&json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" }))
            .unwrap();
        assert_eq!(response.result, Some(json!({})));

        // Notifications (no id) never get a response.
        assert!(
            core.handle_jsonrpc(
                &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
            )
            .is_none()
        );

        // Unknown method → -32601; malformed envelope → -32600.
        let response = core
            .handle_jsonrpc(&json!({ "jsonrpc": "2.0", "id": 4, "method": "bogus" }))
            .unwrap();
        assert_eq!(response.error.unwrap().code, error_code::METHOD_NOT_FOUND);
        let response = core
            .handle_jsonrpc(&json!({ "id": 5, "method": "ping" }))
            .unwrap();
        assert_eq!(response.error.unwrap().code, error_code::INVALID_REQUEST);
    }

    #[test]
    fn call_tool_runs_retrieval_and_reports_errors() {
        let backend = InMemoryRetrievalBackend::new(
            vec![DatasetInfo {
                id: "ds1".into(),
                description: "Project docs".into(),
            }],
            vec![
                RetrievedChunk {
                    dataset_id: "ds1".into(),
                    document_id: "doc1".into(),
                    document_keyword: "guide".into(),
                    content: "Rust is fast".into(),
                    score: 0.87,
                },
                RetrievedChunk {
                    dataset_id: "ds1".into(),
                    document_id: "doc1".into(),
                    document_keyword: "guide".into(),
                    content: "Python is slow".into(),
                    score: 0.45,
                },
            ],
        );
        let core = McpServerCore::with_backend(Arc::new(backend));

        let response = core
            .handle_jsonrpc(&json!({
                "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                "params": { "name": "ragflow_retrieval",
                            "arguments": { "question": "rust", "page_size": 10,
                                           "similarity_threshold": 0.5 } }
            }))
            .expect("tools/call answered");
        assert!(response.error.is_none());
        let result = response.result.unwrap();
        assert_eq!(result["isError"], false);
        let content = result["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        let text: Value = serde_json::from_str(content[0]["text"].as_str().unwrap()).unwrap();
        // Only the 0.87 hit clears the 0.5 threshold.
        assert_eq!(text["pagination"]["total_chunks"], 1);
        assert_eq!(text["chunks"][0]["dataset_name"], "Project docs");
        assert_eq!(text["chunks"][0]["document_name"], "guide");
        assert_eq!(text["query_info"]["question"], "rust");

        // list_datasets returns newline-delimited {"id","description"} rows.
        let response = core
            .handle_jsonrpc(&json!({
                "jsonrpc": "2.0", "id": 8, "method": "tools/call",
                "params": { "name": "list_datasets", "arguments": {} }
            }))
            .unwrap();
        let content = response.result.unwrap()["content"]
            .as_array()
            .unwrap()
            .clone();
        let rows = content[0]["text"].as_str().unwrap();
        assert!(rows.contains("\"id\":\"ds1\""));

        // Unknown tool → server.py's "Tool not found" as -32602.
        let response = core
            .handle_jsonrpc(&json!({
                "jsonrpc": "2.0", "id": 9, "method": "tools/call",
                "params": { "name": "nope", "arguments": {} }
            }))
            .unwrap();
        let error = response.error.unwrap();
        assert_eq!(error.code, error_code::INVALID_PARAMS);
        assert!(error.message.contains("Tool not found: nope"));

        // Missing question → -32602.
        let response = core
            .handle_jsonrpc(&json!({
                "jsonrpc": "2.0", "id": 10, "method": "tools/call",
                "params": { "name": "ragflow_retrieval", "arguments": {} }
            }))
            .unwrap();
        assert_eq!(response.error.unwrap().code, error_code::INVALID_PARAMS);
    }

    #[test]
    fn auth_token_extraction_matches_ragflow_header_handling() {
        let mut headers = HeaderMap::new();
        assert!(extract_token_from_headers(&headers).is_none());

        headers.insert("authorization", header("Bearer abc123"));
        assert_eq!(
            extract_token_from_headers(&headers).as_deref(),
            Some("abc123")
        );
        // Lowercase scheme + surrounding whitespace.
        headers.insert("authorization", header("bearer xyz  "));
        assert_eq!(extract_token_from_headers(&headers).as_deref(), Some("xyz"));

        headers.remove("authorization");
        headers.insert("x-api-key", header("key-1"));
        assert_eq!(
            extract_token_from_headers(&headers).as_deref(),
            Some("key-1")
        );

        headers.remove("x-api-key");
        headers.insert("api_key", header("key-2"));
        assert_eq!(
            extract_token_from_headers(&headers).as_deref(),
            Some("key-2")
        );

        // Host mode without a token → 401; self-host mode never needs one.
        let config = McpServerConfig {
            mode: LaunchMode::Host,
            ..Default::default()
        };
        assert!(resolve_api_key(&config, &HeaderMap::new()).is_err());
        let config = McpServerConfig::default();
        assert_eq!(resolve_api_key(&config, &HeaderMap::new()).unwrap(), "");
        let config = McpServerConfig {
            host_api_key: "ragflow-xxx".into(),
            ..Default::default()
        };
        assert_eq!(
            resolve_api_key(&config, &HeaderMap::new()).unwrap(),
            "ragflow-xxx"
        );
    }

    #[test]
    fn sse_framing_and_endpoint_events_round_trip() {
        let endpoint = sse_endpoint_event("sse-1");
        assert_eq!(
            endpoint,
            "event: endpoint\ndata: /messages/?session_id=sse-1\n\n"
        );

        let payload = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let frame = sse_message_event(payload);
        assert_eq!(frame, format!("event: message\ndata: {payload}\n\n"));

        let (event, data) = parse_sse_frame(&frame).unwrap();
        assert_eq!(event, "message");
        assert_eq!(data, payload);
        let (event, data) = parse_sse_frame(&endpoint).unwrap();
        assert_eq!(event, "endpoint");
        assert_eq!(data, "/messages/?session_id=sse-1");

        // Frames without data lines are ignored.
        assert!(parse_sse_frame("event: ping\n\n").is_none());
    }

    #[test]
    fn retrieval_request_defaults_match_server_py_call_tool() {
        let request = RetrievalRequest::from_arguments(&json!({
            "question": "q", "dataset_ids": ["a"], "document_ids": []
        }));
        assert_eq!(request.question, "q");
        assert_eq!(request.dataset_ids, vec!["a".to_string()]);
        assert!(request.document_ids.is_empty());
        assert_eq!(request.page, 1);
        assert_eq!(request.page_size, 10);
        assert_eq!(request.similarity_threshold, 0.2);
        assert_eq!(request.vector_similarity_weight, 0.3);
        assert_eq!(request.top_k, 1024);
        assert!(!request.keyword);
        assert!(!request.force_refresh);
        assert!(request.rerank_id.is_none());

        let request = RetrievalRequest::from_arguments(&json!({
            "question": "q", "page": 3, "page_size": 50, "keyword": true,
            "similarity_threshold": 0.1, "rerank_id": "bge-reranker"
        }));
        assert_eq!(request.page, 3);
        assert_eq!(request.page_size, 50);
        assert!(request.keyword);
        assert_eq!(request.similarity_threshold, 0.1);
        assert_eq!(request.rerank_id.as_deref(), Some("bge-reranker"));

        // Config env parsing helper.
        assert!(McpServerConfig::parse_bool_flag("true", false));
        assert!(McpServerConfig::parse_bool_flag("YES", false));
        assert!(!McpServerConfig::parse_bool_flag("0", true));
        assert!(McpServerConfig::parse_bool_flag("garbage", true));
    }
}
