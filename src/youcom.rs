//! You.com agent tool — RAGFlow v0.27.2 `agent/tools/youcom.py`.
//!
//! `youcom_search` runs against the keyed `/v1/search` endpoint when an API
//! key is set and the keyless `/v1/agents/search` endpoint otherwise; the
//! endpoint and headers are always chosen together because the keyless
//! endpoint rejects `X-API-Key`. `web` results lead, then `news`, and the
//! merged list is trimmed to the requested count. Unlike Sofya, the upstream
//! retry loop retries every failure (no transient filter).

use crate::Result;
use serde_json::{Value, json};
use std::time::Duration;

pub const YOUCOM_SEARCH_URL: &str = "https://api.you.com/v1/search";
pub const YOUCOM_KEYLESS_SEARCH_URL: &str = "https://api.you.com/v1/agents/search";
pub const YOUCOM_USER_AGENT: &str = "RAGFlow youdotcom-integration/infiniflow-ragflow";
/// You.com clamps `count` to 1-100.
pub const YOUCOM_MAX_COUNT: i64 = 100;
pub const YOUCOM_FRESHNESS_ANY: &str = "any";
pub const YOUCOM_FRESHNESS_VALUES: [&str; 5] = ["any", "day", "week", "month", "year"];
const MAX_RESPONSE_BYTES: usize = 16 << 20;

/// `_freshness`: blank and `any` both mean no restriction.
pub fn normalize_freshness(value: Option<&str>) -> Result<String> {
    let freshness = value.unwrap_or_default().trim().to_ascii_lowercase();
    if freshness.is_empty() || freshness == YOUCOM_FRESHNESS_ANY {
        return Ok(String::new());
    }
    if !YOUCOM_FRESHNESS_VALUES.contains(&freshness.as_str()) {
        anyhow::bail!(
            "Freshness {freshness} is not supported, it should be in {YOUCOM_FRESHNESS_VALUES:?}"
        );
    }
    Ok(freshness)
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn py_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// `_result_content`: extracted page passages first, description as fallback.
pub fn result_content(result: &Value) -> String {
    if let Some(snippets) = result.get("snippets").and_then(Value::as_array) {
        let joined = snippets
            .iter()
            .map(|snippet| collapse_whitespace(&py_text(Some(snippet))))
            .filter(|snippet| !snippet.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !joined.is_empty() {
            return joined;
        }
    }
    collapse_whitespace(&py_text(result.get("description")))
}

/// `_merge_sections`: web results lead, then news, trimmed to `top_n`.
pub fn merge_sections(results: &Value, top_n: usize) -> Vec<Value> {
    let mut merged = Vec::new();
    for section in ["web", "news"] {
        if let Some(items) = results.get(section).and_then(Value::as_array) {
            merged.extend(items.iter().filter(|item| item.is_object()).cloned());
        }
    }
    merged.truncate(top_n);
    merged
}

/// `count` is clamped to 1..=100 and `freshness` is only forwarded when set.
pub fn build_params(query: &str, count: i64, freshness: &str) -> Value {
    let mut params = json!({
        "query": query,
        "count": count.clamp(1, YOUCOM_MAX_COUNT),
    });
    if !freshness.is_empty() {
        params["freshness"] = json!(freshness);
    }
    params
}

/// Async You.com HTTP client.
#[derive(Debug, Clone)]
pub struct YouComClient {
    client: reqwest::Client,
    keyed_endpoint: reqwest::Url,
    keyless_endpoint: reqwest::Url,
}

impl Default for YouComClient {
    fn default() -> Self {
        Self::new_with_endpoints(YOUCOM_SEARCH_URL, YOUCOM_KEYLESS_SEARCH_URL)
            .expect("fixed You.com endpoints and HTTP client configuration are valid")
    }
}

impl YouComClient {
    pub(crate) fn new_with_endpoints(keyed_endpoint: &str, keyless_endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .map_err(|error| anyhow::anyhow!("could not build You.com HTTP client: {error}"))?;
        Ok(Self {
            client,
            keyed_endpoint: keyed_endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid You.com endpoint: {error}"))?,
            keyless_endpoint: keyless_endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid You.com keyless endpoint: {error}"))?,
        })
    }

    /// Retries every failure up to `attempts` (upstream has no transient
    /// filter here), sleeping `delay` between attempts.
    pub async fn search(
        &self,
        api_key: &str,
        params: &Value,
        attempts: usize,
        delay: Duration,
    ) -> Result<Value> {
        let api_key = api_key.trim();
        let mut last_error = String::from("unknown error");
        let attempts = attempts.max(1);
        for attempt in 0..attempts {
            let request = if api_key.is_empty() {
                self.client.get(self.keyless_endpoint.clone())
            } else {
                self.client
                    .get(self.keyed_endpoint.clone())
                    .header("X-API-Key", api_key)
            };
            let outcome = request
                .header(reqwest::header::ACCEPT, "application/json")
                .header(reqwest::header::USER_AGENT, YOUCOM_USER_AGENT)
                .query(&[
                    (
                        "query",
                        params["query"].as_str().unwrap_or_default().to_owned(),
                    ),
                    ("count", params["count"].to_string()),
                ])
                .query(&freshness_query(params))
                .send()
                .await;
            match outcome {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        let body = read_bounded_body(response).await?;
                        return serde_json::from_slice(&body).map_err(|error| {
                            anyhow::anyhow!("You.com API response was not valid JSON: {error}")
                        });
                    }
                    last_error = format!("HTTP {status}");
                    if attempt + 1 >= attempts {
                        anyhow::bail!("You.com search failed: HTTP {status}");
                    }
                }
                Err(error) => {
                    last_error = error.to_string();
                    if attempt + 1 >= attempts {
                        anyhow::bail!("You.com search failed: {error}");
                    }
                }
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        anyhow::bail!("You.com search failed: {last_error}");
    }
}

fn freshness_query(params: &Value) -> Vec<(&'static str, String)> {
    match params.get("freshness").and_then(Value::as_str) {
        Some(freshness) if !freshness.is_empty() => {
            vec![("freshness", freshness.to_owned())]
        }
        _ => Vec::new(),
    }
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    use futures_util::StreamExt;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| anyhow::anyhow!("You.com response read: {error}"))?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!("You.com response exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}


