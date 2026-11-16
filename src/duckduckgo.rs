//! Fixed-endpoint DuckDuckGo protocols for Canvas search tools.
//!
//! The executable Canvas path follows the fixed Python tool's text/news
//! result shape. The public Instant Answer method records the distinct Go
//! tool contract without allowing Canvas data to select arbitrary URLs.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use regex::Regex;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::OnceLock;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const DUCKDUCKGO_TEXT_ENDPOINT: &str = "https://html.duckduckgo.com/html";
const DUCKDUCKGO_HOME_ENDPOINT: &str = "https://duckduckgo.com/";
const DUCKDUCKGO_NEWS_ENDPOINT: &str = "https://duckduckgo.com/news.js";
const DUCKDUCKGO_INSTANT_ENDPOINT: &str = "https://api.duckduckgo.com/";
const MAX_DUCKDUCKGO_RESPONSE_BODY: usize = 16 << 20;
const DUCKDUCKGO_USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 RayRAG/",
    env!("CARGO_PKG_VERSION")
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckDuckGoChannel {
    Text,
    News,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuckDuckGoSearchRequest {
    pub query: String,
    pub channel: DuckDuckGoChannel,
    pub top_n: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DuckDuckGoTopic {
    pub text: String,
    pub first_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DuckDuckGoInstantAnswer {
    pub abstract_text: String,
    pub abstract_url: String,
    pub related_topics: Vec<DuckDuckGoTopic>,
}

#[async_trait]
pub trait DuckDuckGoProvider: Send + Sync {
    async fn search(&self, request: &DuckDuckGoSearchRequest) -> Result<Vec<Value>>;
}

#[derive(Debug, Clone)]
pub struct DuckDuckGoClient {
    client: reqwest::Client,
    text_endpoint: reqwest::Url,
    home_endpoint: reqwest::Url,
    news_endpoint: reqwest::Url,
    instant_endpoint: reqwest::Url,
}

impl Default for DuckDuckGoClient {
    fn default() -> Self {
        Self::new(
            DUCKDUCKGO_TEXT_ENDPOINT,
            DUCKDUCKGO_HOME_ENDPOINT,
            DUCKDUCKGO_NEWS_ENDPOINT,
            DUCKDUCKGO_INSTANT_ENDPOINT,
        )
        .expect("fixed DuckDuckGo HTTP client configuration is valid")
    }
}

impl DuckDuckGoClient {
    fn new(text: &str, home: &str, news: &str, instant: &str) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(RedirectPolicy::none())
                .timeout(crate::common::cmd_timeout::duration())
                .user_agent(DUCKDUCKGO_USER_AGENT)
                .build()
                .context("could not build DuckDuckGo HTTP client")?,
            text_endpoint: text.parse().context("invalid DuckDuckGo text endpoint")?,
            home_endpoint: home.parse().context("invalid DuckDuckGo home endpoint")?,
            news_endpoint: news.parse().context("invalid DuckDuckGo news endpoint")?,
            instant_endpoint: instant
                .parse()
                .context("invalid DuckDuckGo Instant Answer endpoint")?,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_with_endpoints(
        text: &str,
        home: &str,
        news: &str,
        instant: &str,
    ) -> Result<Self> {
        Self::new(text, home, news, instant)
    }

    async fn response_bytes(response: reqwest::Response) -> Result<Vec<u8>> {
        let status = response.status();
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("could not read DuckDuckGo response body")?;
            let remaining = MAX_DUCKDUCKGO_RESPONSE_BODY.saturating_sub(body.len());
            if chunk.len() > remaining {
                bail!(
                    "DuckDuckGo response body exceeds {} bytes",
                    MAX_DUCKDUCKGO_RESPONSE_BODY
                );
            }
            body.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&body);
            bail!("DuckDuckGo returned HTTP {status}: {detail}");
        }
        Ok(body)
    }

    async fn text_search(&self, request: &DuckDuckGoSearchRequest) -> Result<Vec<Value>> {
        let response = self
            .client
            .post(self.text_endpoint.clone())
            .form(&[("q", request.query.as_str()), ("b", ""), ("kl", "wt-wt")])
            .send()
            .await
            .context("DuckDuckGo text request failed")?;
        let body = Self::response_bytes(response).await?;
        let html = String::from_utf8(body).context("DuckDuckGo text response was not UTF-8")?;
        parse_text_results(&html, request.top_n)
    }

    async fn vqd(&self, query: &str) -> Result<String> {
        let response = self
            .client
            .get(self.home_endpoint.clone())
            .query(&[("q", query)])
            .send()
            .await
            .context("DuckDuckGo vqd request failed")?;
        let body = Self::response_bytes(response).await?;
        extract_vqd(&body).ok_or_else(|| anyhow!("DuckDuckGo response did not contain vqd"))
    }

    async fn news_search(&self, request: &DuckDuckGoSearchRequest) -> Result<Vec<Value>> {
        let vqd = self.vqd(&request.query).await?;
        let mut offset: Option<String> = None;
        let mut seen = HashSet::new();
        let mut results = Vec::new();
        for _ in 0..5 {
            let mut url = self.news_endpoint.clone();
            {
                let mut query = url.query_pairs_mut();
                query
                    .append_pair("l", "wt-wt")
                    .append_pair("o", "json")
                    .append_pair("noamp", "1")
                    .append_pair("q", &request.query)
                    .append_pair("vqd", &vqd)
                    .append_pair("p", "-1");
                if let Some(offset) = &offset {
                    query.append_pair("s", offset);
                }
            }
            let response = self
                .client
                .get(url)
                .header(reqwest::header::REFERER, "https://duckduckgo.com/")
                .send()
                .await
                .context("DuckDuckGo news request failed")?;
            let body = Self::response_bytes(response).await?;
            let envelope: Value = serde_json::from_slice(&body)
                .context("DuckDuckGo news response was not valid JSON")?;
            let rows = envelope
                .get("results")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for row in rows {
                let row = normalize_news_row(&row)?;
                let url = row
                    .get("url")
                    .and_then(Value::as_str)
                    .expect("normalized news row always has a URL");
                if seen.insert(url.to_owned()) {
                    results.push(Value::Object(row));
                    if results.len() >= request.top_n {
                        return Ok(results);
                    }
                }
            }
            offset = envelope
                .get("next")
                .and_then(Value::as_str)
                .and_then(next_offset);
            if offset.is_none() {
                break;
            }
        }
        Ok(results)
    }

    /// Execute the fixed Go tool's distinct Instant Answer wire contract.
    pub async fn instant_answer(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<DuckDuckGoInstantAnswer> {
        if query.is_empty() {
            bail!("DuckDuckGo query is required");
        }
        let response = self
            .client
            .get(self.instant_endpoint.clone())
            .query(&[
                ("q", query),
                ("format", "json"),
                ("no_html", "1"),
                ("skip_disambig", "1"),
            ])
            .send()
            .await
            .context("DuckDuckGo Instant Answer request failed")?;
        let body = Self::response_bytes(response).await?;
        let envelope: Value = serde_json::from_slice(&body)
            .context("DuckDuckGo Instant Answer response was not valid JSON")?;
        let abstract_text = string_alias(&envelope, "abstract_text", "AbstractText")
            .filter(|value| !value.is_empty())
            .or_else(|| string_alias(&envelope, "abstract", "Abstract"))
            .unwrap_or_default()
            .to_owned();
        let abstract_url = string_alias(&envelope, "abstract_url", "AbstractURL")
            .unwrap_or_default()
            .to_owned();
        let topics = value_alias(&envelope, "related_topics", "RelatedTopics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut related_topics = Vec::new();
        flatten_topics(&topics, &mut related_topics);
        related_topics.truncate(if max_results == 0 { 5 } else { max_results });
        Ok(DuckDuckGoInstantAnswer {
            abstract_text,
            abstract_url,
            related_topics,
        })
    }
}

#[async_trait]
impl DuckDuckGoProvider for DuckDuckGoClient {
    async fn search(&self, request: &DuckDuckGoSearchRequest) -> Result<Vec<Value>> {
        if request.query.is_empty() {
            bail!("DuckDuckGo query is required");
        }
        if request.top_n == 0 {
            bail!("DuckDuckGo top_n must be a positive integer");
        }
        match request.channel {
            DuckDuckGoChannel::Text => self.text_search(request).await,
            DuckDuckGoChannel::News => self.news_search(request).await,
        }
    }
}

fn parse_text_results(html: &str, top_n: usize) -> Result<Vec<Value>> {
    let mut results = Vec::new();
    for captures in text_result_regex().captures_iter(html) {
        let href = normalize_url(&captures[1]);
        let title = normalize_html(&captures[2])?;
        let body = normalize_html(&captures[3])?;
        if href.is_empty() {
            continue;
        }
        results.push(serde_json::json!({
            "title": title,
            "href": href,
            "body": body
        }));
        if results.len() >= top_n {
            break;
        }
    }
    Ok(results)
}

fn text_result_regex() -> &'static Regex {
    static RESULT: OnceLock<Regex> = OnceLock::new();
    RESULT.get_or_init(|| {
        Regex::new(
            r#"(?is)<h2[^>]*class=[\"'][^\"']*result__title[^\"']*[\"'][^>]*>\s*<a[^>]*href=[\"']([^\"']+)[\"'][^>]*>(.*?)</a>.*?<a[^>]*class=[\"'][^\"']*result__snippet[^\"']*[\"'][^>]*>(.*?)</a>"#,
        )
        .expect("static DuckDuckGo result regex is valid")
    })
}

fn html_tag_regex() -> &'static Regex {
    static TAG: OnceLock<Regex> = OnceLock::new();
    TAG.get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("static HTML tag regex is valid"))
}

