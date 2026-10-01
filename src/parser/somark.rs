//! SoMark remote PDF parser.
//!
//! This module follows the fixed RAGFlow v0.26.4 async HTTP contract and
//! normalizes SoMark JSON blocks for RayRAG's marker-aware chunk pipeline.

use crate::Result;
use rand::Rng;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

const SOMARK_SAAS_BASE_URL: &str = "https://somark.tech/api/v1";
const SOMARK_QPS_LIMIT_CODE: i64 = 1124;
const SOMARK_INVALID_API_KEY_CODE: i64 = 1107;

/// RAGFlow-compatible SoMark provider configuration.
#[derive(Clone)]
pub struct SoMarkConfig {
    base_url: String,
    api_key: Option<String>,
    element_formats: BTreeMap<String, String>,
    feature_config: BTreeMap<String, bool>,
    usage_request_timeout: Duration,
    submit_request_timeout: Duration,
    poll_request_timeout: Duration,
    submit_budget: Duration,
    poll_budget: Duration,
    submit_backoff_base: Duration,
    submit_backoff_max: Duration,
    submit_backoff_jitter: Duration,
    poll_interval_base: Duration,
    poll_interval_max: Duration,
}

impl fmt::Debug for SoMarkConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SoMarkConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("element_formats", &self.element_formats)
            .field("feature_config", &self.feature_config)
            .field("usage_request_timeout", &self.usage_request_timeout)
            .field("submit_request_timeout", &self.submit_request_timeout)
            .field("poll_request_timeout", &self.poll_request_timeout)
            .field("submit_budget", &self.submit_budget)
            .field("poll_budget", &self.poll_budget)
            .field("submit_backoff_base", &self.submit_backoff_base)
            .field("submit_backoff_max", &self.submit_backoff_max)
            .field("submit_backoff_jitter", &self.submit_backoff_jitter)
            .field("poll_interval_base", &self.poll_interval_base)
            .field("poll_interval_max", &self.poll_interval_max)
            .finish()
    }
}

