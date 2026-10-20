//! Web-search providers for prompt-config-driven chat retrieval — RAGFlow
//! v0.27.2 `rag/utils/web_search_conn.py` together with its
//! `querit_conn.py` / `serply_conn.py` / `youcom_conn.py` siblings.
//!
//! `create_web_search_provider` mirrors the upstream resolver: the provider
//! key (`web_search_provider`, default `tavily`) plus per-provider API keys
//! (`tavily_api_key`, `querit_api_key`, `serply_api_key`, `youcom_api_key`)
//! live in the knowledge-base `prompt_config`. You.com serves a keyless
//! endpoint, so it is usable without credentials; every other provider needs
//! a non-empty key before it can be selected.
//!
//! Upstream connectors swallow request/parse failures, log only the error
//! type or HTTP status (never the query text or response body) and return an
//! empty result list. RayRAG keeps that contract and adds one `tracing::warn`
//! with the same information. Endpoints are fixed by construction and tests
//! inject loopback endpoints through crate-private constructors, mirroring
//! `src/tavily.rs`.

use crate::Result;
use serde_json::{Value, json};

pub const WEB_SEARCH_PROVIDER_TAVILY: &str = "tavily";
pub const WEB_SEARCH_PROVIDER_QUERIT: &str = "querit";
pub const WEB_SEARCH_PROVIDER_SERPLY: &str = "serply";
pub const WEB_SEARCH_PROVIDER_YOUCOM: &str = "youcom";

/// You.com serves a keyless endpoint, so it needs no credentials at all.
pub const KEYLESS_WEB_SEARCH_PROVIDERS: [&str; 1] = [WEB_SEARCH_PROVIDER_YOUCOM];

const QUERIT_SEARCH_URL: &str = "https://api.querit.ai/v1/search";
const SERPLY_SEARCH_URL: &str = "https://api.serply.io/v1/search/";
const YOUCOM_SEARCH_URL: &str = "https://api.you.com/v1/search";
const YOUCOM_KEYLESS_SEARCH_URL: &str = "https://api.you.com/v1/agents/search";
/// Identifies the caller to You.com; on the keyless endpoint this is the
/// only attribution signal available.
const YOUCOM_USER_AGENT: &str = "RAGFlow youdotcom-integration/infiniflow-ragflow";
const RESULT_COUNT: usize = 6;
const MAX_RESPONSE_BYTES: usize = 16 << 20;

/// One normalized web result (upstream connector `search()` dictionaries).
#[derive(Debug, Clone, PartialEq)]
pub struct WebSearchResult {
    pub url: String,
    pub title: String,
    pub content: String,
    pub score: f64,
}

/// RAGFlow's chunk/document aggregate shape (`{"chunks": [...], "doc_aggs": [...]}`).
#[derive(Debug, Clone, PartialEq)]
pub struct WebSearchChunks {
    pub chunks: Vec<Value>,
    pub doc_aggs: Vec<Value>,
}

impl WebSearchChunks {
    /// Build the aggregate exactly like the upstream connectors: every result
    /// becomes one chunk whose `chunk_id`/`doc_id` share a fresh id, plus one
    /// `doc_aggs` entry with `count: 1`.
    pub fn from_results(results: &[WebSearchResult]) -> Self {
        let mut chunks = Vec::with_capacity(results.len());
        let mut doc_aggs = Vec::with_capacity(results.len());
        for result in results {
            // Upstream `get_uuid()` returns `uuid.uuid1().hex` (32 hex chars);
            // RayRAG uses uuid4 simple-hex — same shape, no time-ordering.
            let chunk_id = uuid::Uuid::new_v4().simple().to_string();
            chunks.push(json!({
                "chunk_id": chunk_id,
                "content_ltks": tokenize(&result.content),
                "content_with_weight": result.content,
                "doc_id": chunk_id,
                "docnm_kwd": result.title,
                "kb_id": [],
                "important_kwd": [],
                "image_id": "",
                "similarity": result.score,
                "vector_similarity": 1.0,
                "term_similarity": 0,
                "vector": [],
                "positions": [],
                "url": result.url,
            }));
            doc_aggs.push(json!({
                "doc_name": result.title,
                "doc_id": chunk_id,
                "count": 1,
                "url": result.url,
            }));
        }
        Self { chunks, doc_aggs }
    }

    pub fn to_json(&self) -> Value {
        json!({"chunks": self.chunks, "doc_aggs": self.doc_aggs})
    }
}

