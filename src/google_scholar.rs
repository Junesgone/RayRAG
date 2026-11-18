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


