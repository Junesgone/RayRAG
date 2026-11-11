//! MinerU PDF parsers.
//!
//! The remote client follows both fixed RAGFlow v0.26.4 protocols: the Python
//! parser's synchronous ZIP/content-list response and the Go parser's
//! submit/task-result Markdown response. The legacy local parser remains a
//! lightweight, dependency-free fallback for direct PDF text extraction.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use serde_json::Value;
use std::fmt;
use std::io::{Cursor, Read};
use std::time::{Duration, Instant};

const MINERU_BACKENDS: [&str; 7] = [
    "pipeline",
    "vlm-http-client",
    "vlm-transformers",
    "vlm-vllm-engine",
    "vlm-mlx-engine",
    "vlm-vllm-async-engine",
    "vlm-lmdeploy-engine",
];
const MINERU_PARSE_METHODS: [&str; 3] = ["auto", "txt", "ocr"];
const MINERU_ZIP_JSON_LIMIT: u64 = 64 * 1024 * 1024;

/// RAGFlow-compatible configuration for the local MinerU HTTP service.
#[derive(Clone)]
pub struct MinerUConfig {
    api_url: String,
    api_key: Option<String>,
    output_dir: Option<String>,
    backend: String,
    server_url: Option<String>,
    delete_output: bool,
    request_timeout: Duration,
    poll_timeout: Duration,
}

impl fmt::Debug for MinerUConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MinerUConfig")
            .field("api_url", &self.api_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("output_dir", &self.output_dir)
            .field("backend", &self.backend)
            .field("server_url", &self.server_url)
            .field("delete_output", &self.delete_output)
            .field("request_timeout", &self.request_timeout)
            .field("poll_timeout", &self.poll_timeout)
            .finish()
    }
}

impl MinerUConfig {
    /// Resolve the fixed wrapper's nested/flat key JSON and environment aliases.
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

        let api_url = resolve_mineru_value(provider, "mineru_apiserver", "MINERU_APISERVER")
            .or_else(|| fallback_api_url.map(str::to_owned))
            .unwrap_or_default()
            .trim()
            .trim_end_matches('/')
            .to_owned();
        let api_key = resolve_mineru_value(provider, "mineru_api_key", "MINERU_API_KEY")
            .and_then(non_empty_trimmed);
        let output_dir = resolve_mineru_value(provider, "mineru_output_dir", "MINERU_OUTPUT_DIR")
            .and_then(non_empty_trimmed);
        let backend = resolve_mineru_value(provider, "mineru_backend", "MINERU_BACKEND")
            .and_then(non_empty_trimmed)
            .unwrap_or_else(|| "pipeline".to_owned());
        if !MINERU_BACKENDS.contains(&backend.as_str()) {
            anyhow::bail!(
                "Invalid MinerU backend '{backend}'; expected one of {}",
                MINERU_BACKENDS.join(", ")
            );
        }
        let server_url = resolve_mineru_value(provider, "mineru_server_url", "MINERU_SERVER_URL")
            .and_then(non_empty_trimmed)
            .map(|url| url.trim_end_matches('/').to_owned());
        let delete_output =
            resolve_mineru_value(provider, "mineru_delete_output", "MINERU_DELETE_OUTPUT")
                .map(|value| parse_mineru_bool(&value))
                .transpose()?
                .unwrap_or(true);
        let request_timeout = resolve_mineru_value(
            provider,
            "mineru_request_timeout_seconds",
            "MINERU_REQUEST_TIMEOUT_SECONDS",
        )
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(1800);
        let poll_timeout =
            resolve_mineru_value(provider, "mineru_timeout_seconds", "MINERU_TIMEOUT_SECONDS")
                .or_else(|| {
                    std::env::var("MINERU_POLL_TIMEOUT_SECONDS")
                        .ok()
                        .and_then(non_empty_trimmed)
                })
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|seconds| *seconds > 0)
                .unwrap_or(30);

        Ok(Self {
            api_url,
            api_key,
            output_dir,
            backend,
            server_url,
            delete_output,
            request_timeout: Duration::from_secs(request_timeout),
            poll_timeout: Duration::from_secs(poll_timeout),
        })
    }

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

    pub fn output_dir(&self) -> Option<&str> {
        self.output_dir.as_deref()
    }

    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn server_url(&self) -> Option<&str> {
        self.server_url.as_deref()
    }

    pub fn delete_output(&self) -> bool {
        self.delete_output
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    pub fn poll_timeout(&self) -> Duration {
        self.poll_timeout
    }
}

