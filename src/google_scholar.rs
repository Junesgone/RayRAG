//! Fixed-domain Google Scholar HTML protocol for RAGFlow-compatible tools.
//!
//! RAGFlow's Python Canvas component delegates to `scholarly==1.7.11`, while
//! the independent Go tool sends a smaller one-page request and exposes a
//! different result envelope. [`GoogleScholarProvider::search`] preserves the
//! Python-facing filters, pagination and core publication fields. The
//! [`GoogleScholarClient::search_go`] entry point preserves the Go model
//! contract without allowing Canvas data to choose an arbitrary outbound URL.

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, USER_AGENT};
use reqwest::redirect::Policy as RedirectPolicy;
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::HashSet;

const GOOGLE_SCHOLAR_ENDPOINT: &str = "https://scholar.google.com/scholar";
const SCHOLARLY_USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/80.0.3987.149 Safari/537.36";
const GOOGLE_SCHOLAR_GO_USER_AGENT: &str = "Mozilla/5.0 (compatible; ragflow/1.0)";
const SCHOLARLY_ACCEPT: &str = "text/html,application/xhtml+xml,application/xml";
const GOOGLE_SCHOLAR_GO_ACCEPT: &str = "text/html,application/xhtml+xml";
const MAX_GOOGLE_SCHOLAR_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleScholarSortBy {
    Relevance,
    Date,
}

impl GoogleScholarSortBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Date => "date",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoogleScholarSearchRequest {
    pub query: String,
    pub top_n: usize,
    pub sort_by: GoogleScholarSortBy,
    pub year_low: Option<i32>,
    pub year_high: Option<i32>,
    pub patents: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleScholarPublication {
    pub title: String,
    pub authors: Vec<String>,
    pub abstract_text: Option<String>,
    pub pub_url: String,
    pub venue: String,
    pub year: String,
    pub gsrank: usize,
}

impl GoogleScholarPublication {
    pub fn formatted_content(&self) -> String {
        format!(
            "\n author: {}\n Abstract: {}",
            self.authors.join(","),
            self.abstract_text.as_deref().unwrap_or("no abstract")
        )
    }

    /// Core `scholarly` publication shape written by the Python component.
    pub fn to_python_json(&self) -> Value {
        let mut bib = Map::from_iter([
            ("title".into(), Value::String(self.title.clone())),
            ("author".into(), serde_json::json!(self.authors)),
            ("pub_year".into(), Value::String(self.year.clone())),
            ("venue".into(), Value::String(self.venue.clone())),
        ]);
        if let Some(abstract_text) = &self.abstract_text {
            bib.insert("abstract".into(), Value::String(abstract_text.clone()));
        }
        Value::Object(Map::from_iter([
            ("container_type".into(), Value::String("Publication".into())),
            (
                "source".into(),
                Value::String("PUBLICATION_SEARCH_SNIPPET".into()),
            ),
            ("bib".into(), Value::Object(bib)),
            ("filled".into(), Value::Bool(false)),
            ("gsrank".into(), serde_json::json!(self.gsrank)),
            ("pub_url".into(), Value::String(self.pub_url.clone())),
        ]))
    }

