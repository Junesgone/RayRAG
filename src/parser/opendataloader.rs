//! OpenDataLoader + TCADP parsers.
//!
//! OpenDataLoader includes RAGFlow's remote PDF parsing provider as well as the
//! legacy local generic data loader for XML, YAML, TOML, and log files.
//! TCADP: domain-specific template parser (placeholder).

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

// ── OpenDataLoader ──────────────────────────────────────────────

/// RAGFlow-compatible OpenDataLoader HTTP provider configuration.
#[derive(Clone)]
pub struct OpenDataLoaderConfig {
    api_url: String,
    api_key: Option<String>,
    timeout: Duration,
}

impl fmt::Debug for OpenDataLoaderConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenDataLoaderConfig")
            .field("api_url", &self.api_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl OpenDataLoaderConfig {
    /// Resolve RAGFlow's nested or flat provider credential JSON.
    ///
    /// Invalid JSON behaves like an empty provider object, matching the Python
    /// wrapper. Lowercase keys take precedence over uppercase keys and then
    /// process environment variables.
    pub fn from_ragflow_key(key: &str, fallback_api_url: Option<&str>) -> Result<Self> {
        let root: Value =
            serde_json::from_str(key).unwrap_or_else(|_| Value::Object(Default::default()));
        let empty_provider = Value::Object(Default::default());
        let provider = match root.get("api_key") {
            Some(value) if value.is_object() => value,
            Some(_) => &empty_provider,
            None if root.is_object() => &root,
            None => &empty_provider,
        };

        let api_url = resolve_provider_value(
            provider,
            "opendataloader_apiserver",
            "OPENDATALOADER_APISERVER",
        )
        .or_else(|| fallback_api_url.map(str::to_owned))
        .unwrap_or_default()
        .trim()
        .trim_end_matches('/')
        .to_owned();
        let api_key =
            resolve_provider_value(provider, "opendataloader_api_key", "OPENDATALOADER_API_KEY")
                .and_then(non_empty_trimmed);
        let timeout =
            resolve_provider_value(provider, "opendataloader_timeout", "OPENDATALOADER_TIMEOUT")
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|seconds| *seconds > 0)
                .unwrap_or(600);

        Ok(Self {
            api_url,
            api_key,
            timeout: Duration::from_secs(timeout),
        })
    }

    /// Resolve provider configuration from RAGFlow's deployment variables.
    pub fn from_env() -> Result<Option<Self>> {
        let config = Self::from_ragflow_key("", None)?;
        if config.api_url.is_empty() {
            return Ok(None);
        }
        Ok(Some(config))
    }

    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

fn resolve_provider_value(provider: &Value, lower: &str, upper: &str) -> Option<String> {
    if let Some(value) = provider.get(lower) {
        return value_string(value);
    }
    if let Some(value) = provider.get(upper) {
        return value_string(value);
    }
    std::env::var(upper).ok()
}

fn value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn non_empty_trimmed(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// Optional multipart fields accepted by OpenDataLoader's `/file_parse`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenDataLoaderOptions {
    pub hybrid: Option<String>,
    pub image_output: Option<String>,
    pub sanitize: Option<bool>,
}

/// OpenDataLoader output adapted to RayRAG's marker-aware document pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDataLoaderOutput {
    pub content: String,
    pub structured: bool,
    pub section_count: usize,
    pub table_count: usize,
    pub image_count: usize,
}

/// HTTP client for RAGFlow's OpenDataLoader PDF provider.
#[derive(Clone)]
pub struct OpenDataLoaderClient {
    config: OpenDataLoaderConfig,
    client: reqwest::Client,
}

