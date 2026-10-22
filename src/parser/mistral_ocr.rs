//! Mistral OCR remote PDF parser — RAGFlow v0.27.2
//! `deepdoc/parser/mistral_parser.py` with `rag/llm/ocr_model.py::MistralOcrModel`.
//!
//! Calls Mistral's dedicated `POST /v1/ocr` endpoint (not chat completions)
//! and normalizes the per-block response into RayRAG's marker-aware content:
//! block types map exactly like upstream (`MISTRAL_TYPE_TO_RAGFLOW`), header
//! and footer blocks obey `keep_header_footer`, positions use the shared
//! `@@page\tx0\tx1\ttop\tbott##` tag format, and tables stay inline (HTML or
//! the configured `table_format`). Documents at or below 20 MiB travel as
//! base64 data URIs; larger ones use the `/files` upload → signed URL → OCR →
//! delete flow, exactly like the upstream parser.
//!
//! Bounded divergences (documented for the parity ledger): upstream rescales
//! bboxes to locally rendered page pixels before tagging, and best-effort
//! captions figures through the tenant vision model. RayRAG keeps the OCR
//! coordinate space and renders `<!--IMAGE_START-->` markers; page rendering
//! and figure description stay with the existing figure/vision stage.

use crate::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use serde_json::{Value, json};
use std::time::Duration;

/// Upstream default service root (`MISTRAL_OCR_BASE_URL`).
pub const DEFAULT_BASE_URL: &str = "https://api.mistral.ai/v1";
/// Upstream default model (constructor default in `MistralParser`).
const DEFAULT_MODEL: &str = "mistral-ocr-latest";
/// Upstream default table format.
const DEFAULT_TABLE_FORMAT: &str = "html";
/// Upstream default request timeout (600 seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 600;
/// Upstream inline threshold: larger documents go through `/files`.
const DEFAULT_INLINE_MAX_BYTES: usize = 20 * 1024 * 1024;
/// `common/constants.py::MAXIMUM_PAGE_NUMBER`.
pub const MAXIMUM_PAGE_NUMBER: u32 = 100_000;
/// Upstream `/models` readiness probe timeout.
const MODELS_CHECK_TIMEOUT_SECS: u64 = 10;

/// Upstream `MISTRAL_TYPE_TO_RAGFLOW`, with runtime header/footer handling.
fn internal_type(block_type: &str, keep_header_footer: bool) -> Option<&'static str> {
    match block_type {
        "header" | "footer" => keep_header_footer.then_some("text"),
        "table" => Some("table"),
        "image" => Some("image"),
        "equation" => Some("equation"),
        "code" => Some("code"),
        // text / title / list and unknown types fall back to text so content
        // is never silently dropped (mirrors upstream `_resolve_internal_type`).
        _ => Some("text"),
    }
}

/// RAGFlow-compatible Mistral OCR configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct MistralOcrConfig {
    base_url: String,
    api_key: String,
    model: String,
    table_format: String,
    keep_header_footer: bool,
    request_timeout: Duration,
    inline_max_bytes: usize,
}