fn resolve_mineru_value(provider: &Value, lower: &str, upper: &str) -> Option<String> {
    if let Some(value) = provider.get(lower) {
        return mineru_value_string(value);
    }
    if let Some(value) = provider.get(upper) {
        return mineru_value_string(value);
    }
    std::env::var(upper).ok()
}

fn mineru_value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(if *value { "1" } else { "0" }.to_owned()),
        _ => None,
    }
}

fn non_empty_trimmed(value: impl AsRef<str>) -> Option<String> {
    let value = value.as_ref().trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse_mineru_bool(value: &str) -> Result<bool> {
    let value = value
        .trim()
        .parse::<i64>()
        .map_err(|_| anyhow::anyhow!("MINERU_DELETE_OUTPUT must be an integer boolean"))?;
    Ok(value != 0)
}

/// Per-document MinerU options stored in RAGFlow's parser configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinerURequestOptions {
    pub language: String,
    pub method: String,
    pub formula_enable: bool,
    pub table_enable: bool,
}

impl Default for MinerURequestOptions {
    fn default() -> Self {
        Self {
            language: "English".to_owned(),
            method: "auto".to_owned(),
            formula_enable: true,
            table_enable: true,
        }
    }
}

impl MinerURequestOptions {
    pub fn from_parser_config(config: &crate::ParserConfig) -> Result<Self> {
        let options = Self {
            language: config.mineru_lang.clone(),
            method: config.mineru_parse_method.clone(),
            formula_enable: config.mineru_formula_enable,
            table_enable: config.mineru_table_enable,
        };
        options.validate()?;
        Ok(options)
    }

    fn validate(&self) -> Result<()> {
        if !MINERU_PARSE_METHODS.contains(&self.method.as_str()) {
            anyhow::bail!(
                "Invalid MinerU parse method '{}'; expected auto, txt, or ocr",
                self.method
            );
        }
        Ok(())
    }

    fn language_code(&self) -> &'static str {
        match self.language.as_str() {
            "English" => "en",
            "Chinese" => "ch",
            "Traditional Chinese" => "chinese_cht",
            "Russian" | "Ukrainian" => "east_slavic",
            "Indonesian" | "Spanish" | "Vietnamese" | "Portuguese BR" | "German" | "French"
            | "Italian" | "Turkish" => "latin",
            "Japanese" => "japan",
            "Korean" => "korean",
            "Tamil" => "ta",
            "Telugu" => "te",
            "Kannada" => "ka",
            "Thai" => "th",
            "Greek" => "el",
            "Hindi" => "devanagari",
            "Bulgarian" => "cyrillic",
            _ => "ch",
        }
    }
}

/// MinerU result normalized for RayRAG's marker-aware parser pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinerUOutput {
    pub content: String,
    pub structured: bool,
    pub block_count: usize,
}

/// HTTP client supporting both fixed RAGFlow MinerU service protocols.
#[derive(Clone)]
pub struct MinerUClient {
    config: MinerUConfig,
    client: reqwest::Client,
}

impl MinerUClient {
    pub fn new(config: MinerUConfig) -> Result<Self> {
        if config.api_url.is_empty() {
            anyhow::bail!("MinerU requires mineru_apiserver or MINERU_APISERVER");
        }
        let client = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|error| anyhow::anyhow!("MinerU HTTP client: {error}"))?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Option<Self>> {
        MinerUConfig::from_env()?.map(Self::new).transpose()
    }

