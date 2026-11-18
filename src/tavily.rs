//! Tavily Search and Extract HTTP protocol used by Agent Canvas tools.
//!
//! The production endpoints are fixed deliberately: Canvas data can choose
//! search/extraction arguments, but cannot turn this client into an arbitrary
//! outbound HTTP primitive. Tests inject loopback endpoints through the
//! crate-private constructor.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::Serialize;
use serde_json::Value;

const TAVILY_SEARCH_ENDPOINT: &str = "https://api.tavily.com/search";
const TAVILY_EXTRACT_ENDPOINT: &str = "https://api.tavily.com/extract";
const MAX_TAVILY_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TavilySearchRequest {
    pub query: String,
    pub search_depth: String,
    pub topic: String,
    pub max_results: usize,
    pub days: usize,
    pub include_answer: bool,
    pub include_raw_content: bool,
    pub include_images: bool,
    pub include_image_descriptions: bool,
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TavilyExtractRequest {
    pub urls: Vec<String>,
    pub extract_depth: String,
    pub format: String,
    pub include_images: bool,
}

#[async_trait]
pub trait TavilyProvider: Send + Sync {
    async fn search(&self, api_key: &str, request: &TavilySearchRequest) -> Result<Vec<Value>>;
    async fn extract(&self, api_key: &str, request: &TavilyExtractRequest) -> Result<Vec<Value>>;
}

#[derive(Debug, Clone)]
pub struct TavilyClient {
    client: reqwest::Client,
    search_endpoint: reqwest::Url,
    extract_endpoint: reqwest::Url,
}

impl Default for TavilyClient {
    fn default() -> Self {
        Self::new_with_endpoints(TAVILY_SEARCH_ENDPOINT, TAVILY_EXTRACT_ENDPOINT)
            .expect("fixed Tavily endpoints and HTTP client configuration are valid")
    }
}

impl TavilyClient {
    pub(crate) fn new_with_endpoints(
        search_endpoint: &str,
        extract_endpoint: &str,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .context("could not build Tavily HTTP client")?;
        Ok(Self {
            client,
            search_endpoint: search_endpoint
                .parse()
                .context("invalid Tavily Search endpoint")?,
            extract_endpoint: extract_endpoint
                .parse()
                .context("invalid Tavily Extract endpoint")?,
        })
    }

    async fn post_results<T: Serialize + Sync>(
        &self,
        endpoint: reqwest::Url,
        api_key: &str,
        request: &T,
    ) -> Result<Vec<Value>> {
        if api_key.trim().is_empty() {
            bail!("Tavily api_key is required");
        }
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(api_key)
            .json(request)
            .send()
            .await
            .context("Tavily request failed")?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&body);
            bail!("Tavily API returned HTTP {status}: {detail}");
        }
        let envelope: Value =
            serde_json::from_slice(&body).context("Tavily API response was not valid JSON")?;
        envelope
            .get("results")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| anyhow!("Tavily API response results must be an array"))
    }
}

#[async_trait]
impl TavilyProvider for TavilyClient {
    async fn search(&self, api_key: &str, request: &TavilySearchRequest) -> Result<Vec<Value>> {
        self.post_results(self.search_endpoint.clone(), api_key, request)
            .await
    }

    async fn extract(&self, api_key: &str, request: &TavilyExtractRequest) -> Result<Vec<Value>> {
        self.post_results(self.extract_endpoint.clone(), api_key, request)
            .await
    }
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("could not read Tavily response body")?;
        let remaining = MAX_TAVILY_RESPONSE_BODY.saturating_sub(body.len());
        if chunk.len() > remaining {
            bail!(
                "Tavily response body exceeds {} bytes",
                MAX_TAVILY_RESPONSE_BODY
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}


