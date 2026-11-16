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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::Query,
        http::{HeaderMap, StatusCode},
        routing::get,
    };
    use std::collections::HashMap;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn freshness_payload_and_content() {
        assert_eq!(normalize_freshness(None).unwrap(), "");
        assert_eq!(normalize_freshness(Some("ANY")).unwrap(), "");
        assert_eq!(normalize_freshness(Some("month")).unwrap(), "month");
        assert!(normalize_freshness(Some("hour")).is_err());

        let params = build_params("q", 150, "day");
        assert_eq!(params["count"], 100);
        assert_eq!(params["freshness"], "day");
        assert!(build_params("q", 10, "").get("freshness").is_none());

        let web = json!({"snippets": [" a  b ", "", "c"], "description": "ignored"});
        assert_eq!(result_content(&web), "a b c");
        let news = json!({"description": " the  news "});
        assert_eq!(result_content(&news), "the news");
    }

    #[test]
    fn merge_sections_prefers_web_then_news_and_trims() {
        let results = json!({
            "web": [{"url": "w1"}, {"url": "w2"}, {"url": "w3"}],
            "news": [{"url": "n1"}, {"url": "n2"}]
        });
        let merged = merge_sections(&results, 3);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0]["url"], "w1");
        assert_eq!(merged[2]["url"], "w3");
        let merged_more = merge_sections(&results, 10);
        assert_eq!(merged_more.len(), 5);
        assert_eq!(merged_more[4]["url"], "n2");
        assert!(merge_sections(&json!("nope"), 5).is_empty());
    }

    async fn spawn_sequence(
        statuses: Vec<StatusCode>,
    ) -> (
        String,
        Arc<AtomicUsize>,
        Arc<std::sync::Mutex<Vec<Value>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let counter = Arc::new(AtomicUsize::new(0));
        let capture = Arc::new(std::sync::Mutex::new(Vec::new()));
        let state = counter.clone();
        let seen = capture.clone();
        let app = Router::new()
            .route(
                "/v1/search",
                get({
                    let state = state.clone();
                    let seen = seen.clone();
                    move |headers: HeaderMap, Query(params): Query<HashMap<String, String>>| {
                        let state = state.clone();
                        let seen = seen.clone();
                        let statuses = statuses.clone();
                        async move {
                            let index = state.fetch_add(1, Ordering::SeqCst);
                            seen.lock().unwrap().push(json!({
                                "key": headers.get("x-api-key").and_then(|v| v.to_str().ok()).unwrap_or_default(),
                                "user_agent": headers.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or_default(),
                                "params": params,
                            }));
                            let status = statuses.get(index).copied().unwrap_or(StatusCode::OK);
                            if status == StatusCode::OK {
                                (status, Json(json!({"results": {"web": [{"url": "w", "snippets": ["s"]}]}})))
                            } else {
                                (status, Json(json!({"error": "bad"})))
                            }
                        }
                    }
                }),
            )
            .route("/v1/agents/search", get(|| async { (StatusCode::UNAUTHORIZED, "keyless reached") }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), counter, capture, server)
    }

    #[tokio::test]
    async fn keyed_search_sends_key_and_params() {
        let (base, counter, capture, server) = spawn_sequence(vec![StatusCode::OK]).await;
        let client = YouComClient::new_with_endpoints(
            &format!("{base}/v1/search"),
            &format!("{base}/v1/agents/search"),
        )
        .unwrap();
        let params = build_params("hello world", 10, "week");
        let data = client
            .search("sk-y", &params, 1, Duration::from_millis(1))
            .await
            .unwrap();
        server.abort();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        assert_eq!(data["results"]["web"][0]["url"], "w");
        let seen = capture.lock().unwrap();
        assert_eq!(seen[0]["key"], "sk-y");
        assert_eq!(seen[0]["user_agent"], YOUCOM_USER_AGENT);
        assert_eq!(seen[0]["params"]["query"], "hello world");
        assert_eq!(seen[0]["params"]["count"], "10");
        assert_eq!(seen[0]["params"]["freshness"], "week");
    }

    #[tokio::test]
    async fn retries_every_failure_then_reports_status() {
        let (base, counter, _, server) =
            spawn_sequence(vec![StatusCode::BAD_REQUEST, StatusCode::BAD_REQUEST]).await;
        let client = YouComClient::new_with_endpoints(
            &format!("{base}/v1/search"),
            &format!("{base}/v1/agents/search"),
        )
        .unwrap();
        let error = client
            .search(
                "sk",
                &build_params("q", 10, ""),
                2,
                Duration::from_millis(1),
            )
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("400"), "{error}");
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