    /// Probe the Python service's fixed OpenAPI endpoint.
    pub async fn check_installation(&self) -> Result<()> {
        let mut request = self
            .client
            .head(format!("{}/openapi.json", self.config.api_url))
            .timeout(Duration::from_secs(5));
        if let Some(api_key) = self.config.api_key() {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("MinerU API check failed: {error}"))?;
        if !matches!(response.status().as_u16(), 200 | 301 | 302 | 307 | 308) {
            anyhow::bail!(
                "MinerU API not accessible: {}/openapi.json returned {}",
                self.config.api_url,
                response.status()
            );
        }

        if self.config.backend == "vlm-http-client" {
            let server_url = self.config.server_url.as_deref().ok_or_else(|| {
                anyhow::anyhow!("MINERU_SERVER_URL is required for vlm-http-client backend")
            })?;
            // The fixed Python parser treats this probe as informational once a
            // non-empty URL exists.
            let _ = self
                .client
                .head(server_url)
                .timeout(Duration::from_secs(5))
                .send()
                .await;
        }
        Ok(())
    }

    pub async fn parse_pdf(
        &self,
        file_name: &str,
        data: &[u8],
        options: &MinerURequestOptions,
    ) -> Result<MinerUOutput> {
        if data.is_empty() {
            anyhow::bail!("MinerU PDF content is empty");
        }
        options.validate()?;

        let upload_name = mineru_upload_name(file_name);
        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(upload_name.clone())
            .mime_str("application/pdf")
            .map_err(|error| anyhow::anyhow!("MinerU PDF multipart: {error}"))?;
        let mut form = reqwest::multipart::Form::new()
            .part("files", part)
            .text("output_dir", "./output")
            .text("lang_list", options.language_code().to_owned())
            .text("backend", self.config.backend.clone())
            .text("parse_method", options.method.clone())
            .text("formula_enable", python_bool(options.formula_enable))
            .text("table_enable", python_bool(options.table_enable))
            .text("return_md", "True")
            .text("return_middle_json", "True")
            .text("return_model_output", "True")
            .text("return_content_list", "True")
            .text("return_images", "True")
            .text("response_format_zip", "True")
            .text("start_page_id", "0")
            .text("end_page_id", "99999");
        if let Some(server_url) = self.config.server_url.as_deref() {
            form = form.text("server_url", server_url.to_owned());
        }

        let mut request = self
            .client
            .post(format!("{}/file_parse", self.config.api_url))
            .header(reqwest::header::ACCEPT, "application/json")
            .multipart(form);
        if let Some(api_key) = self.config.api_key() {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("MinerU submit: {error}"))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let body = response
            .bytes()
            .await
            .map_err(|error| anyhow::anyhow!("MinerU response read: {error}"))?;
        if !status.is_success() {
            anyhow::bail!("MinerU HTTP {status}: {}", String::from_utf8_lossy(&body));
        }

        if content_type.starts_with("application/zip") {
            return output_from_zip(&body, &upload_name, options.table_enable);
        }

        let payload: Value = serde_json::from_slice(&body)
            .map_err(|error| anyhow::anyhow!("MinerU submit response decode: {error}"))?;
        let task_id = payload
            .pointer("/data/task_id")
            .or_else(|| payload.get("task_id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|task_id| !task_id.is_empty())
            .ok_or_else(|| anyhow::anyhow!("MinerU submit response has no task_id: {payload}"))?;
        let markdown = self.poll_task(task_id).await?;
        Ok(MinerUOutput {
            content: markdown,
            structured: false,
            block_count: 0,
        })
    }

    async fn poll_task(&self, task_id: &str) -> Result<String> {
        let deadline = Instant::now() + self.config.poll_timeout;
        let mut last_error = anyhow::anyhow!("empty MinerU task content");
        loop {
            match self.poll_task_once(task_id).await {
                Ok(content) if !content.trim().is_empty() => return Ok(content),
                Ok(_) => last_error = anyhow::anyhow!("empty MinerU task content"),
                Err(error) => last_error = error,
            }
            let now = Instant::now();
            if now >= deadline {
                anyhow::bail!("timed out waiting for MinerU task {task_id}: {last_error}");
            }
            tokio::time::sleep(
                Duration::from_millis(200).min(deadline.saturating_duration_since(now)),
            )
            .await;
        }
    }

    async fn poll_task_once(&self, task_id: &str) -> Result<String> {
        let url = mineru_task_result_url(&self.config.api_url, task_id)?;
        let mut request = self.client.get(url);
        if let Some(api_key) = self.config.api_key() {
            request = request.bearer_auth(api_key);
        }
        let response = request
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("MinerU result request: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| anyhow::anyhow!("MinerU result read: {error}"))?;
        if !matches!(status.as_u16(), 200 | 202) {
            anyhow::bail!("MinerU result HTTP {status}: {body}");
        }
        let payload: Value = serde_json::from_str(&body)
            .map_err(|error| anyhow::anyhow!("MinerU result decode: {error}"))?;
        let results = payload
            .get("results")
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow::anyhow!("MinerU result is missing results"))?;
        results
            .values()
            .filter_map(Value::as_object)
            .find_map(|file| file.get("md_content").and_then(Value::as_str))
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("MinerU result has no md_content"))
    }
}

fn mineru_upload_name(file_name: &str) -> String {
    let stem = std::path::Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("document")
        .replace(' ', "");
    format!("{}.pdf", if stem.is_empty() { "document" } else { &stem })
}

fn python_bool(value: bool) -> String {
    (if value { "True" } else { "False" }).to_owned()
}

fn mineru_task_result_url(base_url: &str, task_id: &str) -> Result<reqwest::Url> {
    let mut url = reqwest::Url::parse(&format!("{}/", base_url.trim_end_matches('/')))
        .map_err(|error| anyhow::anyhow!("Invalid MinerU API URL: {error}"))?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("MinerU API URL cannot be a base URL"))?;
        segments.pop_if_empty();
        segments.push("tasks");
        segments.push(task_id);
        segments.push("result");
    }
    Ok(url)
}

