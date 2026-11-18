//! SoMarkAI/OXR document parsing adapter.
//!
//! OXR is an optional 1.3B-parameter document parsing model. It is compatible
//! with the SoMark parsing API and can be used through the free API
//! (`https://free-oxr.somark.ai/api/v1`) or a self-hosted OXR service.
//! RayRAG uses the synchronous `/parse/sync` endpoint and requests Markdown.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

const DEFAULT_OXR_TIMEOUT_SECONDS: u64 = 600;

#[derive(Clone)]
pub struct OxrConfig {
    base_url: String,
    api_key: String,
    timeout: Duration,
}

impl std::fmt::Debug for OxrConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OxrConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl OxrConfig {
    pub fn from_env() -> Result<Option<Self>> {
        let base_url = std::env::var("OXR_BASE_URL")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_owned())
            .filter(|value| !value.is_empty());
        let Some(base_url) = base_url else {
            return Ok(None);
        };
        let api_key = std::env::var("OXR_API_KEY")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "dummy".to_owned());
        let timeout = std::env::var("OXR_TIMEOUT")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(DEFAULT_OXR_TIMEOUT_SECONDS);
        Ok(Some(Self {
            base_url,
            api_key,
            timeout: Duration::from_secs(timeout),
        }))
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OxrOutput {
    pub content: String,
    pub page_count: usize,
}

#[derive(Clone)]
pub struct OxrClient {
    config: OxrConfig,
    client: reqwest::Client,
}

impl OxrClient {
    pub fn new(config: OxrConfig) -> Result<Self> {
        if config.base_url().is_empty() {
            anyhow::bail!("OXR requires OXR_BASE_URL");
        }
        let client = reqwest::Client::builder()
            .timeout(config.timeout())
            .build()
            .map_err(|error| anyhow::anyhow!("OXR HTTP client: {error}"))?;
        Ok(Self { config, client })
    }

    pub fn from_env() -> Result<Option<Self>> {
        OxrConfig::from_env()?.map(Self::new).transpose()
    }

    /// Parse a PDF or image through OXR's synchronous API.
    pub async fn parse_file(&self, file_name: &str, data: &[u8]) -> Result<OxrOutput> {
        if data.is_empty() {
            anyhow::bail!("[OXR] file content is empty");
        }
        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name.to_owned())
            .mime_str("application/octet-stream")
            .map_err(|error| anyhow::anyhow!("OXR multipart: {error}"))?;
        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("api_key", self.config.api_key().to_owned())
            .text("output_formats", "markdown")
            .text("output_formats", "json");

        let request = self
            .client
            .post(format!("{}/parse/sync", self.config.base_url()))
            .multipart(form);
        let response = request
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("OXR submit: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| anyhow::anyhow!("OXR response read: {error}"))?;
        if !status.is_success() {
            anyhow::bail!("OXR HTTP {status}: {body}");
        }
        let payload = serde_json::from_str::<OxrResponse>(&body)
            .map_err(|error| anyhow::anyhow!("OXR response decode: {error}"))?;
        if payload.code != 0 {
            anyhow::bail!(
                "OXR parse failed: code={}, message={}",
                payload.code,
                payload.message
            );
        }
        let content = payload
            .outputs
            .get("markdown")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| {
                payload
                    .outputs
                    .get("json")
                    .and_then(|value| value.get("markdown"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
            })
            .map(str::to_owned);
        let Some(content) = content else {
            anyhow::bail!("OXR returned no Markdown content");
        };
        let page_count = payload
            .metadata
            .get("page_num")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        Ok(OxrOutput {
            content,
            page_count,
        })
    }
}

#[derive(Deserialize, Default)]
struct OxrResponse {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    outputs: serde_json::Map<String, Value>,
    #[serde(default)]
    metadata: serde_json::Map<String, Value>,
}

#[derive(Default)]
pub struct OxrParser;

impl OxrParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parse for OxrParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        // Synchronous parser trait cannot call async HTTP; the pipeline uses
        // OxrClient directly. This fallback keeps the parser registry honest
        // for local text extraction.
        Ok(new_document(
            name,
            String::from_utf8_lossy(data).to_string(),
            "application/octet-stream",
            data.len(),
        ))
    }
}


