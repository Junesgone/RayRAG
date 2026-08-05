//! OCR provider adapters.
//!
//! The legacy GPU proxy remains available for existing deployments. The
//! PaddleOCR adapter follows RAGFlow v0.26.4's asynchronous
//! submit → poll → JSONL-result contract.

use crate::Result;
use serde_json::{Map, Value};
use std::time::{Duration, Instant};

const DEFAULT_LEGACY_OCR_URL: &str = "http://127.0.0.1:8097";
const DEFAULT_PADDLEOCR_URL: &str = "https://paddleocr.aistudio-app.com";
const SUPPORTED_PADDLEOCR_ALGORITHMS: &[&str] = &[
    "PaddleOCR-VL",
    "PaddleOCR-VL-1.6",
    "PP-OCRv5",
    "PP-OCRv6",
    "PP-StructureV3",
    "PaddleOCR-VL-1.5",
];

/// OCR result from paddleocr-gpu-proxy.
#[derive(Debug, Clone, serde::Deserialize)]
struct OcrResponse {
    #[serde(default)]
    texts: Vec<String>,
    #[serde(default)]
    full_text: String,
}

/// A single layout block from PaddleOCR `layoutParsingResults` — mirrors
/// `paddleocr_parser.py` `parsing_res_list` items. Bboxes are normalized
/// (corners swapped so left≤right and top≤bottom, see `_normalize_bbox`).
#[derive(Debug, Clone)]
pub struct PaddleLayoutBlock {
    /// 1-based page index (upstream uses `page_idx + 1` in position tags).
    pub page: u32,
    /// Block label (`block_label`), e.g. `text`, `table`, `figure`, `title`,
    /// or the fallback markers `markdown` / `ocr` introduced by RayRAG when
    /// the corresponding upstream fallback branch produced the block.
    pub label: String,
    /// Block content with `<img .../>` tags stripped (mirrors
    /// `_remove_images_from_markdown`).
    pub content: String,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Clone)]
enum OcrBackend {
    LegacyProxy { base_url: String },
    PaddleOcr(PaddleOcrConfig),
}

/// RAGFlow-compatible PaddleOCR provider configuration.
#[derive(Clone)]
pub struct PaddleOcrConfig {
    base_url: String,
    access_token: Option<String>,
    algorithm: String,
    request_timeout: Duration,
    initial_poll_interval: Duration,
    max_poll_interval: Duration,
}

impl std::fmt::Debug for PaddleOcrConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PaddleOcrConfig")
            .field("base_url", &self.base_url)
            .field(
                "access_token",
                &self.access_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("algorithm", &self.algorithm)
            .field("request_timeout", &self.request_timeout)
            .field("initial_poll_interval", &self.initial_poll_interval)
            .field("max_poll_interval", &self.max_poll_interval)
            .finish()
    }
}

