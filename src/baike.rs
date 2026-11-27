//! Baidu Baike (百度百科) connector — mainland-China native replacement for
//! the Wikipedia Canvas tool. No API key required; queries the public
//! BaikeLemmaCardApi JSON endpoint and returns RAGFlow-shaped rows.
//!
//! RAGFlow ships `wikipedia.py`; deployments inside mainland China often
//! cannot reach `zh.wikipedia.org` reliably, so RayRAG offers this tool as a
//! domestic drop-in with the same input contract (query + top_n).

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BAIKE_ENDPOINT: &str = "https://baike.baidu.com/api/openapi/BaikeLemmaCardApi";
const BAIKE_APP_ID: &str = "379020";
const MAX_BAIKE_RESPONSE_BODY: usize = 4 << 20;
const BAIKE_USER_AGENT: &str = concat!(
    "RayRAG/",
    env!("CARGO_PKG_VERSION"),
    " (Baidu Baike connector)"
);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaikeSearchRequest {
    pub query: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaikeArticle {
    pub title: String,
    pub url: String,
    pub summary: String,
    pub snippet: String,
}

#[async_trait]
pub trait BaikeProvider: Send + Sync {
    async fn search(&self, request: &BaikeSearchRequest) -> Result<Vec<BaikeArticle>>;
}

#[derive(Debug, Clone)]
pub struct BaikeClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl Default for BaikeClient {
    fn default() -> Self {
        Self::new_with_endpoint(BAIKE_ENDPOINT).expect("fixed Baike endpoint is valid")
    }
}

impl BaikeClient {
    pub(crate) fn new_with_endpoint(endpoint: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::limited(3))
            .timeout(crate::common::cmd_timeout::duration())
            .user_agent(BAIKE_USER_AGENT)
            .build()
            .context("build Baike HTTP client")?;
        let endpoint = reqwest::Url::parse(endpoint).context("parse Baike endpoint")?;
        Ok(Self { client, endpoint })
    }

    /// Query the Baike lemma card API for a single keyword. The upstream API
    /// resolves redirects (disambiguation) via the `lemmaId` field and returns
    /// an HTML description; we normalize it to plain text snippets.
    async fn fetch_lemma(&self, query: &str) -> Result<Option<BaikeArticle>> {
        let mut endpoint = self.endpoint.clone();
        let url = endpoint
            .query_pairs_mut()
            .append_pair("scope", "103")
            .append_pair("format", "json")
            .append_pair("appid", BAIKE_APP_ID)
            .append_pair("bk_key", query)
            .append_pair("bk_length", "600")
            .finish();
        let mut response = self
            .client
            .get(url.clone())
            .send()
            .await
            .context("request Baike lemma card")?;
        let status = response.status();
        if !status.is_success() {
            if status == reqwest::StatusCode::NOT_FOUND {
                return Ok(None);
            }
            bail!("Baike lemma card HTTP {status}");
        }
        let mut body = String::new();
        while let Some(chunk) = response.chunk().await.context("read Baike response")? {
            body.push_str(std::str::from_utf8(&chunk).context("Baike response not UTF-8")?);
            if body.len() > MAX_BAIKE_RESPONSE_BODY {
                bail!("Baike response exceeded size limit");
            }
        }
        let value: Value = serde_json::from_str(&body).context("parse Baike JSON")?;
        let title = value.get("title").and_then(Value::as_str).unwrap_or(query);
        let url = value
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "https://baike.baidu.com/item/{}",
                    percent_encode_query(query)
                )
            });
        let description = value
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let snippet = strip_html(description);
        if snippet.is_empty() {
            return Ok(None);
        }
        Ok(Some(BaikeArticle {
            title: title.to_string(),
            url,
            summary: snippet.clone(),
            snippet,
        }))
    }
}

/// Strip the HTML tags Baike returns in `description`.
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Percent-encode a Baike lemma keyword for use in an item URL. Keeps ASCII
/// unreserved characters as-is (including `/` which Baike accepts unencoded
/// inside the item path) and UTF-8 encodes everything else.
fn percent_encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len() * 2);
    for byte in query.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[async_trait]
impl BaikeProvider for BaikeClient {
    async fn search(&self, request: &BaikeSearchRequest) -> Result<Vec<BaikeArticle>> {
        let Some(article) = self.fetch_lemma(request.query.trim()).await? else {
            return Ok(Vec::new());
        };
        Ok(std::iter::once(article)
            .take(request.top_n.max(1))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_html_from_baike_description() {
        assert_eq!(
            strip_html("<b>中山市</b>是中国广东省下辖的地级市。"),
            "中山市是中国广东省下辖的地级市。"
        );
        assert_eq!(strip_html("<p>a &amp; b</p>"), "a &amp; b");
        assert_eq!(strip_html("no tags"), "no tags");
    }

    #[tokio::test]
    async fn lemma_card_parses_zhang_san_example() {
        // Deterministic parse of a fixture-shaped payload without network.
        let json = r#"{"title":"中山市","url":"https://baike.baidu.com/item/%E4%B8%AD%E5%B1%B1%E5%B8%82","description":"<b>中山市</b>，古称香山，是广东省下辖的地级市。"}"#;
        let value: Value = serde_json::from_str(json).unwrap();
        let title = value.get("title").and_then(Value::as_str).unwrap();
        let description = value.get("description").and_then(Value::as_str).unwrap();
        assert_eq!(title, "中山市");
        assert_eq!(
            strip_html(description),
            "中山市，古称香山，是广东省下辖的地级市。"
        );
    }
}
