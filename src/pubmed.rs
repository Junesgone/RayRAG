//! Fixed-endpoint NCBI E-utilities protocols for RAGFlow-compatible PubMed tools.
//!
//! RAGFlow's Python Canvas component and independent Go agent tool use different
//! NCBI response formats. [`PubMedProvider::search`] preserves the Python
//! `esearch` XML -> `efetch` XML path, while [`PubMedClient::search_go`] exposes
//! the Go `esearch` JSON -> `esummary` JSON result envelope.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::header::{ACCEPT, USER_AGENT};
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

const PUBMED_ESEARCH_ENDPOINT: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi";
const PUBMED_EFETCH_ENDPOINT: &str = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/efetch.fcgi";
const PUBMED_ESUMMARY_ENDPOINT: &str =
    "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi";
const PUBMED_PYTHON_TOOL: &str = "biopython";
const PUBMED_GO_USER_AGENT: &str = "ragflow/1.0";
const PUBMED_DEFAULT_EMAIL: &str = "A.N.Other@example.com";
const MAX_PUBMED_RESPONSE_BODY: usize = 16 << 20;
const PUBMED_UNKEYED_REQUEST_DELAY: Duration = Duration::from_millis(370);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PubMedSearchRequest {
    pub query: String,
    pub top_n: usize,
    pub email: String,
}

