//! Querit agent tools — RAGFlow v0.27.2 `agent/tools/querit.py`.
//!
//! Two Canvas tools: `querit_search` (POST /v1/search with sites/timeRange/geo/
//! language filters) and `querit_contents` (POST /v1/contents for 1–10 absolute
//! HTTP(S) URLs). Both follow the upstream retry contract: up to three
//! attempts, retrying the fixed status set {429, 500, 502, 503, 504} and
//! network errors with `delay_after_error` in between; non-retryable statuses
//! and JSON decode failures raise immediately. Error text redacts the API key
//! before it reaches logs or the canvas.

use crate::Result;
use serde_json::{Map, Value, json};
use std::time::Duration;

pub const QUERIT_SEARCH_URL: &str = "https://api.querit.ai/v1/search";
pub const QUERIT_CONTENTS_URL: &str = "https://api.querit.ai/v1/contents";
pub const QUERIT_MAX_ATTEMPTS: usize = 3;
pub const QUERIT_RETRYABLE_STATUS_CODES: [u16; 5] = [429, 500, 502, 503, 504];
pub const QUERIT_CONTENT_FORMATS: [&str; 3] = ["text", "markdown", "html"];
/// `common/http_client.DEFAULT_TIMEOUT` (`HTTP_CLIENT_TIMEOUT`, default 15s).
pub const QUERIT_DEFAULT_TIMEOUT_SECS: u64 = 15;
const MAX_RESPONSE_BYTES: usize = 16 << 20;

/// Querit search parameters (`QueritSearchParam` meta defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct QueritSearchParams {
    pub query: String,
    pub count: i64,
    pub chunks_per_doc: Option<i64>,
    pub site_include: Vec<String>,
    pub site_exclude: Vec<String>,
    pub time_range: String,
    pub country_include: Vec<String>,
    pub language_include: Vec<String>,
}

impl Default for QueritSearchParams {
    fn default() -> Self {
        Self {
            query: String::new(),
            count: 10,
            chunks_per_doc: Some(3),
            site_include: Vec::new(),
            site_exclude: Vec::new(),
            time_range: String::new(),
            country_include: Vec::new(),
            language_include: Vec::new(),
        }
    }
}

/// Querit contents parameters (`QueritContentsParam` meta defaults).
#[derive(Debug, Clone, PartialEq)]
pub struct QueritContentsParams {
    pub urls: Vec<String>,
    pub format: String,
    pub crawl_timeout: i64,
    pub extras_meta: bool,
}

impl Default for QueritContentsParams {
    fn default() -> Self {
        Self {
            urls: Vec::new(),
            format: "markdown".to_owned(),
            crawl_timeout: 10,
            extras_meta: false,
        }
    }
}

/// Upstream `TIME_RANGE_PATTERN`: `dN`/`wN`/`mN`/`yN` (N >= 1) or
/// `YYYY-MM-DDtoYYYY-MM-DD`.
fn is_valid_time_range(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        return true;
    }
    if matches!(bytes[0], b'd' | b'w' | b'm' | b'y') {
        let digits = &value[1..];
        return !digits.is_empty()
            && digits.as_bytes()[0] != b'0'
            && digits.bytes().all(|byte| byte.is_ascii_digit());
    }
    // YYYY-MM-DDtoYYYY-MM-DD (22 bytes).
    if bytes.len() != 22 {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && &bytes[10..12] == b"to"
        && digits(12..16)
        && bytes[16] == b'-'
        && digits(17..19)
        && bytes[19] == b'-'
        && digits(20..22)
}

/// `_validate_search_inputs` (messages copied from upstream).
pub fn validate_search_inputs(params: &QueritSearchParams) -> Result<()> {
    if params.count < 1 {
        anyhow::bail!("Querit count must be an integer greater than or equal to 1.");
    }
    if let Some(chunks_per_doc) = params.chunks_per_doc
        && !(1..=3).contains(&chunks_per_doc)
    {
        anyhow::bail!("Querit chunks_per_doc must be an integer from 1 to 3.");
    }
    if !is_valid_time_range(&params.time_range) {
        anyhow::bail!("Querit time_range must use dN, wN, mN, yN, or YYYY-MM-DDtoYYYY-MM-DD.");
    }
    Ok(())
}