    pub fn to_go_result(&self) -> GoogleScholarGoResult {
        GoogleScholarGoResult {
            title: self.title.trim().to_owned(),
            link: self.pub_url.clone(),
            snippet: self.abstract_text.clone().unwrap_or_default(),
            authors: self.authors.join(", "),
            year: self.year.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleScholarGoResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
    pub authors: String,
    pub year: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleScholarGoEnvelope {
    pub results: Vec<GoogleScholarGoResult>,
}

#[async_trait]
pub trait GoogleScholarProvider: Send + Sync {
    async fn search(
        &self,
        request: &GoogleScholarSearchRequest,
    ) -> Result<Vec<GoogleScholarPublication>>;
}

#[derive(Debug, Clone)]
pub struct GoogleScholarClient {
    client: reqwest::Client,
    python_endpoint: reqwest::Url,
    go_endpoint: reqwest::Url,
}

impl Default for GoogleScholarClient {
    fn default() -> Self {
        Self::new(GOOGLE_SCHOLAR_ENDPOINT, GOOGLE_SCHOLAR_ENDPOINT)
            .expect("fixed Google Scholar endpoints and HTTP client configuration are valid")
    }
}

impl GoogleScholarClient {
    fn new(python_endpoint: &str, go_endpoint: &str) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(RedirectPolicy::none())
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .context("could not build Google Scholar HTTP client")?,
            python_endpoint: python_endpoint
                .parse()
                .context("invalid Google Scholar Python endpoint")?,
            go_endpoint: go_endpoint
                .parse()
                .context("invalid Google Scholar Go endpoint")?,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_with_endpoints(python_endpoint: &str, go_endpoint: &str) -> Result<Self> {
        Self::new(python_endpoint, go_endpoint)
    }

    async fn get_html(
        &self,
        url: reqwest::Url,
        user_agent: &'static str,
        accept: &'static str,
        python_captcha_check: bool,
    ) -> Result<String> {
        let mut request = self
            .client
            .get(url)
            .header(USER_AGENT, user_agent)
            .header(ACCEPT, accept);
        if python_captcha_check {
            request = request.header(ACCEPT_LANGUAGE, "en-US,en");
        }
        let response = request
            .send()
            .await
            .context("Google Scholar request failed")?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        if !status.is_success() {
            bail!("Google Scholar upstream returned HTTP {status}");
        }
        let body = String::from_utf8_lossy(&body).replace('\u{a0}', " ");
        if python_captcha_check {
            check_scholarly_captcha(&body)?;
        }
        Ok(body)
    }

    /// Execute the independent Go tool's one-page request and result envelope.
    pub async fn search_go(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<GoogleScholarGoEnvelope> {
        if query.trim().is_empty() {
            bail!("google_scholar: query is required");
        }
        let parse_limit = if max_results == 0 { 5 } else { max_results };
        let request_limit = parse_limit.min(20);
        let mut url = self.go_endpoint.clone();
        url.set_query(None);
        url.query_pairs_mut()
            .append_pair("hl", "en")
            .append_pair("num", &request_limit.to_string())
            .append_pair("q", query);
        let body = self
            .get_html(
                url,
                GOOGLE_SCHOLAR_GO_USER_AGENT,
                GOOGLE_SCHOLAR_GO_ACCEPT,
                false,
            )
            .await?;
        Ok(GoogleScholarGoEnvelope {
            results: parse_google_scholar_go_html(&body, parse_limit)?,
        })
    }
}

#[async_trait]
impl GoogleScholarProvider for GoogleScholarClient {
    async fn search(
        &self,
        request: &GoogleScholarSearchRequest,
    ) -> Result<Vec<GoogleScholarPublication>> {
        if request.query.is_empty() {
            bail!("Google Scholar query is required");
        }
        if request.top_n == 0 {
            bail!("Google Scholar top_n must be a positive integer");
        }

        let mut results = Vec::with_capacity(request.top_n.min(20));
        let mut start = 0usize;
        let mut visited = HashSet::new();
        loop {
            if !visited.insert(start) {
                bail!("Google Scholar pagination repeated start={start}");
            }
            let url = build_scholarly_url(&self.python_endpoint, request, start);
            let body = self
                .get_html(url, SCHOLARLY_USER_AGENT, SCHOLARLY_ACCEPT, true)
                .await?;
            let page = parse_scholarly_html(&body, start)?;
            results.extend(page.publications);
            if results.len() >= request.top_n {
                results.truncate(request.top_n);
                break;
            }
            let Some(next_start) = page.next_start else {
                break;
            };
            start = next_start;
        }
        Ok(results)
    }
}

fn build_scholarly_url(
    endpoint: &reqwest::Url,
    request: &GoogleScholarSearchRequest,
    start: usize,
) -> reqwest::Url {
    let mut url = endpoint.clone();
    url.set_query(None);
    let mut query = url.query_pairs_mut();
    query
        .append_pair("hl", "en")
        .append_pair("q", &request.query);
    if let Some(year_low) = request.year_low {
        query.append_pair("as_ylo", &year_low.to_string());
    }
    if let Some(year_high) = request.year_high {
        query.append_pair("as_yhi", &year_high.to_string());
    }
    query
        .append_pair("as_vis", "0")
        .append_pair("as_sdt", if request.patents { "0,33" } else { "1,33" });
    if request.sort_by == GoogleScholarSortBy::Date {
        query.append_pair("scisbd", "1");
    }
    if start > 0 {
        query.append_pair("start", &start.to_string());
    }
    drop(query);
    url
}

#[derive(Debug, Default)]
struct ParsedScholarlyPage {
    publications: Vec<GoogleScholarPublication>,
    next_start: Option<usize>,
}

fn parse_scholarly_html(body: &str, fallback_start: usize) -> Result<ParsedScholarlyPage> {
    let document = Html::parse_document(body);
    let card_selector = selector("div.gs_r.gs_or.gs_scl");
    let result_selector = selector("div.gs_ri");
    let title_selector = selector("h3.gs_rt");
    let anchor_selector = selector("a");
    let authors_selector = selector("div.gs_a");
    let abstract_selector = selector("div.gs_rs");
    let mut publications = Vec::new();

    for (position, card) in document.select(&card_selector).enumerate() {
        let result = card
            .select(&result_selector)
            .next()
            .ok_or_else(|| anyhow!("scholarly: result card is missing gs_ri"))?;
        let title = result
            .select(&title_selector)
            .next()
            .ok_or_else(|| anyhow!("scholarly: result card is missing gs_rt"))?;
        let anchor = title
            .select(&anchor_selector)
            .next()
            .ok_or_else(|| anyhow!("scholarly: publication is missing pub_url"))?;
        let pub_url = anchor
            .value()
            .attr("href")
            .ok_or_else(|| anyhow!("scholarly: publication link is missing href"))?
            .to_owned();
        let title = element_text(anchor).trim().to_owned();
        let author_info = result
            .select(&authors_selector)
            .next()
            .map(element_text)
            .ok_or_else(|| anyhow!("scholarly: publication is missing author metadata"))?;
        let authors = scholarly_authors(&author_info);
        let (venue, year) = scholarly_venue_year(&author_info);
        let abstract_text = result
            .select(&abstract_selector)
            .next()
            .map(element_text)
            .map(normalize_scholarly_abstract);
        let gsrank = card
            .value()
            .attr("data-rp")
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value.saturating_add(1))
            .unwrap_or_else(|| fallback_start.saturating_add(position).saturating_add(1));
        publications.push(GoogleScholarPublication {
            title,
            authors,
            abstract_text,
            pub_url,
            venue,
            year,
            gsrank,
        });
    }

    Ok(ParsedScholarlyPage {
        publications,
        next_start: scholar_next_start(&document),
    })
}

fn parse_google_scholar_go_html(
    body: &str,
    max_results: usize,
) -> Result<Vec<GoogleScholarGoResult>> {
    let document = Html::parse_document(body);
    let card_selector = selector(".gs_ri");
    let title_anchor_selector = selector(".gs_rt a");
    let authors_selector = selector(".gs_a");
    let snippet_selector = selector(".gs_rs");
    let mut results = Vec::new();
    for card in document.select(&card_selector) {
        if results.len() >= max_results {
            break;
        }
        let Some(anchor) = card.select(&title_anchor_selector).next() else {
            continue;
        };
        let title = element_text(anchor).trim().to_owned();
        if title.is_empty() {
            continue;
        }
        let link = anchor.value().attr("href").unwrap_or_default().to_owned();
        let (authors, year) = card
            .select(&authors_selector)
            .next()
            .map(element_text)
            .map(|line| split_scholar_authors_year(&line))
            .unwrap_or_default();
        let snippet = card
            .select(&snippet_selector)
            .next()
            .map(element_text)
            .unwrap_or_default()
            .trim()
            .to_owned();
        results.push(GoogleScholarGoResult {
            title,
            link,
            snippet,
            authors,
            year,
        });
    }
    Ok(results)
}

fn scholar_next_start(document: &Html) -> Option<usize> {
    let anchor_selector = selector("a");
    let next_selector = selector(".gs_ico.gs_ico_nav_next");
    let base = reqwest::Url::parse("https://scholar.google.com").ok()?;
    document.select(&anchor_selector).find_map(|anchor| {
        anchor.select(&next_selector).next()?;
        let href = anchor.value().attr("href")?;
        base.join(href)
            .ok()?
            .query_pairs()
            .find_map(|(key, value)| (key == "start").then(|| value.parse::<usize>().ok()))?
    })
}

fn scholarly_authors(line: &str) -> Vec<String> {
    line.split(" - ")
        .next()
        .unwrap_or_default()
        .split(',')
        .filter_map(|author| {
            let author = author.trim();
            if author.chars().any(|character| character.is_ascii_digit())
                || [
                    "Proceedings",
                    "Conference",
                    "Journal",
                    "(",
                    ")",
                    "[",
                    "]",
                    "Transactions",
                ]
                .iter()
                .any(|blocked| author.contains(blocked))
            {
                return None;
            }
            Some(author.replace('…', ""))
        })
        .collect()
}

fn scholarly_venue_year(line: &str) -> (String, String) {
    let parts: Vec<_> = line.split(" - ").collect();
    if parts.len() <= 2 {
        return ("NA".into(), "NA".into());
    }
    let venue_year: Vec<_> = parts[1].split(',').collect();
    let candidate = venue_year.last().copied().unwrap_or_default().trim();
    if candidate.len() == 4
        && candidate
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        let venue = if venue_year.len() >= 2 {
            venue_year[..venue_year.len() - 1].join(",")
        } else {
            "NA".into()
        };
        (venue, candidate.into())
    } else {
        (venue_year.join(","), "NA".into())
    }
}

fn normalize_scholarly_abstract(value: String) -> String {
    let value = value.replace('…', "").replace('\n', " ");
    let value = value.trim();
    if value
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("abstract"))
    {
        value.get(9..).unwrap_or_default().trim().to_owned()
    } else {
        value.to_owned()
    }
}

fn split_scholar_authors_year(line: &str) -> (String, String) {
    let cleaned = line.trim();
    if let Some((authors, venue)) = cleaned.split_once(" - ") {
        return (
            authors.trim().to_owned(),
            first_four_digit_year(venue.trim()),
        );
    }
    (cleaned.to_owned(), first_four_digit_year(cleaned))
}

fn first_four_digit_year(value: &str) -> String {
    value
        .as_bytes()
        .windows(4)
        .find_map(|candidate| {
            std::str::from_utf8(candidate)
                .ok()?
                .parse::<u16>()
                .ok()
                .filter(|year| (1900..=2099).contains(year))
                .map(|_| String::from_utf8_lossy(candidate).into_owned())
        })
        .unwrap_or_default()
}

fn element_text(element: ElementRef<'_>) -> String {
    element.text().collect()
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static Google Scholar selector is valid")
}

fn check_scholarly_captcha(body: &str) -> Result<()> {
    if body.contains("rc-doscaptcha-body") {
        bail!("scholarly: Google Scholar returned a denial-of-service CAPTCHA");
    }
    if ["gs_captcha_ccl", "recaptcha", "captcha-form"]
        .iter()
        .any(|marker| body.contains(marker))
    {
        bail!("scholarly: Google Scholar returned a CAPTCHA");
    }
    Ok(())
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("could not read Google Scholar response body")?;
        let remaining = MAX_GOOGLE_SCHOLAR_RESPONSE_BODY.saturating_sub(body.len());
        if chunk.len() > remaining {
            bail!(
                "Google Scholar response body exceeds {} bytes",
                MAX_GOOGLE_SCHOLAR_RESPONSE_BODY
            );
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, Uri};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use std::sync::{Arc, Mutex};

    const PYTHON_PAGE_ONE: &str = r#"<html><body>
<div class="gs_r gs_or gs_scl" data-cid="one" data-rp="0"><div class="gs_ri">
  <h3 class="gs_rt"><span class="gs_ctc">[PDF]</span><a href="https://example.com/paper1">Rust &amp; Retrieval</a></h3>
  <div class="gs_a">Alice, Proceedings Team, Bob - Systems, Search, 2024 - example.org</div>
  <div class="gs_rs">Abstract: A useful study…
with entities.</div>
</div></div>
<a href="/scholar?start=10&amp;q=rust"><span class="gs_ico gs_ico_nav_next"></span></a>
</body></html>"#;

    const PYTHON_PAGE_TWO: &str = r#"<html><body>
<div class="gs_r gs_or gs_scl" data-cid="two" data-rp="10"><div class="gs_ri">
  <h3 class="gs_rt"><a href="https://example.com/paper2">Second Paper</a></h3>
  <div class="gs_a">Carol - 2021</div>
</div></div>
</body></html>"#;

    const GO_PAGE: &str = r#"<html><body>
<div class="gs_ri"><h3 class="gs_rt"><a href="https://example.com/paper1">Attention is all you need</a></h3>
<div class="gs_a">Ashish Vaswani, Noam Shazeer - NeurIPS 2017</div>
<div class="gs_rs">The dominant sequence transduction models are recurrent.</div></div>
<div class="gs_ri"><h3 class="gs_rt"><a href="https://example.com/paper2">BERT</a></h3>
<div class="gs_a">Jacob Devlin - NAACL 2019</div><div class="gs_rs">Language representations.</div></div>
</body></html>"#;

    #[derive(Clone, Default)]
    struct Recorded(Arc<Mutex<Vec<(Uri, HeaderMap)>>>);

    async fn scholar_handler(
        State(recorded): State<Recorded>,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        recorded.0.lock().unwrap().push((uri.clone(), headers));
        if uri.query().is_some_and(|query| query.contains("start=10")) {
            PYTHON_PAGE_TWO
        } else if uri.query().is_some_and(|query| query.contains("num=")) {
            GO_PAGE
        } else {
            PYTHON_PAGE_ONE
        }
    }

    async fn captcha_handler() -> impl IntoResponse {
        r#"<html><form id="captcha-form"></form></html>"#
    }

    async fn status_handler() -> impl IntoResponse {
        (StatusCode::TOO_MANY_REQUESTS, "slow down")
    }

    async fn server() -> (GoogleScholarClient, Recorded, tokio::task::JoinHandle<()>) {
        let recorded = Recorded::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/scholar", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/scholar", get(scholar_handler))
            .route("/captcha", get(captcha_handler))
            .route("/status", get(status_handler))
            .with_state(recorded.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            GoogleScholarClient::new_with_endpoints(&endpoint, &endpoint).unwrap(),
            recorded,
            handle,
        )
    }