impl PaddleOcrConfig {
    /// Decode RAGFlow's provider credential JSON. Both the nested UI shape
    /// (`{"api_key": {...}}`) and flat auto-provisioned keys are accepted.
    pub fn from_ragflow_key(key: &str, fallback_base_url: Option<&str>) -> Result<Self> {
        let raw = serde_json::from_str::<Value>(key).unwrap_or_else(|_| Value::Object(Map::new()));
        let raw = raw.as_object().cloned().unwrap_or_default();
        let config = raw
            .get("api_key")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or(raw);

        let resolve = |lower: &str, upper: &str| {
            config
                .get(lower)
                .or_else(|| config.get(upper))
                .and_then(config_string)
                .or_else(|| std::env::var(upper).ok())
        };
        let base_url = resolve("paddleocr_base_url", "PADDLEOCR_BASE_URL")
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                resolve("paddleocr_api_url", "PADDLEOCR_API_URL")
                    .filter(|value| !value.trim().is_empty())
            })
            .or_else(|| fallback_base_url.map(str::to_owned))
            .unwrap_or_else(|| DEFAULT_PADDLEOCR_URL.to_owned());
        let algorithm = resolve("paddleocr_algorithm", "PADDLEOCR_ALGORITHM")
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "PaddleOCR-VL".to_owned());
        if !SUPPORTED_PADDLEOCR_ALGORITHMS.contains(&algorithm.as_str()) {
            anyhow::bail!("Unsupported PaddleOCR algorithm: {algorithm}");
        }
        let access_token = resolve("paddleocr_access_token", "PADDLEOCR_ACCESS_TOKEN")
            .filter(|value| !value.trim().is_empty());
        let request_timeout = std::env::var("PADDLEOCR_REQUEST_TIMEOUT_SECONDS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(600));

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            access_token,
            algorithm,
            request_timeout,
            initial_poll_interval: Duration::from_secs(3),
            max_poll_interval: Duration::from_secs(15),
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn algorithm(&self) -> &str {
        &self.algorithm
    }

    pub fn has_access_token(&self) -> bool {
        self.access_token.is_some()
    }

    pub fn with_request_timing(
        mut self,
        request_timeout: Duration,
        initial_poll_interval: Duration,
        max_poll_interval: Duration,
    ) -> Self {
        self.request_timeout = request_timeout;
        self.initial_poll_interval = initial_poll_interval;
        self.max_poll_interval = max_poll_interval;
        self
    }

    fn validate(&self) -> Result<()> {
        if self.base_url.trim().is_empty() {
            anyhow::bail!("[PaddleOCR] Base URL missing");
        }
        if self.access_token.is_none() {
            anyhow::bail!("[PaddleOCR] Access token not configured");
        }
        if self.request_timeout.is_zero()
            || self.initial_poll_interval.is_zero()
            || self.max_poll_interval.is_zero()
        {
            anyhow::bail!("[PaddleOCR] request and polling durations must be positive");
        }
        Ok(())
    }
}

fn config_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

/// OCR client for the legacy proxy or the RAGFlow PaddleOCR async API.
#[derive(Clone)]
pub struct OcrClient {
    backend: OcrBackend,
    client: reqwest::Client,
}

impl OcrClient {
    /// Create a legacy `POST /ocr` GPU-proxy client.
    pub fn new(base_url: &str) -> Self {
        Self {
            backend: OcrBackend::LegacyProxy {
                base_url: base_url.trim_end_matches('/').to_owned(),
            },
            client: reqwest::Client::new(),
        }
    }