/// Whitespace-split approximation of `rag_tokenizer.tokenize` (rayrag's
/// established `content_ltks` convention, see `src/tag.rs`).
fn tokenize(content: &str) -> Vec<String> {
    content.split_whitespace().map(str::to_owned).collect()
}

/// Shared JSON transport for the fixed-endpoint connectors.
#[derive(Debug, Clone)]
struct WebSearchHttp {
    client: reqwest::Client,
}

impl WebSearchHttp {
    fn new() -> Result<Self> {
        // Same discipline as `src/tavily.rs`: fixed endpoints, no proxy
        // environment capture and no redirects.
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .map_err(|error| anyhow::anyhow!("could not build web-search HTTP client: {error}"))?;
        Ok(Self { client })
    }

    async fn read_bounded(response: reqwest::Response) -> Result<(reqwest::StatusCode, Vec<u8>)> {
        use futures_util::StreamExt;
        let status = response.status();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|error| anyhow::anyhow!("web-search response read: {error}"))?;
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                anyhow::bail!("web-search response exceeds {MAX_RESPONSE_BYTES} bytes");
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, body))
    }

    async fn get_json(
        &self,
        endpoint: &reqwest::Url,
        headers: &[(&str, &str)],
        params: &[(&str, String)],
    ) -> Result<Value> {
        let mut request = self.client.get(endpoint.clone());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request
            .query(params)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("web-search request failed: {error}"))?;
        let (status, body) = Self::read_bounded(response).await?;
        if !status.is_success() {
            anyhow::bail!("web-search endpoint returned HTTP {status}");
        }
        serde_json::from_slice(&body)
            .map_err(|error| anyhow::anyhow!("web-search response was not valid JSON: {error}"))
    }

    async fn post_json(
        &self,
        endpoint: &reqwest::Url,
        headers: &[(&str, &str)],
        body: &Value,
    ) -> Result<Value> {
        let mut request = self.client.post(endpoint.clone());
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request
            .json(body)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("web-search request failed: {error}"))?;
        let (status, body) = Self::read_bounded(response).await?;
        if !status.is_success() {
            anyhow::bail!("web-search endpoint returned HTTP {status}");
        }
        serde_json::from_slice(&body)
            .map_err(|error| anyhow::anyhow!("web-search response was not valid JSON: {error}"))
    }
}

fn text_field(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

/// Querit connector (`rag/utils/querit_conn.py`).
#[derive(Debug, Clone)]
pub struct QueritProvider {
    api_key: String,
    endpoint: reqwest::Url,
    http: WebSearchHttp,
}

impl QueritProvider {
    pub fn new(api_key: &str) -> Result<Self> {
        Self::with_endpoint(api_key, QUERIT_SEARCH_URL)
    }

    pub(crate) fn with_endpoint(api_key: &str, endpoint: &str) -> Result<Self> {
        Ok(Self {
            api_key: api_key.to_owned(),
            endpoint: endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid Querit endpoint: {error}"))?,
            http: WebSearchHttp::new()?,
        })
    }

    /// Failure semantics mirror upstream: log type/status only, return `[]`.
    pub async fn search(&self, query: &str) -> Vec<WebSearchResult> {
        let headers = [("Accept", "application/json")];
        let body = json!({"query": query, "count": RESULT_COUNT, "chunksPerDoc": 1});
        let authorization = format!("Bearer {}", self.api_key);
        let mut request_headers = headers.to_vec();
        request_headers.push(("Authorization", authorization.as_str()));
        request_headers.push(("Content-Type", "application/json"));
        let response = match self
            .http
            .post_json(&self.endpoint, &request_headers, &body)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(error = %error, "Querit search failed");
                return Vec::new();
            }
        };
        let Some(results) = response
            .get("results")
            .and_then(|results| results.get("result"))
            .and_then(Value::as_array)
        else {
            tracing::warn!("Querit search failed: response field results.result must be an array");
            return Vec::new();
        };
        let mut normalized = Vec::new();
        for result in results {
            if !result.is_object() {
                continue;
            }
            let content = text_field(result.get("snippet"));
            if content.trim().is_empty() {
                continue;
            }
            normalized.push(WebSearchResult {
                url: text_field(result.get("url")),
                title: text_field(result.get("title")),
                content,
                score: 1.0,
            });
        }
        normalized
    }

    pub async fn retrieve_chunks(&self, question: &str) -> WebSearchChunks {
        let results = self.search(question).await;
        tracing::info!(count = results.len(), "Querit search returned chunks");
        WebSearchChunks::from_results(&results)
    }
}