fn output_from_zip(body: &[u8], upload_name: &str, table_enable: bool) -> Result<MinerUOutput> {
    let mut archive = zip::ZipArchive::new(Cursor::new(body))
        .map_err(|error| anyhow::anyhow!("MinerU ZIP decode: {error}"))?;
    let stem = std::path::Path::new(upload_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let expected = format!("{stem}_content_list.json");
    let mut selected = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| anyhow::anyhow!("MinerU ZIP entry: {error}"))?;
        let name = entry.name().replace('\\', "/");
        let base_name = name.rsplit('/').next().unwrap_or_default();
        let rank = if base_name == expected {
            Some(0)
        } else if base_name == "content_list.json" {
            Some(1)
        } else if base_name.ends_with("_content_list.json") {
            Some(2)
        } else {
            None
        };
        if let Some(rank) = rank
            && selected.is_none_or(|(best_rank, _)| rank < best_rank)
        {
            selected = Some((rank, index));
        }
    }
    let (_, index) =
        selected.ok_or_else(|| anyhow::anyhow!("MinerU ZIP has no content_list JSON"))?;
    let mut entry = archive
        .by_index(index)
        .map_err(|error| anyhow::anyhow!("MinerU ZIP content-list entry: {error}"))?;
    if entry.size() > MINERU_ZIP_JSON_LIMIT {
        anyhow::bail!(
            "MinerU content-list JSON exceeds {} bytes",
            MINERU_ZIP_JSON_LIMIT
        );
    }
    let mut json = String::new();
    entry
        .read_to_string(&mut json)
        .map_err(|error| anyhow::anyhow!("MinerU content-list read: {error}"))?;
    let outputs: Value = serde_json::from_str(&json)
        .map_err(|error| anyhow::anyhow!("MinerU content-list decode: {error}"))?;
    let outputs = outputs
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("MinerU content-list root must be an array"))?;
    convert_content_list(outputs, table_enable)
}