    #[tokio::test]
    async fn python_contract_filters_paginates_and_emits_core_scholarly_shape() {
        let (client, recorded, handle) = server().await;
        let publications = client
            .search(&GoogleScholarSearchRequest {
                query: "rust retrieval".into(),
                top_n: 2,
                sort_by: GoogleScholarSortBy::Date,
                year_low: Some(2020),
                year_high: Some(2025),
                patents: false,
            })
            .await
            .unwrap();
        handle.abort();

        assert_eq!(publications.len(), 2);
        assert_eq!(publications[0].title, "Rust & Retrieval");
        assert_eq!(publications[0].authors, ["Alice", "Bob"]);
        assert_eq!(publications[0].venue, "Systems, Search");
        assert_eq!(publications[0].year, "2024");
        assert_eq!(
            publications[0].abstract_text.as_deref(),
            Some("A useful study with entities.")
        );
        assert_eq!(publications[1].year, "NA");
        assert_eq!(publications[1].gsrank, 11);
        let raw = publications[0].to_python_json();
        assert_eq!(raw["source"], "PUBLICATION_SEARCH_SNIPPET");
        assert_eq!(raw["bib"]["author"], serde_json::json!(["Alice", "Bob"]));

        let requests = recorded.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let first = reqwest::Url::parse(&format!("http://localhost{}", requests[0].0)).unwrap();
        let query: std::collections::HashMap<_, _> = first.query_pairs().into_owned().collect();
        assert_eq!(query["q"], "rust retrieval");
        assert_eq!(query["as_ylo"], "2020");
        assert_eq!(query["as_yhi"], "2025");
        assert_eq!(query["as_vis"], "0");
        assert_eq!(query["as_sdt"], "1,33");
        assert_eq!(query["scisbd"], "1");
        assert!(!query.contains_key("start"));
        assert!(requests[1].0.query().unwrap().contains("start=10"));
        assert_eq!(requests[0].1[USER_AGENT], SCHOLARLY_USER_AGENT);
        assert_eq!(requests[0].1[ACCEPT_LANGUAGE], "en-US,en");
    }