/// `_build_payload`.
pub fn build_search_payload(params: &QueritSearchParams) -> Value {
    let mut payload = json!({"query": params.query, "count": params.count});
    if let Some(chunks_per_doc) = params.chunks_per_doc {
        payload["chunksPerDoc"] = json!(chunks_per_doc);
    }
    let mut filters = Map::new();
    if !params.site_include.is_empty() || !params.site_exclude.is_empty() {
        let mut sites = Map::new();
        if !params.site_include.is_empty() {
            sites.insert("include".to_owned(), json!(params.site_include));
        }
        if !params.site_exclude.is_empty() {
            sites.insert("exclude".to_owned(), json!(params.site_exclude));
        }
        filters.insert("sites".to_owned(), Value::Object(sites));
    }
    if !params.time_range.is_empty() {
        filters.insert("timeRange".to_owned(), json!({"date": params.time_range}));
    }
    if !params.country_include.is_empty() {
        filters.insert(
            "geo".to_owned(),
            json!({"countries": {"include": params.country_include}}),
        );
    }
    if !params.language_include.is_empty() {
        filters.insert(
            "languages".to_owned(),
            json!({"include": params.language_include}),
        );
    }
    if !filters.is_empty() {
        payload["filters"] = Value::Object(filters);
    }
    payload
}

/// `_normalize_contents_urls`: a comma-separated string becomes a list.
pub fn normalize_contents_urls(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => text
            .split(',')
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
            .collect(),
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().unwrap_or_default().trim().to_owned())
            .collect(),
        _ => Vec::new(),
    }
}

/// `_validate_contents_inputs` (messages copied from upstream).
pub fn validate_contents_inputs(params: &QueritContentsParams) -> Result<()> {
    if params.urls.is_empty()
        || params.urls.len() > 10
        || params.urls.iter().any(|url| url.trim().is_empty())
    {
        anyhow::bail!("Querit urls must contain between 1 and 10 non-empty strings.");
    }
    for url in &params.urls {
        let parsed = reqwest::Url::parse(url)
            .map_err(|_| anyhow::anyhow!("Querit urls must be absolute HTTP or HTTPS URLs."))?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host().is_none() {
            anyhow::bail!("Querit urls must be absolute HTTP or HTTPS URLs.");
        }
    }
    if !QUERIT_CONTENT_FORMATS.contains(&params.format.as_str()) {
        anyhow::bail!("Querit format must be text, markdown, or html.");
    }
    if !(1..=60).contains(&params.crawl_timeout) {
        anyhow::bail!("Querit crawl_timeout must be an integer from 1 to 60.");
    }
    Ok(())
}

/// `_build_contents_payload`.
pub fn build_contents_payload(params: &QueritContentsParams) -> Value {
    json!({
        "urls": params.urls,
        "format": params.format,
        "crawlTimeout": params.crawl_timeout,
        "extrasMeta": params.extras_meta,
    })
}

/// `_validate_contents_response`.
pub fn validate_contents_response(response: &Value) -> Result<()> {
    if !response.is_object() {
        anyhow::bail!("Querit API response must be a JSON object.");
    }
    if let Some(results) = response.get("results")
        && !results.is_array()
    {
        anyhow::bail!("Querit API response field results must be an array.");
    }
    if let Some(statuses) = response.get("statuses")
        && !statuses.is_array()
    {
        anyhow::bail!("Querit API response field statuses must be an array.");
    }
    Ok(())
}

/// `_safe_error_message`: never let the API key reach logs or the canvas.
pub fn safe_error_message(message: &str, api_key: &str) -> String {
    if api_key.is_empty() {
        message.to_owned()
    } else {
        message.replace(api_key, "[REDACTED]")
    }
}

/// Async Querit HTTP client. Endpoints are fixed by construction; module tests
/// inject loopback endpoints through the crate-private constructor.
#[derive(Debug, Clone)]
pub struct QueritClient {
    client: reqwest::Client,
    search_endpoint: reqwest::Url,
    contents_endpoint: reqwest::Url,
}

