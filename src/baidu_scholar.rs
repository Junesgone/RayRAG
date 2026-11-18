//! Baidu Scholar (百度学术) connector — mainland-China academic search, no API
//! key required. Parses the `xueshu.baidu.com` result page into RAGFlow-shaped
//! tool rows (title/link/snippet) with extra metadata when available.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use regex::Regex;
use reqwest::redirect::Policy as RedirectPolicy;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::OnceLock;

const BAIDU_SCHOLAR_ENDPOINT: &str = "https://xueshu.baidu.com/s";
const MAX_BAIDU_SCHOLAR_RESPONSE_BODY: usize = 16 << 20;
const BAIDU_SCHOLAR_USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) RayRAG/",
    env!("CARGO_PKG_VERSION")
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaiduScholarRequest {
    pub query: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaiduScholarResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
    #[serde(default)]
    pub authors: String,
    #[serde(default)]
    pub year: String,
}

#[async_trait]
pub trait BaiduScholarProvider: Send + Sync {
    async fn search(&self, request: &BaiduScholarRequest) -> Result<Vec<BaiduScholarResult>>;
}

#[derive(Debug, Clone)]
pub struct BaiduScholarClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl Default for BaiduScholarClient {
    fn default() -> Self {
        Self::new_with_endpoints(BAIDU_SCHOLAR_ENDPOINT)
            .expect("fixed Baidu Scholar endpoint is valid")
    }
}

impl BaiduScholarClient {
    pub(crate) fn new_with_endpoints(endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::limited(3))
            .cookie_store(true)
            .timeout(crate::common::cmd_timeout::duration())
            .user_agent(BAIDU_SCHOLAR_USER_AGENT)
            .build()
            .context("could not build Baidu Scholar HTTP client")?;
        Ok(Self {
            client,
            endpoint: endpoint.parse().context("invalid Baidu Scholar endpoint")?,
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

fn parse_baidu_scholar_results(html: &str) -> Vec<BaiduScholarResult> {
    let document = Html::parse_document(html);
    let result_selector = Selector::parse("div.result, div.sc_content, div[class*='result']")
        .expect("valid result selector");
    let title_selector = Selector::parse("h3 a, h3").expect("valid title selector");
    let mut results = Vec::new();
    for item in document.select(&result_selector) {
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
                &Selector::parse(".sc_content, .c_abstract, .sc_abstract")
                    .expect("valid snippet selector"),
            )
            .next()
            .map(|node| decode_entities(strip_tags(&node.html()).trim()).to_string())
            .unwrap_or_default();
        let authors = item
            .select(&Selector::parse(".sc_author").expect("valid author selector"))
            .next()
            .map(|node| decode_entities(strip_tags(&node.html()).trim()).to_string())
            .unwrap_or_default();
        let year = item
            .select(&Selector::parse(".sc_year, .sc_time").expect("valid year selector"))
            .next()
            .map(|node| decode_entities(strip_tags(&node.html()).trim()).to_string())
            .unwrap_or_default();
        if !title.is_empty() {
            results.push(BaiduScholarResult {
                title,
                link,
                snippet,
                authors,
                year,
            });
        }
    }
    let mut seen = std::collections::HashSet::new();
    results
        .into_iter()
        .filter(|result| seen.insert(result.title.clone()))
        .collect()
}

#[async_trait]
impl BaiduScholarProvider for BaiduScholarClient {
    async fn search(&self, request: &BaiduScholarRequest) -> Result<Vec<BaiduScholarResult>> {
        if request.query.trim().is_empty() {
            bail!("baidu_scholar: query is required");
        }
        let url = self
            .endpoint
            .clone()
            .query_pairs_mut()
            .extend_pairs([("wd", request.query.as_str()), ("rsv_bp", "0")])
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
            .context("Baidu Scholar request failed")?;
        let status = response.status();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("could not read Baidu Scholar response body")?;
            let remaining = MAX_BAIDU_SCHOLAR_RESPONSE_BODY.saturating_sub(body.len());
            if chunk.len() > remaining {
                bail!(
                    "Baidu Scholar response exceeds {} bytes",
                    MAX_BAIDU_SCHOLAR_RESPONSE_BODY
                );
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            bail!("baidu_scholar: upstream returned {}", status.as_u16());
        }
        let html = String::from_utf8_lossy(&body);
        if html.contains("百度安全验证") || html.contains("wappass") {
            bail!(
                "baidu_scholar: safety verification page returned; retry later or use another provider"
            );
        }
        let top_n = request.top_n.clamp(1, 20);
        Ok(parse_baidu_scholar_results(&html)
            .into_iter()
            .take(top_n)
            .collect())
    }
}

pub fn baidu_scholar_results_to_tool_rows(results: &[BaiduScholarResult]) -> Vec<Value> {
    results
        .iter()
        .map(|result| {
            let mut row = Map::new();
            row.insert("title".into(), Value::String(result.title.clone()));
            row.insert("link".into(), Value::String(result.link.clone()));
            row.insert("snippet".into(), Value::String(result.snippet.clone()));
            if !result.authors.is_empty() {
                row.insert("authors".into(), Value::String(result.authors.clone()));
            }
            if !result.year.is_empty() {
                row.insert("year".into(), Value::String(result.year.clone()));
            }
            Value::Object(row)
        })
        .collect()
}


