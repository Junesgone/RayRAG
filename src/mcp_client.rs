//! MCP (Model Context Protocol) client — Rust port of RAGFlow `mcp/client/`.
//!
//! RAGFlow's `mcp/client/client.py` and `mcp/client/streamable_http_client.py`
//! wrap the official Python MCP SDK: they connect to an MCP server over SSE or
//! Streamable HTTP, `initialize()` a session, `list_tools()` and
//! `call_tool(name, arguments)`. This module keeps the same shape against the
//! raw JSON-RPC 2.0 wire protocol:
//!
//! - [`McpTransport`] — how to reach the server: `stdio` (spawn a local
//!   command, newline-delimited JSON-RPC over stdin/stdout) or `http`
//!   (Streamable HTTP POST with `Accept: application/json, text/event-stream`).
//! - [`McpClient`] — one session: `initialize` (protocol handshake),
//!   `list_tools` (pull the tool catalogue) and `call_tool` (invoke a tool
//!   with JSON arguments). Matches the Python `ClientSession` surface used by
//!   the RAGFlow examples.
//!
//! The wire helpers (`build_*_request`, `parse_*`) are pure functions so the
//! protocol framing is unit-testable without spawning processes or sockets.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// MCP protocol version negotiated during `initialize`. The 2025-03-26 spec
/// is the baseline for the current Python SDK; the server may reply with an
/// older version, which the client accepts.
pub const PROTOCOL_VERSION: &str = "2025-03-26";

/// MCP JSON-RPC method names.
pub mod method {
    pub const INITIALIZE: &str = "initialize";
    pub const TOOLS_LIST: &str = "tools/list";
    pub const TOOLS_CALL: &str = "tools/call";
    pub const NOTIFICATION_INITIALIZED: &str = "notifications/initialized";
}

// ── JSON-RPC 2.0 wire types ───────────────────────────────────────────────

/// A JSON-RPC 2.0 request (`id` + `method` + optional `params`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A JSON-RPC 2.0 response (`result` xor `error`).
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<Value>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// MCP `initialize` result (`serverInfo`, `capabilities`, `protocolVersion`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    #[serde(default)]
    pub capabilities: Value,
    #[serde(default)]
    pub server_info: Value,
}

/// MCP `tools/list` entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `inputSchema` — JSON Schema for the tool arguments.
    #[serde(default)]
    pub input_schema: Value,
}

/// A content item inside a `tools/call` result (`text` | `image` | ...).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpContent {
    Text {
        text: String,
    },
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    Resource {
        uri: String,
        text: Option<String>,
        blob: Option<String>,
    },
    /// Unknown content types pass through verbatim.
    #[serde(untagged)]
    Other(Value),
}

/// MCP `tools/call` result.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpCallResult {
    #[serde(default)]
    pub content: Vec<McpContent>,
    #[serde(default)]
    pub is_error: bool,
}

// ── Transport configuration ───────────────────────────────────────────────

/// How the client reaches the MCP server. Mirrors the Python SDK's
/// `stdio_client` (spawn a command) and `streamablehttp_client` (HTTP POST).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum McpTransport {
    /// `mcp.client.stdio.stdio_client` — spawn `<command> <args>` with env,
    /// JSON-RPC messages are newline-delimited on stdin/stdout.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// `mcp.client.streamable_http.streamablehttp_client` — POST JSON-RPC to
    /// `<url>` with optional headers (RAGFlow passes `api_key` or
    /// `Authorization: Bearer`).
    Http {
        url: String,
        #[serde(default)]
        headers: BTreeMap<String, String>,
    },
}

impl McpTransport {
    pub fn stdio(command: impl Into<String>) -> Self {
        Self::Stdio {
            command: command.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
        }
    }

    pub fn http(url: impl Into<String>) -> Self {
        Self::Http {
            url: url.into(),
            headers: BTreeMap::new(),
        }
    }
}

// ── Pure wire helpers (unit-testable framing) ─────────────────────────────

/// Build an `initialize` request with the client capability block
/// (RAGFlow's `ClientSession.initialize()` handshake).
pub fn build_initialize_request(id: u64) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id,
        method: method::INITIALIZE.into(),
        params: Some(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "rayrag-mcp-client", "version": "0.1.0" }
        })),
    }
}

/// Build a `tools/list` request.
pub fn build_tools_list_request(id: u64) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id,
        method: method::TOOLS_LIST.into(),
        params: None,
    }
}

/// Build a `tools/call` request.
pub fn build_tools_call_request(id: u64, name: &str, arguments: Value) -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".into(),
        id,
        method: method::TOOLS_CALL.into(),
        params: Some(json!({ "name": name, "arguments": arguments })),
    }
}

/// Serialize a request for the stdio transport: one JSON line per message.
pub fn encode_stdio_message(request: &JsonRpcRequest) -> String {
    let mut line = serde_json::to_string(request).expect("request serializes");
    line.push('\n');
    line
}