fn normalize_html(raw: &str) -> Result<String> {
    let stripped = html_tag_regex().replace_all(raw, "");
    Ok(quick_xml::escape::unescape(&stripped)
        .context("DuckDuckGo result contained an invalid HTML entity")?
        .into_owned())
}

fn normalize_url(raw: &str) -> String {
    let decoded = quick_xml::escape::unescape(raw)
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| raw.to_owned());
    let absolute = if decoded.starts_with("//") {
        format!("https:{decoded}")
    } else {
        decoded
    };
    if let Ok(url) = reqwest::Url::parse(&absolute)
        && url
            .host_str()
            .is_some_and(|host| host.ends_with("duckduckgo.com"))
        && let Some(target) = url
            .query_pairs()
            .find_map(|(key, value)| (key == "uddg").then(|| value.into_owned()))
    {
        return target.replace(' ', "+");
    }
    percent_decode(&absolute).replace(' ', "+")
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            output.push(high * 16 + low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn extract_vqd(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    for (prefix, suffix) in [("vqd=\"", '"'), ("vqd='", '\''), ("vqd=", '&')] {
        if let Some(rest) = text.split_once(prefix).map(|(_, rest)| rest)
            && let Some((value, _)) = rest.split_once(suffix)
        {
            return Some(value.to_owned());
        }
    }
    None
}

fn normalize_news_row(value: &Value) -> Result<Map<String, Value>> {
    let row = value
        .as_object()
        .ok_or_else(|| anyhow!("DuckDuckGo news result must be an object"))?;
    let required = |field: &str| {
        row.get(field)
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("DuckDuckGo news result {field} must be a string"))
    };
    let timestamp = row
        .get("date")
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_f64().map(|value| value as i64))
        })
        .ok_or_else(|| anyhow!("DuckDuckGo news result date must be a Unix timestamp"))?;
    let date = OffsetDateTime::from_unix_timestamp(timestamp)
        .context("DuckDuckGo news result date was out of range")?
        .format(&Rfc3339)
        .context("could not format DuckDuckGo news date")?;
    Ok(Map::from_iter([
        ("date".into(), Value::String(date)),
        ("title".into(), Value::String(required("title")?.to_owned())),
        (
            "body".into(),
            Value::String(normalize_html(required("excerpt")?)?),
        ),
        ("url".into(), Value::String(normalize_url(required("url")?))),
        (
            "image".into(),
            Value::String(
                row.get("image")
                    .and_then(Value::as_str)
                    .map(normalize_url)
                    .unwrap_or_default(),
            ),
        ),
        (
            "source".into(),
            Value::String(required("source")?.to_owned()),
        ),
    ]))
}

