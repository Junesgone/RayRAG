//! Sofya agent tool — RAGFlow v0.27.2 `agent/tools/sofya.py`.
//!
//! `sofya_search` returns the result pages' content (falling back to search
//! snippets) through `POST /v1/search`. The retry policy mirrors upstream:
//! `max_retries + 1` attempts, retrying HTTP {429, 500, 502, 503, 504} and
//! network errors while other statuses fail at once. Endpoints are fixed by
//! construction; tests inject loopback endpoints through the crate-private
//! constructor (same discipline as `src/tavily.rs`).

use crate::Result;
use serde_json::{Value, json};
use std::time::Duration;

pub const SOFYA_SEARCH_URL: &str = "https://sofya.co/v1/search";
/// Sofya returns at most 20 results per search.
pub const SOFYA_MAX_RESULTS: i64 = 20;
pub const SOFYA_SEARCH_DEPTHS: [&str; 2] = ["basic", "snippets"];
pub const SOFYA_DEFAULT_SEARCH_DEPTH: &str = "basic";
pub const SOFYA_TOPICS: [&str; 2] = ["general", "news"];
pub const SOFYA_DEFAULT_TOPIC: &str = "general";
pub const SOFYA_FRESHNESS_ANY: &str = "any";
pub const SOFYA_FRESHNESS_VALUES: [&str; 5] = ["any", "day", "week", "month", "year"];
pub const SOFYA_RETRY_STATUSES: [u16; 5] = [429, 500, 502, 503, 504];
pub const SOFYA_USER_AGENT: &str = "RAGFlow sofya-integration/infiniflow-ragflow";
const MAX_RESPONSE_BYTES: usize = 16 << 20;

/// `_search_depth`: an unknown depth falls back to the default instead of
/// failing the run (the value is node config, not caller input).
pub fn normalize_search_depth(value: Option<&str>) -> String {
    let depth = value.unwrap_or_default().trim().to_ascii_lowercase();
    if SOFYA_SEARCH_DEPTHS.contains(&depth.as_str()) {
        return depth;
    }
    if !depth.is_empty() {
        tracing::warn!(
            depth,
            "Sofya search depth is not supported; using the default"
        );
    }
    SOFYA_DEFAULT_SEARCH_DEPTH.to_owned()
}

/// `_topic`: blank means the default, anything else must be known.
pub fn normalize_topic(value: Option<&str>) -> Result<String> {
    let topic = value.unwrap_or_default().trim().to_ascii_lowercase();
    if topic.is_empty() {
        return Ok(SOFYA_DEFAULT_TOPIC.to_owned());
    }
    if !SOFYA_TOPICS.contains(&topic.as_str()) {
        anyhow::bail!("Topic {topic} is not supported, it should be in {SOFYA_TOPICS:?}");
    }
    Ok(topic)
}

/// `_freshness`: blank and `any` both mean no restriction.
pub fn normalize_freshness(value: Option<&str>) -> Result<String> {
    let freshness = value.unwrap_or_default().trim().to_ascii_lowercase();
    if freshness.is_empty() || freshness == SOFYA_FRESHNESS_ANY {
        return Ok(String::new());
    }
    if !SOFYA_FRESHNESS_VALUES.contains(&freshness.as_str()) {
        anyhow::bail!(
            "Freshness {freshness} is not supported, it should be in {SOFYA_FRESHNESS_VALUES:?}"
        );
    }
    Ok(freshness)
}

/// `_max_results`: caller value first, else the node's Top N; clamped 1..=20.
pub fn resolve_max_results(caller: Option<i64>, fallback: Option<i64>) -> i64 {
    caller.or(fallback).unwrap_or(1).clamp(1, SOFYA_MAX_RESULTS)
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

/// `_result_content`: page content first, search snippet as the fallback.
pub fn result_content(result: &Value) -> String {
    let content = collapse_whitespace(&py_text(result.get("content")));
    if !content.is_empty() {
        return content;
    }
    collapse_whitespace(&py_text(result.get("description")))
}

/// `_results`: object entries only, capped at `top_n`.
pub fn results_from_response(data: &Value, top_n: usize) -> Vec<Value> {
    data.get("results")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.is_object())
                .take(top_n)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

pub fn build_search_payload(
    query: &str,
    search_depth: &str,
    max_results: i64,
    topic: &str,
    freshness: &str,
) -> Value {
    let mut payload = json!({
        "query": query,
        "search_depth": search_depth,
        "max_results": max_results,
        "topic": topic,
    });
    if !freshness.is_empty() {
        payload["freshness"] = json!(freshness);
    }
    payload
}

/// Async Sofya HTTP client.
#[derive(Debug, Clone)]
pub struct SofyaClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl Default for SofyaClient {
    fn default() -> Self {
        Self::new_with_endpoint(SOFYA_SEARCH_URL)
            .expect("fixed Sofya endpoint and HTTP client configuration are valid")
    }
}

impl SofyaClient {
    pub(crate) fn new_with_endpoint(endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .map_err(|error| anyhow::anyhow!("could not build Sofya HTTP client: {error}"))?;
        Ok(Self {
            client,
            endpoint: endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid Sofya endpoint: {error}"))?,
        })
    }

    /// `_invoke`'s retry loop: transient failures (retryable statuses or
    /// network errors) retry up to `attempts`; everything else fails at once.
    pub async fn search(
        &self,
        api_key: &str,
        payload: &Value,
        attempts: usize,
        delay: Duration,
    ) -> Result<Value> {
        let attempts = attempts.max(1);
        let mut last_error = String::from("unknown error");
        for attempt in 0..attempts {
            let outcome = self
                .client
                .post(self.endpoint.clone())
                .bearer_auth(api_key.trim())
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header(reqwest::header::ACCEPT, "application/json")
                .header(reqwest::header::USER_AGENT, SOFYA_USER_AGENT)
                .json(payload)
                .send()
                .await;
            match outcome {
                Ok(response) => {
                    let status = response.status();
                    if status.is_success() {
                        let body = read_bounded_body(response).await?;
                        return serde_json::from_slice(&body).map_err(|error| {
                            anyhow::anyhow!("Sofya API response was not valid JSON: {error}")
                        });
                    }
                    last_error = format!("HTTP {status}");
                    let transient = SOFYA_RETRY_STATUSES.contains(&status.as_u16());
                    if !transient || attempt + 1 >= attempts {
                        anyhow::bail!("Sofya search failed: HTTP {status}");
                    }
                }
                Err(error) => {
                    last_error = error.to_string();
                    if attempt + 1 >= attempts {
                        anyhow::bail!("Sofya search failed: {error}");
                    }
                }
            }
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        anyhow::bail!("Sofya search failed: {last_error}");
    }
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    use futures_util::StreamExt;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| anyhow::anyhow!("Sofya response read: {error}"))?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!("Sofya response exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}