    #[tokio::test]
    async fn go_contract_defaults_five_clamps_request_and_parses_result_cards() {
        let (client, recorded, handle) = server().await;
        let envelope = client.search_go("a b", 0).await.unwrap();
        let high = client.search_go("rust", 99).await.unwrap();
        handle.abort();

        assert_eq!(envelope.results.len(), 2);
        assert_eq!(envelope.results[0].year, "2017");
        assert_eq!(envelope.results[0].authors, "Ashish Vaswani, Noam Shazeer");
        assert!(
            envelope.results[0]
                .snippet
                .contains("sequence transduction")
        );
        assert_eq!(high.results.len(), 2);
        let requests = recorded.0.lock().unwrap();
        assert!(requests[0].0.query().unwrap().contains("num=5"));
        assert!(requests[1].0.query().unwrap().contains("num=20"));
        assert_eq!(requests[0].1[USER_AGENT], GOOGLE_SCHOLAR_GO_USER_AGENT);
        assert_eq!(requests[0].1[ACCEPT], GOOGLE_SCHOLAR_GO_ACCEPT);
    }

    #[tokio::test]
    async fn empty_query_captcha_and_http_status_fail_closed() {
        let (client, _, handle) = server().await;
        let empty = client.search_go("  ", 5).await.unwrap_err();
        assert!(empty.to_string().contains("query is required"));

        let base = client.python_endpoint.clone();
        let captcha = GoogleScholarClient::new_with_endpoints(
            base.join("captcha").unwrap().as_str(),
            base.as_str(),
        )
        .unwrap()
        .search(&GoogleScholarSearchRequest {
            query: "rust".into(),
            top_n: 1,
            sort_by: GoogleScholarSortBy::Relevance,
            year_low: None,
            year_high: None,
            patents: true,
        })
        .await
        .unwrap_err();
        assert!(captcha.to_string().contains("CAPTCHA"));

        let status = GoogleScholarClient::new_with_endpoints(
            base.join("status").unwrap().as_str(),
            base.as_str(),
        )
        .unwrap()
        .search(&GoogleScholarSearchRequest {
            query: "rust".into(),
            top_n: 1,
            sort_by: GoogleScholarSortBy::Relevance,
            year_low: None,
            year_high: None,
            patents: true,
        })
        .await
        .unwrap_err();
        assert!(status.to_string().contains("429 Too Many Requests"));
        handle.abort();
    }

    #[test]
    fn parser_preserves_python_and_go_missing_field_differences() {
        let go = parse_google_scholar_go_html(
            r#"<div class="gs_ri"><h3 class="gs_rt">No link</h3></div>"#,
            5,
        )
        .unwrap();
        assert!(go.is_empty());

        let python = parse_scholarly_html(
            r#"<div class="gs_r gs_or gs_scl"><div class="gs_ri"><h3 class="gs_rt">No link</h3><div class="gs_a">A - 2020</div></div></div>"#,
            0,
        )
        .unwrap_err();
        assert!(python.to_string().contains("pub_url"));
        assert_eq!(first_four_digit_year("x 1899 2026"), "2026");
    }

    #[tokio::test]
    #[ignore = "requires live access to Google Scholar and may encounter anti-bot controls"]
    async fn live_google_scholar_search() {
        let publications = GoogleScholarClient::default()
            .search(&GoogleScholarSearchRequest {
                query: "rust programming language".into(),
                top_n: 1,
                sort_by: GoogleScholarSortBy::Relevance,
                year_low: None,
                year_high: None,
                patents: true,
            })
            .await
            .unwrap();
        assert!(!publications.is_empty());
        assert!(!publications[0].title.is_empty());
    }
}
