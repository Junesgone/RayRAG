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

    #[cfg(test)]
    pub(crate) fn new_with_endpoint(endpoint: &str) -> Result<Self> {
        Self::new(Some(
            endpoint
                .parse()
                .context("invalid Wikipedia test endpoint")?,
        ))
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        extract::{Query, State},
        http::StatusCode,
        response::IntoResponse,
        routing::get,
    };
    use serde_json::{Map, json};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Recorded(Arc<Mutex<Vec<Map<String, Value>>>>);

    async fn wikipedia_mock(
        State(recorded): State<Recorded>,
        Query(query): Query<Map<String, Value>>,
    ) -> impl IntoResponse {
        recorded.0.lock().unwrap().push(query.clone());
        if query.get("list").and_then(Value::as_str) == Some("search") {
            return (
                StatusCode::OK,
                axum::Json(json!({"query": {"search": [
                    {"title": "Rust (programming language)", "snippet": "<span>Rust</span> language"},
                    {"title": "Rust disambiguation", "snippet": "many meanings"},
                    {"title": "Missing page", "snippet": "gone"}
                ]}})),
            );
        }
        let title = query
            .get("titles")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let page = match title {
            "Rust (programming language)" => json!({
                "123": {
                    "pageid": 123,
                    "title": "Rust (programming language)",
                    "fullurl": "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                    "extract": "Rust is a programming language."
                }
            }),
            "Rust disambiguation" => json!({
                "456": {"pageid": 456, "title": "Rust", "pageprops": {"disambiguation": ""}}
            }),
            _ => json!({"-1": {"title": "Missing page", "missing": ""}}),
        };
        (
            StatusCode::OK,
            axum::Json(json!({"query": {"pages": page}})),
        )
    }

    async fn mock_server(recorded: Recorded) -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route("/w/api.php", get(wikipedia_mock))
            .with_state(recorded);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}/w/api.php"), handle)
    }

    #[tokio::test]
    async fn search_builds_fixed_action_queries_and_skips_missing_or_disambiguation_pages() {
        let recorded = Recorded::default();
        let (endpoint, server) = mock_server(recorded.clone()).await;
        let client = WikipediaClient::new_with_endpoint(&endpoint).unwrap();
        let articles = client
            .search(&WikipediaSearchRequest {
                query: "rust language".into(),
                language: "en".into(),
                top_n: 3,
            })
            .await
            .unwrap();
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].title, "Rust (programming language)");
        assert_eq!(articles[0].summary, "Rust is a programming language.");
        assert_eq!(articles[0].snippet, "<span>Rust</span> language");

        let calls = recorded.0.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(calls[0]["action"], "query");
        assert_eq!(calls[0]["list"], "search");
        assert_eq!(calls[0]["srsearch"], "rust language");
        assert_eq!(calls[0]["srlimit"], "3");
        assert_eq!(calls[1]["prop"], "info|pageprops|extracts");
        assert_eq!(calls[1]["explaintext"], "");
        server.abort();
    }

    #[tokio::test]
    async fn status_shape_and_language_validation_fail_closed() {
        async fn bad(Query(query): Query<Map<String, Value>>) -> impl IntoResponse {
            if query.get("srsearch").and_then(Value::as_str) == Some("status") {
                return (
                    StatusCode::BAD_GATEWAY,
                    axum::Json(json!({"error": "down"})),
                );
            }
            (StatusCode::OK, axum::Json(json!({"query": {}})))
        }
        let app = Router::new().route("/", get(bad));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = WikipediaClient::new_with_endpoint(&format!("http://{address}/")).unwrap();
        for (query, expected) in [
            ("status", "HTTP 502"),
            ("shape", "query.search must be an array"),
        ] {
            let error = client
                .search(&WikipediaSearchRequest {
                    query: query.into(),
                    language: "en".into(),
                    top_n: 5,
                })
                .await
                .unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        let error = client
            .search(&WikipediaSearchRequest {
                query: "x".into(),
                language: "not-a-language".into(),
                top_n: 1,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not supported"));
        server.abort();
    }

    #[test]
    fn production_url_is_https_and_language_scoped() {
        let url = wikipedia_api_url("zh").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("zh.wikipedia.org"));
        assert_eq!(url.path(), "/w/api.php");
        assert!(wikipedia_api_url("zh.example.test").is_err());
    }

    #[tokio::test]
    #[ignore = "requires live access to the public Wikipedia API"]
    async fn live_public_api_returns_a_rust_article() {
        let articles = WikipediaClient::default()
            .search(&WikipediaSearchRequest {
                query: "Rust programming language".into(),
                language: "en".into(),
                top_n: 1,
            })
            .await
            .unwrap();
        assert_eq!(articles.len(), 1);
        assert!(!articles[0].summary.is_empty());
        assert!(
            articles[0]
                .url
                .starts_with("https://en.wikipedia.org/wiki/")
        );
    }
}