/// Serply connector (`rag/utils/serply_conn.py`).
#[derive(Debug, Clone)]
pub struct SerplyProvider {
    api_key: String,
    endpoint: reqwest::Url,
    http: WebSearchHttp,
}

impl SerplyProvider {
    pub fn new(api_key: &str) -> Result<Self> {
        Self::with_endpoint(api_key, SERPLY_SEARCH_URL)
    }

    pub(crate) fn with_endpoint(api_key: &str, endpoint: &str) -> Result<Self> {
        Ok(Self {
            api_key: api_key.to_owned(),
            endpoint: endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid Serply endpoint: {error}"))?,
            http: WebSearchHttp::new()?,
        })
    }

    pub async fn search(&self, query: &str) -> Vec<WebSearchResult> {
        // Serply sits behind Cloudflare, which rejects requests without an
        // explicit User-Agent, so one is always sent.
        let headers = [
            ("Accept", "application/json"),
            ("User-Agent", "ragflow-web-search"),
        ];
        let mut request_headers = headers.to_vec();
        request_headers.push(("X-Api-Key", self.api_key.as_str()));
        let params = [("q", query.to_owned()), ("num", RESULT_COUNT.to_string())];
        let response = match self
            .http
            .get_json(&self.endpoint, &request_headers, &params)
            .await
        {
            Ok(value) => value,
            Err(error) => {
                // Never log the exception text: it embeds the request URL,
                // and the query travels as a parameter.
                tracing::warn!(error = %error, "Serply search failed");
                return Vec::new();
            }
        };
        let Some(results) = response.get("results").and_then(Value::as_array) else {
            tracing::warn!("Serply search failed: response field results must be an array");
            return Vec::new();
        };
        let mut normalized = Vec::new();
        for result in results {
            if !result.is_object() {
                continue;
            }
            let content = text_field(result.get("description")).trim().to_owned();
            if content.is_empty() {
                continue;
            }
            normalized.push(WebSearchResult {
                url: text_field(result.get("link")),
                title: text_field(result.get("title")),
                content,
                score: 1.0,
            });
        }
        normalized
    }

    pub async fn retrieve_chunks(&self, question: &str) -> WebSearchChunks {
        let results = self.search(question).await;
        tracing::info!(count = results.len(), "Serply search returned results");
        WebSearchChunks::from_results(&results)
    }
}

/// You.com connector (`rag/utils/youcom_conn.py`).
#[derive(Debug, Clone)]
pub struct YouComProvider {
    api_key: String,
    keyed_endpoint: reqwest::Url,
    keyless_endpoint: reqwest::Url,
    http: WebSearchHttp,
}

impl YouComProvider {
    pub fn new(api_key: &str) -> Result<Self> {
        Self::with_endpoints(api_key, YOUCOM_SEARCH_URL, YOUCOM_KEYLESS_SEARCH_URL)
    }

    pub(crate) fn with_endpoints(
        api_key: &str,
        keyed_endpoint: &str,
        keyless_endpoint: &str,
    ) -> Result<Self> {
        Ok(Self {
            api_key: api_key.trim().to_owned(),
            keyed_endpoint: keyed_endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid You.com endpoint: {error}"))?,
            keyless_endpoint: keyless_endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid You.com keyless endpoint: {error}"))?,
            http: WebSearchHttp::new()?,
        })
    }

