//! Fixed-endpoint Google search protocols used by RAGFlow Agent tools.
//!
//! Canvas follows `agent/tools/google.py` and therefore calls SerpApi.  The
//! separate Programmable Search method records `internal/agent/tool/google.go`
//! without changing the Python Canvas contract or accepting arbitrary URLs.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SERPAPI_ENDPOINT: &str = "https://serpapi.com/search";
const PROGRAMMABLE_SEARCH_ENDPOINT: &str = "https://www.googleapis.com/customsearch/v1";
const MAX_GOOGLE_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleSearchRequest {
    pub query: String,
    pub country: String,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleProgrammableSearchRequest {
    pub api_key: String,
    pub cx: String,
    pub query: String,
    pub max_results: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleProgrammableSearchResult {
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub link: String,
    #[serde(default, deserialize_with = "deserialize_string_or_default")]
    pub snippet: String,
}

fn deserialize_string_or_default<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

#[async_trait]
pub trait GoogleProvider: Send + Sync {
    async fn search(&self, api_key: &str, request: &GoogleSearchRequest) -> Result<Vec<Value>>;
}

#[derive(Debug, Clone)]
pub struct GoogleClient {
    client: reqwest::Client,
    serpapi_endpoint: reqwest::Url,
    programmable_endpoint: reqwest::Url,
}

impl Default for GoogleClient {
    fn default() -> Self {
        Self::new_with_endpoints(SERPAPI_ENDPOINT, PROGRAMMABLE_SEARCH_ENDPOINT)
            .expect("fixed Google endpoints and HTTP configuration are valid")
    }
}

impl GoogleClient {
    pub(crate) fn new_with_endpoints(serpapi: &str, programmable: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .context("could not build Google HTTP client")?;
        Ok(Self {
            client,
            serpapi_endpoint: serpapi.parse().context("invalid SerpApi endpoint")?,
            programmable_endpoint: programmable
                .parse()
                .context("invalid Google Programmable Search endpoint")?,
        })
    }

    /// Execute the distinct Go CSE contract and return its stable result rows.
    pub async fn programmable_search(
        &self,
        request: &GoogleProgrammableSearchRequest,
    ) -> Result<Vec<GoogleProgrammableSearchResult>> {
        if request.query.is_empty() {
            bail!("google: query is required");
        }
        if request.api_key.is_empty() || request.cx.is_empty() {
            bail!("google: api_key and cx are required");
        }
        let num = if request.max_results <= 0 {
            5
        } else {
            request.max_results.min(10)
        };
        let response = self
            .client
            .get(self.programmable_endpoint.clone())
            .query(&[
                ("key", request.api_key.as_str()),
                ("cx", request.cx.as_str()),
                ("q", request.query.as_str()),
                ("num", &num.to_string()),
            ])
            .send()
            .await
            .context("Google Programmable Search request failed")?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        if !status.is_success() {
            bail!("google: upstream returned {}", status.as_u16());
        }
        #[derive(Deserialize)]
        struct Envelope {
            #[serde(default, deserialize_with = "deserialize_vec_or_default")]
            items: Vec<GoogleProgrammableSearchResult>,
        }
        let envelope: Envelope = serde_json::from_slice(&body)
            .context("google: decode response: response was not valid JSON")?;
        Ok(envelope.items)
    }
}

fn deserialize_vec_or_default<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

#[async_trait]
impl GoogleProvider for GoogleClient {
    async fn search(&self, api_key: &str, request: &GoogleSearchRequest) -> Result<Vec<Value>> {
        if api_key.is_empty() {
            bail!("SerpApi API key is required");
        }
        let response = self
            .client
            .get(self.serpapi_endpoint.clone())
            .query(&[
                ("api_key", api_key),
                ("engine", "google"),
                ("q", request.query.as_str()),
                ("google_domain", "google.com"),
                ("gl", request.country.as_str()),
                ("hl", request.language.as_str()),
                // google-search-results 2.4.2 mutates these into the request.
                ("source", "python"),
                ("output", "json"),
            ])
            .send()
            .await
            .context("SerpApi Google request failed")?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        let envelope: Value = serde_json::from_slice(&body).with_context(|| {
            format!("SerpApi Google response (HTTP {status}) was not valid JSON")
        })?;
        envelope
            .get("organic_results")
            .and_then(Value::as_array)
            .cloned()
            .ok_or_else(|| {
                let detail = envelope
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("organic_results must be an array");
                anyhow!("SerpApi Google response (HTTP {status}): {detail}")
            })
    }
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("could not read Google response body")?;
        let remaining = MAX_GOOGLE_RESPONSE_BODY.saturating_sub(body.len());
        if chunk.len() > remaining {
            bail!(
                "Google response body exceeds {} bytes",
                MAX_GOOGLE_RESPONSE_BODY
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}