impl OpenDataLoaderClient {
    pub fn new(config: OpenDataLoaderConfig) -> Result<Self> {
        if config.api_url.is_empty() {
            anyhow::bail!(
                "OpenDataLoader requires OPENDATALOADER_APISERVER or opendataloader_apiserver"
            );
        }
        let client = reqwest::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|error| anyhow::anyhow!("OpenDataLoader HTTP client: {error}"))?;
        Ok(Self { config, client })
    }

    /// Construct the client only when `OPENDATALOADER_APISERVER` is present.
    pub fn from_env() -> Result<Option<Self>> {
        OpenDataLoaderConfig::from_env()?.map(Self::new).transpose()
    }

    /// Probe the exact `/health` endpoint with RAGFlow's fixed five-second timeout.
    pub async fn check_installation(&self) -> bool {
        let mut request = self
            .client
            .get(format!("{}/health", self.config.api_url))
            .timeout(Duration::from_secs(5));
        if let Some(api_key) = self.config.api_key() {
            request = request.bearer_auth(api_key);
        }
        request
            .send()
            .await
            .is_ok_and(|response| response.status() == reqwest::StatusCode::OK)
    }

    /// Submit PDF bytes to `/file_parse`, rebuilding the multipart body for
    /// each of RAGFlow's three immediate attempts.
    pub async fn parse_pdf(
        &self,
        file_name: &str,
        data: &[u8],
        options: &OpenDataLoaderOptions,
    ) -> Result<OpenDataLoaderOutput> {
        let mut last_error = None;
        for _ in 0..3 {
            match self.parse_pdf_once(file_name, data, options).await {
                Ok(output) => return Ok(output),
                Err(error) => last_error = Some(error),
            }
        }
        let error = last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown error".to_owned());
        anyhow::bail!("OpenDataLoader service call failed after 3 attempts: {error}")
    }

    async fn parse_pdf_once(
        &self,
        file_name: &str,
        data: &[u8],
        options: &OpenDataLoaderOptions,
    ) -> Result<OpenDataLoaderOutput> {
        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name.to_owned())
            .mime_str("application/pdf")
            .map_err(|error| anyhow::anyhow!("OpenDataLoader PDF multipart: {error}"))?;
        let mut form = reqwest::multipart::Form::new().part("file", part);
        if let Some(hybrid) = options.hybrid.as_deref() {
            form = form.text("hybrid", hybrid.to_owned());
        }
        if let Some(image_output) = options.image_output.as_deref() {
            form = form.text("image_output", image_output.to_owned());
        }
        if let Some(sanitize) = options.sanitize {
            form = form.text(
                "sanitize",
                if sanitize { "true" } else { "false" }.to_owned(),
            );
        }

        let mut request = self
            .client
            .post(format!("{}/file_parse", self.config.api_url))
            .multipart(form);
        if let Some(api_key) = self.config.api_key() {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("OpenDataLoader submit: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| anyhow::anyhow!("OpenDataLoader response read: {error}"))?;
        if !status.is_success() {
            anyhow::bail!("OpenDataLoader HTTP {status}: {body}");
        }
        let payload: OpenDataLoaderResponse = serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("OpenDataLoader response decode: {error}"))?;
        response_to_output(payload.json_doc, payload.md_text.as_deref())
    }
}

#[derive(Deserialize)]
struct OpenDataLoaderResponse {
    json_doc: Option<Value>,
    md_text: Option<String>,
}

#[derive(Default)]
struct ConvertedElements {
    blocks: Vec<String>,
    sections: usize,
    tables: usize,
    images: usize,
}

fn response_to_output(
    json_doc: Option<Value>,
    md_text: Option<&str>,
) -> Result<OpenDataLoaderOutput> {
    let mut converted = ConvertedElements::default();
    if let Some(root) = json_doc.as_ref() {
        walk_elements(root, &mut converted);
    }

    let structured = !converted.blocks.is_empty();
    if converted.sections == 0
        && let Some(markdown) = md_text.map(str::trim).filter(|text| !text.is_empty())
    {
        converted.blocks.push(markdown.to_owned());
    }
    if converted.blocks.is_empty() {
        anyhow::bail!("OpenDataLoader returned no parsed content");
    }

    Ok(OpenDataLoaderOutput {
        content: converted.blocks.join("\n\n"),
        structured,
        section_count: converted.sections,
        table_count: converted.tables,
        image_count: converted.images,
    })
}

