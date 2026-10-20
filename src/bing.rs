//! Bing search connector — mainland-China reachable (`cn.bing.com`), no API
//! key required. Mirrors the RAGFlow Agent search-tool result shape so canvas
//! workflows can swap Google/SerpApi for Bing without changing downstream
//! consumers.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use regex::Regex;
use reqwest::redirect::Policy as RedirectPolicy;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::OnceLock;

const BING_ENDPOINT: &str = "https://cn.bing.com/search";
const BING_ENDPOINT_FALLBACK: &str = "https://www.bing.com/search";
const MAX_BING_RESPONSE_BODY: usize = 16 << 20;
const BING_USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) RayRAG/",
    env!("CARGO_PKG_VERSION")
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BingSearchRequest {
    pub query: String,
    /// `general` or `news` (Bing news vertical via `qft` filter).
    pub channel: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BingSearchResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
}

#[async_trait]
pub trait BingProvider: Send + Sync {
    async fn search(&self, request: &BingSearchRequest) -> Result<Vec<BingSearchResult>>;
}

#[derive(Debug, Clone)]
pub struct BingClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
    fallback_endpoint: reqwest::Url,
}

impl Default for BingClient {
    fn default() -> Self {
        Self::new_with_endpoints(BING_ENDPOINT, BING_ENDPOINT_FALLBACK)
            .expect("fixed Bing endpoints and HTTP configuration are valid")
    }
}

impl BingClient {
    pub(crate) fn new_with_endpoints(endpoint: &str, fallback: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .user_agent(BING_USER_AGENT)
            .build()
            .context("could not build Bing HTTP client")?;
        Ok(Self {
            client,
            endpoint: endpoint.parse().context("invalid Bing endpoint")?,
            fallback_endpoint: fallback.parse().context("invalid Bing fallback endpoint")?,
        })
    }

    async fn fetch(&self, url: reqwest::Url) -> Result<(reqwest::StatusCode, Vec<u8>)> {
        let response = self
            .client
            .get(url)
            .header("Accept-Language", "zh-CN,zh;q=0.9,en;q=0.8")
            .send()
            .await
            .context("Bing request failed")?;
        let status = response.status();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("could not read Bing response body")?;
            let remaining = MAX_BING_RESPONSE_BODY.saturating_sub(body.len());
            if chunk.len() > remaining {
                bail!(
                    "Bing response body exceeds {} bytes",
                    MAX_BING_RESPONSE_BODY
                );
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, body))
    }
}

fn bing_result_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();
    SELECTOR.get_or_init(|| Selector::parse("li.b_algo").expect("valid Bing result selector"))
}

fn strip_tags(html: &str) -> String {
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    TAG_RE
        .get_or_init(|| Regex::new(r"<[^>]+>").expect("valid tag regex"))
        .replace_all(html, " ")
        .to_string()
}

fn parse_bing_results(html: &str) -> Vec<BingSearchResult> {
    let document = Html::parse_document(html);
    let mut results = Vec::new();
    for item in document.select(bing_result_selector()) {
        let title = item
            .select(&Selector::parse("h2 a").expect("valid title selector"))
            .next()
            .map(|link| strip_tags(&link.html()).trim().to_string())
            .unwrap_or_default();
        let link = item
            .select(&Selector::parse("h2 a").expect("valid link selector"))
            .next()
            .and_then(|link| link.value().attr("href"))
            .unwrap_or_default()
            .to_string();
        let snippet = item
            .select(&Selector::parse("p, .b_caption p").expect("valid snippet selector"))
            .next()
            .map(|paragraph| strip_tags(&paragraph.html()).trim().to_string())
            .unwrap_or_default();
        if !title.is_empty() && !link.is_empty() {
            results.push(BingSearchResult {
                title,
                link,
                snippet,
            });
        }
    }
    results
}

#[async_trait]
impl BingProvider for BingClient {
    async fn search(&self, request: &BingSearchRequest) -> Result<Vec<BingSearchResult>> {
        if request.query.trim().is_empty() {
            bail!("bing: query is required");
        }
        let mut params: Vec<(&str, String)> = vec![
            ("q", request.query.clone()),
            ("count", request.top_n.clamp(1, 20).to_string()),
        ];
        if request.channel.eq_ignore_ascii_case("news") {
            // Bing news vertical: qft filter for news results.
            params.push(("qft", "interval%3d%227%22".to_string()));
        }
        let url = self
            .endpoint
            .clone()
            .query_pairs_mut()
            .extend_pairs(params)
            .finish()
            .clone();
        let (status, body) = self.fetch(url).await?;
        if !status.is_success() {
            // Fall back to the international endpoint once (CN redirects/bans).
            let fallback_url = self
                .fallback_endpoint
                .clone()
                .query_pairs_mut()
                .extend_pairs([("q", request.query.as_str())])
                .finish()
                .clone();
            let (status, body) = self.fetch(fallback_url).await?;
            if !status.is_success() {
                bail!("bing: upstream returned {}", status.as_u16());
            }
            return Ok(parse_bing_results(&String::from_utf8_lossy(&body))
                .into_iter()
                .take(request.top_n.clamp(1, 20))
                .collect());
        }
        Ok(parse_bing_results(&String::from_utf8_lossy(&body))
            .into_iter()
            .take(request.top_n.clamp(1, 20))
            .collect())
    }
}

/// Convert Bing results into the RAGFlow tool shape `[{title, link, snippet}]`.
pub fn bing_results_to_tool_rows(results: &[BingSearchResult]) -> Vec<Value> {
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
    fn parses_bing_html_into_rows() {
        let html = r#"
<html><body>
<li class="b_algo">
  <h2><a href="https://example.com/page">Example Title</a></h2>
  <div class="b_caption"><p>Some <b>snippet</b> text.</p></div>
</li>
<li class="b_algo">
  <h2><a href="https://example.org/other">Other</a></h2>
  <p>Second snippet</p>
</li>
</body></html>"#;
        let results = parse_bing_results(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example Title");
        assert_eq!(results[0].link, "https://example.com/page");
        assert!(results[0].snippet.contains("snippet"));
        assert_eq!(results[1].title, "Other");
    }

    #[test]
    fn rows_are_ragflow_shaped() {
        let rows = bing_results_to_tool_rows(&[BingSearchResult {
            title: "t".into(),
            link: "https://l".into(),
            snippet: "s".into(),
        }]);
        assert_eq!(rows[0]["title"], "t");
        assert_eq!(rows[0]["link"], "https://l");
        assert_eq!(rows[0]["snippet"], "s");
    }

    #[tokio::test]
    async fn empty_query_is_rejected() {
        let client = BingClient::default();
        let error = client
            .search(&BingSearchRequest {
                query: "   ".into(),
                channel: "general".into(),
                top_n: 5,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("query is required"));
    }

    #[tokio::test]
    #[ignore = "requires live access to cn.bing.com"]
    async fn live_bing_search() {
        let results = BingClient::default()
            .search(&BingSearchRequest {
                query: "RAGFlow".into(),
                channel: "general".into(),
                top_n: 5,
            })
            .await
            .unwrap();
        assert!(!results.is_empty());
    }
}