    /// Create a RAGFlow-compatible PaddleOCR async-job client.
    pub fn paddleocr(config: PaddleOcrConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            backend: OcrBackend::PaddleOcr(config),
            client: reqwest::Client::new(),
        })
    }

    /// Resolve an optional deployment adapter without making OCR mandatory.
    pub fn from_env() -> Result<Option<Self>> {
        let provider = std::env::var("RAYRAG_OCR_PROVIDER").unwrap_or_default();
        match provider.trim().to_ascii_lowercase().as_str() {
            "" | "none" | "disabled" => Ok(None),
            "legacy" | "proxy" | "paddleocr-gpu-proxy" => {
                let base_url = std::env::var("RAYRAG_OCR_BASE_URL")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| DEFAULT_LEGACY_OCR_URL.to_owned());
                Ok(Some(Self::new(&base_url)))
            }
            "paddleocr" => {
                let config = PaddleOcrConfig::from_ragflow_key("", None)?;
                Ok(Some(Self::paddleocr(config)?))
            }
            other => anyhow::bail!("Unsupported RAYRAG_OCR_PROVIDER: {other}"),
        }
    }

    /// RAGFlow's Paddle wrapper checks configuration locally; the legacy
    /// proxy retains its network health endpoint.
    pub async fn health(&self) -> Result<bool> {
        match &self.backend {
            OcrBackend::LegacyProxy { base_url } => {
                match self.client.get(format!("{base_url}/health")).send().await {
                    Ok(response) => Ok(response.status().is_success()),
                    Err(_) => Ok(false),
                }
            }
            OcrBackend::PaddleOcr(config) => Ok(config.validate().is_ok()),
        }
    }

    /// Run OCR on raw image bytes (jpg/png).
    /// Returns extracted text.
    pub async fn ocr(&self, image_bytes: &[u8]) -> Result<String> {
        self.ocr_file("image.png", image_bytes).await
    }

    /// Run OCR with a stable upload filename. PaddleOCR accepts PDFs and
    /// images through the same asynchronous job endpoint.
    pub async fn ocr_file(&self, file_name: &str, data: &[u8]) -> Result<String> {
        match &self.backend {
            OcrBackend::LegacyProxy { base_url } => self.legacy_ocr(base_url, data).await,
            OcrBackend::PaddleOcr(config) => self.paddleocr_file(config, file_name, data).await,
        }
    }

    async fn legacy_ocr(&self, base_url: &str, image_bytes: &[u8]) -> Result<String> {
        use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
        let b64 = BASE64.encode(image_bytes);

        let resp = self
            .client
            .post(format!("{base_url}/ocr"))
            .json(&serde_json::json!({"image": b64}))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("OCR API error ({}): {}", status, body);
        }

        let ocr_resp: OcrResponse = resp.json().await?;
        if ocr_resp.full_text.is_empty() {
            Ok(ocr_resp.texts.join("\n"))
        } else {
            Ok(ocr_resp.full_text)
        }
    }

    async fn paddleocr_file(
        &self,
        config: &PaddleOcrConfig,
        file_name: &str,
        data: &[u8],
    ) -> Result<String> {
        let body = self
            .paddleocr_submit_and_fetch(config, file_name, data)
            .await?;
        parse_paddleocr_jsonl(&body)
    }

    /// True when the client is backed by the RAGFlow PaddleOCR async API
    /// (the only backend able to produce layout-parsing results).
    pub fn is_paddleocr(&self) -> bool {
        matches!(self.backend, OcrBackend::PaddleOcr(_))
    }

    /// Run PaddleOCR with layout recognition and return structured blocks
    /// (page / label / normalized bbox / image-stripped content) parsed from
    /// `layoutParsingResults` — the input of the layout parser's section
    /// renderer. Requires the RAGFlow PaddleOCR backend.
    pub async fn paddleocr_layout_file(
        &self,
        file_name: &str,
        data: &[u8],
    ) -> Result<Vec<PaddleLayoutBlock>> {
        match &self.backend {
            OcrBackend::PaddleOcr(config) => {
                let body = self
                    .paddleocr_submit_and_fetch(config, file_name, data)
                    .await?;
                parse_paddleocr_blocks(&body)
            }
            OcrBackend::LegacyProxy { .. } => anyhow::bail!(
                "[PaddleOCR] layout parsing requires the RAGFlow PaddleOCR backend \
                 (RAYRAG_OCR_PROVIDER=paddleocr)"
            ),
        }
    }

    /// Shared RAGFlow PaddleOCR wire flow: submit the file as multipart,
    /// poll `/jobs/{id}` until `done`, then fetch the JSONL result body.
    async fn paddleocr_submit_and_fetch(
        &self,
        config: &PaddleOcrConfig,
        file_name: &str,
        data: &[u8],
    ) -> Result<String> {
        config.validate()?;
        if data.is_empty() {
            anyhow::bail!("[PaddleOCR] file content is empty");
        }

        let deadline = Instant::now() + config.request_timeout;
        let jobs_url = format!("{}/api/v2/ocr/jobs", config.base_url);
        let optional_payload = paddleocr_optional_payload(&config.algorithm);
        let file_part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name.to_owned())
            .mime_str("application/octet-stream")?;
        let form = reqwest::multipart::Form::new()
            .text("model", config.algorithm.clone())
            .text("optionalPayload", serde_json::to_string(&optional_payload)?)
            .part("file", file_part);

        let mut request = self
            .client
            .post(&jobs_url)
            .header("Client-Platform", "ragflow")
            .multipart(form)
            .timeout(remaining(deadline, config.request_timeout)?);
        if let Some(token) = &config.access_token {
            request = request.bearer_auth(token);
        }
        let response = request
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("[PaddleOCR] submit failed: {error}"))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if status != reqwest::StatusCode::OK {
            anyhow::bail!("[PaddleOCR] submit failed: HTTP {status} {body}");
        }
        let submit = serde_json::from_str::<Value>(&body)
            .map_err(|error| anyhow::anyhow!("[PaddleOCR] submit response is not JSON: {error}"))?;
        let job_id = value_string(&submit, &["data", "jobId"])
            .or_else(|| value_string(&submit, &["jobId"]))
            .ok_or_else(|| anyhow::anyhow!("[PaddleOCR] job ID not found in response: {submit}"))?;

        let poll_url = format!("{jobs_url}/{job_id}");
        let mut poll_interval = config.initial_poll_interval;
        let result_url = loop {
            let mut request = self
                .client
                .get(&poll_url)
                .header("Client-Platform", "ragflow")
                .timeout(remaining(deadline, config.request_timeout)?);
            if let Some(token) = &config.access_token {
                request = request.bearer_auth(token);
            }
            let response = request
                .send()
                .await
                .map_err(|error| anyhow::anyhow!("[PaddleOCR] poll failed: {error}"))?;
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            if status != reqwest::StatusCode::OK {
                anyhow::bail!("[PaddleOCR] poll failed: HTTP {status} {body}");
            }
            let poll = serde_json::from_str::<Value>(&body).map_err(|error| {
                anyhow::anyhow!("[PaddleOCR] poll response is not JSON: {error}")
            })?;
            let state = value_string(&poll, &["data", "state"])
                .or_else(|| value_string(&poll, &["state"]))
                .unwrap_or_default();
            match state.as_str() {
                "done" => {
                    let url = value_string(&poll, &["data", "resultJsonUrl"])
                        .or_else(|| value_string(&poll, &["data", "resultUrl", "jsonUrl"]))
                        .or_else(|| value_string(&poll, &["resultJsonUrl"]))
                        .or_else(|| value_string(&poll, &["resultUrl", "jsonUrl"]))
                        .ok_or_else(|| {
                            anyhow::anyhow!("[PaddleOCR] result URL not found: {poll}")
                        })?;
                    break url;
                }
                "failed" => {
                    let message = value_string(&poll, &["data", "errorMsg"])
                        .or_else(|| value_string(&poll, &["errorMsg"]))
                        .unwrap_or_else(|| "Unknown error".to_owned());
                    anyhow::bail!("[PaddleOCR] job failed: {message}");
                }
                _ => {}
            }

            let sleep_for = poll_interval.min(remaining(deadline, config.request_timeout)?);
            tokio::time::sleep(sleep_for).await;
            poll_interval = Duration::from_secs_f64(
                (poll_interval.as_secs_f64() * 1.5).min(config.max_poll_interval.as_secs_f64()),
            );
        };

        let response = self
            .client
            .get(result_url)
            .timeout(remaining(deadline, config.request_timeout)?)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("[PaddleOCR] failed to fetch result: {error}"))?;
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            anyhow::bail!("[PaddleOCR] failed to fetch result: HTTP {status} {body}");
        }
        Ok(body)
    }
}