/// Parse one incoming stdio line. Notification frames (no `id`) are skipped
/// by the caller; a line without a JSON-RPC object is an error.
pub fn parse_stdio_line(line: &str) -> anyhow::Result<JsonRpcResponse> {
    let value: Value = serde_json::from_str(line)?;
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        anyhow::bail!("not a JSON-RPC 2.0 message: {line}");
    }
    let response: JsonRpcResponse = serde_json::from_value(value)?;
    Ok(response)
}

/// Extract the `data:` payload from one Streamable HTTP SSE event. Returns
/// `None` for non-`message` events (`event: ping` etc.).
pub fn parse_sse_event(event: &str) -> Option<&str> {
    let mut is_message = false;
    let mut data: Option<&str> = None;
    for line in event.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            is_message = value.trim() == "message";
        } else if let Some(value) = line.strip_prefix("data:") {
            data = Some(value.trim());
        }
    }
    if is_message { data } else { None }
}

/// Parse an `initialize` result into [`InitializeResult`].
pub fn parse_initialize_result(result: &Value) -> anyhow::Result<InitializeResult> {
    Ok(serde_json::from_value(result.clone())?)
}

/// Parse a `tools/list` result into tool rows. The server may also send
/// `nextCursor`; it is ignored here (single-page pull, like the example).
pub fn parse_tools_result(result: &Value) -> anyhow::Result<Vec<McpTool>> {
    let tools = result
        .get("tools")
        .ok_or_else(|| anyhow::anyhow!("tools/list result missing `tools`"))?;
    let tools: Vec<McpTool> = serde_json::from_value(tools.clone())?;
    Ok(tools)
}

/// Parse a `tools/call` result.
pub fn parse_call_tool_result(result: &Value) -> anyhow::Result<McpCallResult> {
    Ok(serde_json::from_value(result.clone())?)
}