impl MistralOcrConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        base_url: &str,
        api_key: &str,
        model: &str,
        table_format: &str,
        keep_header_footer: bool,
    ) -> Result<Self> {
        let base_url = base_url.trim().trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            anyhow::bail!("Mistral OCR requires a base URL (MISTRAL_OCR_BASE_URL)");
        }
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            anyhow::bail!("MISTRAL_OCR_BASE_URL must start with http:// or https://");
        }
        reqwest::Url::parse(&base_url)
            .map_err(|error| anyhow::anyhow!("Invalid Mistral OCR base URL: {error}"))?;
        let model = model.trim();
        if model.is_empty() {
            anyhow::bail!("Mistral OCR requires a model name");
        }
        let table_format = table_format.trim().to_ascii_lowercase();
        if table_format.is_empty() {
            anyhow::bail!("Mistral OCR table format must not be empty");
        }
        Ok(Self {
            base_url,
            api_key: api_key.trim().to_owned(),
            model: model.to_owned(),
            table_format,
            keep_header_footer,
            request_timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
            inline_max_bytes: DEFAULT_INLINE_MAX_BYTES,
        })
    }

    /// Resolve from a RAGFlow provider-config JSON key (flat or nested under
    /// `api_key`), falling back to the `MISTRAL_OCR_*` environment variables
    /// exactly like `MistralOcrModel._resolve`.
    pub fn from_ragflow_key(key: &str, fallback_base_url: Option<&str>) -> Result<Self> {
        let trimmed = key.trim();
        let root: Value = if trimmed.starts_with('{') {
            serde_json::from_str(trimmed).unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        let empty = Value::Object(Default::default());
        let provider = match root.get("api_key") {
            Some(value) if value.is_object() => value,
            // A flat config whose `api_key` is a string keeps that key.
            Some(_) => &root,
            None if root.is_object() => &root,
            None => &empty,
        };
        let key_as_secret = if !trimmed.is_empty() && !trimmed.starts_with('{') {
            trimmed
        } else {
            ""
        };
        let resolve = |ui: &str, upper: &str| -> Option<String> {
            provider
                .get(ui)
                .and_then(value_string)
                .or_else(|| provider.get(upper).and_then(value_string))
                .or_else(|| std::env::var(upper).ok())
                .filter(|value| !value.trim().is_empty())
        };
        let base_url = resolve("mistral_ocr_base_url", "MISTRAL_OCR_BASE_URL")
            .or_else(|| fallback_base_url.map(str::to_owned))
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_owned());
        let api_key =
            resolve("api_key", "MISTRAL_OCR_API_KEY").unwrap_or_else(|| key_as_secret.to_owned());
        let model = resolve("mistral_ocr_model", "MISTRAL_OCR_MODEL")
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned());
        let table_format = resolve("mistral_ocr_table_format", "MISTRAL_OCR_TABLE_FORMAT")
            .unwrap_or_else(|| DEFAULT_TABLE_FORMAT.to_owned());
        let keep_header_footer = resolve(
            "mistral_ocr_keep_header_footer",
            "MISTRAL_OCR_KEEP_HEADER_FOOTER",
        )
        .is_some_and(|value| parse_python_bool(&value));
        Self::new(
            &base_url,
            &api_key,
            &model,
            &table_format,
            keep_header_footer,
        )
    }

    /// Enabled only when an API key is configured; `check_installation`
    /// rejects an empty key exactly like upstream.
    pub fn from_env() -> Result<Option<Self>> {
        let config = Self::from_ragflow_key("", None)?;
        if config.api_key.is_empty() {
            return Ok(None);
        }
        Ok(Some(config))
    }

    pub fn with_request_timeout(mut self, seconds: u64) -> Self {
        if seconds > 0 {
            self.request_timeout = Duration::from_secs(seconds);
        }
        self
    }

    pub fn with_inline_max_bytes(mut self, bytes: usize) -> Self {
        if bytes > 0 {
            self.inline_max_bytes = bytes;
        }
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn table_format(&self) -> &str {
        &self.table_format
    }

    pub fn keep_header_footer(&self) -> bool {
        self.keep_header_footer
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub fn inline_max_bytes(&self) -> usize {
        self.inline_max_bytes
    }
}

/// Mistral OCR result normalized for RayRAG's marker-aware parser pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MistralOcrOutput {
    pub content: String,
    pub structured: bool,
    pub block_count: usize,
    pub page_count: usize,
}

/// Async Mistral OCR HTTP client.
#[derive(Clone)]
pub struct MistralOcrClient {
    config: MistralOcrConfig,
    client: reqwest::Client,
}

impl MistralOcrClient {
    pub fn new(config: MistralOcrConfig) -> Result<Self> {
        Ok(Self {
            config,
            client: crate::common::cmd_timeout::model_client(),
        })
    }

    pub fn from_env() -> Result<Option<Self>> {
        MistralOcrConfig::from_env()?.map(Self::new).transpose()
    }

