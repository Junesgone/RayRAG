//! Bocha (博查) search connector — mainland-China AI search API, requires a
//! Bocha API key. Returns RAGFlow-shaped tool rows (title/link/snippet) plus
//! a richer reference payload for LLM citations.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const BOCHA_ENDPOINT: &str = "https://api.bochaai.com/v1/web-search";
const MAX_BOCHA_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BochaSearchRequest {
    pub query: String,
    /// `web` or `news` (博查支持 news 垂直检索).
    pub channel: String,
    pub top_n: usize,
    pub freshness: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BochaSearchResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub publish_date: String,
}

#[async_trait]
pub trait BochaProvider: Send + Sync {
    async fn search(
        &self,
        api_key: &str,
        request: &BochaSearchRequest,
    ) -> Result<Vec<BochaSearchResult>>;
}

#[derive(Debug, Clone)]
pub struct BochaClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl Default for BochaClient {
    fn default() -> Self {
        Self::new_with_endpoints(BOCHA_ENDPOINT).expect("fixed Bocha endpoint is valid")
    }
}

impl BochaClient {
    pub(crate) fn new_with_endpoints(endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .context("could not build Bocha HTTP client")?;
        Ok(Self {
            client,
            endpoint: endpoint.parse().context("invalid Bocha endpoint")?,
        })
    }
}

#[async_trait]
impl BochaProvider for BochaClient {
    async fn search(
        &self,
        api_key: &str,
        request: &BochaSearchRequest,
    ) -> Result<Vec<BochaSearchResult>> {
        if api_key.trim().is_empty() {
            bail!("bocha: API key is required");
        }
        if request.query.trim().is_empty() {
            bail!("bocha: query is required");
        }
        let mut body = Map::new();
        body.insert("query".into(), Value::String(request.query.clone()));
        body.insert(
            "count".into(),
            Value::Number(request.top_n.clamp(1, 20).into()),
        );
        let summary = if request.channel.eq_ignore_ascii_case("news") {
            "true"
        } else {
            "false"
        };
        body.insert("summary".into(), Value::Bool(summary == "true"));
        if let Some(freshness) = request.freshness.as_deref().filter(|f| !f.is_empty()) {
            body.insert("freshness".into(), Value::String(freshness.to_string()));
        }
        let response = self
            .client
            .post(self.endpoint.clone())
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .json(&Value::Object(body))
            .send()
            .await
            .context("Bocha request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("could not read Bocha response body")?;
        if body.len() > MAX_BOCHA_RESPONSE_BODY {
            bail!(
                "Bocha response body exceeds {} bytes",
                MAX_BOCHA_RESPONSE_BODY
            );
        }
        let envelope: Value = serde_json::from_slice(&body)
            .with_context(|| format!("Bocha response (HTTP {status}) was not valid JSON"))?;
        if !status.is_success() {
            let detail = envelope
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("upstream error");
            bail!("bocha: upstream returned {}: {detail}", status.as_u16());
        }
        let pages = envelope
            .pointer("/data/webPages/value")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("bocha: missing data.webPages.value in response"))?;
        Ok(pages
            .iter()
            .filter_map(|page| {
                let title = page
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let link = page.get("url").and_then(Value::as_str).unwrap_or_default();
                if title.is_empty() || link.is_empty() {
                    return None;
                }
                Some(BochaSearchResult {
                    title: title.to_string(),
                    link: link.to_string(),
                    snippet: page
                        .get("summary")
                        .and_then(Value::as_str)
                        .or_else(|| page.get("description").and_then(Value::as_str))
                        .unwrap_or_default()
                        .to_string(),
                    source: page
                        .get("siteName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    publish_date: page
                        .get("publishTime")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
            })
            .take(request.top_n.clamp(1, 20))
            .collect())
    }
}

/// Convert Bocha results into RAGFlow tool rows.
pub fn bocha_results_to_tool_rows(results: &[BochaSearchResult]) -> Vec<Value> {
    results
        .iter()
        .map(|result| {
            let mut row = Map::new();
            row.insert("title".into(), Value::String(result.title.clone()));
            row.insert("link".into(), Value::String(result.link.clone()));
            row.insert("snippet".into(), Value::String(result.snippet.clone()));
            Value::Object(row)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_ragflow_shaped() {
        let rows = bocha_results_to_tool_rows(&[BochaSearchResult {
            title: "t".into(),
            link: "https://l".into(),
            snippet: "s".into(),
            source: "src".into(),
            publish_date: String::new(),
        }]);
        assert_eq!(rows[0]["title"], "t");
        assert_eq!(rows[0]["link"], "https://l");
        assert_eq!(rows[0]["snippet"], "s");
    }

    #[tokio::test]
    async fn missing_key_or_query_is_rejected() {
        let client = BochaClient::default();
        let error = client
            .search(
                "",
                &BochaSearchRequest {
                    query: "q".into(),
                    channel: "web".into(),
                    top_n: 5,
                    freshness: None,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("API key"));
        let error = client
            .search(
                "key",
                &BochaSearchRequest {
                    query: "".into(),
                    channel: "web".into(),
                    top_n: 5,
                    freshness: None,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("query"));
    }

    #[tokio::test]
    #[ignore = "requires a live Bocha API key (BOCHA_API_KEY)"]
    async fn live_bocha_search() {
        let api_key = std::env::var("BOCHA_API_KEY").expect("BOCHA_API_KEY must be set");
        let results = BochaClient::default()
            .search(
                &api_key,
                &BochaSearchRequest {
                    query: "RAGFlow".into(),
                    channel: "web".into(),
                    top_n: 5,
                    freshness: None,
                },
            )
            .await
            .unwrap();
        assert!(!results.is_empty());
    }
}