    /// The keyless endpoint rejects an `X-API-Key` header, so the endpoint
    /// and the headers are always chosen together.
    pub async fn search(&self, query: &str) -> Vec<WebSearchResult> {
        let mut headers = vec![
            ("Accept", "application/json"),
            ("User-Agent", YOUCOM_USER_AGENT),
        ];
        let endpoint = if self.api_key.is_empty() {
            &self.keyless_endpoint
        } else {
            headers.push(("X-API-Key", self.api_key.as_str()));
            &self.keyed_endpoint
        };
        let params = [
            ("query", query.to_owned()),
            ("count", RESULT_COUNT.to_string()),
        ];
        let response = match self.http.get_json(endpoint, &headers, &params).await {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(error = %error, "You.com search failed");
                return Vec::new();
            }
        };
        let Some(results) = response.get("results") else {
            tracing::warn!("You.com search failed: response field results must be an object");
            return Vec::new();
        };
        if !results.is_object() {
            tracing::warn!("You.com search failed: response field results must be an object");
            return Vec::new();
        }
        let mut normalized = Vec::new();
        // `count` applies per section, so web + news can exceed it; web leads
        // and the merged list is trimmed back afterwards.
        for section in ["web", "news"] {
            let Some(section_results) = results.get(section) else {
                continue;
            };
            if section_results.is_null() {
                continue;
            }
            let Some(section_results) = section_results.as_array() else {
                tracing::warn!(
                    section,
                    "You.com search failed: result section must be an array"
                );
                return Vec::new();
            };
            for result in section_results {
                if !result.is_object() {
                    continue;
                }
                let content = youcom_content(result);
                if content.trim().is_empty() {
                    continue;
                }
                normalized.push(WebSearchResult {
                    url: text_field(result.get("url")),
                    title: text_field(result.get("title")),
                    content,
                    score: 1.0,
                });
            }
        }
        normalized.truncate(RESULT_COUNT);
        normalized
    }

    pub async fn retrieve_chunks(&self, question: &str) -> WebSearchChunks {
        let results = self.search(question).await;
        tracing::info!(
            count = results.len(),
            keyed = !self.api_key.is_empty(),
            "You.com retrieved chunks"
        );
        WebSearchChunks::from_results(&results)
    }
}

