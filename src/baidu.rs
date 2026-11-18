//! Baidu search connector — mainland-China native, no API key required.
//! Parses the desktop result page and returns RAGFlow-shaped tool rows.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use regex::Regex;
use reqwest::redirect::Policy as RedirectPolicy;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::OnceLock;

const BAIDU_ENDPOINT: &str = "https://www.baidu.com/s";
const MAX_BAIDU_RESPONSE_BODY: usize = 16 << 20;
const BAIDU_USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) RayRAG/",
    env!("CARGO_PKG_VERSION")
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaiduSearchRequest {
    pub query: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaiduSearchResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
}

#[async_trait]
pub trait BaiduProvider: Send + Sync {
    async fn search(&self, request: &BaiduSearchRequest) -> Result<Vec<BaiduSearchResult>>;
}

#[derive(Debug, Clone)]
pub struct BaiduClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl Default for BaiduClient {
    fn default() -> Self {
        Self::new_with_endpoints(BAIDU_ENDPOINT).expect("fixed Baidu endpoint is valid")
    }
}

impl BaiduClient {
    pub(crate) fn new_with_endpoints(endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            // Baidu issues a 302 safety-verification hop before the result page;
            // follow a bounded number of redirects while keeping the timeout.
            .redirect(RedirectPolicy::limited(3))
            .cookie_store(true)
            .timeout(crate::common::cmd_timeout::duration())
            .user_agent(BAIDU_USER_AGENT)
            .build()
            .context("could not build Baidu HTTP client")?;
        Ok(Self {
            client,
            endpoint: endpoint.parse().context("invalid Baidu endpoint")?,
        })
    }
}

fn strip_tags(html: &str) -> String {
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    TAG_RE
        .get_or_init(|| Regex::new(r"<[^>]+>").expect("valid tag regex"))
        .replace_all(html, " ")
        .to_string()
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

fn parse_baidu_results(html: &str) -> Vec<BaiduSearchResult> {
    let document = Html::parse_document(html);
    let container = Selector::parse("div#content_left").expect("valid container selector");
    let result_selector = Selector::parse("div.result, div.c-container, div[class*='result']")
        .expect("valid result selector");
    let title_selector = Selector::parse("h3 a, h3").expect("valid title selector");
    let mut results = Vec::new();
    if let Some(content) = document.select(&container).next() {
        for item in content.select(&result_selector) {
            let title = item
                .select(&title_selector)
                .next()
                .map(|node| decode_entities(strip_tags(&node.html()).trim()).to_string())
                .unwrap_or_default();
            let link = item
                .select(&Selector::parse("h3 a").expect("valid link selector"))
                .next()
                .and_then(|node| node.value().attr("href"))
                .unwrap_or_default()
                .to_string();
            let snippet = item
                .select(
                    &Selector::parse(".c-abstract, .content-right_8Zs40, .c-span-last, span")
                        .expect("valid snippet selector"),
                )
                .next()
                .map(|node| decode_entities(strip_tags(&node.html()).trim()).to_string())
                .unwrap_or_default();
            if !title.is_empty() {
                results.push(BaiduSearchResult {
                    title,
                    link,
                    snippet,
                });
            }
        }
    }
    // Deduplicate by title, keep first N.
    let mut seen = std::collections::HashSet::new();
    results
        .into_iter()
        .filter(|result| seen.insert(result.title.clone()))
        .collect()
}

#[async_trait]
impl BaiduProvider for BaiduClient {
    async fn search(&self, request: &BaiduSearchRequest) -> Result<Vec<BaiduSearchResult>> {
        if request.query.trim().is_empty() {
            bail!("baidu: query is required");
        }
        let url = self
            .endpoint
            .clone()
            .query_pairs_mut()
            .extend_pairs([("wd", request.query.as_str())])
            .finish()
            .clone();
        let response = self
            .client
            .get(url)
            .header("Accept-Language", "zh-CN,zh;q=0.9")
            .header(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            )
            .header("Sec-Fetch-Dest", "document")
            .header("Sec-Fetch-Mode", "navigate")
            .header("Sec-Fetch-Site", "none")
            .header(
                "sec-ch-ua",
                "\"Not_A Brand\";v=\"8\", \"Chromium\";v=\"120\", \"Google Chrome\";v=\"120\"",
            )
            .send()
            .await
            .context("Baidu request failed")?;
        let status = response.status();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("could not read Baidu response body")?;
            let remaining = MAX_BAIDU_RESPONSE_BODY.saturating_sub(body.len());
            if chunk.len() > remaining {
                bail!(
                    "Baidu response body exceeds {} bytes",
                    MAX_BAIDU_RESPONSE_BODY
                );
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            bail!("baidu: upstream returned {}", status.as_u16());
        }
        let html = String::from_utf8_lossy(&body);
        // Baidu serves a "百度安全验证" interstitial instead of results when it
        // suspects automation. Surface that as a clear error rather than an
        // empty result set so the agent layer can retry or fall back.
        if html.contains("百度安全验证")
            || html.contains("wappass")
            || (html.contains("content_left") && !html.contains("id=\"content_left\""))
        {
            bail!("baidu: safety verification page returned; retry later or use another provider");
        }
        let top_n = request.top_n.clamp(1, 20);
        Ok(parse_baidu_results(&html).into_iter().take(top_n).collect())
    }
}

/// Convert Baidu results into RAGFlow tool rows.
pub fn baidu_results_to_tool_rows(results: &[BaiduSearchResult]) -> Vec<Value> {
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