    pub fn config(&self) -> &MistralOcrConfig {
        &self.config
    }

    /// Upstream `check_installation`: `GET /models` with the bearer key.
    pub async fn check_installation(&self) -> Result<()> {
        if self.config.api_key.is_empty() {
            anyhow::bail!("Mistral API key is not configured.");
        }
        let response = self
            .client
            .get(format!("{}/models", self.config.base_url))
            .bearer_auth(&self.config.api_key)
            .timeout(Duration::from_secs(MODELS_CHECK_TIMEOUT_SECS))
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("Mistral API check failed: {error}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(200).collect();
            anyhow::bail!("Mistral API check failed: HTTP {status} {snippet}");
        }
        Ok(())
    }

    async fn upload_document(&self, file_name: &str, data: &[u8]) -> Result<String> {
        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name.to_owned())
            .mime_str("application/pdf")
            .map_err(|error| anyhow::anyhow!("Mistral /files multipart: {error}"))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("purpose", "ocr");
        let response = self
            .client
            .post(format!("{}/files", self.config.base_url))
            .bearer_auth(&self.config.api_key)
            .timeout(self.config.request_timeout)
            .multipart(form)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("Mistral /files upload failed: {error}"))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let snippet: String = body.chars().take(300).collect();
            anyhow::bail!("Mistral /files upload failed: {status} {snippet}");
        }
        let payload: Value = serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("Mistral /files response decode: {error}"))?;
        let file_id = payload
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Mistral /files response has no id"))?
            .to_owned();

        let signed = async {
            let response = self
                .client
                .get(format!("{}/files/{file_id}/url", self.config.base_url))
                .bearer_auth(&self.config.api_key)
                .query(&[("expiry", "24")])
                .timeout(self.config.request_timeout)
                .send()
                .await
                .map_err(|error| anyhow::anyhow!("Mistral signed-url fetch failed: {error}"))?;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if !status.is_success() {
                let snippet: String = body.chars().take(300).collect();
                anyhow::bail!("Mistral signed-url fetch failed: {status} {snippet}");
            }
            let payload: Value = serde_json::from_str(&body)
                .map_err(|error| anyhow::anyhow!("Mistral signed-url response decode: {error}"))?;
            payload
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("Mistral signed-url response has no url"))
        }
        .await;

        // Best-effort cleanup, exactly like upstream's `finally` branch.
        match self
            .client
            .delete(format!("{}/files/{file_id}", self.config.base_url))
            .bearer_auth(&self.config.api_key)
            .timeout(self.config.request_timeout)
            .send()
            .await
        {
            Ok(response) if !response.status().is_success() => {
                tracing::warn!(
                    file_id,
                    status = %response.status(),
                    "failed to delete uploaded Mistral OCR file"
                );
            }
            Err(error) => {
                tracing::warn!(file_id, %error, "failed to delete uploaded Mistral OCR file");
            }
            _ => {}
        }
        signed
    }

    fn ocr_payload(&self, document: &Value, pages: Option<&[u32]>) -> Value {
        let mut payload = json!({
            "model": self.config.model,
            "document": document,
            "include_blocks": true,
            "table_format": self.config.table_format,
            "include_image_base64": false,
        });
        if let Some(pages) = pages {
            payload["pages"] = json!(pages);
        }
        payload
    }

    async fn post_ocr(&self, payload: &Value) -> Result<Value> {
        let response = self
            .client
            .post(format!("{}/ocr", self.config.base_url))
            .bearer_auth(&self.config.api_key)
            .timeout(self.config.request_timeout)
            .json(payload)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("Mistral OCR request failed: {error}"))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let snippet: String = body.chars().take(300).collect();
            anyhow::bail!("Mistral OCR failed: {status} {snippet}");
        }
        serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("Mistral OCR response decode: {error}"))
    }

    /// Parse one PDF through Mistral OCR. `from_page`/`to_page` behave like
    /// upstream's page selector: `pages` is only sent when the caller
    /// restricted the range.
    pub async fn parse_pdf(
        &self,
        file_name: &str,
        data: &[u8],
        from_page: u32,
        to_page: u32,
    ) -> Result<MistralOcrOutput> {
        if data.is_empty() {
            anyhow::bail!("Mistral OCR PDF content is empty");
        }
        let pages: Option<Vec<u32>> = if from_page > 0 || to_page < MAXIMUM_PAGE_NUMBER {
            let end = to_page.min(MAXIMUM_PAGE_NUMBER);
            if end <= from_page {
                return Ok(MistralOcrOutput {
                    content: String::new(),
                    structured: true,
                    block_count: 0,
                    page_count: 0,
                });
            }
            Some((from_page..end).collect())
        } else {
            None
        };

        let document = if data.len() <= self.config.inline_max_bytes {
            let encoded = BASE64_STANDARD.encode(data);
            json!({
                "type": "document_url",
                "document_url": format!("data:application/pdf;base64,{encoded}"),
            })
        } else {
            let signed = self.upload_document(file_name, data).await?;
            json!({"type": "document_url", "document_url": signed})
        };
        let payload = self.ocr_payload(&document, pages.as_deref());
        let response = self.post_ocr(&payload).await?;
        convert_response(&response, self.config.keep_header_footer)
    }
}