/// Prefer the extracted page passages; news hits only carry a description.
fn youcom_content(result: &Value) -> String {
    if let Some(snippets) = result.get("snippets").and_then(Value::as_array) {
        let joined = snippets
            .iter()
            .map(|snippet| text_field(Some(snippet)))
            .filter(|snippet| !snippet.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !joined.is_empty() {
            return joined;
        }
    }
    text_field(result.get("description"))
}

/// Tavily connector adapter (`rag/utils/tavily_conn.py`) over the existing
/// Canvas-tool client: `search_depth="advanced"`, `max_results=6`.
#[derive(Debug, Clone)]
pub struct TavilySearchProvider {
    api_key: String,
    client: crate::tavily::TavilyClient,
}

impl TavilySearchProvider {
    pub fn new(api_key: &str) -> Self {
        Self {
            api_key: api_key.to_owned(),
            client: crate::tavily::TavilyClient::default(),
        }
    }

    pub async fn search(&self, query: &str) -> Vec<WebSearchResult> {
        use crate::tavily::TavilyProvider as _;
        let request = crate::tavily::TavilySearchRequest {
            query: query.to_owned(),
            search_depth: "advanced".to_owned(),
            topic: "general".to_owned(),
            max_results: RESULT_COUNT,
            days: 0,
            include_answer: false,
            include_raw_content: false,
            include_images: false,
            include_image_descriptions: false,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
        };
        match self.client.search(&self.api_key, &request).await {
            Ok(items) => items
                .iter()
                .filter_map(|item| {
                    let content = item.get("content").and_then(Value::as_str)?.to_owned();
                    Some(WebSearchResult {
                        url: text_field(item.get("url")),
                        title: text_field(item.get("title")),
                        content,
                        score: item.get("score").and_then(Value::as_f64).unwrap_or(1.0),
                    })
                })
                .collect(),
            Err(error) => {
                // Upstream logs the exception type only; nothing from the
                // client exception text is forwarded.
                tracing::warn!(error = %error, "Tavily search failed");
                Vec::new()
            }
        }
    }

    pub async fn retrieve_chunks(&self, question: &str) -> WebSearchChunks {
        let results = self.search(question).await;
        tracing::info!(count = results.len(), "Tavily search returned chunks");
        WebSearchChunks::from_results(&results)
    }
}

/// Resolved provider handle returned by `create_web_search_provider`.
#[derive(Debug, Clone)]
pub enum WebSearchProvider {
    Tavily(TavilySearchProvider),
    Querit(QueritProvider),
    Serply(SerplyProvider),
    YouCom(YouComProvider),
}

impl WebSearchProvider {
    pub async fn retrieve_chunks(&self, question: &str) -> WebSearchChunks {
        match self {
            Self::Tavily(provider) => provider.retrieve_chunks(question).await,
            Self::Querit(provider) => provider.retrieve_chunks(question).await,
            Self::Serply(provider) => provider.retrieve_chunks(question).await,
            Self::YouCom(provider) => provider.retrieve_chunks(question).await,
        }
    }
}

fn api_key_field(prompt_config: &Value, field: &str) -> String {
    prompt_config
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default()
        .to_owned()
}

fn provider_name(prompt_config: &Value) -> String {
    prompt_config
        .get("web_search_provider")
        .and_then(Value::as_str)
        .unwrap_or(WEB_SEARCH_PROVIDER_TAVILY)
        .to_owned()
}

/// `rag/utils/web_search_conn.py::has_web_search_provider`.
pub fn has_web_search_provider(prompt_config: Option<&Value>) -> bool {
    let Some(prompt_config) = prompt_config else {
        return false;
    };
    if !prompt_config.is_object()
        || prompt_config
            .as_object()
            .is_some_and(|object| object.is_empty())
    {
        return false;
    }
    let provider = provider_name(prompt_config);
    if KEYLESS_WEB_SEARCH_PROVIDERS.contains(&provider.as_str()) {
        return true;
    }
    match provider.as_str() {
        WEB_SEARCH_PROVIDER_TAVILY => !api_key_field(prompt_config, "tavily_api_key").is_empty(),
        WEB_SEARCH_PROVIDER_QUERIT => !api_key_field(prompt_config, "querit_api_key").is_empty(),
        WEB_SEARCH_PROVIDER_SERPLY => !api_key_field(prompt_config, "serply_api_key").is_empty(),
        _ => false,
    }
}

/// `rag/utils/web_search_conn.py::create_web_search_provider`.
pub fn create_web_search_provider(
    prompt_config: Option<&Value>,
) -> Result<Option<WebSearchProvider>> {
    let Some(prompt_config) = prompt_config else {
        tracing::debug!("Web search provider resolution: provider=none status=disabled");
        return Ok(None);
    };
    if !prompt_config.is_object()
        || prompt_config
            .as_object()
            .is_some_and(|object| object.is_empty())
    {
        tracing::debug!("Web search provider resolution: provider=none status=disabled");
        return Ok(None);
    }
    let provider = provider_name(prompt_config);
    if ![
        WEB_SEARCH_PROVIDER_TAVILY,
        WEB_SEARCH_PROVIDER_QUERIT,
        WEB_SEARCH_PROVIDER_SERPLY,
        WEB_SEARCH_PROVIDER_YOUCOM,
    ]
    .contains(&provider.as_str())
    {
        tracing::debug!(provider, "Web search provider resolution: status=invalid");
        return Ok(None);
    }
    if !has_web_search_provider(Some(prompt_config)) {
        tracing::debug!(provider, "Web search provider resolution: status=disabled");
        return Ok(None);
    }
    tracing::debug!(provider, "Web search provider resolution: status=resolved");
    let resolved = match provider.as_str() {
        WEB_SEARCH_PROVIDER_QUERIT => WebSearchProvider::Querit(QueritProvider::new(
            &api_key_field(prompt_config, "querit_api_key"),
        )?),
        WEB_SEARCH_PROVIDER_SERPLY => WebSearchProvider::Serply(SerplyProvider::new(
            &api_key_field(prompt_config, "serply_api_key"),
        )?),
        WEB_SEARCH_PROVIDER_YOUCOM => WebSearchProvider::YouCom(YouComProvider::new(
            &api_key_field(prompt_config, "youcom_api_key"),
        )?),
        _ => WebSearchProvider::Tavily(TavilySearchProvider::new(&api_key_field(
            prompt_config,
            "tavily_api_key",
        ))),
    };
    Ok(Some(resolved))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        extract::Query,
        http::{HeaderMap, StatusCode},
        routing::{get, post},
    };
    use serde_json::json;
    use std::collections::HashMap;

    async fn spawn_get(handler: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, handler).await.unwrap() });
        (format!("http://{address}"), server)
    }

    #[test]
    fn resolver_mirrors_upstream_rules() {
        assert!(!has_web_search_provider(None));
        assert!(!has_web_search_provider(Some(&json!({}))));
        // Default provider is tavily and requires a key.
        assert!(!has_web_search_provider(Some(
            &json!({"tavily_api_key": "  "})
        )));
        assert!(has_web_search_provider(Some(
            &json!({"tavily_api_key": "k"})
        )));
        assert!(has_web_search_provider(Some(
            &json!({"web_search_provider": "querit", "querit_api_key": "k"})
        )));
        assert!(!has_web_search_provider(Some(
            &json!({"web_search_provider": "serply"})
        )));
        // You.com is keyless.
        assert!(has_web_search_provider(Some(
            &json!({"web_search_provider": "youcom"})
        )));
        // Unknown providers are never "configured".
        assert!(!has_web_search_provider(Some(
            &json!({"web_search_provider": "bing", "bing_api_key": "k"})
        )));
        assert!(!has_web_search_provider(Some(&json!("nope"))));
    }

    #[test]
    fn resolver_creates_the_expected_provider_kind() {
        let provider = create_web_search_provider(Some(&json!({"tavily_api_key": "k"})))
            .unwrap()
            .unwrap();
        assert!(matches!(provider, WebSearchProvider::Tavily(_)));
        let provider = create_web_search_provider(Some(
            &json!({"web_search_provider": "querit", "querit_api_key": "k"}),
        ))
        .unwrap()
        .unwrap();
        assert!(matches!(provider, WebSearchProvider::Querit(_)));
        let provider = create_web_search_provider(Some(&json!({"web_search_provider": "youcom"})))
            .unwrap()
            .unwrap();
        assert!(matches!(provider, WebSearchProvider::YouCom(_)));
        assert!(create_web_search_provider(None).unwrap().is_none());
        assert!(
            create_web_search_provider(Some(&json!({"web_search_provider": "nope"})))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn aggregate_shape_matches_upstream_connectors() {
        let chunks = WebSearchChunks::from_results(&[WebSearchResult {
            url: "https://example.com".to_owned(),
            title: "Title".to_owned(),
            content: "hello world".to_owned(),
            score: 1.0,
        }]);
        assert_eq!(chunks.chunks.len(), 1);
        let chunk = &chunks.chunks[0];
        let chunk_id = chunk["chunk_id"].as_str().unwrap();
        assert_eq!(chunk_id.len(), 32);
        assert_eq!(chunk["content_ltks"], json!(["hello", "world"]));
        assert_eq!(chunk["content_with_weight"], "hello world");
        assert_eq!(chunk["doc_id"], chunk_id);
        assert_eq!(chunk["docnm_kwd"], "Title");
        assert_eq!(chunk["kb_id"], json!([]));
        assert_eq!(chunk["important_kwd"], json!([]));
        assert_eq!(chunk["image_id"], "");
        assert_eq!(chunk["similarity"], 1.0);
        assert_eq!(chunk["vector_similarity"], 1.0);
        assert_eq!(chunk["term_similarity"], 0);
        assert_eq!(chunk["vector"], json!([]));
        assert_eq!(chunk["positions"], json!([]));
        assert_eq!(chunk["url"], "https://example.com");
        assert_eq!(
            chunks.doc_aggs[0],
            json!({"doc_name": "Title", "doc_id": chunk_id, "count": 1, "url": "https://example.com"})
        );
    }

    #[tokio::test]
    async fn querit_posts_expected_payload_and_normalizes_results() {
        let app = Router::new().route(
            "/v1/search",
            post(|headers: HeaderMap, body: String| async move {
                let payload: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(payload["count"], 6);
                assert_eq!(payload["chunksPerDoc"], 1);
                assert_eq!(headers["authorization"], "Bearer sk-q");
                Json(json!({
                    "results": {
                        "result": [
                            {"snippet": "alpha", "url": "u1", "title": "t1"},
                            {"snippet": "  ", "url": "u2", "title": "t2"},
                            {"url": "u3"}
                        ]
                    }
                }))
            }),
        );
        let (base, server) = spawn_get(app).await;
        let provider = QueritProvider::with_endpoint("sk-q", &format!("{base}/v1/search")).unwrap();
        let results = provider.search("hello").await;
        server.abort();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "alpha");
        assert_eq!(results[0].url, "u1");
        assert_eq!(results[0].score, 1.0);
    }

    #[tokio::test]
    async fn querit_failures_return_empty_results() {
        let app = Router::new().route(
            "/v1/search",
            post(|| async { (StatusCode::TOO_MANY_REQUESTS, "slow down") }),
        );
        let (base, server) = spawn_get(app).await;
        let provider = QueritProvider::with_endpoint("sk", &format!("{base}/v1/search")).unwrap();
        assert!(provider.search("q").await.is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn serply_sends_key_and_user_agent_and_reads_description() {
        let app = Router::new().route(
            "/v1/search",
            get(
                |headers: HeaderMap, Query(params): Query<HashMap<String, String>>| async move {
                    assert_eq!(headers["x-api-key"], "sk-s");
                    assert_eq!(headers["user-agent"], "ragflow-web-search");
                    assert_eq!(params.get("q").map(String::as_str), Some("hello"));
                    assert_eq!(params.get("num").map(String::as_str), Some("6"));
                    Json(json!({
                        "results": [
                            {"description": "desc", "link": "u1", "title": "t1"},
                            {"description": "", "link": "u2", "title": "t2"}
                        ]
                    }))
                },
            ),
        );
        let (base, server) = spawn_get(app).await;
        let provider = SerplyProvider::with_endpoint("sk-s", &format!("{base}/v1/search")).unwrap();
        let results = provider.search("hello").await;
        server.abort();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "desc");
        assert_eq!(results[0].url, "u1");
    }

    #[tokio::test]
    async fn youcom_keyless_uses_agents_endpoint_without_api_key() {
        let app = Router::new()
            .route(
                "/v1/search",
                get(|| async { (StatusCode::UNAUTHORIZED, "keyed endpoint reached") }),
            )
            .route(
                "/v1/agents/search",
                get(
                    |headers: HeaderMap, Query(params): Query<HashMap<String, String>>| async move {
                        assert!(headers.get("x-api-key").is_none());
                        assert_eq!(params.get("count").map(String::as_str), Some("6"));
                        Json(json!({
                            "results": {
                                "web": [
                                    {"url": "u1", "title": "t1", "snippets": ["s1", " ", "s2"]}
                                ],
                                "news": [
                                    {"url": "u2", "title": "t2", "description": "news desc"}
                                ]
                            }
                        }))
                    },
                ),
            );
        let (base, server) = spawn_get(app).await;
        let provider = YouComProvider::with_endpoints(
            "",
            &format!("{base}/v1/search"),
            &format!("{base}/v1/agents/search"),
        )
        .unwrap();
        let results = provider.search("hello").await;
        server.abort();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].content, "s1\ns2");
        assert_eq!(results[1].content, "news desc");
    }

    #[tokio::test]
    async fn youcom_keyed_uses_api_endpoint_and_trims_to_six() {
        let app = Router::new()
            .route(
                "/v1/agents/search",
                get(|| async { (StatusCode::UNAUTHORIZED, "keyless reached") }),
            )
            .route(
                "/v1/search",
                get(|headers: HeaderMap| async move {
                    assert_eq!(headers["x-api-key"], "sk-y");
                    let web: Vec<Value> = (0..5)
                        .map(|index| json!({"url": format!("w{index}"), "title": "t", "description": "d"}))
                        .collect();
                    let news: Vec<Value> = (0..5)
                        .map(|index| json!({"url": format!("n{index}"), "title": "t", "description": "d"}))
                        .collect();
                    Json(json!({"results": {"web": web, "news": news}}))
                }),
            );
        let (base, server) = spawn_get(app).await;
        let provider = YouComProvider::with_endpoints(
            "sk-y",
            &format!("{base}/v1/search"),
            &format!("{base}/v1/agents/search"),
        )
        .unwrap();
        let results = provider.search("hello").await;
        server.abort();
        assert_eq!(results.len(), 6);
        // Web results lead; the merged list is trimmed back to six, so the
        // fifth web hit is followed by the first news hit.
        assert!(
            results[..5]
                .iter()
                .all(|result| result.url.starts_with('w'))
        );
        assert_eq!(results[5].url, "n0");
    }

    #[tokio::test]
    async fn tavily_adapter_maps_advanced_search_results() {
        let app = Router::new().route(
            "/search",
            post(|headers: HeaderMap, body: String| async move {
                assert_eq!(headers["authorization"], "Bearer sk-t");
                let payload: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(payload["search_depth"], "advanced");
                assert_eq!(payload["max_results"], 6);
                Json(json!({
                    "results": [
                        {"url": "u1", "title": "t1", "content": "body", "score": 0.5}
                    ]
                }))
            }),
        );
        let (base, server) = spawn_get(app).await;
        let client = crate::tavily::TavilyClient::new_with_endpoints(
            &format!("{base}/search"),
            &format!("{base}/extract"),
        )
        .unwrap();
        let provider = TavilySearchProvider {
            api_key: "sk-t".to_owned(),
            client,
        };
        let results = provider.search("hello").await;
        server.abort();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].score, 0.5);
        assert_eq!(results[0].content, "body");
    }
}