impl SoMarkConfig {
    /// Resolve direct, JSON, nested UI, Go setup and environment forms.
    pub fn from_ragflow_key(key: &str, fallback_base_url: Option<&str>) -> Result<Self> {
        let parsed = serde_json::from_str::<Value>(key).ok();
        let plain_secret = parsed
            .is_none()
            .then(|| key.to_owned())
            .filter(|key| !key.is_empty());
        let empty = Map::new();
        let root = parsed.as_ref().and_then(Value::as_object).unwrap_or(&empty);
        let provider = root
            .get("api_key")
            .and_then(Value::as_object)
            .unwrap_or(root);

        let base_url = resolve_config_string(provider, "somark_base_url", "SOMARK_BASE_URL")
            .or_else(|| fallback_base_url.map(str::to_owned))
            .unwrap_or_else(|| SOMARK_SAAS_BASE_URL.to_owned())
            .trim()
            .trim_end_matches('/')
            .to_owned();
        let api_key = resolve_api_key(provider)
            .or(plain_secret)
            .or_else(|| std::env::var("SOMARK_API_KEY").ok())
            .filter(|value| !value.is_empty());

        let element_formats = BTreeMap::from([
            (
                "image".to_owned(),
                resolve_format(
                    provider,
                    "somark_image_format",
                    "SOMARK_IMAGE_FORMAT",
                    "url",
                ),
            ),
            (
                "formula".to_owned(),
                resolve_format(
                    provider,
                    "somark_formula_format",
                    "SOMARK_FORMULA_FORMAT",
                    "latex",
                ),
            ),
            (
                "table".to_owned(),
                resolve_format(
                    provider,
                    "somark_table_format",
                    "SOMARK_TABLE_FORMAT",
                    "html",
                ),
            ),
            (
                "cs".to_owned(),
                resolve_format(provider, "somark_cs_format", "SOMARK_CS_FORMAT", "image"),
            ),
        ]);
        let feature_config = BTreeMap::from([
            (
                "enable_text_cross_page".to_owned(),
                resolve_bool(
                    provider,
                    "somark_enable_text_cross_page",
                    "SOMARK_ENABLE_TEXT_CROSS_PAGE",
                    false,
                ),
            ),
            (
                "enable_table_cross_page".to_owned(),
                resolve_bool(
                    provider,
                    "somark_enable_table_cross_page",
                    "SOMARK_ENABLE_TABLE_CROSS_PAGE",
                    false,
                ),
            ),
            (
                "enable_title_level_recognition".to_owned(),
                resolve_bool(
                    provider,
                    "somark_enable_title_level_recognition",
                    "SOMARK_ENABLE_TITLE_LEVEL_RECOGNITION",
                    false,
                ),
            ),
            (
                "enable_inline_image".to_owned(),
                resolve_bool(
                    provider,
                    "somark_enable_inline_image",
                    "SOMARK_ENABLE_INLINE_IMAGE",
                    true,
                ),
            ),
            (
                "enable_table_image".to_owned(),
                resolve_bool(
                    provider,
                    "somark_enable_table_image",
                    "SOMARK_ENABLE_TABLE_IMAGE",
                    true,
                ),
            ),
            (
                "enable_image_understanding".to_owned(),
                resolve_bool(
                    provider,
                    "somark_enable_image_understanding",
                    "SOMARK_ENABLE_IMAGE_UNDERSTANDING",
                    true,
                ),
            ),
            (
                "keep_header_footer".to_owned(),
                resolve_bool(
                    provider,
                    "somark_keep_header_footer",
                    "SOMARK_KEEP_HEADER_FOOTER",
                    false,
                ),
            ),
        ]);

        Ok(Self {
            base_url,
            api_key,
            element_formats,
            feature_config,
            usage_request_timeout: seconds_setting(
                provider,
                "somark_usage_request_timeout_seconds",
                "SOMARK_USAGE_REQUEST_TIMEOUT_SECONDS",
                10,
            ),
            submit_request_timeout: seconds_setting(
                provider,
                "somark_submit_request_timeout_seconds",
                "SOMARK_SUBMIT_REQUEST_TIMEOUT_SECONDS",
                60,
            ),
            poll_request_timeout: seconds_setting(
                provider,
                "somark_poll_request_timeout_seconds",
                "SOMARK_POLL_REQUEST_TIMEOUT_SECONDS",
                30,
            ),
            submit_budget: seconds_setting(
                provider,
                "somark_submit_budget_seconds",
                "SOMARK_SUBMIT_BUDGET_SECONDS",
                600,
            ),
            poll_budget: seconds_setting(
                provider,
                "somark_poll_budget_seconds",
                "SOMARK_POLL_BUDGET_SECONDS",
                600,
            ),
            submit_backoff_base: millis_setting(
                provider,
                "somark_submit_backoff_base_ms",
                "SOMARK_SUBMIT_BACKOFF_BASE_MS",
                1_000,
            ),
            submit_backoff_max: millis_setting(
                provider,
                "somark_submit_backoff_max_ms",
                "SOMARK_SUBMIT_BACKOFF_MAX_MS",
                10_000,
            ),
            submit_backoff_jitter: millis_setting(
                provider,
                "somark_submit_backoff_jitter_ms",
                "SOMARK_SUBMIT_BACKOFF_JITTER_MS",
                500,
            ),
            poll_interval_base: millis_setting(
                provider,
                "somark_poll_interval_base_ms",
                "SOMARK_POLL_INTERVAL_BASE_MS",
                2_000,
            ),
            poll_interval_max: millis_setting(
                provider,
                "somark_poll_interval_max_ms",
                "SOMARK_POLL_INTERVAL_MAX_MS",
                10_000,
            ),
        })
    }

    pub fn from_env() -> Result<Option<Self>> {
        let configured = ["SOMARK_BASE_URL", "SOMARK_API_KEY"]
            .iter()
            .any(|key| std::env::var(key).is_ok_and(|value| !value.is_empty()));
        if !configured {
            return Ok(None);
        }
        Self::from_ragflow_key("", Some(SOMARK_SAAS_BASE_URL)).map(Some)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    pub fn element_formats(&self) -> &BTreeMap<String, String> {
        &self.element_formats
    }

    pub fn feature_config(&self) -> &BTreeMap<String, bool> {
        &self.feature_config
    }
}

fn resolve_config_string(
    provider: &Map<String, Value>,
    lower: &str,
    upper: &str,
) -> Option<String> {
    provider
        .get(lower)
        .and_then(value_string)
        .or_else(|| provider.get(upper).and_then(value_string))
        .or_else(|| std::env::var(upper).ok())
}

fn resolve_api_key(provider: &Map<String, Value>) -> Option<String> {
    provider
        .get("somark_api_key")
        .and_then(value_string)
        .or_else(|| provider.get("api_key").and_then(value_string))
        .or_else(|| provider.get("SOMARK_API_KEY").and_then(value_string))
}

fn value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(if *value { "1" } else { "0" }.to_owned()),
        _ => None,
    }
}

fn resolve_format(
    provider: &Map<String, Value>,
    lower: &str,
    upper: &str,
    default: &str,
) -> String {
    resolve_config_string(provider, lower, upper)
        .unwrap_or_else(|| default.to_owned())
        .trim()
        .to_ascii_lowercase()
}