fn next_offset(next: &str) -> Option<String> {
    reqwest::Url::parse(&format!("https://duckduckgo.com{next}"))
        .ok()?
        .query_pairs()
        .find_map(|(key, value)| (key == "s").then(|| value.into_owned()))
}

fn string_alias<'a>(value: &'a Value, snake: &str, pascal: &str) -> Option<&'a str> {
    value_alias(value, snake, pascal).and_then(Value::as_str)
}

fn value_alias<'a>(value: &'a Value, snake: &str, pascal: &str) -> Option<&'a Value> {
    value.get(snake).or_else(|| value.get(pascal))
}

fn flatten_topics(values: &[Value], output: &mut Vec<DuckDuckGoTopic>) {
    for value in values {
        let text = string_alias(value, "text", "Text").unwrap_or_default();
        let url = string_alias(value, "first_url", "FirstURL").unwrap_or_default();
        if !text.is_empty() && !url.is_empty() {
            output.push(DuckDuckGoTopic {
                text: text.to_owned(),
                first_url: url.to_owned(),
            });
        }
        if let Some(children) = value_alias(value, "topics", "Topics").and_then(Value::as_array) {
            flatten_topics(children, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Form, Json, Router,
        extract::{Query, State},
        http::StatusCode,
        response::{Html, IntoResponse},
        routing::{get, post},
    };
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    type RecordedCalls = Vec<(String, BTreeMap<String, String>)>;

    #[derive(Clone, Default)]
    struct Recorded(Arc<Mutex<RecordedCalls>>);

    async fn text_handler(
        State(recorded): State<Recorded>,
        Form(form): Form<BTreeMap<String, String>>,
    ) -> Html<&'static str> {
        recorded.0.lock().unwrap().push(("text".into(), form));
        Html(
            r#"<div class="result results_links">
              <h2 class="result__title"><a href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.test%2Frust">Rust &amp; safety</a></h2>
              <a class="result__snippet">Memory <b>safety</b> without GC.</a>
            </div>"#,
        )
    }

    async fn home_handler(Query(query): Query<BTreeMap<String, String>>) -> Html<String> {
        Html(format!("vqd='token-for-{}'&x=1", query["q"]))
    }

    async fn news_handler(
        State(recorded): State<Recorded>,
        Query(query): Query<BTreeMap<String, String>>,
    ) -> Json<Value> {
        recorded.0.lock().unwrap().push(("news".into(), query));
        Json(serde_json::json!({
            "results": [{
                "date": 1_700_000_000,
                "title": "Rust news",
                "excerpt": "New <b>release</b>",
                "url": "https%3A%2F%2Fexample.test%2Fnews",
                "image": null,
                "source": "Example"
            }]
        }))
    }

    async fn instant_handler(Query(query): Query<BTreeMap<String, String>>) -> Json<Value> {
        assert_eq!(query["no_html"], "1");
        Json(serde_json::json!({
            "AbstractText": "Rust is a language.",
            "AbstractURL": "https://example.test/rust",
            "RelatedTopics": [{
                "Text": "Languages",
                "Topics": [
                    {"Text": "Rust", "FirstURL": "https://example.test/rust"},
                    {"Text": "Cargo", "FirstURL": "https://example.test/cargo"}
                ]
            }]
        }))
    }

    async fn server() -> (DuckDuckGoClient, Recorded, tokio::task::JoinHandle<()>) {
        let recorded = Recorded::default();
        let app = Router::new()
            .route("/text", post(text_handler))
            .route("/home", get(home_handler))
            .route("/news", get(news_handler))
            .route("/instant", get(instant_handler))
            .with_state(recorded.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let base = format!("http://{address}");
        (
            DuckDuckGoClient::new_with_endpoints(
                &format!("{base}/text"),
                &format!("{base}/home"),
                &format!("{base}/news"),
                &format!("{base}/instant"),
            )
            .unwrap(),
            recorded,
            handle,
        )
    }

    #[tokio::test]
    async fn text_and_news_follow_fixed_python_request_and_result_shapes() {
        let (client, recorded, server) = server().await;
        let text = client
            .search(&DuckDuckGoSearchRequest {
                query: "rust language".into(),
                channel: DuckDuckGoChannel::Text,
                top_n: 10,
            })
            .await
            .unwrap();
        assert_eq!(text.len(), 1);
        assert_eq!(text[0]["title"], "Rust & safety");
        assert_eq!(text[0]["href"], "https://example.test/rust");
        assert_eq!(text[0]["body"], "Memory safety without GC.");

        let news = client
            .search(&DuckDuckGoSearchRequest {
                query: "rust language".into(),
                channel: DuckDuckGoChannel::News,
                top_n: 10,
            })
            .await
            .unwrap();
        assert_eq!(news.len(), 1);
        assert_eq!(news[0]["body"], "New release");
        assert_eq!(news[0]["url"], "https://example.test/news");
        assert_eq!(news[0]["image"], "");
        assert!(news[0]["date"].as_str().unwrap().ends_with('Z'));

        let calls = recorded.0.lock().unwrap();
        assert_eq!(calls[0].0, "text");
        assert_eq!(calls[0].1["q"], "rust language");
        assert_eq!(calls[0].1["kl"], "wt-wt");
        assert_eq!(calls[1].0, "news");
        assert_eq!(calls[1].1["vqd"], "token-for-rust language");
        assert_eq!(calls[1].1["p"], "-1");
        server.abort();
    }

    #[tokio::test]
    async fn instant_answer_flattens_both_real_and_go_fixture_key_styles() {
        let (client, _, server) = server().await;
        let answer = client.instant_answer("rust", 1).await.unwrap();
        assert_eq!(answer.abstract_text, "Rust is a language.");
        assert_eq!(answer.related_topics.len(), 1);
        assert_eq!(answer.related_topics[0].text, "Rust");
        let default_limit = client.instant_answer("rust", 0).await.unwrap();
        assert_eq!(default_limit.related_topics.len(), 2);
        server.abort();
    }

    #[tokio::test]
    async fn status_body_limit_and_input_validation_fail_closed() {
        async fn bad(Query(query): Query<BTreeMap<String, String>>) -> impl IntoResponse {
            match query.get("q").map(String::as_str) {
                Some("status") => (StatusCode::TOO_MANY_REQUESTS, "limited".to_owned()),
                Some("large") => (StatusCode::OK, "x".repeat(MAX_DUCKDUCKGO_RESPONSE_BODY + 1)),
                _ => (StatusCode::OK, "not json".to_owned()),
            }
        }
        let app = Router::new().route("/", get(bad));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let endpoint = format!("http://{address}/");
        let client =
            DuckDuckGoClient::new_with_endpoints(&endpoint, &endpoint, &endpoint, &endpoint)
                .unwrap();
        for (query, expected) in [("status", "HTTP 429"), ("large", "exceeds")] {
            let error = client.instant_answer(query, 1).await.unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
        assert!(client.instant_answer("", 1).await.is_err());
        assert!(
            client
                .search(&DuckDuckGoSearchRequest {
                    query: "x".into(),
                    channel: DuckDuckGoChannel::Text,
                    top_n: 0,
                })
                .await
                .is_err()
        );
        server.abort();
    }

    #[tokio::test]
    #[ignore = "requires live access to DuckDuckGo search"]
    async fn live_public_text_search_returns_a_result() {
        let results = DuckDuckGoClient::default()
            .search(&DuckDuckGoSearchRequest {
                query: "Rust programming language".into(),
                channel: DuckDuckGoChannel::Text,
                top_n: 1,
            })
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0]["title"].as_str().unwrap().is_empty());
        assert!(!results[0]["body"].as_str().unwrap().is_empty());
    }
}
