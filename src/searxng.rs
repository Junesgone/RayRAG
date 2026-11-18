//! SearXNG metasearch connector — mirrors RAGFlow `agent/tools/searxng.py`.
//! SearXNG is a self-hostable privacy metasearch engine aggregating many
//! engines (Google/Bing/Baidu/...), so one instance works both for mainland
//! China (self-hosted on a domestic server) and globally. No API key needed;
//! the operator provides the instance base URL (`SEARXNG_URL`, or per-node
//! `searxng_url`).
//!
//! RayRAG follows the RAGFlow JSON API shape (`/search?q=..&format=json`),
//! maps results to the standard RAGFlow tool rows, and applies the shared
//! domestic-search output path (chunks/references/doc_aggs).

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const MAX_SEARXNG_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearxngRequest {
    pub query: String,
    /// SearXNG instance base URL, e.g. `http://localhost:4000`.
    pub searxng_url: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearxngResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub score: f64,
    #[serde(default)]
    pub category: String,
}

#[async_trait]
pub trait SearxngProvider: Send + Sync {
    async fn search(&self, request: &SearxngRequest) -> Result<Vec<SearxngResult>>;
}

#[derive(Debug, Clone)]
pub struct SearxngClient {
    client: reqwest::Client,
}

impl Default for SearxngClient {
    fn default() -> Self {
        Self::new().expect("SearXNG client construction cannot fail")
    }
}

impl SearxngClient {
    pub(crate) fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .context("could not build SearXNG HTTP client")?;
        Ok(Self { client })
    }

    fn parse_results(payload: &Value) -> Result<Vec<SearxngResult>> {
        let data = payload
            .as_object()
            .ok_or_else(|| anyhow!("searxng: response is not a JSON object"))?;
        let results = data
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("searxng: missing results array"))?;
        Ok(results
            .iter()
            .filter_map(|item| {
                let obj = item.as_object()?;
                let title = obj.get("title").and_then(Value::as_str).unwrap_or_default();
                let link = obj.get("url").and_then(Value::as_str).unwrap_or_default();
                if title.is_empty() && link.is_empty() {
                    return None;
                }
                Some(SearxngResult {
                    title: title.to_string(),
                    link: link.to_string(),
                    snippet: obj
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    engine: obj
                        .get("engine")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    score: obj.get("score").and_then(Value::as_f64).unwrap_or(0.0),
                    category: obj
                        .get("category")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
            })
            .collect())
    }
}

#[async_trait]
impl SearxngProvider for SearxngClient {
    async fn search(&self, request: &SearxngRequest) -> Result<Vec<SearxngResult>> {
        let base = request.searxng_url.trim().trim_end_matches('/');
        if base.is_empty() {
            bail!("searxng: instance URL is required (SEARXNG_URL or searxng_url)");
        }
        if request.query.trim().is_empty() {
            bail!("searxng: query is required");
        }
        let url = format!(
            "{base}/search?q={}&format=json&categories=general&language=auto&safesearch=1&pageno=1",
            urlencoding(request.query.trim())
        );
        let response = self
            .client
            .get(&url)
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
            )
            .send()
            .await
            .context("SearXNG request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("could not read SearXNG response body")?;
        if body.len() > MAX_SEARXNG_RESPONSE_BODY {
            bail!(
                "SearXNG response body exceeds {} bytes",
                MAX_SEARXNG_RESPONSE_BODY
            );
        }
        let payload: Value = serde_json::from_slice(&body)
            .with_context(|| format!("SearXNG response (HTTP {status}) was not valid JSON"))?;
        if !status.is_success() {
            let message = payload
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("upstream error");
            bail!("searxng: upstream returned {}: {message}", status.as_u16());
        }
        let mut results = Self::parse_results(&payload)?;
        if request.top_n > 0 && results.len() > request.top_n {
            results.truncate(request.top_n);
        }
        Ok(results)
    }
}

/// Minimal URL-encode for query params.
fn urlencoding(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

/// Convert SearXNG results into RAGFlow tool rows.
pub fn searxng_results_to_tool_rows(results: &[SearxngResult]) -> Vec<Value> {
    results
        .iter()
        .map(|result| {
            let mut row = Map::new();
            row.insert("title".into(), Value::String(result.title.clone()));
            row.insert("link".into(), Value::String(result.link.clone()));
            row.insert("snippet".into(), Value::String(result.snippet.clone()));
            if !result.engine.is_empty() {
                row.insert("engine".into(), Value::String(result.engine.clone()));
            }
            Value::Object(row)
        })
        .collect()
}


