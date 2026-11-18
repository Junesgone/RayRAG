//! MediaWiki protocol for RAGFlow-compatible Wikipedia Canvas tools.
//!
//! Production hosts are derived only from the fixed frontend/Python language
//! allowlist. Tests may inject a loopback endpoint, but Canvas data cannot
//! configure an arbitrary outbound URL.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_WIKIPEDIA_RESPONSE_BODY: usize = 16 << 20;
const WIKIPEDIA_USER_AGENT: &str = concat!(
    "RayRAG/",
    env!("CARGO_PKG_VERSION"),
    " (RAGFlow-compatible Wikipedia tool)"
);

pub const WIKIPEDIA_LANGUAGES: &[&str] = &[
    "af", "pl", "ar", "ast", "az", "bg", "nan", "bn", "be", "ca", "cs", "cy", "da", "de", "et",
    "el", "en", "es", "eo", "eu", "fa", "fr", "gl", "ko", "hy", "hi", "hr", "id", "it", "he", "ka",
    "lld", "la", "lv", "lt", "hu", "mk", "arz", "ms", "min", "my", "nl", "ja", "nb", "nn", "ce",
    "uz", "pt", "kk", "ro", "ru", "ceb", "sk", "sl", "sr", "sh", "fi", "sv", "ta", "tt", "th",
    "tg", "azb", "tr", "uk", "ur", "vi", "war", "zh", "yue",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikipediaSearchRequest {
    pub query: String,
    pub language: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WikipediaArticle {
    pub title: String,
    pub url: String,
    pub summary: String,
    pub snippet: String,
}

#[async_trait]
pub trait WikipediaProvider: Send + Sync {
    async fn search(&self, request: &WikipediaSearchRequest) -> Result<Vec<WikipediaArticle>>;
}

#[derive(Debug, Clone)]
pub struct WikipediaClient {
    client: reqwest::Client,
    endpoint_override: Option<reqwest::Url>,
}

impl Default for WikipediaClient {
    fn default() -> Self {
        Self::new(None).expect("fixed Wikipedia HTTP client configuration is valid")
    }
}

impl WikipediaClient {
    fn new(endpoint_override: Option<reqwest::Url>) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(RedirectPolicy::none())
                .timeout(crate::common::cmd_timeout::duration())
                .user_agent(WIKIPEDIA_USER_AGENT)
                .build()
                .context("could not build Wikipedia HTTP client")?,
            endpoint_override,
        })
    }

    

    fn endpoint(&self, language: &str) -> Result<reqwest::Url> {
        if let Some(endpoint) = &self.endpoint_override {
            return Ok(endpoint.clone());
        }
        wikipedia_api_url(language)
    }

    async fn request_json(&self, url: reqwest::Url) -> Result<Value> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Wikipedia request failed")?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&body);
            bail!("Wikipedia API returned HTTP {status}: {detail}");
        }
        let envelope: Value =
            serde_json::from_slice(&body).context("Wikipedia API response was not valid JSON")?;
        if let Some(info) = envelope
            .get("error")
            .and_then(|error| error.get("info"))
            .and_then(Value::as_str)
        {
            bail!("Wikipedia API error: {info}");
        }
        Ok(envelope)
    }

    async fn search_titles(
        &self,
        request: &WikipediaSearchRequest,
    ) -> Result<Vec<(String, String)>> {
        let mut url = self.endpoint(&request.language)?;
        url.query_pairs_mut()
            .append_pair("action", "query")
            .append_pair("list", "search")
            .append_pair("srprop", "")
            .append_pair("srlimit", &request.top_n.to_string())
            .append_pair("limit", &request.top_n.to_string())
            .append_pair("srsearch", &request.query)
            .append_pair("format", "json");
        let envelope = self.request_json(url).await?;
        let rows = envelope
            .get("query")
            .and_then(|query| query.get("search"))
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Wikipedia search response query.search must be an array"))?;
        rows.iter()
            .map(|row| {
                let title = row
                    .get("title")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("Wikipedia search result title must be a string"))?;
                let snippet = row
                    .get("snippet")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Ok((title.to_owned(), snippet.to_owned()))
            })
            .collect()
    }

    async fn fetch_article(
        &self,
        language: &str,
        title: &str,
        snippet: &str,
    ) -> Result<Option<WikipediaArticle>> {
        let mut url = self.endpoint(language)?;
        url.query_pairs_mut()
            .append_pair("action", "query")
            .append_pair("prop", "info|pageprops|extracts")
            .append_pair("inprop", "url")
            .append_pair("ppprop", "disambiguation")
            .append_pair("redirects", "")
            .append_pair("explaintext", "")
            .append_pair("exintro", "")
            .append_pair("titles", title)
            .append_pair("format", "json");
        let envelope = self.request_json(url).await?;
        let pages = envelope
            .get("query")
            .and_then(|query| query.get("pages"))
            .and_then(Value::as_object)
            .ok_or_else(|| anyhow!("Wikipedia page response query.pages must be an object"))?;
        let Some(page) = pages.values().next().and_then(Value::as_object) else {
            return Ok(None);
        };
        if page.contains_key("missing") || page.contains_key("pageprops") {
            return Ok(None);
        }
        let title = page
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Wikipedia page title must be a string"))?;
        let url = page
            .get("fullurl")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Wikipedia page fullurl must be a string"))?;
        let summary = page
            .get("extract")
            .and_then(Value::as_str)
            .unwrap_or_default();
        Ok(Some(WikipediaArticle {
            title: title.to_owned(),
            url: url.to_owned(),
            summary: summary.to_owned(),
            snippet: snippet.to_owned(),
        }))
    }
}

#[async_trait]
impl WikipediaProvider for WikipediaClient {
    async fn search(&self, request: &WikipediaSearchRequest) -> Result<Vec<WikipediaArticle>> {
        if !WIKIPEDIA_LANGUAGES.contains(&request.language.as_str()) {
            bail!("Wikipedia language '{}' is not supported", request.language);
        }
        if request.query.is_empty() {
            bail!("Wikipedia query is required");
        }
        if request.top_n == 0 {
            bail!("Wikipedia top_n must be a positive integer");
        }
        let titles = self.search_titles(request).await?;
        let mut articles = Vec::with_capacity(titles.len());
        for (title, snippet) in titles {
            match self
                .fetch_article(&request.language, &title, &snippet)
                .await
            {
                Ok(Some(article)) => articles.push(article),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(title = %title, error = %error, "Wikipedia page was skipped");
                }
            }
        }
        Ok(articles)
    }
}

fn wikipedia_api_url(language: &str) -> Result<reqwest::Url> {
    if !WIKIPEDIA_LANGUAGES.contains(&language) {
        bail!("Wikipedia language '{language}' is not supported");
    }
    format!("https://{language}.wikipedia.org/w/api.php")
        .parse()
        .context("could not construct Wikipedia API URL")
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("could not read Wikipedia response body")?;
        let remaining = MAX_WIKIPEDIA_RESPONSE_BODY.saturating_sub(body.len());
        if chunk.len() > remaining {
            bail!(
                "Wikipedia response body exceeds {} bytes",
                MAX_WIKIPEDIA_RESPONSE_BODY
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}