fn resolve_bool(provider: &Map<String, Value>, lower: &str, upper: &str, default: bool) -> bool {
    let value = provider
        .get(lower)
        .or_else(|| provider.get(upper))
        .cloned()
        .or_else(|| std::env::var(upper).ok().map(Value::String))
        .unwrap_or(Value::Bool(default));
    match value {
        Value::Bool(value) => value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        _ => false,
    }
}

fn integer_setting(provider: &Map<String, Value>, lower: &str, upper: &str, default: u64) -> u64 {
    resolve_config_string(provider, lower, upper)
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn seconds_setting(
    provider: &Map<String, Value>,
    lower: &str,
    upper: &str,
    default: u64,
) -> Duration {
    Duration::from_secs(integer_setting(provider, lower, upper, default))
}

fn millis_setting(
    provider: &Map<String, Value>,
    lower: &str,
    upper: &str,
    default: u64,
) -> Duration {
    let value = resolve_config_string(provider, lower, upper)
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default);
    Duration::from_millis(value)
}

/// Optional document-specific SoMark overrides from `parser_config`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoMarkRequestOptions {
    element_formats: BTreeMap<String, String>,
    feature_config: BTreeMap<String, bool>,
}

impl SoMarkRequestOptions {
    pub fn from_parser_config(config: &crate::ParserConfig) -> Self {
        let mut element_formats = BTreeMap::new();
        for (name, value) in [
            ("image", config.somark_image_format.as_ref()),
            ("formula", config.somark_formula_format.as_ref()),
            ("table", config.somark_table_format.as_ref()),
            ("cs", config.somark_cs_format.as_ref()),
        ] {
            if let Some(value) = value {
                element_formats.insert(name.to_owned(), value.trim().to_ascii_lowercase());
            }
        }
        let mut feature_config = BTreeMap::new();
        for (name, value) in [
            (
                "enable_text_cross_page",
                config.somark_enable_text_cross_page,
            ),
            (
                "enable_table_cross_page",
                config.somark_enable_table_cross_page,
            ),
            (
                "enable_title_level_recognition",
                config.somark_enable_title_level_recognition,
            ),
            ("enable_inline_image", config.somark_enable_inline_image),
            ("enable_table_image", config.somark_enable_table_image),
            (
                "enable_image_understanding",
                config.somark_enable_image_understanding,
            ),
            ("keep_header_footer", config.somark_keep_header_footer),
        ] {
            if let Some(value) = value {
                feature_config.insert(name.to_owned(), value);
            }
        }
        Self {
            element_formats,
            feature_config,
        }
    }
}

#[derive(Debug, Clone)]
struct EffectiveOptions {
    element_formats: BTreeMap<String, String>,
    feature_config: BTreeMap<String, bool>,
}

/// SoMark result normalized for RayRAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoMarkOutput {
    pub content: String,
    pub structured: bool,
    pub block_count: usize,
    pub page_count: usize,
}

/// Async SoMark HTTP client.
#[derive(Clone)]
pub struct SoMarkClient {
    config: SoMarkConfig,
    client: reqwest::Client,
}

impl SoMarkClient {
    pub fn new(config: SoMarkConfig) -> Result<Self> {
        if config.base_url.is_empty() {
            anyhow::bail!("SoMark requires somark_base_url or SOMARK_BASE_URL");
        }
        if !config.base_url.starts_with("http://") && !config.base_url.starts_with("https://") {
            anyhow::bail!("SOMARK_BASE_URL must start with http:// or https://");
        }
        reqwest::Url::parse(&config.base_url)
            .map_err(|error| anyhow::anyhow!("Invalid SoMark base URL: {error}"))?;
        Ok(Self {
            config,
            client: crate::common::cmd_timeout::model_client(),
        })
    }

    pub fn from_env() -> Result<Option<Self>> {
        SoMarkConfig::from_env()?.map(Self::new).transpose()
    }

    pub async fn check_installation(&self) -> Result<()> {
        if self.config.base_url == SOMARK_SAAS_BASE_URL {
            return self.check_saas_usage().await;
        }
        let response = self
            .client
            .head(&self.config.base_url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("SoMark server check failed: {error}"))?;
        if response.status().as_u16() >= 500 {
            anyhow::bail!(
                "SoMark server unreachable: {} returned {}",
                self.config.base_url,
                response.status()
            );
        }
        Ok(())
    }

