//! Docling parser — IBM Docling-compatible document parsing.
//!
//! RayRAG keeps a lightweight Rust-native fallback (PDF text, DOCX tables,
//! generic text) for environments without a Docling service. The full Docling
//! pipeline is available through `docling-serve` and is selected with the
//! `Docling` layout recognizer plus `DOCLING_SERVER_URL`.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

const DEFAULT_DOCLING_TIMEOUT_SECONDS: u64 = 600;

/// Configuration for a remote `docling-serve` instance.
#[derive(Clone)]
pub struct DoclingConfig {
    server_url: String,
    api_key: Option<String>,
    timeout: Duration,
}

impl std::fmt::Debug for DoclingConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DoclingConfig")
            .field("server_url", &self.server_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl DoclingConfig {
    pub fn from_env() -> Result<Option<Self>> {
        let server_url = std::env::var("DOCLING_SERVER_URL")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty());
        let Some(server_url) = server_url else {
            return Ok(None);
        };
        let api_key = std::env::var("DOCLING_API_KEY")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let timeout = std::env::var("DOCLING_TIMEOUT")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .or_else(|| {
                std::env::var("RAYRAG_DOCLING_TIMEOUT")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|seconds| *seconds > 0)
            })
            .unwrap_or(DEFAULT_DOCLING_TIMEOUT_SECONDS);
        Ok(Some(Self {
            server_url,
            api_key,
            timeout: Duration::from_secs(timeout),
        }))
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoclingOutput {
    pub content: String,
}

#[derive(Clone)]
pub struct DoclingClient {
    config: DoclingConfig,
    client: reqwest::Client,
}

impl DoclingClient {
    pub fn new(config: DoclingConfig) -> Result<Self> {
        if config.server_url().is_empty() {
            anyhow::bail!("Docling requires DOCLING_SERVER_URL");
        }
        let client = reqwest::Client::builder()
            .timeout(config.timeout())
            .build()
            .map_err(|error| anyhow::anyhow!("Docling HTTP client: {error}"))?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Option<Self>> {
        DoclingConfig::from_env()?.map(Self::new).transpose()
    }

    pub async fn check_installation(&self) -> bool {
        if self
            .probe_endpoint("/health", reqwest::Method::GET)
            .await
            .is_ok_and(|response| response.status() == reqwest::StatusCode::OK)
        {
            return true;
        }
        // RAGFlow also supports Docling deployments that expose API/docs endpoints.
        for endpoint in ["/openapi.json", "/docs", "/v1/convert/source"] {
            let response = match self.probe_endpoint(endpoint, reqwest::Method::HEAD).await {
                Ok(response) => Ok(response),
                Err(_) => self.probe_endpoint(endpoint, reqwest::Method::GET).await,
            };
            if response.is_ok_and(|response| {
                matches!(
                    response.status(),
                    reqwest::StatusCode::OK
                        | reqwest::StatusCode::MOVED_PERMANENTLY
                        | reqwest::StatusCode::FOUND
                        | reqwest::StatusCode::TEMPORARY_REDIRECT
                        | reqwest::StatusCode::PERMANENT_REDIRECT
                )
            }) {
                return true;
            }
        }
        false
    }

    async fn probe_endpoint(
        &self,
        endpoint: &str,
        method: reqwest::Method,
    ) -> reqwest::Result<reqwest::Response> {
        let mut request = self
            .client
            .request(method, format!("{}{endpoint}", self.config.server_url()))
            .timeout(Duration::from_secs(5));
        if let Some(api_key) = self.config.api_key() {
            request = request.bearer_auth(api_key);
        }
        request.send().await
    }

    /// Convert one PDF through the stable `v1` API, with a `v1alpha` fallback
    /// for older docling-serve deployments.
    pub async fn parse_pdf(&self, file_name: &str, data: &[u8]) -> Result<DoclingOutput> {
        if data.is_empty() {
            anyhow::bail!("[Docling] file content is empty");
        }
        let encoded = BASE64.encode(data);
        let payload = json!({
            "sources": [{
                "kind": "file",
                "filename": file_name,
                "base64_string": encoded,
            }],
            "options": {
                "from_formats": ["pdf"],
                "to_formats": ["md", "text"],
                "do_ocr": true,
                "force_ocr": false,
                "table_mode": "fast",
                "image_export_mode": "placeholder",
            }
        });

        let mut last_error = None;
        for endpoint in ["/v1/convert/source", "/v1alpha/convert/source"] {
            match self.convert_once(endpoint, &payload).await {
                Ok(output) => return Ok(output),
                Err(error) => last_error = Some(error),
            }
        }
        let error = last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "unknown error".to_owned());
        anyhow::bail!("Docling conversion failed: {error}")
    }

    async fn convert_once(&self, endpoint: &str, payload: &Value) -> Result<DoclingOutput> {
        let mut request = self
            .client
            .post(format!("{}{}", self.config.server_url(), endpoint))
            .json(payload);
        if let Some(api_key) = self.config.api_key() {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("Docling submit: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| anyhow::anyhow!("Docling response read: {error}"))?;
        if !status.is_success() {
            anyhow::bail!("Docling HTTP {status}: {body}");
        }
        let payload = serde_json::from_str::<DoclingResponse>(&body)
            .map_err(|error| anyhow::anyhow!("Docling response decode: {error}"))?;
        let content = payload.content();
        if content.trim().is_empty() {
            anyhow::bail!("Docling returned no usable document content");
        }
        Ok(DoclingOutput { content })
    }
}

#[derive(Deserialize, Default)]
struct DoclingResponse {
    #[serde(default)]
    document: Option<ExportDocument>,
    #[serde(default)]
    documents: Vec<ExportDocument>,
    #[serde(default)]
    results: Vec<DoclingResult>,
}

impl DoclingResponse {
    fn content(&self) -> String {
        let mut docs = Vec::new();
        if let Some(document) = &self.document {
            docs.push(document.clone());
        }
        docs.extend(self.documents.clone());
        for result in &self.results {
            if let Some(document) = &result.document {
                docs.push(document.clone());
            } else if let Some(document) = &result.result {
                docs.push(document.clone());
            }
        }
        for document in docs {
            if let Some(markdown) = document.md_content.as_deref() {
                if !markdown.trim().is_empty() {
                    return markdown.to_owned();
                }
            }
            if let Some(text) = document.text_content.as_deref() {
                if !text.trim().is_empty() {
                    return text.to_owned();
                }
            }
            if let Some(markdown) = document
                .json_content
                .as_ref()
                .and_then(|value| value.get("md_content"))
                .and_then(Value::as_str)
            {
                if !markdown.trim().is_empty() {
                    return markdown.to_owned();
                }
            }
        }
        String::new()
    }
}

#[derive(Deserialize, Clone, Default)]
struct ExportDocument {
    md_content: Option<String>,
    text_content: Option<String>,
    json_content: Option<Value>,
}

#[derive(Deserialize, Default)]
struct DoclingResult {
    document: Option<ExportDocument>,
    result: Option<ExportDocument>,
}

/// Native fallback used when no remote Docling service is configured.
#[derive(Default)]
pub struct DoclingParser;

impl DoclingParser {
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
            "pdf" => {
                let mut text = String::from("<!--DOCUMENT_START-->\n");
                let doc = lopdf::Document::load_mem(data)?;
                for (page_num, &page_id) in &doc.get_pages() {
                    let decoded = super::pdf_stream::page_content_limited(
                        &doc,
                        page_id,
                        super::pdf_stream::stream_limit_bytes(),
                        &format!("PDF page {page_num} content"),
                    )
                    .and_then(|bytes| {
                        lopdf::content::Content::decode(&bytes)
                            .map_err(|error| anyhow::anyhow!("PDF page {page_num}: {error}"))
                    });
                    match decoded {
                        Ok(content) => {
                            let page_text = super::pdf::content_to_text_page(&content);
                            if !page_text.trim().is_empty() {
                                text.push_str(&format!("[PAGE {}]\n{}\n", page_num, page_text));
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, page = page_num, "Skipping PDF page content");
                        }
                    }
                }
                text.push_str("<!--DOCUMENT_END-->\n");
                Ok(text)
            }
            "docx" => {
                let parser = super::docx::DocxParser::new();
                let doc = parser.parse(name, data)?;
                Ok(format!(
                    "<!--DOCUMENT_START-->\n{}\n<!--DOCUMENT_END-->\n",
                    doc.content
                ))
            }
            _ => Ok(format!(
                "<!--DOCUMENT_START-->\n{}\n<!--DOCUMENT_END-->\n",
                String::from_utf8_lossy(data)
            )),
        }
    }
}

impl Parse for DoclingParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(name, data)?;
        Ok(new_document(name, content, "application/pdf", data.len()))
    }
}