impl PubMedSearchRequest {
    pub fn with_defaults(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            top_n: 12,
            email: PUBMED_DEFAULT_EMAIL.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PubMedArticle {
    pub pmid: String,
    pub title: String,
    pub authors: Vec<String>,
    pub journal: String,
    pub volume: String,
    pub issue: String,
    pub pages: String,
    pub doi: Option<String>,
    pub abstract_text: String,
    pub publication_date: String,
}

impl PubMedArticle {
    pub fn url(&self) -> String {
        format!("https://pubmed.ncbi.nlm.nih.gov/{}", self.pmid)
    }

    pub fn formatted_content(&self) -> String {
        let title = nonempty_or(&self.title, "No title");
        let abstract_text = nonempty_or(&self.abstract_text, "No abstract available");
        let journal = nonempty_or(&self.journal, "Unknown Journal");
        let volume = nonempty_or(&self.volume, "-");
        let issue = nonempty_or(&self.issue, "-");
        let pages = nonempty_or(&self.pages, "-");
        let authors = if self.authors.is_empty() {
            "Unknown Authors".to_owned()
        } else {
            self.authors.join(", ")
        };
        format!(
            "Title: {title}\nAuthors: {authors}\nJournal: {journal}\nVolume: {volume}\nIssue: {issue}\nPages: {pages}\nDOI: {}\nAbstract: {}",
            self.doi.as_deref().unwrap_or("-"),
            abstract_text.trim()
        )
    }

    pub fn to_go_result(&self) -> PubMedGoResult {
        PubMedGoResult {
            pmid: self.pmid.clone(),
            title: self.title.trim().to_owned(),
            authors: join_go_author_names(&self.authors),
            journal: self.journal.clone(),
            year: first_four_digit_year(&self.publication_date),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PubMedGoResult {
    pub pmid: String,
    pub title: String,
    pub authors: String,
    pub journal: String,
    pub year: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PubMedGoEnvelope {
    pub results: Vec<PubMedGoResult>,
}

#[async_trait]
pub trait PubMedProvider: Send + Sync {
    async fn search(&self, request: &PubMedSearchRequest) -> Result<Vec<PubMedArticle>>;
}

#[derive(Debug, Clone)]
pub struct PubMedClient {
    client: reqwest::Client,
    esearch_endpoint: reqwest::Url,
    efetch_endpoint: reqwest::Url,
    esummary_endpoint: reqwest::Url,
    inter_request_delay: Duration,
}

impl Default for PubMedClient {
    fn default() -> Self {
        Self::new(
            PUBMED_ESEARCH_ENDPOINT,
            PUBMED_EFETCH_ENDPOINT,
            PUBMED_ESUMMARY_ENDPOINT,
            PUBMED_UNKEYED_REQUEST_DELAY,
        )
        .expect("fixed PubMed endpoints and HTTP client configuration are valid")
    }
}

impl PubMedClient {
    fn new(
        esearch_endpoint: &str,
        efetch_endpoint: &str,
        esummary_endpoint: &str,
        inter_request_delay: Duration,
    ) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(RedirectPolicy::none())
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .context("could not build PubMed HTTP client")?,
            esearch_endpoint: esearch_endpoint
                .parse()
                .context("invalid PubMed esearch endpoint")?,
            efetch_endpoint: efetch_endpoint
                .parse()
                .context("invalid PubMed efetch endpoint")?,
            esummary_endpoint: esummary_endpoint
                .parse()
                .context("invalid PubMed esummary endpoint")?,
            inter_request_delay,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_with_endpoints(
        esearch_endpoint: &str,
        efetch_endpoint: &str,
        esummary_endpoint: &str,
    ) -> Result<Self> {
        Self::new(
            esearch_endpoint,
            efetch_endpoint,
            esummary_endpoint,
            Duration::ZERO,
        )
    }

    async fn get_bounded(
        &self,
        url: reqwest::Url,
        accept: &'static str,
        user_agent: Option<&'static str>,
        operation: &'static str,
    ) -> Result<Vec<u8>> {
        let mut request = self.client.get(url).header(ACCEPT, accept);
        if let Some(user_agent) = user_agent {
            request = request.header(USER_AGENT, user_agent);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("PubMed {operation} request failed"))?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&body);
            bail!("PubMed {operation} returned HTTP {status}: {detail}");
        }
        Ok(body)
    }

    /// Execute the fixed Go tool's JSON request and result contract.
    pub async fn search_go(&self, query: &str, max_results: usize) -> Result<PubMedGoEnvelope> {
        if query.trim().is_empty() {
            bail!("pubmed: query is required");
        }
        let max_results = match max_results {
            0 => 5,
            value => value.min(100),
        };
        let mut search_url = self.esearch_endpoint.clone();
        search_url
            .query_pairs_mut()
            .append_pair("db", "pubmed")
            .append_pair("term", query)
            .append_pair("retmax", &max_results.to_string())
            .append_pair("retmode", "json");
        let search_body = self
            .get_bounded(
                search_url,
                "application/json",
                Some(PUBMED_GO_USER_AGENT),
                "esearch",
            )
            .await?;
        let search: GoESearchResponse =
            serde_json::from_slice(&search_body).context("pubmed: decode esearch response")?;
        let pmids = search.esearch_result.id_list;
        if pmids.is_empty() {
            return Ok(PubMedGoEnvelope {
                results: Vec::new(),
            });
        }

        let mut summary_url = self.esummary_endpoint.clone();
        summary_url
            .query_pairs_mut()
            .append_pair("db", "pubmed")
            .append_pair("id", &pmids.join(","))
            .append_pair("retmode", "json");
        let summary_body = self
            .get_bounded(
                summary_url,
                "application/json",
                Some(PUBMED_GO_USER_AGENT),
                "esummary",
            )
            .await?;
        let articles = decode_go_esummary(&summary_body)?;
        let results = pmids
            .into_iter()
            .filter_map(|pmid| {
                articles.get(&pmid).map(|article| PubMedGoResult {
                    pmid,
                    title: article.title.trim().to_owned(),
                    authors: join_go_summary_authors(&article.authors),
                    journal: article.full_journal_name.clone(),
                    year: first_four_digit_year(&article.pub_date),
                })
            })
            .collect();
        Ok(PubMedGoEnvelope { results })
    }
}

#[async_trait]
impl PubMedProvider for PubMedClient {
    async fn search(&self, request: &PubMedSearchRequest) -> Result<Vec<PubMedArticle>> {
        if request.query.is_empty() {
            bail!("PubMed query is required");
        }
        if request.top_n == 0 {
            bail!("PubMed top_n must be a positive integer");
        }

        let mut search_url = self.esearch_endpoint.clone();
        search_url
            .query_pairs_mut()
            .append_pair("db", "pubmed")
            .append_pair("retmax", &request.top_n.to_string())
            .append_pair("term", &request.query)
            .append_pair("tool", PUBMED_PYTHON_TOOL)
            .append_pair("email", &request.email);
        let search_body = self
            .get_bounded(search_url, "application/xml", None, "esearch")
            .await?;
        let pmids = parse_esearch_xml(&search_body)?;

        if !self.inter_request_delay.is_zero() {
            tokio::time::sleep(self.inter_request_delay).await;
        }
        let mut fetch_url = self.efetch_endpoint.clone();
        fetch_url
            .query_pairs_mut()
            .append_pair("db", "pubmed")
            .append_pair("id", &pmids.join(","))
            .append_pair("retmode", "xml")
            .append_pair("tool", PUBMED_PYTHON_TOOL)
            .append_pair("email", &request.email);
        let fetch_body = self
            .get_bounded(fetch_url, "application/xml", None, "efetch")
            .await?;
        parse_pubmed_articles(&fetch_body)
    }
}

#[derive(Debug, Default, Deserialize)]
struct GoESearchResponse {
    #[serde(default, rename = "esearchresult")]
    esearch_result: GoESearchResult,
}

#[derive(Debug, Default, Deserialize)]
struct GoESearchResult {
    #[serde(default, rename = "idlist")]
    id_list: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct GoESummaryResponse {
    #[serde(default)]
    result: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct GoESummaryArticle {
    #[serde(default)]
    title: String,
    #[serde(default)]
    authors: Vec<GoESummaryAuthor>,
    #[serde(default, rename = "fulljournalname")]
    full_journal_name: String,
    #[serde(default, rename = "pubdate")]
    pub_date: String,
}

#[derive(Debug, Default, Deserialize)]
struct GoESummaryAuthor {
    #[serde(default)]
    name: String,
}

fn decode_go_esummary(body: &[u8]) -> Result<HashMap<String, GoESummaryArticle>> {
    let raw: GoESummaryResponse =
        serde_json::from_slice(body).context("pubmed: parse esummary response")?;
    let mut articles = HashMap::with_capacity(raw.result.len());
    for (key, value) in raw.result {
        if value.is_array() {
            continue;
        }
        let Ok(article) = serde_json::from_value(value) else {
            continue;
        };
        articles.insert(key, article);
    }
    Ok(articles)
}

fn parse_esearch_xml(body: &[u8]) -> Result<Vec<String>> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(false);
    let mut path = Vec::new();
    let mut ids = Vec::new();
    let mut current_id = String::new();
    let mut saw_root = false;
    let mut closed_root = false;

    loop {
        match reader.read_event().context("pubmed: decode esearch XML")? {
            Event::Start(event) => {
                let name = String::from_utf8_lossy(event.local_name().as_ref()).into_owned();
                path.push(name);
                if path.len() == 1 && path[0] == "eSearchResult" {
                    saw_root = true;
                }
                if path_ends_with(&path, &["eSearchResult", "IdList", "Id"]) {
                    current_id.clear();
                }
            }
            Event::Text(event) if path_ends_with(&path, &["eSearchResult", "IdList", "Id"]) => {
                current_id.push_str(
                    event
                        .unescape()
                        .context("pubmed: unescape esearch PMID")?
                        .as_ref(),
                );
            }
            Event::CData(event) if path_ends_with(&path, &["eSearchResult", "IdList", "Id"]) => {
                current_id.push_str(&String::from_utf8_lossy(event.as_ref()));
            }
            Event::End(event) => {
                let name = event.local_name();
                if name.as_ref() == b"Id"
                    && path_ends_with(&path, &["eSearchResult", "IdList", "Id"])
                {
                    let id = current_id.trim();
                    if !id.is_empty() {
                        ids.push(id.to_owned());
                    }
                    current_id.clear();
                } else if name.as_ref() == b"eSearchResult" && path.len() == 1 {
                    closed_root = true;
                }
                path.pop();
            }
            Event::Eof => {
                if !saw_root || !closed_root {
                    bail!("pubmed: decode esearch XML: missing or unclosed eSearchResult root");
                }
                break;
            }
            _ => {}
        }
    }
    Ok(ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PubMedField {
    Pmid,
    Title,
    Abstract,
    Journal,
    Volume,
    Issue,
    Pages,
    Doi,
    AuthorLastName,
    AuthorForeName,
    PublicationYear,
    PublicationMedlineDate,
}

#[derive(Debug, Default)]
struct PubMedArticleBuilder {
    pmid: String,
    title: String,
    authors: Vec<String>,
    journal: String,
    volume: String,
    issue: String,
    pages: String,
    doi: Option<String>,
    abstract_text: String,
    publication_year: String,
    publication_medline_date: String,
    abstract_seen: bool,
    author_last_name: String,
    author_fore_name: String,
}

impl PubMedArticleBuilder {
    fn finish_author(&mut self) {
        let fullname = format!(
            "{} {}",
            self.author_fore_name.trim(),
            self.author_last_name.trim()
        )
        .trim()
        .to_owned();
        if !fullname.is_empty() {
            self.authors.push(fullname);
        }
        self.author_last_name.clear();
        self.author_fore_name.clear();
    }

    fn finish(self) -> Result<PubMedArticle> {
        if self.pmid.is_empty() {
            bail!("pubmed: PubmedArticle is missing MedlineCitation/PMID");
        }
        if self.title.is_empty() {
            bail!("pubmed: PubmedArticle is missing Article/ArticleTitle text");
        }
        let publication_date = if self.publication_year.is_empty() {
            self.publication_medline_date
        } else {
            self.publication_year
        };
        Ok(PubMedArticle {
            pmid: self.pmid,
            title: self.title,
            authors: self.authors,
            journal: self.journal,
            volume: self.volume,
            issue: self.issue,
            pages: self.pages,
            doi: self.doi.filter(|doi| !doi.is_empty()),
            abstract_text: self.abstract_text,
            publication_date,
        })
    }
}

fn parse_pubmed_articles(body: &[u8]) -> Result<Vec<PubMedArticle>> {
    let xml = std::str::from_utf8(body).context("pubmed: efetch XML was not UTF-8")?;
    let xml = xml
        .replace("<b>", "")
        .replace("</b>", "")
        .replace("<i>", "")
        .replace("</i>", "");
    let mut reader = Reader::from_str(&xml);
    reader.config_mut().trim_text(false);
    let mut path = Vec::new();
    let mut articles = Vec::new();
    let mut article: Option<PubMedArticleBuilder> = None;
    let mut field = None;
    let mut saw_root = false;
    let mut closed_root = false;

    loop {
        match reader.read_event().context("pubmed: decode efetch XML")? {
            Event::Start(event) => {
                let name = String::from_utf8_lossy(event.local_name().as_ref()).into_owned();
                path.push(name);
                if path.len() == 1 && path[0] == "PubmedArticleSet" {
                    saw_root = true;
                } else if path.len() == 2
                    && path[0] == "PubmedArticleSet"
                    && path[1] == "PubmedArticle"
                {
                    article = Some(PubMedArticleBuilder::default());
                }
                let Some(builder) = article.as_mut() else {
                    continue;
                };
                if path_ends_with(&path, &["MedlineCitation", "PMID"]) {
                    field = Some(PubMedField::Pmid);
                } else if path_ends_with(&path, &["Article", "ArticleTitle"]) {
                    field = Some(PubMedField::Title);
                } else if path_ends_with(&path, &["Article", "Abstract", "AbstractText"])
                    && !builder.abstract_seen
                {
                    field = Some(PubMedField::Abstract);
                } else if path_ends_with(&path, &["Article", "Journal", "Title"]) {
                    field = Some(PubMedField::Journal);
                } else if path_ends_with(&path, &["Journal", "JournalIssue", "Volume"]) {
                    field = Some(PubMedField::Volume);
                } else if path_ends_with(&path, &["Journal", "JournalIssue", "Issue"]) {
                    field = Some(PubMedField::Issue);
                } else if path_ends_with(&path, &["Article", "Pagination", "MedlinePgn"]) {
                    field = Some(PubMedField::Pages);
                } else if path_ends_with(&path, &["AuthorList", "Author"]) {
                    builder.author_last_name.clear();
                    builder.author_fore_name.clear();
                } else if path_ends_with(&path, &["AuthorList", "Author", "LastName"]) {
                    field = Some(PubMedField::AuthorLastName);
                } else if path_ends_with(&path, &["AuthorList", "Author", "ForeName"]) {
                    field = Some(PubMedField::AuthorForeName);
                } else if path_ends_with(&path, &["PubDate", "Year"]) {
                    field = Some(PubMedField::PublicationYear);
                } else if path_ends_with(&path, &["PubDate", "MedlineDate"]) {
                    field = Some(PubMedField::PublicationMedlineDate);
                } else if path.last().is_some_and(|name| name == "ArticleId")
                    && builder.doi.is_none()
                    && article_id_is_doi(&event, reader.decoder())?
                {
                    builder.doi = Some(String::new());
                    field = Some(PubMedField::Doi);
                }
            }
            Event::Text(event) => {
                let value = event.unescape().context("pubmed: unescape efetch text")?;
                append_pubmed_text(article.as_mut(), field, value.as_ref());
            }
            Event::CData(event) => {
                let value = String::from_utf8_lossy(event.as_ref());
                append_pubmed_text(article.as_mut(), field, value.as_ref());
            }
            Event::End(event) => {
                let name = event.local_name();
                if name.as_ref() == b"AbstractText"
                    && field == Some(PubMedField::Abstract)
                    && let Some(builder) = article.as_mut()
                {
                    builder.abstract_seen = true;
                }
                if name.as_ref() == b"Author"
                    && path_ends_with(&path, &["AuthorList", "Author"])
                    && let Some(builder) = article.as_mut()
                {
                    builder.finish_author();
                }
                if name.as_ref() == b"PubmedArticle"
                    && path.len() == 2
                    && let Some(builder) = article.take()
                {
                    articles.push(builder.finish()?);
                }
                if name.as_ref() == b"PubmedArticleSet" && path.len() == 1 {
                    closed_root = true;
                }
                if field.is_some_and(|field| field_end_name(field) == name.as_ref()) {
                    field = None;
                }
                path.pop();
            }
            Event::Eof => {
                if article.is_some() {
                    bail!("pubmed: decode efetch XML: unexpected EOF inside PubmedArticle");
                }
                if !saw_root || !closed_root {
                    bail!("pubmed: decode efetch XML: missing or unclosed PubmedArticleSet root");
                }
                break;
            }
            _ => {}
        }
    }
    Ok(articles)
}

fn append_pubmed_text(
    article: Option<&mut PubMedArticleBuilder>,
    field: Option<PubMedField>,
    value: &str,
) {
    let Some(article) = article else {
        return;
    };
    match field {
        Some(PubMedField::Pmid) => article.pmid.push_str(value),
        Some(PubMedField::Title) => article.title.push_str(value),
        Some(PubMedField::Abstract) => article.abstract_text.push_str(value),
        Some(PubMedField::Journal) => article.journal.push_str(value),
        Some(PubMedField::Volume) => article.volume.push_str(value),
        Some(PubMedField::Issue) => article.issue.push_str(value),
        Some(PubMedField::Pages) => article.pages.push_str(value),
        Some(PubMedField::Doi) => article.doi.get_or_insert_with(String::new).push_str(value),
        Some(PubMedField::AuthorLastName) => article.author_last_name.push_str(value),
        Some(PubMedField::AuthorForeName) => article.author_fore_name.push_str(value),
        Some(PubMedField::PublicationYear) => article.publication_year.push_str(value),
        Some(PubMedField::PublicationMedlineDate) => {
            article.publication_medline_date.push_str(value);
        }
        None => {}
    }
}

fn article_id_is_doi(
    event: &quick_xml::events::BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
) -> Result<bool> {
    for attribute in event.attributes() {
        let attribute = attribute.context("pubmed: decode ArticleId attribute")?;
        if attribute.key.local_name().as_ref() == b"IdType" {
            let value = attribute
                .decode_and_unescape_value(decoder)
                .context("pubmed: unescape ArticleId attribute")?;
            return Ok(value == "doi");
        }
    }
    Ok(false)
}

fn field_end_name(field: PubMedField) -> &'static [u8] {
    match field {
        PubMedField::Pmid => b"PMID",
        PubMedField::Title => b"ArticleTitle",
        PubMedField::Abstract => b"AbstractText",
        PubMedField::Journal => b"Title",
        PubMedField::Volume => b"Volume",
        PubMedField::Issue => b"Issue",
        PubMedField::Pages => b"MedlinePgn",
        PubMedField::Doi => b"ArticleId",
        PubMedField::AuthorLastName => b"LastName",
        PubMedField::AuthorForeName => b"ForeName",
        PubMedField::PublicationYear => b"Year",
        PubMedField::PublicationMedlineDate => b"MedlineDate",
    }
}

fn path_ends_with(path: &[String], suffix: &[&str]) -> bool {
    path.len() >= suffix.len()
        && path[path.len() - suffix.len()..]
            .iter()
            .map(String::as_str)
            .eq(suffix.iter().copied())
}

fn nonempty_or<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() { fallback } else { value }
}

fn join_go_summary_authors(authors: &[GoESummaryAuthor]) -> String {
    join_go_author_names(
        &authors
            .iter()
            .map(|author| author.name.clone())
            .collect::<Vec<_>>(),
    )
}

fn join_go_author_names(authors: &[String]) -> String {
    const CAP: usize = 3;
    if authors.len() <= CAP {
        return authors.join(", ");
    }
    let mut names = authors[..CAP].to_vec();
    names.push("et al.".into());
    names.join(", ")
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

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("could not read PubMed response body")?;
        let remaining = MAX_PUBMED_RESPONSE_BODY.saturating_sub(body.len());
        if chunk.len() > remaining {
            bail!(
                "PubMed response body exceeds {} bytes",
                MAX_PUBMED_RESPONSE_BODY
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
    use axum::http::{HeaderMap, Uri};
    use axum::response::IntoResponse;
    use axum::routing::get;
    use std::sync::{Arc, Mutex};

    const ESEARCH_XML: &str = r#"<?xml version="1.0"?>
<eSearchResult><IdList><Id>11111</Id><Id>22222</Id></IdList></eSearchResult>"#;
    const EFETCH_XML: &str = r#"<?xml version="1.0"?>
<PubmedArticleSet>
  <PubmedArticle>
    <MedlineCitation>
      <PMID>11111</PMID>
      <Article>
        <ArticleTitle>Deep <b>retrieval</b> &amp; generation</ArticleTitle>
        <Abstract><AbstractText>  A <i>short</i> abstract.  </AbstractText></Abstract>
        <Journal><Title>Nature Machine Intelligence</Title><JournalIssue><PubDate><Year>2020</Year></PubDate><Volume>10</Volume><Issue>2</Issue></JournalIssue></Journal>
        <Pagination><MedlinePgn>101-110</MedlinePgn></Pagination>
        <AuthorList>
          <Author><LastName>Khan</LastName><ForeName>Furqan</ForeName></Author>
          <Author><LastName>Smith</LastName><ForeName>Jane</ForeName></Author>
        </AuthorList>
      </Article>
    </MedlineCitation>
    <PubmedData><ArticleIdList><ArticleId IdType="pubmed">11111</ArticleId><ArticleId IdType="doi">10.1000/example.doi</ArticleId></ArticleIdList></PubmedData>
  </PubmedArticle>
  <PubmedArticle>
    <MedlineCitation><PMID>22222</PMID><Article><ArticleTitle>No author paper</ArticleTitle></Article></MedlineCitation>
  </PubmedArticle>
</PubmedArticleSet>"#;
    const ESEARCH_JSON: &str = r#"{"esearchresult":{"idlist":["11111","22222"]}}"#;
    const ESUMMARY_JSON: &str = r#"{
      "result": {
        "uids": ["11111", "22222"],
        "11111": {"title":"  Cochrane review of masks  ","authors":[{"name":"Smith J"},{"name":"Doe A"}],"fulljournalname":"Cochrane Database Syst Rev","pubdate":"2020 Nov 1"},
        "22222": {"title":"Vaccine efficacy","authors":[{"name":"Alice"},{"name":"Bob"},{"name":"Carol"},{"name":"Dave"}],"fulljournalname":"Lancet","pubdate":"2021 Mar-Apr"}
      }
    }"#;

    #[derive(Clone, Default)]
    struct Recorded(Arc<Mutex<Vec<(Uri, HeaderMap)>>>);

    async fn python_handler(
        State(recorded): State<Recorded>,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        recorded.0.lock().unwrap().push((uri.clone(), headers));
        if uri.path().ends_with("esearch.fcgi") {
            (
                [(&axum::http::header::CONTENT_TYPE, "application/xml")],
                ESEARCH_XML,
            )
        } else {
            (
                [(&axum::http::header::CONTENT_TYPE, "application/xml")],
                EFETCH_XML,
            )
        }
    }

    async fn go_handler(
        State(recorded): State<Recorded>,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        recorded.0.lock().unwrap().push((uri.clone(), headers));
        if uri.path().ends_with("esearch.fcgi") {
            (
                [(&axum::http::header::CONTENT_TYPE, "application/json")],
                ESEARCH_JSON,
            )
        } else {
            (
                [(&axum::http::header::CONTENT_TYPE, "application/json")],
                ESUMMARY_JSON,
            )
        }
    }

    async fn empty_go_handler(
        State(recorded): State<Recorded>,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        recorded.0.lock().unwrap().push((uri, headers));
        (
            [(&axum::http::header::CONTENT_TYPE, "application/json")],
            r#"{"esearchresult":{"idlist":[]}}"#,
        )
    }

    async fn server(
        handler: axum::routing::MethodRouter<Recorded>,
    ) -> (String, Recorded, tokio::task::JoinHandle<()>) {
        let recorded = Recorded::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/esearch.fcgi", handler.clone())
            .route("/efetch.fcgi", handler.clone())
            .route("/esummary.fcgi", handler)
            .with_state(recorded.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (base, recorded, handle)
    }

    #[tokio::test]
    async fn python_contract_sends_email_and_formats_structured_xml_articles() {
        let (base, recorded, handle) = server(get(python_handler)).await;
        let client = PubMedClient::new_with_endpoints(
            &format!("{base}/esearch.fcgi"),
            &format!("{base}/efetch.fcgi"),
            &format!("{base}/esummary.fcgi"),
        )
        .unwrap();
        let articles = client
            .search(&PubMedSearchRequest {
                query: "retrieval AND generation".into(),
                top_n: 12,
                email: "reader@example.test".into(),
            })
            .await
            .unwrap();
        handle.abort();

        assert_eq!(articles.len(), 2);
        assert_eq!(articles[0].title, "Deep retrieval & generation");
        assert_eq!(articles[0].authors, ["Furqan Khan", "Jane Smith"]);
        assert_eq!(articles[0].doi.as_deref(), Some("10.1000/example.doi"));
        assert_eq!(articles[0].publication_date, "2020");
        assert_eq!(articles[0].url(), "https://pubmed.ncbi.nlm.nih.gov/11111");
        assert_eq!(
            articles[0].formatted_content(),
            "Title: Deep retrieval & generation\nAuthors: Furqan Khan, Jane Smith\nJournal: Nature Machine Intelligence\nVolume: 10\nIssue: 2\nPages: 101-110\nDOI: 10.1000/example.doi\nAbstract: A short abstract."
        );
        assert!(
            articles[1]
                .formatted_content()
                .contains("Authors: Unknown Authors")
        );

        let requests = recorded.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let search: HashMap<_, _> =
            reqwest::Url::parse(&format!("http://localhost{}", requests[0].0))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
        assert_eq!(search["term"], "retrieval AND generation");
        assert_eq!(search["retmax"], "12");
        assert_eq!(search["tool"], "biopython");
        assert_eq!(search["email"], "reader@example.test");
        assert!(!search.contains_key("retmode"));
        let fetch: HashMap<_, _> =
            reqwest::Url::parse(&format!("http://localhost{}", requests[1].0))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
        assert_eq!(fetch["id"], "11111,22222");
        assert_eq!(fetch["retmode"], "xml");
    }

    #[tokio::test]
    async fn go_contract_defaults_clamps_and_preserves_pmid_order() {
        let (base, recorded, handle) = server(get(go_handler)).await;
        let client = PubMedClient::new_with_endpoints(
            &format!("{base}/esearch.fcgi"),
            &format!("{base}/efetch.fcgi"),
            &format!("{base}/esummary.fcgi"),
        )
        .unwrap();
        let envelope = client.search_go("covid vaccine", 999).await.unwrap();
        handle.abort();

        assert_eq!(envelope.results.len(), 2);
        assert_eq!(envelope.results[0].pmid, "11111");
        assert_eq!(envelope.results[0].title, "Cochrane review of masks");
        assert_eq!(envelope.results[0].authors, "Smith J, Doe A");
        assert_eq!(envelope.results[0].year, "2020");
        assert_eq!(envelope.results[1].authors, "Alice, Bob, Carol, et al.");

        let requests = recorded.0.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let search: HashMap<_, _> =
            reqwest::Url::parse(&format!("http://localhost{}", requests[0].0))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
        assert_eq!(search["retmax"], "100");
        assert_eq!(search["retmode"], "json");
        assert_eq!(requests[0].1[USER_AGENT], PUBMED_GO_USER_AGENT);
        assert_eq!(requests[0].1[ACCEPT], "application/json");
        let summary: HashMap<_, _> =
            reqwest::Url::parse(&format!("http://localhost{}", requests[1].0))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
        assert_eq!(summary["id"], "11111,22222");
    }

    #[tokio::test]
    async fn go_contract_defaults_five_and_skips_esummary_for_empty_ids() {
        let (base, recorded, handle) = server(get(empty_go_handler)).await;
        let client = PubMedClient::new_with_endpoints(
            &format!("{base}/esearch.fcgi"),
            &format!("{base}/efetch.fcgi"),
            &format!("{base}/esummary.fcgi"),
        )
        .unwrap();
        let envelope = client.search_go("no-results", 0).await.unwrap();
        handle.abort();

        assert!(envelope.results.is_empty());
        let requests = recorded.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let search: HashMap<_, _> =
            reqwest::Url::parse(&format!("http://localhost{}", requests[0].0))
                .unwrap()
                .query_pairs()
                .into_owned()
                .collect();
        assert_eq!(search["retmax"], "5");
    }

    #[test]
    fn malformed_xml_and_go_helpers_fail_closed() {
        assert!(parse_esearch_xml(b"<eSearchResult>").is_err());
        assert!(parse_pubmed_articles(b"<PubmedArticleSet><PubmedArticle>").is_err());
        assert!(decode_go_esummary(b"not-json").is_err());
        assert_eq!(first_four_digit_year("Spring 1899 / 2026"), "2026");
        assert_eq!(join_go_author_names(&[]), "");
    }

    #[tokio::test]
    #[ignore = "requires live access to the public NCBI E-utilities API"]
    async fn live_pubmed_search() {
        let articles = PubMedClient::default()
            .search(&PubMedSearchRequest::with_defaults("cancer"))
            .await
            .unwrap();
        assert!(!articles.is_empty());
        assert!(!articles[0].pmid.is_empty());
    }
}