    async fn check_saas_usage(&self) -> Result<()> {
        let mut form = Vec::new();
        if let Some(api_key) = self.config.api_key() {
            form.push(("api_key", api_key));
        }
        let response = self
            .client
            .post(format!("{}/usage", self.config.base_url))
            .timeout(self.config.usage_request_timeout)
            .form(&form)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("SoMark usage check failed: {error}"))?;
        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|error| anyhow::anyhow!("SoMark usage read failed: {error}"))?;
        if status.as_u16() >= 500 {
            anyhow::bail!("SoMark usage HTTP {status}: {}", truncate_body(&body));
        }
        let payload: Value = serde_json::from_str(&body).map_err(|error| {
            anyhow::anyhow!(
                "SoMark usage non-JSON response ({status}): {} ({error})",
                truncate_body(&body)
            )
        })?;
        let code = payload.get("code").and_then(Value::as_i64);
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if code == Some(SOMARK_INVALID_API_KEY_CODE) {
            anyhow::bail!(
                "SoMark {}",
                if message.is_empty() {
                    "Invalid API key"
                } else {
                    message
                }
            );
        }
        if code != Some(0) {
            anyhow::bail!("SoMark usage error code={code:?} message={message}");
        }
        let usage = payload.get("data").and_then(Value::as_object);
        let paid = usage
            .and_then(|usage| usage.get("remaining_paid_pages"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let free = usage
            .and_then(|usage| usage.get("remaining_free_pages_this_month"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if paid == 0 && free == 0 {
            anyhow::bail!(
                "SoMark insufficient parse pages \
                 (remaining_paid_pages=0, remaining_free_pages_this_month=0)"
            );
        }
        Ok(())
    }

    pub async fn parse_pdf(
        &self,
        file_name: &str,
        data: &[u8],
        options: Option<&SoMarkRequestOptions>,
    ) -> Result<SoMarkOutput> {
        if data.is_empty() {
            anyhow::bail!("SoMark PDF content is empty");
        }
        let options = self.effective_options(options);
        let task_id = self.submit_task(file_name, data, &options).await?;
        let result = self.poll_task(&task_id).await?;
        let pages = result
            .pointer("/outputs/json/pages")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("SoMark result is missing outputs.json.pages"))?;
        let keep_header_footer = options
            .feature_config
            .get("keep_header_footer")
            .copied()
            .unwrap_or(false);
        convert_somark_pages(pages, keep_header_footer)
    }

    fn effective_options(&self, options: Option<&SoMarkRequestOptions>) -> EffectiveOptions {
        let mut element_formats = self.config.element_formats.clone();
        let mut feature_config = self.config.feature_config.clone();
        if let Some(options) = options {
            element_formats.extend(options.element_formats.clone());
            feature_config.extend(options.feature_config.clone());
        }
        EffectiveOptions {
            element_formats,
            feature_config,
        }
    }

    async fn submit_task(
        &self,
        file_name: &str,
        data: &[u8],
        options: &EffectiveOptions,
    ) -> Result<String> {
        let deadline = Instant::now() + self.config.submit_budget;
        let mut attempt = 0_u32;
        loop {
            let part = reqwest::multipart::Part::bytes(data.to_vec())
                .file_name(somark_upload_name(file_name))
                .mime_str("application/pdf")
                .map_err(|error| anyhow::anyhow!("SoMark PDF multipart: {error}"))?;
            let mut form = reqwest::multipart::Form::new()
                .part("file", part)
                .text("output_formats", "json")
                .text(
                    "element_formats",
                    serde_json::to_string(&options.element_formats)?,
                )
                .text(
                    "feature_config",
                    serde_json::to_string(&options.feature_config)?,
                );
            if let Some(api_key) = self.config.api_key() {
                form = form.text("api_key", api_key.to_owned());
            }
            let response = self
                .client
                .post(format!("{}/parse/async", self.config.base_url))
                .timeout(self.config.submit_request_timeout)
                .multipart(form)
                .send()
                .await
                .map_err(|error| anyhow::anyhow!("SoMark submit failed: {error}"))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|error| anyhow::anyhow!("SoMark submit read failed: {error}"))?;
            if status.as_u16() >= 500 {
                anyhow::bail!("SoMark submit HTTP {status}: {}", truncate_body(&body));
            }
            let payload: Value = serde_json::from_str(&body).map_err(|error| {
                anyhow::anyhow!(
                    "SoMark submit non-JSON response ({status}): {} ({error})",
                    truncate_body(&body)
                )
            })?;
            let code = payload.get("code").and_then(Value::as_i64);
            if code == Some(0) {
                return payload
                    .pointer("/data/task_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|task_id| !task_id.is_empty())
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        anyhow::anyhow!("SoMark submit returned no task_id: {payload}")
                    });
            }
            if code != Some(SOMARK_QPS_LIMIT_CODE) {
                let message = payload
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                anyhow::bail!("SoMark submit business error code={code:?} message={message}");
            }

            let multiplier = 2_f64.powi(attempt.min(30) as i32);
            let backoff = Duration::from_secs_f64(
                (self.config.submit_backoff_base.as_secs_f64() * multiplier)
                    .min(self.config.submit_backoff_max.as_secs_f64()),
            );
            let jitter = if self.config.submit_backoff_jitter.is_zero() {
                Duration::ZERO
            } else {
                Duration::from_secs_f64(
                    rand::rng().random_range(0.0..=self.config.submit_backoff_jitter.as_secs_f64()),
                )
            };
            let wait = backoff + jitter;
            if Instant::now() + wait > deadline {
                anyhow::bail!("SoMark submit blocked by QPS limit; retry budget exhausted");
            }
            tokio::time::sleep(wait).await;
            attempt = attempt.saturating_add(1);
        }
    }

    async fn poll_task(&self, task_id: &str) -> Result<Value> {
        let deadline = Instant::now() + self.config.poll_budget;
        let mut interval = self.config.poll_interval_base;
        while Instant::now() < deadline {
            tokio::time::sleep(interval).await;
            let mut form = vec![("task_id", task_id)];
            if let Some(api_key) = self.config.api_key() {
                form.push(("api_key", api_key));
            }
            let response = self
                .client
                .post(format!("{}/parse/async_check", self.config.base_url))
                .timeout(self.config.poll_request_timeout)
                .form(&form)
                .send()
                .await
                .map_err(|error| anyhow::anyhow!("SoMark poll request failed: {error}"))?;
            let status = response.status();
            let body = response
                .text()
                .await
                .map_err(|error| anyhow::anyhow!("SoMark poll read failed: {error}"))?;
            let payload = parse_business_json(status, &body, "poll")?;
            let data = payload.get("data").and_then(Value::as_object);
            match data
                .and_then(|data| data.get("status"))
                .and_then(Value::as_str)
            {
                Some("SUCCESS") => {
                    return data
                        .and_then(|data| data.get("result"))
                        .cloned()
                        .filter(|result| !result.is_null())
                        .ok_or_else(|| anyhow::anyhow!("SoMark SUCCESS but no result: {payload}"));
                }
                Some("FAILED") => {
                    let message = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    anyhow::bail!("SoMark task {task_id} FAILED: {message}");
                }
                _ => {}
            }
            interval = Duration::from_secs_f64(
                (interval.as_secs_f64() * 1.5).min(self.config.poll_interval_max.as_secs_f64()),
            );
        }
        anyhow::bail!(
            "SoMark task {task_id} timed out after {}s while waiting",
            self.config.poll_budget.as_secs()
        )
    }
}