impl Default for OcrClient {
    fn default() -> Self {
        Self::new(DEFAULT_LEGACY_OCR_URL)
    }
}

fn remaining(deadline: Instant, request_timeout: Duration) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| {
            anyhow::anyhow!("[PaddleOCR] timed out after {}s", request_timeout.as_secs())
        })
}

fn value_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    current
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn paddleocr_optional_payload(algorithm: &str) -> Value {
    let mut payload = serde_json::json!({
        "prettifyMarkdown": true,
        "showFormulaNumber": true,
        "visualize": false
    });
    if matches!(
        algorithm,
        "PaddleOCR-VL" | "PP-OCRv5" | "PP-StructureV3" | "PaddleOCR-VL-1.5"
    ) && let Some(payload) = payload.as_object_mut()
    {
        payload.insert("useDocOrientationClassify".into(), Value::Bool(false));
        payload.insert("useDocUnwarping".into(), Value::Bool(false));
        payload.insert("formatBlockContent".into(), Value::Bool(true));
        payload.insert("mergeLayoutBlocks".into(), Value::Bool(false));
        payload.insert("restructurePages".into(), Value::Bool(false));
    }
    payload
}

/// Normalize a block bbox: coerce to floats and ensure left≤right, top≤bottom
/// (mirrors `paddleocr_parser.py _normalize_bbox`). Malformed boxes collapse
/// to `(0, 0, 0, 0)` like the upstream `len(bbox) < 4` branch.
fn normalize_bbox(bbox: &Value) -> (f64, f64, f64, f64) {
    let items = match bbox {
        Value::Array(items) => items,
        _ => return (0.0, 0.0, 0.0, 0.0),
    };
    if items.len() < 4 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let number = |value: &Value| value.as_f64().unwrap_or(0.0);
    let (mut left, mut top, mut right, mut bottom) = (
        number(&items[0]),
        number(&items[1]),
        number(&items[2]),
        number(&items[3]),
    );
    if left > right {
        std::mem::swap(&mut left, &mut right);
    }
    if top > bottom {
        std::mem::swap(&mut top, &mut bottom);
    }
    (left, top, right, bottom)
}