fn walk_elements(node: &Value, converted: &mut ConvertedElements) {
    match node {
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str).is_some()
                && (object.get("content").is_some()
                    || object.get("text").is_some()
                    || object.get("cells").is_some()
                    || object.get("html").is_some()
                    || object.get("html_content").is_some())
            {
                convert_element(node, converted);
            }
            for child in object.values() {
                walk_elements(child, converted);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_elements(item, converted);
            }
        }
        _ => {}
    }
}

fn convert_element(element: &Value, converted: &mut ConvertedElements) {
    let element_type = element
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let text = element_text(element);
    match element_type.as_str() {
        "table" => {
            let table = element
                .get("html")
                .or_else(|| element.get("html_content"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .unwrap_or(text.as_str());
            if !table.is_empty() {
                converted
                    .blocks
                    .push(format!("<!--TABLE_START-->\n{table}\n<!--TABLE_END-->"));
                converted.tables += 1;
            }
        }
        "image" | "picture" | "figure" => {
            let caption = if text.is_empty() { "[Image]" } else { &text };
            converted
                .blocks
                .push(format!("<!--IMAGE_START-->\n{caption}\n<!--IMAGE_END-->"));
            converted.images += 1;
        }
        "formula" | "equation" => {
            if !text.is_empty() {
                converted.blocks.push(text);
                converted.sections += 1;
            }
        }
        _ => {
            if !text.is_empty() {
                converted.blocks.push(text);
                converted.sections += 1;
            }
        }
    }
}

fn element_text(element: &Value) -> String {
    for key in ["content", "text"] {
        if let Some(text) = element
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            return text.to_owned();
        }
    }
    cells_text(element.get("cells"))
}

fn cells_text(cells: Option<&Value>) -> String {
    let Some(cells) = cells.and_then(Value::as_array) else {
        return String::new();
    };
    let mut rows: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for cell in cells {
        let row = cell
            .get("row")
            .and_then(Value::as_i64)
            .filter(|row| *row != 0)
            .or_else(|| cell.get("row_index").and_then(Value::as_i64))
            .unwrap_or(0);
        let content = ["content", "text"]
            .into_iter()
            .find_map(|key| cell.get(key).and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned();
        rows.entry(row).or_default().push(content);
    }
    rows.into_values()
        .map(|columns| columns.join(" | "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Legacy local structured-text parser. It remains registered for XML and
/// related formats; remote PDF parsing is handled by `OpenDataLoaderClient`.
#[derive(Default)]
pub struct OpenDataLoaderParser;

impl OpenDataLoaderParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_text(&self, name: &str, data: &[u8]) -> Result<String> {
        let ext = std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "xml" => extract_xml(data),
            "yaml" | "yml" => extract_yaml(data),
            "toml" => Ok(extract_toml(data)),
            "log" | "txt" => Ok(String::from_utf8_lossy(data).to_string()),
            _ => Ok(String::from_utf8_lossy(data).to_string()),
        }
    }
}

impl Parse for OpenDataLoaderParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(name, data)?;
        Ok(new_document(
            name,
            content,
            "application/octet-stream",
            data.len(),
        ))
    }
}

fn extract_xml(data: &[u8]) -> Result<String> {
    let xml = String::from_utf8_lossy(data);
    let re = regex::Regex::new(r"<([a-zA-Z_][^>\s/]*)[^>]*>([^<]*)</([a-zA-Z_][^>\s/]*)>")?;
    let mut text = String::new();
    for cap in re.captures_iter(&xml) {
        if cap[1] != cap[3] {
            continue;
        }
        let tag = &cap[1];
        let value = cap[2].trim();
        if !value.is_empty() {
            text.push_str(&format!("{}: {}\n", tag, value));
        }
    }
    Ok(text)
}

fn extract_yaml(data: &[u8]) -> Result<String> {
    let content = String::from_utf8_lossy(data);
    let mut text = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("---") {
            continue;
        }
        // Convert "key: value" to structured text
        if let Some((k, v)) = trimmed.split_once(':') {
            text.push_str(&format!("{}: {} \n", k.trim(), v.trim()));
        } else {
            text.push_str(&format!("{}\n", trimmed));
        }
    }
    Ok(text)
}