fn parse_business_json(status: reqwest::StatusCode, body: &str, stage: &str) -> Result<Value> {
    if status.as_u16() >= 500 {
        anyhow::bail!("SoMark {stage} HTTP {status}: {}", truncate_body(body));
    }
    let payload: Value = serde_json::from_str(body).map_err(|error| {
        anyhow::anyhow!(
            "SoMark {stage} non-JSON response ({status}): {} ({error})",
            truncate_body(body)
        )
    })?;
    let code = payload.get("code").and_then(Value::as_i64);
    if code != Some(0) {
        let message = payload
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        anyhow::bail!("SoMark {stage} business error code={code:?} message={message}");
    }
    Ok(payload)
}

fn truncate_body(body: &str) -> &str {
    body.get(..200).unwrap_or(body)
}

fn somark_upload_name(file_name: &str) -> String {
    let stem = std::path::Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("document")
        .replace(' ', "");
    format!("{}.pdf", if stem.is_empty() { "document" } else { &stem })
}

fn convert_somark_pages(pages: &[Value], keep_header_footer: bool) -> Result<SoMarkOutput> {
    let mut sections = Vec::new();
    let mut image_sequence = 0_usize;
    for page in pages {
        let page_index = match page.get("page_num") {
            None | Some(Value::Null) => 0,
            Some(value) => value
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("SoMark page_num must be an integer"))?,
        };
        let Some(blocks) = page.get("blocks").and_then(Value::as_array) else {
            continue;
        };
        for block in blocks {
            let block_type = block
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if matches!(block_type.as_str(), "cate" | "cate_item" | "blank") {
                continue;
            }
            if matches!(block_type.as_str(), "header" | "footer") && !keep_header_footer {
                continue;
            }
            let internal_type = somark_internal_type(&block_type);
            if internal_type == "image" {
                let bbox = block.get("bbox").and_then(Value::as_array);
                if bbox.is_none_or(|bbox| bbox.len() != 4) {
                    continue;
                }
                let line_tag = somark_line_tag(page_index, block.get("bbox"))?;
                image_sequence += 1;
                let caption = block
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                let label = if caption.is_empty() {
                    format!("{block_type} {image_sequence}")
                } else {
                    caption.to_owned()
                };
                sections.push(format!(
                    "<!--IMAGE_START-->\n{label}{line_tag}\n<!--IMAGE_END-->"
                ));
                continue;
            }

            let mut content = block
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            if block_type == "title"
                && let Some(level) = block.get("title_level").and_then(Value::as_i64)
                && (1..=6).contains(&level)
            {
                content = format!("{} {content}", "#".repeat(level as usize));
            }
            if content.is_empty() {
                continue;
            }
            let line_tag = somark_line_tag(page_index, block.get("bbox"))?;
            sections.push(if internal_type == "table" {
                format!("<!--TABLE_START-->\n{content}{line_tag}\n<!--TABLE_END-->")
            } else {
                format!("{content}{line_tag}")
            });
        }
    }
    if sections.is_empty() {
        anyhow::bail!("SoMark returned no usable blocks");
    }
    Ok(SoMarkOutput {
        block_count: sections.len(),
        page_count: pages.len(),
        content: sections.join("\n\n"),
        structured: true,
    })
}

