//! Fixed-domain GitHub repository-search protocol for RAGFlow-compatible tools.
//!
//! RAGFlow's Python Canvas component and independent Go Agent tool call the
//! same GitHub REST endpoint but expose different defaults, headers and JSON
//! results. [`GitHubProvider::search`] preserves the Python-facing raw `items`
//! rows used for references, while [`GitHubClient::search_go`] preserves the
//! Go model contract without allowing Canvas input to choose an outbound host.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderName, USER_AGENT};
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

const GITHUB_SEARCH_ENDPOINT: &str = "https://api.github.com/search/repositories";
const GITHUB_MEDIA_TYPE: &str = "application/vnd.github+json";
const GITHUB_API_VERSION: &str = "2022-11-28";
const REQUESTS_USER_AGENT: &str = "python-requests/2.32.5";
const GITHUB_GO_USER_AGENT: &str = "Go-http-client/1.1";
const MAX_GITHUB_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubSearchRequest {
    pub query: String,
    pub top_n: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GitHubRepository {
    pub name: String,
    pub full_name: String,
    pub html_url: String,
    pub description: Value,
    pub watchers: Value,
    pub stargazers_count: u64,
    pub raw: Value,
}

impl GitHubRepository {
    /// Match `str(description) + "\n stars:" + str(watchers)` in github.py.
    pub fn formatted_content(&self) -> String {
        format!(
            "{}\n stars:{}",
            python_scalar_string(&self.description),
            python_scalar_string(&self.watchers)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubGoResult {
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub full_name: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub html_url: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub description: String,
    #[serde(default, deserialize_with = "deserialize_nullable_i64")]
    pub stargazers_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitHubGoEnvelope {
    pub results: Vec<GitHubGoResult>,
}

#[async_trait]
pub trait GitHubProvider: Send + Sync {
    async fn search(&self, request: &GitHubSearchRequest) -> Result<Vec<GitHubRepository>>;
}

#[derive(Debug, Clone)]
pub struct GitHubClient {
    client: reqwest::Client,
    endpoint: reqwest::Url,
}

impl Default for GitHubClient {
    fn default() -> Self {
        Self::new(GITHUB_SEARCH_ENDPOINT)
            .expect("fixed GitHub endpoint and HTTP client configuration are valid")
    }
}

impl GitHubClient {
    fn new(endpoint: &str) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(RedirectPolicy::none())
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .context("could not build GitHub HTTP client")?,
            endpoint: endpoint.parse().context("invalid GitHub search endpoint")?,
        })
    }

    

    async fn send_bounded(&self, request: reqwest::RequestBuilder) -> Result<(u16, Vec<u8>)> {
        let response = request.send().await.context("GitHub request failed")?;
        let status = response.status().as_u16();
        let body = read_bounded_body(response).await?;
        Ok((status, body))
    }

    /// Execute the independent Go tool's repository-search contract.
    pub async fn search_go(
        &self,
        query: &str,
        max_results: i64,
        token: &str,
    ) -> Result<GitHubGoEnvelope> {
        if query.trim().is_empty() {
            bail!("github: query is required");
        }
        let mut url = self.endpoint.clone();
        url.set_query(None);
        let max_results = if max_results <= 0 {
            5
        } else {
            max_results.min(30)
        };
        url.query_pairs_mut()
            .append_pair("per_page", &max_results.to_string())
            .append_pair("q", query);
        let mut request = self
            .client
            .get(url)
            .header(ACCEPT, GITHUB_MEDIA_TYPE)
            // net/http supplies this implicit default for github.go.
            .header(USER_AGENT, GITHUB_GO_USER_AGENT);
        if !token.is_empty() {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let (status, body) = self.send_bounded(request).await?;
        if !(200..300).contains(&status) {
            bail!("github: upstream returned {status}");
        }
        let response: GitHubGoResponse =
            serde_json::from_slice(&body).context("github: decode response")?;
        Ok(GitHubGoEnvelope {
            results: response.items,
        })
    }
}

#[async_trait]
impl GitHubProvider for GitHubClient {
    async fn search(&self, request: &GitHubSearchRequest) -> Result<Vec<GitHubRepository>> {
        if request.query.is_empty() {
            bail!("GitHub query is required");
        }
        if request.top_n == 0 {
            bail!("GitHub top_n must be a positive integer");
        }

        let mut url = self.endpoint.clone();
        url.set_query(None);
        url.query_pairs_mut()
            .append_pair("q", &request.query)
            .append_pair("sort", "stars")
            .append_pair("order", "desc")
            .append_pair("per_page", &request.top_n.to_string());
        let api_version = HeaderName::from_static("x-github-api-version");
        let request = self
            .client
            .get(url)
            .header(CONTENT_TYPE, GITHUB_MEDIA_TYPE)
            .header(ACCEPT, "*/*")
            .header(api_version, GITHUB_API_VERSION)
            // requests supplies this header even though github.py does not
            // spell it out; GitHub's REST API requires a non-empty User-Agent.
            .header(USER_AGENT, REQUESTS_USER_AGENT);
        let (_status, body) = self.send_bounded(request).await?;
        parse_python_response(&body)
    }
}

#[derive(Debug, Deserialize)]
struct GitHubGoResponse {
    #[serde(default)]
    items: Vec<GitHubGoResult>,
}

fn parse_python_response(body: &[u8]) -> Result<Vec<GitHubRepository>> {
    let response: Value = serde_json::from_slice(body).context("GitHub response is not JSON")?;
    let items = response
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("GitHub response is missing items"))?;
    items
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let item = raw
                .as_object()
                .ok_or_else(|| anyhow!("GitHub item {index} must be an object"))?;
            let string = |field: &str| -> Result<String> {
                item.get(field)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("GitHub item {index} requires string {field}"))
            };
            let name = string("name")?;
            let html_url = string("html_url")?;
            let description = item
                .get("description")
                .cloned()
                .ok_or_else(|| anyhow!("GitHub item {index} is missing description"))?;
            let watchers = item
                .get("watchers")
                .cloned()
                .ok_or_else(|| anyhow!("GitHub item {index} is missing watchers"))?;
            Ok(GitHubRepository {
                name,
                full_name: item
                    .get("full_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                html_url,
                description,
                watchers,
                stargazers_count: item
                    .get("stargazers_count")
                    .and_then(Value::as_u64)
                    .unwrap_or_default(),
                raw: raw.clone(),
            })
        })
        .collect()
}

fn python_scalar_string(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        // GitHub's description/watchers fields are scalar. Keeping JSON for an
        // unexpected compound value is deterministic while the row remains
        // visible instead of being silently discarded.
        value => value.to_string(),
    }
}

fn deserialize_nullable_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn deserialize_nullable_i64<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<i64>::deserialize(deserializer)?.unwrap_or_default())
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("could not read GitHub response body")?;
        if body.len().saturating_add(chunk.len()) > MAX_GITHUB_RESPONSE_BODY {
            bail!("GitHub response exceeds {} bytes", MAX_GITHUB_RESPONSE_BODY);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}