fn convert_content_list(outputs: &[Value], table_enable: bool) -> Result<MinerUOutput> {
    let mut blocks = Vec::new();
    for output in outputs {
        let content_type = output
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (section, marker) = match content_type {
            "text" | "equation" => (
                output
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                None,
            ),
            "table" => {
                let mut section = output
                    .get("table_body")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                section.push_str(&mineru_string_list(output.get("table_caption")).join("\n"));
                section.push_str(&mineru_string_list(output.get("table_footnote")).join("\n"));
                if section.trim().is_empty() {
                    section = "FAILED TO PARSE TABLE".to_owned();
                }
                (section, Some("table"))
            }
            "image" => {
                let mut section = mineru_string_list(output.get("image_caption")).join("");
                section.push('\n');
                section.push_str(&mineru_string_list(output.get("image_footnote")).join(""));
                (section, Some("image"))
            }
            "code" => {
                let mut section = output
                    .get("code_body")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                section.push_str(&mineru_string_list(output.get("code_caption")).join("\n"));
                (section, None)
            }
            "list" => (
                mineru_string_list(output.get("list_items")).join("\n"),
                None,
            ),
            "header" | "footer" | "page_number" | "discarded" => continue,
            _ => continue,
        };
        let section = if table_enable {
            section.trim().to_owned()
        } else {
            sanitize_section_text(&section)?
        };
        if section.is_empty() {
            continue;
        }
        blocks.push(match marker {
            Some("table") => {
                format!("<!--TABLE_START-->\n{section}\n<!--TABLE_END-->")
            }
            Some("image") => {
                format!("<!--IMAGE_START-->\n{section}\n<!--IMAGE_END-->")
            }
            _ => section,
        });
    }
    if blocks.is_empty() {
        anyhow::bail!("MinerU returned no usable content blocks");
    }
    Ok(MinerUOutput {
        block_count: blocks.len(),
        content: blocks.join("\n\n"),
        structured: true,
    })
}

fn mineru_string_list(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn sanitize_section_text(section: &str) -> Result<String> {
    if section.is_empty() {
        return Ok(String::new());
    }
    let section = quick_xml::escape::unescape(section)
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| section.to_owned());
    let section = regex::Regex::new(r"(?is)<\s*br\s*/?\s*>")?.replace_all(&section, "\n");
    let section = regex::Regex::new(r"(?is)</\s*(p|div|li|tr|h[1-6]|table|caption)\s*>")?
        .replace_all(&section, "\n");
    let section = regex::Regex::new(r"(?is)<[^>]+>")?.replace_all(&section, "");
    let section = regex::Regex::new(r"[ \t]+\n")?.replace_all(&section, "\n");
    let section = regex::Regex::new(r"\n{3,}")?.replace_all(&section, "\n\n");
    let section = regex::Regex::new(r"[ \t]{2,}")?.replace_all(&section, " ");
    Ok(section.trim().to_owned())
}

/// Lightweight local fallback that reads PDF text without a MinerU service.
#[derive(Default)]
pub struct MinerUParser;

impl MinerUParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        let doc = lopdf::Document::load_mem(data)?;
        let mut text = String::new();

        // Extract with structure detection
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
                        text.push_str(&format!("# Page {}\n", page_num));
                        text.push_str(&detect_structure(&page_text));
                        text.push('\n');
                    }
                }
                // A capped refusal must be visible, not an empty page.
                Err(error) => {
                    tracing::warn!(%error, page = page_num, "Skipping PDF page content");
                }
            }
        }

        Ok(text)
    }
}

impl Parse for MinerUParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(data)?;
        Ok(new_document(name, content, "application/pdf", data.len()))
    }
}