fn somark_line_tag(page_index: i64, bbox: Option<&Value>) -> Result<String> {
    let mut coordinates = [0.0_f64; 4];
    if let Some(value) = bbox
        && !value.is_null()
    {
        let values = value
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("SoMark bbox must be an array"))?;
        if values.len() == 4 {
            for (target, value) in coordinates.iter_mut().zip(values) {
                *target = value
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("SoMark bbox coordinates must be numbers"))?;
            }
        }
    }
    let [mut left, mut top, mut right, mut bottom] = coordinates;
    if left > right {
        std::mem::swap(&mut left, &mut right);
    }
    if top > bottom {
        std::mem::swap(&mut top, &mut bottom);
    }
    Ok(format!(
        "@@{}\t{left:.1}\t{right:.1}\t{top:.1}\t{bottom:.1}##",
        page_index + 1
    ))
}

fn somark_internal_type(block_type: &str) -> &'static str {
    match block_type {
        "figure" | "cs" | "qrcode" | "stamp" => "image",
        "table" => "table",
        "equation" => "equation",
        "code" => "code",
        _ => "text",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Bytes,
        extract::{Multipart, State},
        http::StatusCode,
        response::{IntoResponse, Response},
        routing::{head, post},
    };
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn somark_config_accepts_wrapper_shapes_defaults_and_redacts_secrets() {
        let nested = SoMarkConfig::from_ragflow_key(
            r#"{
                "api_key": {
                    "somark_base_url": " http://somark.example/api/v1/ ",
                    "SOMARK_API_KEY": " secret with spaces ",
                    "somark_image_format": " BASE64 ",
                    "SOMARK_FORMULA_FORMAT": "mathml",
                    "somark_enable_text_cross_page": "yes",
                    "SOMARK_ENABLE_INLINE_IMAGE": "0"
                }
            }"#,
            None,
        )
        .unwrap();
        assert_eq!(nested.base_url(), "http://somark.example/api/v1");
        assert_eq!(nested.api_key(), Some(" secret with spaces "));
        assert_eq!(nested.element_formats()["image"], "base64");
        assert_eq!(nested.element_formats()["formula"], "mathml");
        assert!(nested.feature_config()["enable_text_cross_page"]);
        assert!(!nested.feature_config()["enable_inline_image"]);
        assert_eq!(nested.element_formats()["table"], "html");
        assert!(nested.feature_config()["enable_table_image"]);
        assert!(!format!("{nested:?}").contains("secret with spaces"));
        assert!(format!("{nested:?}").contains("[REDACTED]"));

        let plain_secret =
            SoMarkConfig::from_ragflow_key(" raw-api-key ", Some("https://somark.tech/api/v1"))
                .unwrap();
        assert_eq!(plain_secret.api_key(), Some(" raw-api-key "));
        assert_eq!(plain_secret.base_url(), "https://somark.tech/api/v1");

        let explicit_empty = SoMarkConfig::from_ragflow_key(
            r#"{"somark_base_url":"","SOMARK_BASE_URL":"http://must-not-win"}"#,
            Some("http://fallback-must-not-win"),
        )
        .unwrap();
        assert_eq!(explicit_empty.base_url(), "");
    }

    #[test]
    fn somark_blocks_match_fixed_type_filtering_and_marker_contract() {
        let pages = json!([
            {
                "page_num": 0,
                "page_size": {"w": 600, "h": 800},
                "blocks": [
                    {"type":"title","content":"Chapter","title_level":2,"bbox":[1,2,3,4]},
                    {"type":"text","content":" body ","bbox":[1,2,3,4]},
                    {"type":"figure","content":"Figure caption","bbox":[1,2,3,4]},
                    {"type":"cs","content":"","bbox":[1,2,3,4]},
                    {"type":"qrcode","content":"","bbox":[1,2,3,4]},
                    {"type":"stamp","content":"","bbox":[1,2,3,4]},
                    {"type":"figure","content":"no geometry"},
                    {"type":"table","content":"<table><tr><td>x</td></tr></table>","bbox":[1,2,3,4]},
                    {"type":"equation","content":"E=mc^2","bbox":[1,2,3,4]},
                    {"type":"code","content":"fn main() {}","bbox":[1,2,3,4]},
                    {"type":"figure_caption","content":"caption text","bbox":[1,2,3,4]},
                    {"type":"cate","content":"toc","bbox":[1,2,3,4]},
                    {"type":"cate_item","content":"toc item","bbox":[1,2,3,4]},
                    {"type":"blank","content":"blank","bbox":[1,2,3,4]},
                    {"type":"header","content":"header","bbox":[1,2,3,4]},
                    {"type":"footer","content":"footer","bbox":[1,2,3,4]},
                    {"type":"brand_new","content":"future text","bbox":[1,2,3,4]}
                ]
            }
        ]);
        let output = convert_somark_pages(pages.as_array().unwrap(), false).unwrap();
        assert_eq!(output.page_count, 1);
        assert_eq!(output.block_count, 11);
        assert!(output.content.contains("## Chapter"));
        assert!(output.content.contains("body"));
        assert!(output.content.contains("<!--IMAGE_START-->"));
        assert!(output.content.contains("Figure caption"));
        assert!(output.content.contains("cs 2"));
        assert!(output.content.contains("qrcode 3"));
        assert!(output.content.contains("stamp 4"));
        assert!(!output.content.contains("no geometry"));
        assert!(output.content.contains("<!--TABLE_START-->"));
        assert!(output.content.contains("<table>"));
        assert!(output.content.contains("E=mc^2"));
        assert!(output.content.contains("fn main() {}"));
        assert!(output.content.contains("future text"));
        assert!(!output.content.contains("toc"));
        assert!(!output.content.contains("header"));
        assert!(!output.content.contains("footer"));

        let kept = convert_somark_pages(pages.as_array().unwrap(), true).unwrap();
        assert!(kept.content.contains("header"));
        assert!(kept.content.contains("footer"));
    }

    #[test]
    fn somark_line_tags_use_one_based_pages_normalized_raw_bbox_and_zero_fallback() {
        let pages = json!([{
            "page_num":4,
            "page_size":{"w":600,"h":800},
            "blocks":[
                {"type":"text","content":"reversed","bbox":[100,40,10,20]},
                {"type":"footnote","content":"no geometry"}
            ]
        }]);

        let output = convert_somark_pages(pages.as_array().unwrap(), false).unwrap();
        assert!(
            output
                .content
                .contains("reversed@@5\t10.0\t100.0\t20.0\t40.0##")
        );
        assert!(
            output
                .content
                .contains("no geometry@@5\t0.0\t0.0\t0.0\t0.0##")
        );
    }

    #[derive(Default)]
    struct SoMarkState {
        submit_attempts: AtomicUsize,
        poll_attempts: AtomicUsize,
        fields: Mutex<HashMap<String, String>>,
    }

    async fn somark_head() -> StatusCode {
        StatusCode::NOT_FOUND
    }

    async fn somark_submit(
        State(state): State<Arc<SoMarkState>>,
        mut multipart: Multipart,
    ) -> Response {
        let mut fields = HashMap::new();
        while let Some(field) = multipart.next_field().await.unwrap() {
            let name = field.name().unwrap_or_default().to_owned();
            let file_name = field.file_name().map(str::to_owned);
            let content_type = field.content_type().map(str::to_owned);
            let body = field.bytes().await.unwrap();
            fields.insert(name.clone(), String::from_utf8_lossy(&body).into_owned());
            if name == "file" {
                fields.insert("file_name".into(), file_name.unwrap_or_default());
                fields.insert("file_content_type".into(), content_type.unwrap_or_default());
            }
        }
        *state.fields.lock().unwrap() = fields;
        if state.submit_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Json(json!({"code":1124,"message":"busy"})).into_response();
        }
        Json(json!({"code":0,"data":{"task_id":"task/with space"}})).into_response()
    }

    async fn somark_poll(State(state): State<Arc<SoMarkState>>, body: Bytes) -> Response {
        state.fields.lock().unwrap().insert(
            "poll_body".into(),
            String::from_utf8_lossy(&body).into_owned(),
        );
        if state.poll_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Json(json!({"code":0,"data":{"status":"PROCESSING"}})).into_response();
        }
        Json(json!({
            "code":0,
            "data":{
                "status":"SUCCESS",
                "result":{
                    "outputs":{
                        "json":{
                            "pages":[{
                                "page_num":0,
                                "page_size":{"w":600,"h":800},
                                "blocks":[
                                    {"type":"title","content":"HTTP title","title_level":1,"bbox":[1,2,3,4]},
                                    {"type":"figure","content":"HTTP figure","bbox":[1,2,3,4]}
                                ]
                            }]
                        }
                    }
                }
            }
        }))
        .into_response()
    }

    #[tokio::test]
    async fn somark_async_contract_retries_qps_polls_and_converts_result() {
        let state = Arc::new(SoMarkState::default());
        let app = Router::new()
            .route("/", head(somark_head))
            .route("/parse/async", post(somark_submit))
            .route("/parse/async_check", post(somark_poll))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = SoMarkConfig::from_ragflow_key(
            &format!(
                r#"{{
                    "somark_base_url":"http://{address}",
                    "somark_api_key":"somark secret",
                    "somark_image_format":"base64",
                    "somark_formula_format":"mathml",
                    "somark_table_format":"markdown",
                    "somark_cs_format":"image",
                    "somark_enable_text_cross_page":true,
                    "somark_enable_inline_image":false,
                    "somark_submit_backoff_base_ms":1,
                    "somark_submit_backoff_max_ms":2,
                    "somark_submit_backoff_jitter_ms":0,
                    "somark_poll_interval_base_ms":1,
                    "somark_poll_interval_max_ms":2,
                    "somark_submit_budget_seconds":2,
                    "somark_poll_budget_seconds":2
                }}"#
            ),
            None,
        )
        .unwrap();
        let client = SoMarkClient::new(config).unwrap();
        client.check_installation().await.unwrap();
        let output = client
            .parse_pdf("my document.pdf", b"%PDF-1.4\nmock", None)
            .await
            .unwrap();
        server.abort();

        assert_eq!(state.submit_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(state.poll_attempts.load(Ordering::SeqCst), 2);
        assert_eq!(output.page_count, 1);
        assert!(output.content.contains("# HTTP title"));
        assert!(output.content.contains("<!--IMAGE_START-->"));
        let fields = state.fields.lock().unwrap();
        assert_eq!(
            fields.get("file_name").map(String::as_str),
            Some("mydocument.pdf")
        );
        assert_eq!(
            fields.get("file_content_type").map(String::as_str),
            Some("application/pdf")
        );
        assert_eq!(
            fields.get("output_formats").map(String::as_str),
            Some("json")
        );
        assert_eq!(
            fields.get("api_key").map(String::as_str),
            Some("somark secret")
        );
        let element_formats: Value =
            serde_json::from_str(fields.get("element_formats").unwrap()).unwrap();
        assert_eq!(element_formats["image"], "base64");
        assert_eq!(element_formats["formula"], "mathml");
        assert_eq!(element_formats["table"], "markdown");
        let features: Value = serde_json::from_str(fields.get("feature_config").unwrap()).unwrap();
        assert_eq!(features["enable_text_cross_page"], true);
        assert_eq!(features["enable_inline_image"], false);
        assert!(
            fields
                .get("poll_body")
                .is_some_and(|body| body.contains("task_id=task%2Fwith+space"))
        );
        assert!(
            fields
                .get("poll_body")
                .is_some_and(|body| body.contains("api_key=somark+secret"))
        );
    }

    #[derive(Default)]
    struct UsageState {
        calls: AtomicUsize,
        bodies: Mutex<Vec<String>>,
    }

    async fn somark_usage(State(state): State<Arc<UsageState>>, body: Bytes) -> Json<Value> {
        state
            .bodies
            .lock()
            .unwrap()
            .push(String::from_utf8_lossy(&body).into_owned());
        if state.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Json(json!({
                "code":0,
                "data":{"remaining_paid_pages":2,"remaining_free_pages_this_month":0}
            }))
        } else {
            Json(json!({
                "code":0,
                "data":{"remaining_paid_pages":0,"remaining_free_pages_this_month":0}
            }))
        }
    }

    #[tokio::test]
    async fn somark_saas_usage_requires_remaining_quota_and_sends_api_key() {
        let state = Arc::new(UsageState::default());
        let app = Router::new()
            .route("/usage", post(somark_usage))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = SoMarkConfig::from_ragflow_key(
            &format!(r#"{{"somark_base_url":"http://{address}","somark_api_key":"usage key"}}"#),
            None,
        )
        .unwrap();
        let client = SoMarkClient::new(config).unwrap();

        client.check_saas_usage().await.unwrap();
        let error = client.check_saas_usage().await.unwrap_err();
        server.abort();

        assert!(error.to_string().contains("insufficient parse pages"));
        assert!(
            state
                .bodies
                .lock()
                .unwrap()
                .iter()
                .all(|body| body == "api_key=usage+key")
        );
    }
}