fn extract_toml(data: &[u8]) -> String {
    let content = String::from_utf8_lossy(data);
    let mut text = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') {
            text.push_str(&format!("Section: {}\n", trimmed));
        } else if let Some((k, v)) = trimmed.split_once('=') {
            text.push_str(&format!("{}: {}\n", k.trim(), v.trim().trim_matches('"')));
        } else {
            text.push_str(&format!("{}\n", trimmed));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Multipart, State},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn xml_extraction_matches_only_balanced_tags() {
        let text =
            extract_xml(b"<root><title>Hello</title><bad>Ignored</other><count>2</count></root>")
                .unwrap();
        assert!(text.contains("title: Hello"));
        assert!(text.contains("count: 2"));
        assert!(!text.contains("Ignored"));
    }

    #[test]
    fn opendataloader_config_accepts_ragflow_nested_and_flat_credentials() {
        let nested = OpenDataLoaderConfig::from_ragflow_key(
            r#"{
                "api_key": {
                    "opendataloader_apiserver": " http://odl.example:9383/ ",
                    "OPENDATALOADER_API_KEY": " nested-secret ",
                    "opendataloader_timeout": "not-a-number"
                }
            }"#,
            None,
        )
        .unwrap();
        assert_eq!(nested.api_url(), "http://odl.example:9383");
        assert_eq!(nested.api_key(), Some("nested-secret"));
        assert_eq!(nested.timeout().as_secs(), 600);
        let debug = format!("{nested:?}");
        assert!(!debug.contains("nested-secret"));
        assert!(debug.contains("[REDACTED]"));

        let flat = OpenDataLoaderConfig::from_ragflow_key(
            r#"{
                "OPENDATALOADER_APISERVER": "http://flat.example/",
                "OPENDATALOADER_API_KEY": "flat-secret",
                "OPENDATALOADER_TIMEOUT": "42"
            }"#,
            Some("http://ignored.example"),
        )
        .unwrap();
        assert_eq!(flat.api_url(), "http://flat.example");
        assert_eq!(flat.api_key(), Some("flat-secret"));
        assert_eq!(flat.timeout().as_secs(), 42);

        let explicit_empty = OpenDataLoaderConfig::from_ragflow_key(
            r#"{
                "opendataloader_apiserver": "",
                "OPENDATALOADER_APISERVER": "http://must-not-win.example"
            }"#,
            Some("http://fallback-must-not-win.example"),
        )
        .unwrap();
        assert_eq!(explicit_empty.api_url(), "");

        let invalid_nested = OpenDataLoaderConfig::from_ragflow_key(
            r#"{
                "api_key": "not-an-object",
                "opendataloader_apiserver": "http://sibling-must-not-win.example"
            }"#,
            Some("http://fallback.example"),
        )
        .unwrap();
        assert_ne!(
            invalid_nested.api_url(),
            "http://sibling-must-not-win.example"
        );
    }

    #[test]
    fn opendataloader_response_prefers_json_and_falls_back_to_markdown() {
        let structured = response_to_output(
            Some(json!({
                "type": "document",
                "children": [
                    {"type": "title", "content": "ODL Title"},
                    {"type": "paragraph", "text": "ODL Body"},
                    {
                        "type": "table",
                        "cells": [
                            {"row": 0, "content": "a"},
                            {"row": 0, "content": "b"},
                            {"row": 2, "content": "z"}
                        ]
                    },
                    {"type": "figure", "content": "chart caption"}
                ]
            })),
            Some("# ignored markdown"),
        )
        .unwrap();
        assert!(structured.structured);
        assert_eq!(structured.section_count, 2);
        assert_eq!(structured.table_count, 1);
        assert_eq!(structured.image_count, 1);
        assert!(structured.content.contains("ODL Title"));
        assert!(structured.content.contains("ODL Body"));
        assert!(structured.content.contains("<!--TABLE_START-->"));
        assert!(structured.content.contains("a | b\nz"));
        assert!(structured.content.contains("<!--IMAGE_START-->"));
        assert!(!structured.content.contains("ignored markdown"));

        let fallback = response_to_output(None, Some("# Markdown heading\n\nBody text.")).unwrap();
        assert!(!fallback.structured);
        assert_eq!(fallback.content, "# Markdown heading\n\nBody text.");

        let empty = response_to_output(Some(json!({"type": "document"})), Some("  "));
        assert!(empty.is_err());
    }

    #[derive(Default)]
    struct OpenDataLoaderMockState {
        attempts: AtomicUsize,
        received: Mutex<HashMap<String, String>>,
    }

    async fn health_handler(
        State(state): State<Arc<OpenDataLoaderMockState>>,
        headers: HeaderMap,
    ) -> StatusCode {
        state.received.lock().unwrap().insert(
            "health_authorization".into(),
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        StatusCode::OK
    }

    async fn parse_handler(
        State(state): State<Arc<OpenDataLoaderMockState>>,
        headers: HeaderMap,
        mut multipart: Multipart,
    ) -> Response {
        state.received.lock().unwrap().insert(
            "parse_authorization".into(),
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        while let Some(field) = multipart.next_field().await.unwrap() {
            let field_name = field.name().unwrap_or_default().to_owned();
            let file_name = field.file_name().map(str::to_owned);
            let content_type = field.content_type().map(str::to_owned);
            let body = field.bytes().await.unwrap();
            let mut received = state.received.lock().unwrap();
            received.insert(
                field_name.clone(),
                String::from_utf8_lossy(&body).into_owned(),
            );
            if field_name == "file" {
                received.insert("file_name".into(), file_name.unwrap_or_default());
                received.insert("file_content_type".into(), content_type.unwrap_or_default());
            }
        }

        let attempt = state.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt < 3 {
            return (StatusCode::BAD_GATEWAY, "retry").into_response();
        }
        Json(json!({
            "json_doc": {
                "type": "title",
                "content": "HTTP result"
            },
            "md_text": "# fallback"
        }))
        .into_response()
    }

    #[tokio::test]
    async fn opendataloader_http_contract_health_multipart_and_three_attempts() {
        let state = Arc::new(OpenDataLoaderMockState::default());
        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/file_parse", post(parse_handler))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let key = format!(
            r#"{{
                "opendataloader_apiserver": "http://{address}/",
                "opendataloader_api_key": "odl-secret",
                "opendataloader_timeout": "5"
            }}"#
        );
        let config = OpenDataLoaderConfig::from_ragflow_key(&key, None).unwrap();
        let client = OpenDataLoaderClient::new(config).unwrap();
        assert!(client.check_installation().await);

        let output = client
            .parse_pdf(
                "sample.pdf",
                b"%PDF-1.4\nmock",
                &OpenDataLoaderOptions {
                    hybrid: Some("docling-fast".into()),
                    image_output: Some("embedded".into()),
                    sanitize: Some(true),
                },
            )
            .await
            .unwrap();
        server.abort();

        assert_eq!(state.attempts.load(Ordering::SeqCst), 3);
        assert!(output.content.contains("HTTP result"));
        let received = state.received.lock().unwrap();
        assert_eq!(
            received.get("health_authorization").map(String::as_str),
            Some("Bearer odl-secret")
        );
        assert_eq!(
            received.get("parse_authorization").map(String::as_str),
            Some("Bearer odl-secret")
        );
        assert_eq!(
            received.get("file_name").map(String::as_str),
            Some("sample.pdf")
        );
        assert_eq!(
            received.get("file_content_type").map(String::as_str),
            Some("application/pdf")
        );
        assert_eq!(
            received.get("file").map(String::as_bytes),
            Some(b"%PDF-1.4\nmock".as_slice())
        );
        assert_eq!(
            received.get("hybrid").map(String::as_str),
            Some("docling-fast")
        );
        assert_eq!(
            received.get("image_output").map(String::as_str),
            Some("embedded")
        );
        assert_eq!(received.get("sanitize").map(String::as_str), Some("true"));
    }
}