/// Detect document structure: headings, paragraphs, lists.
fn detect_structure(text: &str) -> String {
    let mut result = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Heading detection: short lines with title case or numbers
        if trimmed.len() < 80
            && (trimmed.chars().next().is_some_and(|c| c.is_numeric())
                || trimmed == trimmed.to_uppercase()
                || trimmed.ends_with(':'))
        {
            result.push_str(&format!("## {}\n", trimmed));
        } else if trimmed.starts_with("•") || trimmed.starts_with("-") || trimmed.starts_with("*")
        {
            result.push_str(&format!("- {}\n", trimmed[1..].trim()));
        } else if trimmed.chars().next().is_some_and(|c| c.is_numeric())
            && trimmed.chars().nth(1).is_some_and(|c| c == '.')
        {
            result.push_str(&format!("- {}\n", trimmed));
        } else {
            result.push_str(&format!("{}\n", trimmed));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::{Multipart, State},
        http::{HeaderMap, HeaderValue, StatusCode, header},
        response::{IntoResponse, Response},
        routing::{get, head, post},
    };
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::{Cursor, Write};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn mineru_config_accepts_nested_flat_aliases_and_redacts_secrets() {
        let nested = MinerUConfig::from_ragflow_key(
            r#"{
                "api_key": {
                    "mineru_apiserver": " http://mineru.example/ ",
                    "MINERU_API_KEY": " nested-secret ",
                    "mineru_backend": "vlm-http-client",
                    "mineru_server_url": " http://vlm.example/ ",
                    "mineru_delete_output": "0",
                    "mineru_timeout_seconds": "12"
                }
            }"#,
            None,
        )
        .unwrap();
        assert_eq!(nested.api_url(), "http://mineru.example");
        assert_eq!(nested.api_key(), Some("nested-secret"));
        assert_eq!(nested.backend(), "vlm-http-client");
        assert_eq!(nested.server_url(), Some("http://vlm.example"));
        assert!(!nested.delete_output());
        assert_eq!(nested.poll_timeout().as_secs(), 12);
        assert!(!format!("{nested:?}").contains("nested-secret"));
        assert!(format!("{nested:?}").contains("[REDACTED]"));

        let flat = MinerUConfig::from_ragflow_key(
            r#"{
                "MINERU_APISERVER": "http://flat.example/",
                "MINERU_BACKEND": "pipeline",
                "MINERU_DELETE_OUTPUT": 1
            }"#,
            Some("http://ignored.example"),
        )
        .unwrap();
        assert_eq!(flat.api_url(), "http://flat.example");
        assert!(flat.delete_output());

        let explicit_empty = MinerUConfig::from_ragflow_key(
            r#"{
                "mineru_apiserver": "",
                "MINERU_APISERVER": "http://must-not-win.example"
            }"#,
            Some("http://fallback-must-not-win.example"),
        )
        .unwrap();
        assert_eq!(explicit_empty.api_url(), "");

        let error = MinerUConfig::from_ragflow_key(
            r#"{"mineru_apiserver":"http://mineru","mineru_backend":"unknown"}"#,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Invalid MinerU backend"));
    }

    #[test]
    fn mineru_content_list_filters_chrome_and_converts_supported_blocks() {
        let outputs = json!([
            {"type":"header","text":"duplicate title","page_idx":0,"bbox":[0,0,1,1]},
            {"type":"text","text":"&lt;p&gt;Body&lt;/p&gt;","page_idx":0,"bbox":[0,0,1,1]},
            {
                "type":"table",
                "table_body":"&lt;table&gt;&lt;tr&gt;&lt;td&gt;A&lt;/td&gt;&lt;/tr&gt;&lt;/table&gt;",
                "table_caption":["Caption"],
                "page_idx":0,
                "bbox":[0,0,1,1]
            },
            {
                "type":"image",
                "image_caption":["Figure"],
                "image_footnote":["Footnote"],
                "page_idx":0,
                "bbox":[0,0,1,1]
            },
            {"type":"equation","text":"E = mc^2","page_idx":0,"bbox":[0,0,1,1]},
            {"type":"code","code_body":"fn main() {}","code_caption":["Rust"],"page_idx":0,"bbox":[0,0,1,1]},
            {"type":"list","list_items":["one","two"],"page_idx":0,"bbox":[0,0,1,1]},
            {"type":"sidebar","text":"must be skipped","page_idx":0,"bbox":[0,0,1,1]},
            {"type":"page_number","text":"77","page_idx":0,"bbox":[0,0,1,1]}
        ]);
        let output = convert_content_list(outputs.as_array().unwrap(), false).unwrap();
        assert_eq!(output.block_count, 6);
        assert!(output.content.contains("Body"));
        assert!(!output.content.contains("&lt;p&gt;"));
        assert!(output.content.contains("<!--TABLE_START-->"));
        assert!(output.content.contains("A\n\nCaption"));
        assert!(output.content.contains("<!--IMAGE_START-->"));
        assert!(output.content.contains("Figure\nFootnote"));
        assert!(output.content.contains("E = mc^2"));
        assert!(output.content.contains("fn main() {}"));
        assert!(output.content.contains("one\ntwo"));
        assert!(!output.content.contains("duplicate title"));
        assert!(!output.content.contains("must be skipped"));
        assert!(!output.content.contains("77"));

        assert_eq!(
            sanitize_section_text(
                "&lt;table&gt;&lt;tr&gt;&lt;td&gt;Alpha&lt;/td&gt;&lt;td&gt;Beta&lt;/td&gt;&lt;/tr&gt;&lt;/table&gt;"
            )
            .unwrap(),
            "AlphaBeta"
        );
    }

    #[derive(Default)]
    struct MinerUSyncState {
        received: Mutex<HashMap<String, String>>,
    }

    async fn mineru_head(
        State(state): State<Arc<MinerUSyncState>>,
        headers: HeaderMap,
    ) -> StatusCode {
        state.received.lock().unwrap().insert(
            "head_authorization".into(),
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        StatusCode::OK
    }

    async fn mineru_zip_parse(
        State(state): State<Arc<MinerUSyncState>>,
        headers: HeaderMap,
        mut multipart: Multipart,
    ) -> Response {
        state.received.lock().unwrap().insert(
            "parse_authorization".into(),
            headers
                .get(header::AUTHORIZATION)
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
            if field_name == "files" {
                received.insert("file_name".into(), file_name.unwrap_or_default());
                received.insert("file_content_type".into(), content_type.unwrap_or_default());
            }
        }

        let cursor = Cursor::new(Vec::new());
        let mut archive = zip::ZipWriter::new(cursor);
        archive
            .start_file(
                "mydocument_content_list.json",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(
                br#"[
                    {"type":"text","text":"ZIP title","page_idx":0,"bbox":[0,0,1,1]},
                    {"type":"table","table_body":"<table><tr><td>x</td></tr></table>","page_idx":0,"bbox":[0,0,1,1]}
                ]"#,
            )
            .unwrap();
        let body = archive.finish().unwrap().into_inner();
        let mut response = body.into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/zip"),
        );
        response
    }

    #[tokio::test]
    async fn mineru_python_zip_contract_sends_full_form_and_reads_content_list() {
        let state = Arc::new(MinerUSyncState::default());
        let app = Router::new()
            .route("/openapi.json", head(mineru_head))
            .route("/file_parse", post(mineru_zip_parse))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = MinerUConfig::from_ragflow_key(
            &format!(
                r#"{{
                    "mineru_apiserver":"http://{address}/",
                    "mineru_api_key":"mineru-secret",
                    "mineru_backend":"pipeline"
                }}"#
            ),
            None,
        )
        .unwrap();
        let client = MinerUClient::new(config).unwrap();
        client.check_installation().await.unwrap();
        let output = client
            .parse_pdf(
                "my document.pdf",
                b"%PDF-1.4\nmock",
                &MinerURequestOptions {
                    language: "English".into(),
                    method: "ocr".into(),
                    formula_enable: false,
                    table_enable: true,
                },
            )
            .await
            .unwrap();
        server.abort();

        assert!(output.structured);
        assert_eq!(output.block_count, 2);
        assert!(output.content.contains("ZIP title"));
        assert!(output.content.contains("<!--TABLE_START-->"));
        let received = state.received.lock().unwrap();
        assert_eq!(
            received.get("head_authorization").map(String::as_str),
            Some("Bearer mineru-secret")
        );
        assert_eq!(
            received.get("parse_authorization").map(String::as_str),
            Some("Bearer mineru-secret")
        );
        assert_eq!(
            received.get("file_name").map(String::as_str),
            Some("mydocument.pdf")
        );
        assert_eq!(
            received.get("file_content_type").map(String::as_str),
            Some("application/pdf")
        );
        assert_eq!(
            received.get("files").map(String::as_bytes),
            Some(b"%PDF-1.4\nmock".as_slice())
        );
        assert_eq!(
            received.get("backend").map(String::as_str),
            Some("pipeline")
        );
        assert_eq!(received.get("lang_list").map(String::as_str), Some("en"));
        assert_eq!(
            received.get("parse_method").map(String::as_str),
            Some("ocr")
        );
        assert_eq!(
            received.get("formula_enable").map(String::as_str),
            Some("False")
        );
        assert_eq!(
            received.get("table_enable").map(String::as_str),
            Some("True")
        );
        assert_eq!(
            received.get("response_format_zip").map(String::as_str),
            Some("True")
        );
        assert_eq!(
            received.get("return_content_list").map(String::as_str),
            Some("True")
        );
        assert_eq!(received.get("start_page_id").map(String::as_str), Some("0"));
        assert_eq!(
            received.get("end_page_id").map(String::as_str),
            Some("99999")
        );
    }

    #[derive(Default)]
    struct MinerUAsyncState {
        polls: AtomicUsize,
        authorization: Mutex<Vec<String>>,
    }

    async fn mineru_async_head() -> StatusCode {
        StatusCode::OK
    }

    async fn mineru_async_submit(
        State(state): State<Arc<MinerUAsyncState>>,
        headers: HeaderMap,
        mut multipart: Multipart,
    ) -> Response {
        state.authorization.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        let mut fields = HashMap::new();
        while let Some(field) = multipart.next_field().await.unwrap() {
            let name = field.name().unwrap_or_default().to_owned();
            fields.insert(
                name,
                String::from_utf8_lossy(&field.bytes().await.unwrap()).into_owned(),
            );
        }
        assert_eq!(fields.get("backend").map(String::as_str), Some("pipeline"));
        assert!(
            fields
                .get("files")
                .is_some_and(|body| body.starts_with("%PDF"))
        );
        (
            StatusCode::ACCEPTED,
            Json(json!({"data":{"task_id":"task-1"}})),
        )
            .into_response()
    }

    async fn mineru_async_poll(
        State(state): State<Arc<MinerUAsyncState>>,
        headers: HeaderMap,
    ) -> Response {
        state.authorization.lock().unwrap().push(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
        );
        if state.polls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Json(json!({"results":{}})).into_response();
        }
        Json(json!({
            "results": {
                "doc": {"md_content":"# Async title\n\nBody paragraph.\n"}
            }
        }))
        .into_response()
    }

    #[tokio::test]
    async fn mineru_go_async_contract_polls_until_markdown_is_available() {
        let state = Arc::new(MinerUAsyncState::default());
        let app = Router::new()
            .route("/openapi.json", head(mineru_async_head))
            .route("/file_parse", post(mineru_async_submit))
            .route("/tasks/task-1/result", get(mineru_async_poll))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = MinerUConfig::from_ragflow_key(
            &format!(
                r#"{{
                    "mineru_apiserver":"http://{address}",
                    "mineru_api_key":"mineru-secret",
                    "mineru_timeout_seconds":"2"
                }}"#
            ),
            None,
        )
        .unwrap();
        let client = MinerUClient::new(config).unwrap();
        client.check_installation().await.unwrap();
        let output = client
            .parse_pdf(
                "sample.pdf",
                b"%PDF-1.4\nmock",
                &MinerURequestOptions::default(),
            )
            .await
            .unwrap();
        server.abort();

        assert!(!output.structured);
        assert_eq!(output.content, "# Async title\n\nBody paragraph.\n");
        assert_eq!(state.polls.load(Ordering::SeqCst), 2);
        assert!(
            state
                .authorization
                .lock()
                .unwrap()
                .iter()
                .all(|value| value == "Bearer mineru-secret")
        );
    }
}
