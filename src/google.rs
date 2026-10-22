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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, http::Uri, routing::get};
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Recorded(Arc<Mutex<Vec<Uri>>>);

    async fn serpapi(State(recorded): State<Recorded>, uri: Uri) -> Json<Value> {
        recorded.0.lock().unwrap().push(uri);
        Json(json!({
            "organic_results": [{
                "title": "RAGFlow",
                "link": "https://ragflow.io",
                "snippet": "Open source RAG engine",
                "extra": "preserved"
            }]
        }))
    }

    async fn programmable(State(recorded): State<Recorded>, uri: Uri) -> Json<Value> {
        recorded.0.lock().unwrap().push(uri);
        Json(json!({
            "items": [
                {"title": "RAGFlow", "link": "https://ragflow.io", "snippet": "RAG engine"},
                {"title": "Missing scalar fields"},
                {"title": null, "link": null, "snippet": null}
            ]
        }))
    }

    async fn mock_server(recorded: Recorded) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/search", get(serpapi))
            .route("/customsearch/v1", get(programmable))
            .with_state(recorded);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), handle)
    }

    fn query_params(uri: &Uri) -> std::collections::HashMap<String, String> {
        reqwest::Url::parse(&format!("http://localhost{uri}"))
            .unwrap()
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    #[tokio::test]
    async fn serpapi_matches_pinned_sdk_query_and_preserves_organic_rows() {
        let recorded = Recorded::default();
        let (base, server) = mock_server(recorded.clone()).await;
        let client = GoogleClient::new_with_endpoints(
            &format!("{base}/search"),
            &format!("{base}/customsearch/v1"),
        )
        .unwrap();
        let results = client
            .search(
                "secret",
                &GoogleSearchRequest {
                    query: "rust rag".into(),
                    country: "us".into(),
                    language: "en".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(results[0]["extra"], "preserved");
        let calls = recorded.0.lock().unwrap();
        let params = query_params(&calls[0]);
        assert_eq!(params["api_key"], "secret");
        assert_eq!(params["engine"], "google");
        assert_eq!(params["q"], "rust rag");
        assert_eq!(params["google_domain"], "google.com");
        assert_eq!(params["gl"], "us");
        assert_eq!(params["hl"], "en");
        assert_eq!(params["source"], "python");
        assert_eq!(params["output"], "json");
        assert!(!params.contains_key("start"));
        assert!(!params.contains_key("num"));
        server.abort();
    }

    #[tokio::test]
    async fn programmable_search_defaults_clamps_and_decodes_go_shape() {
        let recorded = Recorded::default();
        let (base, server) = mock_server(recorded.clone()).await;
        let client = GoogleClient::new_with_endpoints(
            &format!("{base}/search"),
            &format!("{base}/customsearch/v1"),
        )
        .unwrap();
        for (max_results, want_num) in [(0, "5"), (50, "10"), (3, "3")] {
            let results = client
                .programmable_search(&GoogleProgrammableSearchRequest {
                    api_key: "KEY".into(),
                    cx: "CXID".into(),
                    query: "x y".into(),
                    max_results,
                })
                .await
                .unwrap();
            assert_eq!(results.len(), 3);
            assert_eq!(results[0].title, "RAGFlow");
            assert_eq!(results[1].link, "");
            assert_eq!(results[2].title, "");
            let calls = recorded.0.lock().unwrap();
            let params = query_params(calls.last().unwrap());
            assert_eq!(params["key"], "KEY");
            assert_eq!(params["cx"], "CXID");
            assert_eq!(params["q"], "x y");
            assert_eq!(params["num"], want_num);
        }
        server.abort();
    }

    #[tokio::test]
    async fn programmable_search_requires_query_key_and_cx() {
        let client = GoogleClient::default();
        let mut request = GoogleProgrammableSearchRequest {
            api_key: String::new(),
            cx: String::new(),
            query: "x".into(),
            max_results: 5,
        };
        let error = client.programmable_search(&request).await.unwrap_err();
        assert!(error.to_string().contains("api_key and cx"));
        request.api_key = "K".into();
        request.cx = "C".into();
        request.query.clear();
        let error = client.programmable_search(&request).await.unwrap_err();
        assert!(error.to_string().contains("query is required"));
    }

    #[tokio::test]
    #[ignore = "requires a live SerpApi account and SERPAPI_API_KEY"]
    async fn live_google_search() {
        let api_key = std::env::var("SERPAPI_API_KEY")
            .expect("SERPAPI_API_KEY must be set for the live Google test");
        let results = GoogleClient::default()
            .search(
                &api_key,
                &GoogleSearchRequest {
                    query: "RAGFlow".into(),
                    country: "us".into(),
                    language: "en".into(),
                },
            )
            .await
            .unwrap();
        assert!(!results.is_empty());
    }
}