/// Parse PaddleOCR JSONL into structured layout blocks, preserving page
/// index, block label, and bounding box — the input contract of
/// `paddleocr_parser.py _transfer_to_sections`.
///
/// Fallback chain matches upstream exactly, in document order:
///   1. `layoutParsingResults[].prunedResult.parsing_res_list[]` blocks
///      (with `<img>` tags stripped and bboxes normalized); blocks with empty
///      content after stripping are dropped.
///   2. that page's `markdown.text` when no block survived (label `markdown`).
///   3. `ocrResults[].prunedResult.rec_texts` as a last-resort source
///      (label `ocr`).
pub fn parse_paddleocr_blocks(body: &str) -> Result<Vec<PaddleLayoutBlock>> {
    let image_pattern = regex::Regex::new(r"(?is)<div[^>]*>\s*<img[^>]*/?>\s*</div>|<img[^>]*/?>")?;
    let mut blocks = Vec::new();
    for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let value = serde_json::from_str::<Value>(line)
            .map_err(|error| anyhow::anyhow!("[PaddleOCR] result JSONL parse error: {error}"))?;
        let result = value.get("result").unwrap_or(&Value::Null);
        if let Some(layouts) = result.get("layoutParsingResults").and_then(Value::as_array) {
            for (page_idx, layout) in layouts.iter().enumerate() {
                let mut found = false;
                if let Some(parsing_list) = layout
                    .pointer("/prunedResult/parsing_res_list")
                    .and_then(Value::as_array)
                {
                    for block in parsing_list {
                        let raw = block
                            .get("block_content")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        let content = image_pattern.replace_all(raw, "").trim().to_owned();
                        if content.is_empty() {
                            continue;
                        }
                        let label = block
                            .get("block_label")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        let (left, top, right, bottom) =
                            normalize_bbox(block.get("block_bbox").unwrap_or(&Value::Null));
                        blocks.push(PaddleLayoutBlock {
                            page: page_idx as u32 + 1,
                            label,
                            content,
                            left,
                            top,
                            right,
                            bottom,
                        });
                        found = true;
                    }
                }
                if !found
                    && let Some(markdown) = layout.pointer("/markdown/text").and_then(Value::as_str)
                {
                    let markdown = image_pattern.replace_all(markdown, "").trim().to_owned();
                    if !markdown.is_empty() {
                        blocks.push(PaddleLayoutBlock {
                            page: page_idx as u32 + 1,
                            label: "markdown".to_owned(),
                            content: markdown,
                            left: 0.0,
                            top: 0.0,
                            right: 0.0,
                            bottom: 0.0,
                        });
                    }
                }
            }
        }
        if let Some(results) = result.get("ocrResults").and_then(Value::as_array) {
            for (page_idx, ocr_result) in results.iter().enumerate() {
                if let Some(texts) = ocr_result
                    .pointer("/prunedResult/rec_texts")
                    .and_then(Value::as_array)
                {
                    for text in texts
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                    {
                        blocks.push(PaddleLayoutBlock {
                            page: page_idx as u32 + 1,
                            label: "ocr".to_owned(),
                            content: text.to_owned(),
                            left: 0.0,
                            top: 0.0,
                            right: 0.0,
                            bottom: 0.0,
                        });
                    }
                }
            }
        }
    }
    Ok(blocks)
}

