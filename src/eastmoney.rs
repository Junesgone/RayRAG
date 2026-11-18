//! EastMoney (东方财富) A-share news connector — mirrors RAGFlow
//! `agent/tools/akshare.py` AkShare.stock_news_em, but calls the underlying
//! EastMoney JSONP search API directly so no Python/akshare dependency is
//! needed. No API key required; mainland-China direct access.
//!
//! RAGFlow's AkShare component maps the user input to `ak.stock_news_em(symbol)`
//! and renders rows as HTML anchors. RayRAG instead returns the standard
//! RAGFlow tool rows (title/link/snippet) plus a `json` payload so the Agent
//! canvas can use the shared domestic-search output path.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// EastMoney JSONP search endpoint (search-api-web), same one AkShare's
/// `stock_news_em` calls under the hood.
const EASTMONEY_ENDPOINT: &str = "https://search-api-web.eastmoney.com/search/jsonp";
const MAX_EASTMONEY_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EastMoneyNewsRequest {
    /// Stock symbol (e.g. "600519" or "贵州茅台"). RAGFlow passes the whole
    /// user input; empty input short-circuits to an empty result.
    pub symbol: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EastMoneyNewsResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
    #[serde(default)]
    pub publish_time: String,
    #[serde(default)]
    pub media_name: String,
}

#[async_trait]
pub trait EastMoneyProvider: Send + Sync {
    async fn search(&self, request: &EastMoneyNewsRequest) -> Result<Vec<EastMoneyNewsResult>>;
}

#[derive(Debug, Clone)]
pub struct EastMoneyClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl Default for EastMoneyClient {
    fn default() -> Self {
        Self::new_with_endpoints(EASTMONEY_ENDPOINT).expect("fixed EastMoney endpoint is valid")
    }
}

impl EastMoneyClient {
    pub(crate) fn new_with_endpoints(endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .context("could not build EastMoney HTTP client")?;
        Ok(Self {
            client,
            endpoint: endpoint.parse().context("invalid EastMoney endpoint")?,
        })
    }

    /// Build the JSONP `param` argument. The `cb` callback name is arbitrary;
    /// we strip it after the fetch.
    fn build_param(symbol: &str, page_size: usize) -> String {
        let param = serde_json::json!({
            "uid": "",
            "keyword": symbol,
            "type": ["cmsArticleWebOld"],
            "client": "web",
            "clientType": "web",
            "clientVersion": "curr",
            "param": {
                "cmsArticleWebOld": {
                    "searchScope": "default",
                    "sort": "default",
                    "pageIndex": 1,
                    "pageSize": page_size.clamp(1, 50),
                    "preTag": "<em>",
                    "postTag": "</em>"
                }
            }
        });
        // The upstream API expects URL-encoded JSON; serde_json produces
        // compact JSON without spaces, matching AkShare's json.dumps(...).
        param.to_string()
    }

    /// Strip a JSONP wrapper `callback(...)` and parse the inner JSON.
    fn strip_jsonp(body: &str) -> Result<Value> {
        let body = body.trim();
        let start = body
            .find('(')
            .ok_or_else(|| anyhow!("eastmoney: response is not JSONP (no '(')"))?;
        let end = body
            .rfind(')')
            .ok_or_else(|| anyhow!("eastmoney: response is not JSONP (no ')')"))?;
        if end <= start {
            bail!("eastmoney: malformed JSONP wrapper");
        }
        serde_json::from_str(&body[start + 1..end])
            .context("eastmoney: JSONP payload was not valid JSON")
    }
}

#[async_trait]
impl EastMoneyProvider for EastMoneyClient {
    async fn search(&self, request: &EastMoneyNewsRequest) -> Result<Vec<EastMoneyNewsResult>> {
        if request.symbol.trim().is_empty() {
            bail!("eastmoney: symbol is required");
        }
        let param = Self::build_param(request.symbol.trim(), request.top_n);
        let url = self
            .endpoint
            .clone()
            .query_pairs_mut()
            .append_pair("cb", "x")
            .append_pair("param", &param)
            .finish()
            .to_string();
        let response = self
            .client
            .get(&url)
            .header("Referer", "https://so.eastmoney.com/")
            .header(
                "User-Agent",
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
            )
            .send()
            .await
            .context("EastMoney request failed")?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .context("could not read EastMoney response body")?;
        if body.len() > MAX_EASTMONEY_RESPONSE_BODY {
            bail!(
                "EastMoney response body exceeds {} bytes",
                MAX_EASTMONEY_RESPONSE_BODY
            );
        }
        let text = String::from_utf8_lossy(&body);
        if !status.is_success() {
            bail!("eastmoney: upstream returned {}", status.as_u16());
        }
        let envelope = Self::strip_jsonp(&text)?;
        if envelope
            .get("code")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
            != 0
        {
            let msg = envelope
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("upstream error");
            bail!("eastmoney: upstream error: {msg}");
        }
        let articles = envelope
            .pointer("/result/cmsArticleWebOld")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("eastmoney: missing result.cmsArticleWebOld in response"))?;
        Ok(articles
            .iter()
            .filter_map(|article| {
                let title = article
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let link = article
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if title.is_empty() || link.is_empty() {
                    return None;
                }
                Some(EastMoneyNewsResult {
                    // Strip AkShare's <em> highlight tags the same way the
                    // Python client would (it keeps them, but tool rows look
                    // cleaner without them).
                    title: title.replace("<em>", "").replace("</em>", ""),
                    link: link.to_string(),
                    snippet: article
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .replace("<em>", "")
                        .replace("</em>", ""),
                    publish_time: article
                        .get("date")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    media_name: article
                        .get("mediaName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                })
            })
            .take(request.top_n.clamp(1, 50))
            .collect())
    }
}

/// Convert EastMoney results into RAGFlow tool rows.
pub fn eastmoney_results_to_tool_rows(results: &[EastMoneyNewsResult]) -> Vec<Value> {
    results
        .iter()
        .map(|result| {
            let mut row = Map::new();
            row.insert("title".into(), Value::String(result.title.clone()));
            row.insert("link".into(), Value::String(result.link.clone()));
            row.insert("snippet".into(), Value::String(result.snippet.clone()));
            if !result.publish_time.is_empty() {
                row.insert(
                    "publish_time".into(),
                    Value::String(result.publish_time.clone()),
                );
            }
            if !result.media_name.is_empty() {
                row.insert(
                    "media_name".into(),
                    Value::String(result.media_name.clone()),
                );
            }
            Value::Object(row)
        })
        .collect()
}