/// Normalize a `/v1/ocr` response into marker-aware content.
fn convert_response(response: &Value, keep_header_footer: bool) -> Result<MistralOcrOutput> {
    let mut sections: Vec<String> = Vec::new();
    let mut image_sequence = 0usize;
    let mut page_count = 0usize;
    if let Some(pages) = response.get("pages").and_then(Value::as_array) {
        page_count = pages.len();
        for page in pages {
            let page_index = page.get("index").and_then(int_like).unwrap_or(0);
            let Some(blocks) = page.get("blocks").and_then(Value::as_array) else {
                continue;
            };
            for block in blocks {
                let block_type = block
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let Some(internal) = internal_type(&block_type, keep_header_footer) else {
                    continue;
                };
                let bbox = block_bbox(block);
                if internal == "image" {
                    if bbox.is_none() {
                        continue;
                    }
                    let tag = line_tag(page_index, bbox);
                    image_sequence += 1;
                    let caption = block
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .trim();
                    let label = if caption.is_empty() {
                        format!("image {image_sequence}")
                    } else {
                        caption.to_owned()
                    };
                    sections.push(format!(
                        "<!--IMAGE_START-->\n{label}{tag}\n<!--IMAGE_END-->"
                    ));
                    continue;
                }
                let text = block
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if text.is_empty() {
                    continue;
                }
                let tag = line_tag(page_index, bbox);
                if internal == "table" {
                    sections.push(format!("<!--TABLE_START-->\n{text}{tag}\n<!--TABLE_END-->"));
                } else {
                    sections.push(format!("{text}{tag}"));
                }
            }
        }
    }
    if sections.is_empty() {
        anyhow::bail!("Mistral OCR returned no usable blocks");
    }
    Ok(MistralOcrOutput {
        block_count: sections.len(),
        page_count,
        content: sections.join("\n\n"),
        structured: true,
    })
}

/// `@@page\tx0\tx1\ttop\tbott##` (1-based page), one decimal like upstream.
fn line_tag(page_index: i64, bbox: Option<[f64; 4]>) -> String {
    let [mut x0, mut top, mut x1, mut bott] = bbox.unwrap_or([0.0; 4]);
    if x0 > x1 {
        std::mem::swap(&mut x0, &mut x1);
    }
    if top > bott {
        std::mem::swap(&mut top, &mut bott);
    }
    format!(
        "@@{}\t{x0:.1}\t{x1:.1}\t{top:.1}\t{bott:.1}##",
        page_index + 1
    )
}

