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

/// Map a filename to the Docling input-format name. Only formats whose
/// structure Docling can preserve are routed through Docling; legacy binary
/// Office files stay on RayRAG's explicit "convert to DOCX" path.
pub fn source_format(name: &str) -> Option<&'static str> {
    match std::path::Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => Some("pdf"),
        "docx" => Some("docx"),
        "pptx" => Some("pptx"),
        "xlsx" => Some("xlsx"),
        // Docling's native HTML/Markdown readers keep headings and tables
        // better than the generic text fallback.
        "html" | "htm" => Some("html"),
        "md" | "markdown" | "mdx" => Some("md"),
        _ => None,
    }
}

/// Whether the embedded converter (or a remote service) can parse this file
/// without a lossy byte-to-text conversion.
pub fn is_supported_document(name: &str) -> bool {
    matches!(
        source_format(name),
        Some("pdf" | "docx" | "pptx" | "xlsx" | "html" | "md")
    )
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

    /// Convert one supported document through the stable `v1` API, with a
    /// `v1alpha` fallback for older docling-serve deployments.
    pub async fn parse_file(&self, file_name: &str, data: &[u8]) -> Result<DoclingOutput> {
        if data.is_empty() {
            anyhow::bail!("[Docling] file content is empty");
        }
        let format = source_format(file_name).ok_or_else(|| {
            anyhow::anyhow!(
                "[Docling] unsupported input format for {file_name}; supported formats are PDF, DOCX, PPTX, XLSX, HTML and Markdown"
            )
        })?;
        let encoded = BASE64.encode(data);
        let payload = json!({
            "sources": [{
                "kind": "file",
                "filename": file_name,
                "base64_string": encoded,
            }],
            "options": {
                "from_formats": [format],
                "to_formats": ["md", "text"],
                // OCR is only meaningful for scanned PDFs and images; running
                // it on DOCX/XLSX would waste CPU and distort embedded text.
                "do_ocr": format == "pdf",
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

    /// Backwards-compatible PDF entry point used by the CLI and tests.
    pub async fn parse_pdf(&self, file_name: &str, data: &[u8]) -> Result<DoclingOutput> {
        self.parse_file(file_name, data).await
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

/// Convert one document with the embedded Rust port of Docling
/// (`docling-project/docling.rs`). This is the default layout engine and
/// needs no Python, service or model download for the declarative formats and
/// the PDF text layer.
pub fn convert_embedded(
    name: &str,
    data: &[u8],
    media_type: Option<&str>,
) -> Result<DoclingOutput> {
    let source = ::docling::SourceDocument::from_named_bytes(name, data.to_vec(), media_type)
        .map_err(|error| {
            anyhow::anyhow!(
                "[Docling] cannot load {name}: {error}. Supported input formats include PDF, DOCX, PPTX, XLSX, HTML, Markdown, CSV and plain text."
            )
        })?;
    // `pdf-text` is the pure-Rust PDF path compiled into RayRAG by default:
    // text layer, line reconstruction and page structure without ONNX models.
    // Build with RayRAG's optional `docling-ml` feature to enable the layout /
    // table / OCR pipeline when the model files are installed.
    let converter = ::docling::DocumentConverter::new().text_layer_only(true);
    let result = converter.convert(source).map_err(|error| {
        anyhow::anyhow!(
            "[Docling] conversion failed for {name}: {error}. Check that the file is a valid, unencrypted document; scanned PDFs need the optional docling-ml build with OCR models."
        )
    })?;
    let content = result.document.export_to_markdown();
    if content.trim().is_empty() {
        anyhow::bail!(
            "[Docling] produced no text for {name}. The file may be a scanned/image-only PDF; build RayRAG with the `docling-ml` feature and install the Docling OCR models, or upload a text-based PDF/DOCX."
        );
    }
    Ok(DoclingOutput { content })
}

/// Embedded Docling converter used when no remote `docling-serve` is configured.
#[derive(Default)]
pub struct DoclingParser;

impl DoclingParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parse for DoclingParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let mime = super::mime_from_extension(name).unwrap_or("application/octet-stream");
        let output = convert_embedded(name, data, Some(mime))?;
        Ok(new_document(name, output.content, mime, data.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MockDoclingState {
        received: Mutex<HashMap<String, String>>,
    }

    async fn health_handler(
        State(state): State<Arc<MockDoclingState>>,
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

    async fn convert_handler(
        State(state): State<Arc<MockDoclingState>>,
        headers: HeaderMap,
        body: String,
    ) -> Response {
        state.received.lock().unwrap().insert(
            "convert_authorization".into(),
            headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        state
            .received
            .lock()
            .unwrap()
            .insert("convert_body".into(), body.clone());
        let payload: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(payload["sources"][0]["kind"], "file");
        assert_eq!(payload["sources"][0]["filename"], "doc.pdf");
        assert_eq!(payload["options"]["to_formats"][0], "md");
        Json(json!({
            "status": "success",
            "document": {
                "filename": "doc.pdf",
                "md_content": "# Docling title\n\nBody",
                "text_content": "Docling title Body"
            }
        }))
        .into_response()
    }

    #[tokio::test]
    async fn docling_client_converts_pdf_and_prefers_markdown() {
        let state = Arc::new(MockDoclingState::default());
        let app = Router::new()
            .route("/health", get(health_handler))
            .route("/v1/convert/source", post(convert_handler))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let config = DoclingConfig {
            server_url: format!("http://{address}"),
            api_key: Some("doc-secret".into()),
            timeout: Duration::from_secs(10),
        };
        let client = DoclingClient::new(config).unwrap();
        assert!(client.check_installation().await);
        let output = client.parse_pdf("doc.pdf", b"%PDF-1.4").await.unwrap();
        server.abort();

        assert_eq!(output.content, "# Docling title\n\nBody");
        let received = state.received.lock().unwrap();
        assert_eq!(
            received.get("health_authorization").unwrap(),
            "Bearer doc-secret"
        );
        assert_eq!(
            received.get("convert_authorization").unwrap(),
            "Bearer doc-secret"
        );
    }

    #[tokio::test]
    async fn docling_api_endpoints_without_health_reach_the_pdf_pipeline() {
        for endpoint in ["/openapi.json", "/docs", "/v1/convert/source"] {
            let state = Arc::new(MockDoclingState::default());
            let app = Router::new()
                .route(endpoint, axum::routing::head(health_handler))
                .route("/v1/convert/source", post(convert_handler))
                .with_state(state.clone());
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let client = DoclingClient::new(DoclingConfig {
                server_url: format!("http://{address}"),
                api_key: Some("doc-secret".into()),
                timeout: Duration::from_secs(10),
            })
            .unwrap();
            let pipeline = crate::pipeline::Pipeline::new(crate::ParserConfig {
                layout_recognize: "Docling".into(),
                ..crate::ParserConfig::default()
            })
            .with_docling_client(client);
            let directory = tempfile::tempdir().unwrap();
            let file_path = directory.path().join("doc.pdf");
            std::fs::write(&file_path, b"%PDF-1.4").unwrap();

            let (chunks, report) = pipeline
                .process_with_report(file_path.to_str().unwrap())
                .await
                .unwrap();
            server.abort();
            assert!(!chunks.is_empty(), "{endpoint}");
            assert!(
                chunks
                    .iter()
                    .any(|chunk| chunk.content.contains("Docling title")),
                "{endpoint}"
            );
            assert!(
                report
                    .stages
                    .iter()
                    .any(|stage| stage.component == "TokenChunker")
            );
            let received = state.received.lock().unwrap();
            assert_eq!(
                received.get("health_authorization").unwrap(),
                "Bearer doc-secret"
            );
            assert_eq!(
                received.get("convert_authorization").unwrap(),
                "Bearer doc-secret"
            );
        }
    }

    #[tokio::test]
    async fn docling_unavailable_endpoints_preserve_configuration_guidance() {
        let app = Router::new().fallback(|| async { StatusCode::UNAUTHORIZED });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = DoclingClient::new(DoclingConfig {
            server_url: format!("http://{address}"),
            api_key: Some("invalid-test-key".into()),
            timeout: Duration::from_secs(10),
        })
        .unwrap();
        let pipeline = crate::pipeline::Pipeline::new(crate::ParserConfig {
            layout_recognize: "Docling".into(),
            ..crate::ParserConfig::default()
        })
        .with_docling_client(client);
        let directory = tempfile::tempdir().unwrap();
        let file_path = directory.path().join("doc.pdf");
        std::fs::write(&file_path, b"%PDF-1.4").unwrap();
        let error = pipeline
            .process(file_path.to_str().unwrap())
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("DOCLING_SERVER_URL"), "{error}");
        assert!(error.contains("DOCLING_API_KEY"), "{error}");
        assert!(error.contains("/openapi.json"), "{error}");
        assert!(error.contains("reparse"), "{error}");
        assert!(!error.contains("invalid-test-key"), "{error}");
    }

    #[tokio::test]
    async fn docling_conversion_failure_preserves_configuration_guidance() {
        let app = Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .fallback(|| async { StatusCode::UNAUTHORIZED });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = DoclingClient::new(DoclingConfig {
            server_url: format!("http://{address}"),
            api_key: Some("private-test-key".into()),
            timeout: Duration::from_secs(10),
        })
        .unwrap();
        let pipeline = crate::pipeline::Pipeline::new(crate::ParserConfig {
            layout_recognize: "Docling".into(),
            ..crate::ParserConfig::default()
        })
        .with_docling_client(client);
        let directory = tempfile::tempdir().unwrap();
        let file_path = directory.path().join("doc.pdf");
        std::fs::write(&file_path, b"%PDF-1.4").unwrap();
        let result = pipeline.process(file_path.to_str().unwrap()).await;
        server.abort();
        let error = result.unwrap_err().to_string();
        for expected in [
            "Docling parsing failed",
            "401",
            "DOCLING_SERVER_URL",
            "DOCLING_API_KEY",
            "重新解析",
            "模型加载",
        ] {
            assert!(error.contains(expected), "{error}");
        }
        assert!(!error.contains("private-test-key"), "{error}");
    }

    #[test]
    fn embedded_docling_converts_markdown_and_preserves_structure() {
        let output = convert_embedded(
            "notes.md",
            b"# Heading\n\nParagraph one.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
            Some("text/markdown"),
        )
        .expect("embedded docling must parse markdown");
        assert!(output.content.contains("Heading"), "{}", output.content);
        assert!(
            output.content.contains("Paragraph one"),
            "{}",
            output.content
        );
        assert!(output.content.contains("|   a"), "{}", output.content);
    }

    #[test]
    fn embedded_docling_converts_a_docx_fixture() {
        let output = convert_embedded(
            "smoke.docx",
            include_bytes!("../../tests/fixtures/parsing/rayrag-smoke.docx"),
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        )
        .expect("embedded docling must parse DOCX");
        assert!(
            output.content.contains("pipeline docx marker"),
            "{}",
            output.content
        );
    }

    #[test]
    fn docling_config_rejects_empty_server_url() {
        let error = match DoclingClient::new(DoclingConfig {
            server_url: String::new(),
            api_key: None,
            timeout: Duration::from_secs(10),
        }) {
            Ok(_) => panic!("empty Docling server URL must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("DOCLING_SERVER_URL"));
    }
}