/// Concatenate the text of every text content item (the common way agents
/// consume a tool response).
pub fn call_result_text(result: &McpCallResult) -> String {
    result
        .content
        .iter()
        .filter_map(|item| match item {
            McpContent::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ── Live client ───────────────────────────────────────────────────────────

/// One MCP client session over a configured transport. Request/response
/// correlation uses monotonically increasing ids; notifications (no id) are
/// skipped while waiting for the matching response.
pub struct McpClient {
    transport: McpTransport,
    next_id: std::sync::atomic::AtomicU64,
}

/// A spawned stdio session: child process + line reader/writer.
struct StdioSession {
    child: tokio::process::Child,
    stdin: tokio::io::BufWriter<tokio::process::ChildStdin>,
    stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
}

impl StdioSession {
    async fn spawn(transport: &McpTransport) -> anyhow::Result<Self> {
        let McpTransport::Stdio { command, args, env } = transport else {
            anyhow::bail!("stdio session requires the stdio transport");
        };
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("stdio: no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("stdio: no stdout"))?;
        Ok(Self {
            child,
            stdin: tokio::io::BufWriter::new(stdin),
            stdout: tokio::io::BufReader::new(stdout),
        })
    }

    async fn request(&mut self, request: &JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        self.stdin
            .write_all(encode_stdio_message(request).as_bytes())
            .await?;
        self.stdin.flush().await?;
        let mut line = String::new();
        loop {
            line.clear();
            let read = self.stdout.read_line(&mut line).await?;
            if read == 0 {
                anyhow::bail!(
                    "stdio: server closed stdout while awaiting id {}",
                    request.id
                );
            }
            let response = parse_stdio_line(line.trim_end())?;
            // Skip notifications (no id) and unrelated responses.
            let matches = match &response.id {
                Some(Value::Number(n)) => n.as_u64() == Some(request.id),
                _ => false,
            };
            if matches {
                return Ok(response);
            }
        }
    }
}

impl McpClient {
    /// Create a client for a transport; connect lazily on the first call.
    pub fn new(transport: McpTransport) -> Self {
        Self {
            transport,
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn next_id(&self) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn response_or_error(response: JsonRpcResponse) -> anyhow::Result<Value> {
        if let Some(error) = response.error {
            anyhow::bail!("MCP error {}: {}", error.code, error.message);
        }
        response
            .result
            .ok_or_else(|| anyhow::anyhow!("MCP response without result or error"))
    }

    /// `ClientSession.initialize()` — negotiate the protocol version and
    /// return the server capabilities.
    pub async fn initialize(&self) -> anyhow::Result<InitializeResult> {
        let request = build_initialize_request(self.next_id());
        let response = self.round_trip(request).await?;
        let result = Self::response_or_error(response)?;
        parse_initialize_result(&result)
    }

    /// `ClientSession.list_tools()` — pull the server's tool catalogue.
    pub async fn list_tools(&self) -> anyhow::Result<Vec<McpTool>> {
        let request = build_tools_list_request(self.next_id());
        let response = self.round_trip(request).await?;
        let result = Self::response_or_error(response)?;
        parse_tools_result(&result)
    }

    /// `ClientSession.call_tool(name, arguments)` — invoke a tool and return
    /// its content items.
    pub async fn call_tool(&self, name: &str, arguments: Value) -> anyhow::Result<McpCallResult> {
        let request = build_tools_call_request(self.next_id(), name, arguments);
        let response = self.round_trip(request).await?;
        let result = Self::response_or_error(response)?;
        parse_call_tool_result(&result)
    }

    /// One request/response exchange over the configured transport.
    async fn round_trip(&self, request: JsonRpcRequest) -> anyhow::Result<JsonRpcResponse> {
        match &self.transport {
            McpTransport::Stdio { .. } => {
                let mut session = StdioSession::spawn(&self.transport).await?;
                let response = session.request(&request).await;
                let _ = session.child.kill().await;
                response
            }
            McpTransport::Http { url, headers } => {
                self.http_round_trip(url, headers, &request).await
            }
        }
    }

    /// Streamable HTTP round trip: POST the JSON-RPC body with
    /// `Accept: application/json, text/event-stream`; the server may answer
    /// with a plain JSON body or an SSE `message` event.
    async fn http_round_trip(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        request: &JsonRpcRequest,
    ) -> anyhow::Result<JsonRpcResponse> {
        let client = reqwest::Client::new();
        let mut builder = client
            .post(url)
            .header("Accept", "application/json, text/event-stream")
            .header("Content-Type", "application/json")
            .json(request);
        for (key, value) in headers {
            builder = builder.header(key, value);
        }
        let response = builder.send().await?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("MCP HTTP {status}: {body}");
        }
        if content_type.contains("text/event-stream") {
            let payload = parse_sse_event(&body)
                .ok_or_else(|| anyhow::anyhow!("MCP SSE response without a message event"))?;
            parse_stdio_line(payload)
        } else {
            parse_stdio_line(body.trim())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initialize_request_carries_protocol_version_and_client_info() {
        let request = build_initialize_request(7);
        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, 7);
        assert_eq!(request.method, "initialize");
        let params = request.params.unwrap();
        assert_eq!(params["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(params["clientInfo"]["name"], "rayrag-mcp-client");
        assert!(params["capabilities"].is_object());
    }

    #[test]
    fn stdio_framing_round_trips_and_skips_notifications() {
        // Notifications carry no id and must not be returned as responses.
        let notification = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let response = r#"{"jsonrpc":"2.0","id":3,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"srv","version":"1.0"}}}"#;

        let request = build_tools_list_request(3);
        let encoded = encode_stdio_message(&request);
        assert!(encoded.ends_with('\n'));
        let decoded: JsonRpcRequest = serde_json::from_str(encoded.trim_end()).unwrap();
        assert_eq!(decoded.method, method::TOOLS_LIST);
        assert_eq!(decoded.id, 3);

        let parsed_notification = parse_stdio_line(notification).unwrap();
        assert!(parsed_notification.id.is_none());
        let parsed = parse_stdio_line(response).unwrap();
        assert_eq!(parsed.id, Some(json!(3)));
        let result = parsed.result.unwrap();
        let init = parse_initialize_result(&result).unwrap();
        assert_eq!(init.protocol_version, "2025-03-26");
        assert_eq!(init.server_info["name"], "srv");
    }

    #[test]
    fn tools_list_and_call_results_parse() {
        let tools_result = json!({
            "tools": [
                {"name": "ragflow_retrieval", "description": "Retrieve chunks",
                 "inputSchema": {"type": "object", "properties": {"question": {"type": "string"}}}},
                {"name": "web_search", "description": "", "inputSchema": {"type": "object"}}
            ]
        });
        let tools = parse_tools_result(&tools_result).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "ragflow_retrieval");
        assert_eq!(
            tools[0].input_schema["properties"]["question"]["type"],
            "string"
        );
        assert!(tools[1].description.is_empty());

        let call_result = json!({
            "content": [
                {"type": "text", "text": "chunk one"},
                {"type": "text", "text": "chunk two"}
            ],
            "isError": false
        });
        let parsed = parse_call_tool_result(&call_result).unwrap();
        assert!(!parsed.is_error);
        assert_eq!(call_result_text(&parsed), "chunk one\nchunk two");
    }

    #[test]
    fn streamable_http_sse_payload_extraction() {
        // A real Streamable HTTP SSE frame.
        let event =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n\n";
        let payload = parse_sse_event(event).unwrap();
        let response = parse_stdio_line(payload).unwrap();
        assert_eq!(response.id, Some(json!(1)));
        assert!(
            parse_tools_result(&response.result.unwrap())
                .unwrap()
                .is_empty()
        );

        // Ping events carry no data and must be ignored.
        assert!(parse_sse_event("event: ping\ndata: {}\n\n").is_none());
        // A malformed frame surfaces as a parse error, not a panic.
        assert!(parse_stdio_line("not json at all").is_err());
    }

    #[test]
    fn jsonrpc_errors_surface_with_code_and_message() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: Some(json!(9)),
            result: None,
            error: Some(JsonRpcError {
                code: -32602,
                message: "Invalid params: unknown tool `nope`".into(),
                data: None,
            }),
        };
        let err = McpClient::response_or_error(response).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("-32602"));
        assert!(text.contains("unknown tool"));
    }
}