fn parse_paddleocr_jsonl(body: &str) -> Result<String> {
    let blocks = parse_paddleocr_blocks(body)?;
    let layout_texts: Vec<&str> = blocks
        .iter()
        .filter(|block| block.label != "ocr")
        .map(|block| block.content.as_str())
        .collect();
    if layout_texts.is_empty() {
        let ocr_texts: Vec<&str> = blocks
            .iter()
            .filter(|block| block.label == "ocr")
            .map(|block| block.content.as_str())
            .collect();
        Ok(ocr_texts.join("\n"))
    } else {
        Ok(layout_texts.join("\n"))
    }
}

/// Detect if a MIME type supports OCR.
pub fn supports_ocr(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/png"
            | "image/jpeg"
            | "image/jpg"
            | "image/gif"
            | "image/webp"
            | "image/bmp"
            | "image/tiff"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Body,
        extract::{Multipart, Path, State},
        http::{HeaderMap, Response, StatusCode},
        response::IntoResponse,
        routing::{get, post},
    };
    use serde_json::json;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Duration;

    #[derive(Debug)]
    struct Submission {
        authorization: String,
        client_platform: String,
        model: String,
        optional_payload: serde_json::Value,
        file_name: String,
        file: Vec<u8>,
    }

    #[derive(Default)]
    struct MockPaddleState {
        submission: Mutex<Option<Submission>>,
        polls: AtomicUsize,
    }

    async fn submit_job(
        State(state): State<Arc<MockPaddleState>>,
        headers: HeaderMap,
        mut multipart: Multipart,
    ) -> impl IntoResponse {
        let mut model = String::new();
        let mut optional_payload = serde_json::Value::Null;
        let mut file_name = String::new();
        let mut file = Vec::new();
        while let Some(field) = multipart.next_field().await.unwrap() {
            match field.name().unwrap_or_default() {
                "model" => model = field.text().await.unwrap(),
                "optionalPayload" => {
                    optional_payload = serde_json::from_str(&field.text().await.unwrap()).unwrap();
                }
                "file" => {
                    file_name = field.file_name().unwrap_or_default().to_owned();
                    file = field.bytes().await.unwrap().to_vec();
                }
                _ => {}
            }
        }
        *state.submission.lock().unwrap() = Some(Submission {
            authorization: headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
            client_platform: headers
                .get("client-platform")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
            model,
            optional_payload,
            file_name,
            file,
        });
        Json(json!({"data": {"jobId": "job-1"}}))
    }

    async fn poll_job(
        State(state): State<Arc<MockPaddleState>>,
        Path(job_id): Path<String>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        assert_eq!(job_id, "job-1");
        assert_eq!(
            headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer test-secret")
        );
        let poll = state.polls.fetch_add(1, Ordering::SeqCst);
        if poll == 0 {
            return Json(json!({"data": {"state": "running"}}));
        }
        let host = headers
            .get(axum::http::header::HOST)
            .unwrap()
            .to_str()
            .unwrap();
        Json(json!({
            "data": {
                "state": "done",
                "resultUrl": {"jsonUrl": format!("http://{host}/result.jsonl")}
            }
        }))
    }

    async fn result_jsonl() -> Response<Body> {
        Response::builder()
            .status(StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/jsonl")
            .body(Body::from(concat!(
                "{\"result\":{\"layoutParsingResults\":[{\"prunedResult\":{\"parsing_res_list\":[",
                "{\"block_content\":\"Hello <img src=\\\"x\\\"/>\"},",
                "{\"block_content\":\"  World  \"}",
                "]}}]}}\n"
            )))
            .unwrap()
    }

    #[test]
    fn paddleocr_credentials_match_nested_flat_alias_and_algorithm_contract() {
        let nested = PaddleOcrConfig::from_ragflow_key(
            r#"{"api_key":{"paddleocr_api_url":"http://ocr.example/root/","paddleocr_algorithm":"PP-OCRv6","paddleocr_access_token":"nested-secret"}}"#,
            None,
        )
        .unwrap();
        assert_eq!(nested.base_url(), "http://ocr.example/root");
        assert_eq!(nested.algorithm(), "PP-OCRv6");
        assert!(nested.has_access_token());
        assert!(!format!("{nested:?}").contains("nested-secret"));

        let flat = PaddleOcrConfig::from_ragflow_key(
            r#"{"PADDLEOCR_BASE_URL":"http://flat.example","PADDLEOCR_ALGORITHM":"PaddleOCR-VL-1.6","PADDLEOCR_ACCESS_TOKEN":"flat-secret"}"#,
            Some("http://ignored.example"),
        )
        .unwrap();
        assert_eq!(flat.base_url(), "http://flat.example");
        assert_eq!(flat.algorithm(), "PaddleOCR-VL-1.6");

        let error = PaddleOcrConfig::from_ragflow_key(
            r#"{"paddleocr_algorithm":"not-a-ragflow-algorithm"}"#,
            Some("http://ocr.example"),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Unsupported PaddleOCR algorithm")
        );
    }

    #[test]
    fn paddleocr_jsonl_falls_back_to_recognized_text_and_rejects_bad_lines() {
        let fallback = parse_paddleocr_jsonl(concat!(
            "{\"result\":{\"ocrResults\":[{\"prunedResult\":",
            "{\"rec_texts\":[\" First \",\"\",\"Second\"]}}]}}\n"
        ))
        .unwrap();
        assert_eq!(fallback, "First\nSecond");

        let error = parse_paddleocr_jsonl("{not json}\n").unwrap_err();
        assert!(error.to_string().contains("result JSONL parse error"));
    }

    #[tokio::test]
    async fn paddleocr_async_job_uses_ragflow_wire_contract_and_extracts_layout_text() {
        let state = Arc::new(MockPaddleState::default());
        let app = Router::new()
            .route("/api/v2/ocr/jobs", post(submit_job))
            .route("/api/v2/ocr/jobs/{job_id}", get(poll_job))
            .route("/result.jsonl", get(result_jsonl))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let config = PaddleOcrConfig::from_ragflow_key(
            r#"{"api_key":{"paddleocr_algorithm":"PaddleOCR-VL","paddleocr_access_token":"test-secret"}}"#,
            Some(&format!("http://{address}")),
        )
        .unwrap()
        .with_request_timing(
            Duration::from_secs(2),
            Duration::from_millis(1),
            Duration::from_millis(2),
        );
        let client = OcrClient::paddleocr(config).unwrap();
        let result = client
            .ocr_file("scan.png", b"fake-image-content")
            .await
            .unwrap();
        server.abort();

        assert_eq!(result, "Hello\nWorld");
        assert_eq!(state.polls.load(Ordering::SeqCst), 2);
        let submission = state.submission.lock().unwrap();
        let submission = submission.as_ref().unwrap();
        assert_eq!(submission.authorization, "Bearer test-secret");
        assert_eq!(submission.client_platform, "ragflow");
        assert_eq!(submission.model, "PaddleOCR-VL");
        assert_eq!(submission.file_name, "scan.png");
        assert_eq!(submission.file, b"fake-image-content");
        assert_eq!(submission.optional_payload["prettifyMarkdown"], true);
        assert_eq!(submission.optional_payload["showFormulaNumber"], true);
        assert_eq!(submission.optional_payload["visualize"], false);
        assert_eq!(
            submission.optional_payload["useDocOrientationClassify"],
            false
        );
        assert_eq!(submission.optional_payload["formatBlockContent"], true);
    }
}