impl Default for QueritClient {
    fn default() -> Self {
        Self::new_with_endpoints(QUERIT_SEARCH_URL, QUERIT_CONTENTS_URL)
            .expect("fixed Querit endpoints and HTTP client configuration are valid")
    }
}

impl QueritClient {
    pub(crate) fn new_with_endpoints(
        search_endpoint: &str,
        contents_endpoint: &str,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(crate::common::cmd_timeout::duration())
            .build()
            .map_err(|error| anyhow::anyhow!("could not build Querit HTTP client: {error}"))?;
        Ok(Self {
            client,
            search_endpoint: search_endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid Querit search endpoint: {error}"))?,
            contents_endpoint: contents_endpoint
                .parse()
                .map_err(|error| anyhow::anyhow!("invalid Querit contents endpoint: {error}"))?,
        })
    }

    pub async fn search(&self, api_key: &str, payload: &Value, delay: Duration) -> Result<Value> {
        self.post_with_retry(
            &self.search_endpoint,
            api_key,
            payload,
            Duration::from_secs(QUERIT_DEFAULT_TIMEOUT_SECS),
            delay,
        )
        .await
    }

    pub async fn contents(
        &self,
        api_key: &str,
        payload: &Value,
        request_timeout: Duration,
        delay: Duration,
    ) -> Result<Value> {
        self.post_with_retry(
            &self.contents_endpoint,
            api_key,
            payload,
            request_timeout,
            delay,
        )
        .await
    }