fn block_bbox(block: &Value) -> Option<[f64; 4]> {
    let keys = [
        "top_left_x",
        "top_left_y",
        "bottom_right_x",
        "bottom_right_y",
    ];
    if !keys.iter().any(|key| block.get(key).is_some()) {
        return None;
    }
    let coordinate = |key: &str| block.get(key).and_then(float_like).unwrap_or(0.0);
    Some([
        coordinate("top_left_x"),
        coordinate("top_left_y"),
        coordinate("bottom_right_x"),
        coordinate("bottom_right_y"),
    ])
}

fn value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

fn parse_python_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn int_like(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64)),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .map(|float| float.trunc() as i64),
        _ => None,
    }
}

fn float_like(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        extract::{Multipart, State},
        http::{StatusCode, header},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Capture {
        requests: Arc<Mutex<Vec<Value>>>,
        deletes: Arc<Mutex<usize>>,
    }

    fn mock_response(pages: Value) -> Response {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"pages": pages}).to_string()))
            .unwrap()
    }

    fn ocr_pages() -> Value {
        json!([
            {
                "index": 0,
                "dimensions": {"width": 600, "height": 800},
                "blocks": [
                    {
                        "type": "text",
                        "content": "Hello Mistral",
                        "top_left_x": 1,
                        "top_left_y": 2,
                        "bottom_right_x": 3,
                        "bottom_right_y": 4
                    },
                    {
                        "type": "table",
                        "content": "<table><tr><td>ok</td></tr></table>",
                        "top_left_x": 5,
                        "top_left_y": 6,
                        "bottom_right_x": 7,
                        "bottom_right_y": 8
                    },
                    {
                        "type": "image",
                        "content": "diagram",
                        "top_left_x": 9,
                        "top_left_y": 10,
                        "bottom_right_x": 11,
                        "bottom_right_y": 12
                    },
                    {"type": "header", "content": "page header"}
                ]
            },
            {
                "index": 1,
                "dimensions": {"width": 600, "height": 800},
                "blocks": [
                    {"type": "title", "content": "Second page"}
                ]
            }
        ])
    }

    #[test]
    fn config_defaults_and_provider_key_parsing() {
        let config = MistralOcrConfig::from_ragflow_key("", None).unwrap();
        assert_eq!(config.base_url(), DEFAULT_BASE_URL);
        assert_eq!(config.model(), DEFAULT_MODEL);
        assert_eq!(config.table_format(), "html");
        assert!(!config.keep_header_footer());
        assert_eq!(config.request_timeout(), Duration::from_secs(600));
        assert_eq!(config.inline_max_bytes(), 20 * 1024 * 1024);
        assert!(config.api_key().is_empty());

        let flat = MistralOcrConfig::from_ragflow_key(
            r#"{"mistral_ocr_base_url":"http://127.0.0.1:9000","api_key":"sk-flat","mistral_ocr_table_format":"markdown","mistral_ocr_keep_header_footer":1}"#,
            None,
        )
        .unwrap();
        assert_eq!(flat.base_url(), "http://127.0.0.1:9000");
        assert_eq!(flat.api_key(), "sk-flat");
        assert_eq!(flat.table_format(), "markdown");
        assert!(flat.keep_header_footer());

        let nested = MistralOcrConfig::from_ragflow_key(
            r#"{"api_key":{"api_key":"sk-nested","mistral_ocr_model":"custom-ocr"}}"#,
            Some("http://fallback:1234"),
        )
        .unwrap();
        assert_eq!(nested.api_key(), "sk-nested");
        assert_eq!(nested.model(), "custom-ocr");
        assert_eq!(nested.base_url(), "http://fallback:1234");

        let secret = MistralOcrConfig::from_ragflow_key("sk-plain", None).unwrap();
        assert_eq!(secret.api_key(), "sk-plain");
    }

    #[test]
    fn config_validation_rejects_bad_values() {
        assert!(MistralOcrConfig::new("", "k", "m", "html", false).is_err());
        assert!(MistralOcrConfig::new("ftp://x", "k", "m", "html", false).is_err());
        assert!(MistralOcrConfig::new("http://x", "k", "  ", "html", false).is_err());
        assert!(MistralOcrConfig::new("http://x", "k", "m", " ", false).is_err());
    }

    #[test]
    fn convert_renders_markers_tags_and_header_switch() {
        let response = json!({"pages": ocr_pages()});
        let output = convert_response(&response, false).unwrap();
        assert!(
            output
                .content
                .contains("Hello Mistral@@1\t1.0\t3.0\t2.0\t4.0##")
        );
        assert!(output.content.contains(
            "<!--TABLE_START-->\n<table><tr><td>ok</td></tr></table>@@1\t5.0\t7.0\t6.0\t8.0##\n<!--TABLE_END-->"
        ));
        assert!(
            output.content.contains(
                "<!--IMAGE_START-->\ndiagram@@1\t9.0\t11.0\t10.0\t12.0##\n<!--IMAGE_END-->"
            )
        );
        assert!(!output.content.contains("page header"));
        assert!(
            output
                .content
                .contains("Second page@@2\t0.0\t0.0\t0.0\t0.0##")
        );
        assert_eq!(output.block_count, 4);
        assert_eq!(output.page_count, 2);

        let kept = convert_response(&response, true).unwrap();
        assert!(
            kept.content
                .contains("page header@@1\t0.0\t0.0\t0.0\t0.0##")
        );
        assert_eq!(kept.block_count, 5);
    }

    #[test]
    fn convert_skips_geometry_less_images_and_empty_results() {
        let response = json!({
            "pages": [{
                "index": 0,
                "blocks": [{"type": "image", "content": "no geometry"}]
            }]
        });
        let error = convert_response(&response, false).unwrap_err().to_string();
        assert!(error.contains("no usable blocks"), "{error}");

        let empty = json!({});
        assert!(convert_response(&empty, false).is_err());
    }

    async fn spawn_ok_service(capture: Capture) -> (String, tokio::task::JoinHandle<()>) {
        let capture_ocr = capture.clone();
        let capture_models = capture.clone();
        let app = Router::new()
            .route(
                "/models",
                get(move || {
                    let capture = capture_models.clone();
                    async move {
                        capture
                            .requests
                            .lock()
                            .unwrap()
                            .push(json!({"path": "/models"}));
                        StatusCode::OK
                    }
                }),
            )
            .route(
                "/ocr",
                post(
                    move |State(capture): State<Capture>, body: String| async move {
                        let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                        capture.requests.lock().unwrap().push(payload);
                        mock_response(ocr_pages())
                    },
                ),
            )
            .with_state(capture_ocr);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), server)
    }

    #[tokio::test]
    async fn client_sends_inline_document_and_renders_output() {
        let capture = Capture::default();
        let (base, server) = spawn_ok_service(capture.clone()).await;
        let client = MistralOcrClient::new(
            MistralOcrConfig::new(&base, "sk-test", "mistral-ocr-latest", "html", false).unwrap(),
        )
        .unwrap();
        client.check_installation().await.unwrap();
        let output = client
            .parse_pdf("sample.pdf", b"%PDF-1.4 mock", 0, MAXIMUM_PAGE_NUMBER)
            .await
            .unwrap();
        server.abort();

        assert!(
            output
                .content
                .contains("Hello Mistral@@1\t1.0\t3.0\t2.0\t4.0##")
        );
        let requests = capture.requests.lock().unwrap();
        let ocr = requests
            .iter()
            .find(|request| request.get("model").is_some())
            .expect("ocr payload captured");
        assert_eq!(ocr["model"], "mistral-ocr-latest");
        assert_eq!(ocr["include_blocks"], true);
        assert_eq!(ocr["table_format"], "html");
        assert_eq!(ocr["include_image_base64"], false);
        assert_eq!(ocr["document"]["type"], "document_url");
        let document_url = ocr["document"]["document_url"].as_str().unwrap();
        assert!(document_url.starts_with("data:application/pdf;base64,"));
        assert!(ocr.get("pages").is_none());
    }

    #[tokio::test]
    async fn client_sends_page_selector_and_uses_upload_for_large_files() {
        let capture = Capture::default();
        let capture_files = capture.clone();
        let capture_url = capture.clone();
        let capture_delete = capture.clone();
        let capture_ocr = capture.clone();
        let app = Router::new()
            .route("/models", get(|| async { StatusCode::OK }))
            .route(
                "/files",
                post(move |mut multipart: Multipart| {
                    let capture = capture_files.clone();
                    async move {
                        while let Some(field) = multipart.next_field().await.unwrap() {
                            let name = field.name().unwrap_or_default().to_owned();
                            let _ = field.bytes().await;
                            capture.requests.lock().unwrap().push(json!({"part": name}));
                        }
                        axum::Json(json!({"id": "file-123"}))
                    }
                }),
            )
            .route(
                "/files/file-123/url",
                get(move || {
                    let capture = capture_url.clone();
                    async move {
                        capture
                            .requests
                            .lock()
                            .unwrap()
                            .push(json!({"path": "signed-url"}));
                        axum::Json(json!({"url": "https://signed.example/ocr.pdf"}))
                    }
                }),
            )
            .route(
                "/files/file-123",
                axum::routing::delete(move || {
                    let capture = capture_delete.clone();
                    async move {
                        *capture.deletes.lock().unwrap() += 1;
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .route(
                "/ocr",
                post(move |capture: State<Capture>, body: String| async move {
                    let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                    capture.requests.lock().unwrap().push(payload);
                    mock_response(ocr_pages())
                })
                .with_state(capture_ocr),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = MistralOcrClient::new(
            MistralOcrConfig::new(
                &format!("http://{address}"),
                "sk",
                "mistral-ocr-latest",
                "html",
                false,
            )
            .unwrap()
            .with_inline_max_bytes(1),
        )
        .unwrap();
        let output = client
            .parse_pdf("sample.pdf", b"%PDF larger", 1, 3)
            .await
            .unwrap();
        server.abort();

        assert!(output.content.contains("Hello Mistral"));
        let requests = capture.requests.lock().unwrap();
        let ocr = requests
            .iter()
            .find(|request| request.get("document").is_some())
            .expect("ocr payload captured");
        assert_eq!(
            ocr["document"]["document_url"],
            "https://signed.example/ocr.pdf"
        );
        assert_eq!(ocr["pages"], json!([1, 2]));
        assert!(
            requests
                .iter()
                .any(|request| request.get("part").and_then(Value::as_str) == Some("file")),
            "upload multipart captured: {requests:?}"
        );
        drop(requests);
        assert_eq!(*capture.deletes.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn check_installation_rejects_missing_key_and_http_errors() {
        let app = Router::new().route(
            "/models",
            get(|| async {
                Response::builder()
                    .status(StatusCode::FORBIDDEN)
                    .body(Body::from("forbidden"))
                    .unwrap()
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let no_key = MistralOcrClient::new(
            MistralOcrConfig::new(&format!("http://{address}"), "", "m", "html", false).unwrap(),
        )
        .unwrap();
        let error = no_key.check_installation().await.unwrap_err().to_string();
        assert!(error.contains("API key is not configured"), "{error}");

        let forbidden = MistralOcrClient::new(
            MistralOcrConfig::new(&format!("http://{address}"), "sk", "m", "html", false).unwrap(),
        )
        .unwrap();
        let error = forbidden
            .check_installation()
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("HTTP 403"), "{error}");
        assert!(error.contains("forbidden"), "{error}");
    }

    #[tokio::test]
    async fn post_ocr_surfaces_status_and_body() {
        let app = Router::new()
            .route("/models", get(|| async { StatusCode::OK }))
            .route(
                "/ocr",
                post(|| async {
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Body::from("boom"))
                        .unwrap()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = MistralOcrClient::new(
            MistralOcrConfig::new(&format!("http://{address}"), "sk", "m", "html", false).unwrap(),
        )
        .unwrap();
        let error = client
            .parse_pdf("sample.pdf", b"%PDF", 0, MAXIMUM_PAGE_NUMBER)
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("500"), "{error}");
        assert!(error.contains("boom"), "{error}");
    }
}
