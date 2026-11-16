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

    #[cfg(test)]
    pub(crate) fn new_with_endpoint(endpoint: &str) -> Result<Self> {
        Self::new(endpoint)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    type CapturedRequest = (HashMap<String, String>, HeaderMap);

    #[derive(Debug, Clone, Default)]
    struct Recorded {
        requests: Arc<Mutex<Vec<CapturedRequest>>>,
    }

    async fn handler(
        State(recorded): State<Recorded>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        recorded
            .requests
            .lock()
            .unwrap()
            .push((query.clone(), headers));
        match query.get("q").map(String::as_str) {
            Some("missing") => (
                StatusCode::FORBIDDEN,
                Json(json!({"message": "rate limited"})),
            ),
            Some("bad-status") => (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"items": []}))),
            _ => (
                StatusCode::OK,
                Json(json!({
                    "total_count": 2,
                    "items": [
                        {
                            "name": "ragflow",
                            "full_name": "infiniflow/ragflow",
                            "html_url": "https://github.com/infiniflow/ragflow",
                            "description": "RAG engine",
                            "watchers": 12000,
                            "stargazers_count": 12000,
                            "license": {"spdx_id": "Apache-2.0"}
                        },
                        {
                            "name": "empty-description",
                            "full_name": "example/empty-description",
                            "html_url": "https://github.com/example/empty-description",
                            "description": null,
                            "watchers": 5,
                            "stargazers_count": 5
                        }
                    ]
                })),
            ),
        }
    }

    async fn server() -> (GitHubClient, Recorded, tokio::task::JoinHandle<()>) {
        let recorded = Recorded::default();
        let app = Router::new()
            .route("/search/repositories", get(handler))
            .with_state(recorded.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            GitHubClient::new_with_endpoint(&format!("http://{address}/search/repositories"))
                .unwrap(),
            recorded,
            handle,
        )
    }

    #[tokio::test]
    async fn python_search_preserves_headers_fields_raw_items_and_none_formatting() {
        let (client, recorded, handle) = server().await;
        let rows = client
            .search(&GitHubSearchRequest {
                query: "language:rust stars:>100".into(),
                top_n: 10,
            })
            .await
            .unwrap();
        handle.abort();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "ragflow");
        assert_eq!(rows[0].formatted_content(), "RAG engine\n stars:12000");
        assert_eq!(rows[0].raw["license"]["spdx_id"], "Apache-2.0");
        assert_eq!(rows[1].formatted_content(), "None\n stars:5");
        let requests = recorded.requests.lock().unwrap();
        let (query, headers) = &requests[0];
        assert_eq!(query["q"], "language:rust stars:>100");
        assert_eq!(query["sort"], "stars");
        assert_eq!(query["order"], "desc");
        assert_eq!(query["per_page"], "10");
        assert_eq!(headers[CONTENT_TYPE], GITHUB_MEDIA_TYPE);
        assert_eq!(headers[ACCEPT], "*/*");
        assert_eq!(headers["x-github-api-version"], GITHUB_API_VERSION);
        assert_eq!(headers[USER_AGENT], REQUESTS_USER_AGENT);
    }

    #[tokio::test]
    async fn go_search_clamps_limit_and_sends_optional_token() {
        let (client, recorded, handle) = server().await;
        let envelope = client.search_go("ragflow", 99, "ghp_test").await.unwrap();
        let defaults = client.search_go("default", -1, "").await.unwrap();

        assert_eq!(envelope.results.len(), 2);
        assert_eq!(envelope.results[0].full_name, "infiniflow/ragflow");
        assert_eq!(envelope.results[0].stargazers_count, 12000);
        assert_eq!(envelope.results[1].description, "");
        let requests = recorded.requests.lock().unwrap();
        let (query, headers) = &requests[0];
        assert_eq!(query["q"], "ragflow");
        assert_eq!(query["per_page"], "30");
        assert_eq!(headers[ACCEPT], GITHUB_MEDIA_TYPE);
        assert_eq!(headers[USER_AGENT], GITHUB_GO_USER_AGENT);
        assert_eq!(headers[AUTHORIZATION], "Bearer ghp_test");
        assert_eq!(defaults.results.len(), 2);
        let (default_query, default_headers) = &requests[1];
        assert_eq!(default_query["per_page"], "5");
        assert!(default_headers.get(AUTHORIZATION).is_none());
        handle.abort();
    }

    #[tokio::test]
    async fn python_missing_items_and_go_non_success_fail_closed() {
        let (client, _recorded, handle) = server().await;
        let python_error = client
            .search(&GitHubSearchRequest {
                query: "missing".into(),
                top_n: 5,
            })
            .await
            .unwrap_err();
        assert!(python_error.to_string().contains("missing items"));
        let go_error = client.search_go("bad-status", 0, "").await.unwrap_err();
        assert!(go_error.to_string().contains("upstream returned 503"));
        let query_error = client.search_go("  ", 5, "").await.unwrap_err();
        assert!(query_error.to_string().contains("query is required"));
        handle.abort();
    }

    #[test]
    fn python_scalar_rendering_matches_common_json_values() {
        assert_eq!(python_scalar_string(&Value::Null), "None");
        assert_eq!(python_scalar_string(&Value::Bool(true)), "True");
        assert_eq!(python_scalar_string(&json!(12)), "12");
        assert_eq!(python_scalar_string(&json!("repo")), "repo");

        let decoded: GitHubGoResponse = serde_json::from_value(json!({
            "items": [{"description": null, "stargazers_count": null}]
        }))
        .unwrap();
        assert_eq!(decoded.items[0].full_name, "");
        assert_eq!(decoded.items[0].html_url, "");
        assert_eq!(decoded.items[0].description, "");
        assert_eq!(decoded.items[0].stargazers_count, 0);
    }

    #[tokio::test]
    #[ignore = "requires outbound access to api.github.com"]
    async fn live_github_repository_search() {
        let rows = GitHubClient::default()
            .search(&GitHubSearchRequest {
                query: "infiniflow ragflow".into(),
                top_n: 1,
            })
            .await
            .unwrap();
        assert!(!rows.is_empty());
        assert!(!rows[0].html_url.is_empty());
    }
}