    /// `_post_querit`: at most three attempts; retryable statuses and network
    /// errors sleep `delay` and retry, everything else surfaces immediately.
    async fn post_with_retry(
        &self,
        endpoint: &reqwest::Url,
        api_key: &str,
        payload: &Value,
        request_timeout: Duration,
        delay: Duration,
    ) -> Result<Value> {
        if api_key.trim().is_empty() {
            anyhow::bail!("Querit API key is required. Configure api_key or set QUERIT_API_KEY.");
        }
        let mut last_error: Option<String> = None;
        for attempt in 0..QUERIT_MAX_ATTEMPTS {
            match self
                .client
                .post(endpoint.clone())
                .bearer_auth(api_key)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .timeout(request_timeout)
                .json(payload)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    let retryable = QUERIT_RETRYABLE_STATUS_CODES.contains(&status.as_u16());
                    if retryable && attempt + 1 < QUERIT_MAX_ATTEMPTS {
                        last_error = Some(format!("Querit request returned HTTP {status}"));
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        continue;
                    }
                    let body = read_bounded_body(response).await?;
                    if !status.is_success() {
                        let snippet: String =
                            String::from_utf8_lossy(&body).chars().take(300).collect();
                        anyhow::bail!("Querit request failed: HTTP {status} {snippet}");
                    }
                    return serde_json::from_slice(&body).map_err(|error| {
                        anyhow::anyhow!("Querit API response was not valid JSON: {error}")
                    });
                }
                Err(error) => {
                    if attempt + 1 >= QUERIT_MAX_ATTEMPTS {
                        anyhow::bail!("Querit request failed: {error}");
                    }
                    last_error = Some(error.to_string());
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }
        anyhow::bail!(
            "Querit request failed after three attempts: {}",
            last_error.unwrap_or_else(|| "unknown error".to_owned())
        );
    }
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    use futures_util::StreamExt;
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| anyhow::anyhow!("Querit response read: {error}"))?;
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            anyhow::bail!("Querit response exceeds {MAX_RESPONSE_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, routing::post};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    fn params() -> QueritSearchParams {
        QueritSearchParams {
            query: "hello".to_owned(),
            ..QueritSearchParams::default()
        }
    }

    #[test]
    fn search_validation_matches_upstream_messages() {
        assert!(validate_search_inputs(&params()).is_ok());
        let mut bad = params();
        bad.count = 0;
        assert!(
            validate_search_inputs(&bad)
                .unwrap_err()
                .to_string()
                .contains("count")
        );
        let mut bad = params();
        bad.chunks_per_doc = Some(4);
        assert!(
            validate_search_inputs(&bad)
                .unwrap_err()
                .to_string()
                .contains("chunks_per_doc")
        );
        let mut bad = params();
        bad.chunks_per_doc = None;
        assert!(validate_search_inputs(&bad).is_ok());
        let mut bad = params();
        bad.time_range = "d0".to_owned();
        assert!(
            validate_search_inputs(&bad)
                .unwrap_err()
                .to_string()
                .contains("time_range")
        );
        for good in ["d7", "w1", "m3", "y1", "2024-01-01to2024-02-02"] {
            let mut ok = params();
            ok.time_range = good.to_owned();
            assert!(
                validate_search_inputs(&ok).is_ok(),
                "{good} should be valid"
            );
        }
        for bad_range in ["x7", "d", "2024-01-01", "2024-1-1to2024-2-2"] {
            let mut bad = params();
            bad.time_range = bad_range.to_owned();
            assert!(
                validate_search_inputs(&bad).is_err(),
                "{bad_range} should be invalid"
            );
        }
    }

    #[test]
    fn search_payload_includes_filters_only_when_set() {
        let minimal = build_search_payload(&params());
        assert_eq!(minimal["query"], "hello");
        assert_eq!(minimal["count"], 10);
        assert_eq!(minimal["chunksPerDoc"], 3);
        assert!(minimal.get("filters").is_none());

        let full = build_search_payload(&QueritSearchParams {
            count: 5,
            chunks_per_doc: Some(2),
            site_include: vec!["a.com".into()],
            site_exclude: vec!["b.com".into()],
            time_range: "d7".into(),
            country_include: vec!["us".into()],
            language_include: vec!["en".into()],
            ..params()
        });
        assert_eq!(full["filters"]["sites"]["include"], json!(["a.com"]));
        assert_eq!(full["filters"]["sites"]["exclude"], json!(["b.com"]));
        assert_eq!(full["filters"]["timeRange"]["date"], "d7");
        assert_eq!(
            full["filters"]["geo"]["countries"]["include"],
            json!(["us"])
        );
        assert_eq!(full["filters"]["languages"]["include"], json!(["en"]));
    }

    #[test]
    fn contents_validation_and_payload() {
        assert_eq!(
            normalize_contents_urls(&json!("https://a.com, https://b.com")),
            vec!["https://a.com".to_owned(), "https://b.com".to_owned()]
        );
        assert_eq!(
            normalize_contents_urls(&json!(["https://a.com", ""])),
            vec!["https://a.com".to_owned(), String::new()]
        );
        let params = QueritContentsParams {
            urls: vec!["https://a.com".into()],
            ..QueritContentsParams::default()
        };
        assert!(validate_contents_inputs(&params).is_ok());
        let payload = build_contents_payload(&params);
        assert_eq!(payload["urls"], json!(["https://a.com"]));
        assert_eq!(payload["format"], "markdown");
        assert_eq!(payload["crawlTimeout"], 10);
        assert_eq!(payload["extrasMeta"], false);

        let bad_scheme = QueritContentsParams {
            urls: vec!["ftp://a.com".into()],
            ..QueritContentsParams::default()
        };
        assert!(validate_contents_inputs(&bad_scheme).is_err());
        let too_many = QueritContentsParams {
            urls: vec!["https://a.com".into(); 11],
            ..QueritContentsParams::default()
        };
        assert!(validate_contents_inputs(&too_many).is_err());
        let bad_timeout = QueritContentsParams {
            urls: vec!["https://a.com".into()],
            crawl_timeout: 61,
            ..QueritContentsParams::default()
        };
        assert!(validate_contents_inputs(&bad_timeout).is_err());
        let bad_format = QueritContentsParams {
            urls: vec!["https://a.com".into()],
            format: "pdf".into(),
            ..QueritContentsParams::default()
        };
        assert!(validate_contents_inputs(&bad_format).is_err());
    }

    #[test]
    fn contents_response_and_redaction() {
        assert!(validate_contents_response(&json!({"results": []})).is_ok());
        assert!(validate_contents_response(&json!({"statuses": []})).is_ok());
        assert!(validate_contents_response(&json!("nope")).is_err());
        assert!(validate_contents_response(&json!({"results": {}})).is_err());
        assert!(validate_contents_response(&json!({"statuses": {}})).is_err());
        assert_eq!(
            safe_error_message("failed with sk-abc in text", "sk-abc"),
            "failed with [REDACTED] in text"
        );
        assert_eq!(safe_error_message("plain", ""), "plain");
    }

    async fn spawn_sequence(
        responses: Vec<StatusCode>,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let counter = Arc::new(AtomicUsize::new(0));
        let state = counter.clone();
        let app = Router::new().route(
            "/v1/search",
            post(move || {
                let state = state.clone();
                let responses = responses.clone();
                async move {
                    let index = state.fetch_add(1, Ordering::SeqCst);
                    let status = responses.get(index).copied().unwrap_or(StatusCode::OK);
                    if status == StatusCode::OK {
                        (status, Json(json!({"results": {"result": []}})))
                    } else {
                        (status, Json(json!({"error": "bad"})))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}"), counter, server)
    }

    #[tokio::test]
    async fn retries_retryable_statuses_then_succeeds() {
        let (base, counter, server) = spawn_sequence(vec![
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::OK,
        ])
        .await;
        let client = QueritClient::new_with_endpoints(
            &format!("{base}/v1/search"),
            &format!("{base}/v1/contents"),
        )
        .unwrap();
        let payload = build_search_payload(&params());
        let response = client
            .search("sk-test", &payload, Duration::from_millis(1))
            .await
            .unwrap();
        server.abort();
        assert_eq!(response["results"]["result"], json!([]));
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn retryable_status_exhaustion_surfaces_http_status() {
        let (base, counter, server) = spawn_sequence(vec![
            StatusCode::BAD_GATEWAY,
            StatusCode::BAD_GATEWAY,
            StatusCode::BAD_GATEWAY,
        ])
        .await;
        let client = QueritClient::new_with_endpoints(
            &format!("{base}/v1/search"),
            &format!("{base}/v1/contents"),
        )
        .unwrap();
        let error = client
            .search(
                "sk",
                &build_search_payload(&params()),
                Duration::from_millis(1),
            )
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("502"), "{error}");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_retryable_status_raises_immediately() {
        let (base, counter, server) = spawn_sequence(vec![StatusCode::NOT_FOUND]).await;
        let client = QueritClient::new_with_endpoints(
            &format!("{base}/v1/search"),
            &format!("{base}/v1/contents"),
        )
        .unwrap();
        let error = client
            .search(
                "sk",
                &build_search_payload(&params()),
                Duration::from_millis(1),
            )
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("404"), "{error}");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn empty_key_is_rejected_before_any_request() {
        let client = QueritClient::default();
        let error = client
            .search(
                "  ",
                &build_search_payload(&params()),
                Duration::from_millis(1),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("API key is required"), "{error}");
    }

    #[tokio::test]
    async fn contents_posts_expected_payload() {
        let app = Router::new().route(
            "/v1/contents",
            post(|headers: axum::http::HeaderMap, body: String| async move {
                assert_eq!(headers["authorization"], "Bearer sk-c");
                let payload: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(payload["urls"], json!(["https://a.com"]));
                assert_eq!(payload["format"], "markdown");
                assert_eq!(payload["crawlTimeout"], 10);
                assert_eq!(payload["extrasMeta"], false);
                Json(json!({"results": [], "statuses": []}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = QueritClient::new_with_endpoints(
            &format!("http://{address}/v1/search"),
            &format!("http://{address}/v1/contents"),
        )
        .unwrap();
        let params = QueritContentsParams {
            urls: vec!["https://a.com".into()],
            ..QueritContentsParams::default()
        };
        let response = client
            .contents(
                "sk-c",
                &build_contents_payload(&params),
                Duration::from_secs(15),
                Duration::from_millis(1),
            )
            .await
            .unwrap();
        server.abort();
        validate_contents_response(&response).unwrap();
    }
}
