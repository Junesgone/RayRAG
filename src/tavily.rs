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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
    use serde_json::{Map, json};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Recorded(Arc<Mutex<Vec<(String, HeaderMap, Value)>>>);

    async fn record(
        State(recorded): State<Recorded>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let path = if body.get("query").is_some() {
            "search"
        } else {
            "extract"
        };
        recorded
            .0
            .lock()
            .unwrap()
            .push((path.into(), headers, body));
        Json(
            json!({"results": [{"url": "https://example.test", "title": "A", "content": "alpha", "score": 0.9}]}),
        )
    }

    async fn mock_server(recorded: Recorded) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/search", post(record))
            .route("/extract", post(record))
            .with_state(recorded);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), handle)
    }

    #[tokio::test]
    async fn search_and_extract_use_bearer_json_and_preserve_results() {
        let recorded = Recorded::default();
        let (base, server) = mock_server(recorded.clone()).await;
        let client =
            TavilyClient::new_with_endpoints(&format!("{base}/search"), &format!("{base}/extract"))
                .unwrap();
        let search = TavilySearchRequest {
            query: "rust rag".into(),
            search_depth: "advanced".into(),
            topic: "news".into(),
            max_results: 3,
            days: 7,
            include_answer: true,
            include_raw_content: false,
            include_images: false,
            include_image_descriptions: true,
            include_domains: vec!["rust-lang.org".into()],
            exclude_domains: Vec::new(),
        };
        let results = client.search("secret", &search).await.unwrap();
        assert_eq!(results[0]["content"], "alpha");
        let extract = TavilyExtractRequest {
            urls: vec!["https://example.test".into()],
            extract_depth: "basic".into(),
            format: "markdown".into(),
            include_images: false,
        };
        client.extract("secret", &extract).await.unwrap();

        let calls = recorded.0.lock().unwrap();
        assert_eq!(calls.len(), 2);
        for (_, headers, _) in calls.iter() {
            assert_eq!(headers["authorization"], "Bearer secret");
            assert!(
                headers["content-type"]
                    .to_str()
                    .unwrap()
                    .starts_with("application/json")
            );
        }
        assert_eq!(calls[0].0, "search");
        assert_eq!(calls[0].2["query"], "rust rag");
        assert_eq!(calls[0].2["max_results"], 3);
        assert_eq!(calls[1].0, "extract");
        assert_eq!(calls[1].2["urls"], json!(["https://example.test"]));
        server.abort();
    }

    #[tokio::test]
    async fn response_shape_status_and_missing_key_fail_closed() {
        async fn bad(State(response): State<Value>) -> (axum::http::StatusCode, Json<Value>) {
            let status = response
                .get("status")
                .and_then(Value::as_u64)
                .and_then(|status| axum::http::StatusCode::from_u16(status as u16).ok())
                .unwrap_or(axum::http::StatusCode::OK);
            (status, Json(response))
        }
        async fn serve(response: Value) -> (String, tokio::task::JoinHandle<()>) {
            let app = Router::new().route("/", post(bad)).with_state(response);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            (format!("http://{address}/"), handle)
        }
        let request = TavilySearchRequest {
            query: "x".into(),
            search_depth: "basic".into(),
            topic: "general".into(),
            max_results: 5,
            days: 14,
            include_answer: false,
            include_raw_content: false,
            include_images: false,
            include_image_descriptions: false,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
        };
        for (response, expected) in [
            (json!({"answer": "missing"}), "results must be an array"),
            (json!({"status": 401, "detail": "denied"}), "HTTP 401"),
        ] {
            let (endpoint, server) = serve(response).await;
            let client = TavilyClient::new_with_endpoints(&endpoint, &endpoint).unwrap();
            let error = client.search("k", &request).await.unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
            server.abort();
        }
        let (endpoint, server) = serve(json!({"results": []})).await;
        let client = TavilyClient::new_with_endpoints(&endpoint, &endpoint).unwrap();
        assert!(
            client
                .search("", &request)
                .await
                .unwrap_err()
                .to_string()
                .contains("api_key")
        );
        server.abort();
    }

    #[test]
    fn request_debug_does_not_contain_credentials() {
        let request = TavilySearchRequest {
            query: "x".into(),
            search_depth: "basic".into(),
            topic: "general".into(),
            max_results: 5,
            days: 14,
            include_answer: false,
            include_raw_content: false,
            include_images: false,
            include_image_descriptions: false,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
        };
        let object: Map<String, Value> = serde_json::to_value(request)
            .unwrap()
            .as_object()
            .unwrap()
            .clone();
        assert!(!object.contains_key("api_key"));
    }
}
