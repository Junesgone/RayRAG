//! Fixed-endpoint arXiv Atom protocol for RAGFlow-compatible Canvas tools.
//!
//! The Python Canvas component and the independent Go agent tool expose
//! different request contracts. [`ArxivProvider::search`] follows the Python
//! path (raw query plus sort criterion), while [`ArxivClient::search_go`]
//! preserves the Go `all:<query>` envelope without allowing Canvas data to
//! select an arbitrary outbound URL.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use futures_util::StreamExt;
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};

const ARXIV_PYTHON_ENDPOINT: &str = "https://export.arxiv.org/api/query";
const ARXIV_GO_ENDPOINT: &str = "http://export.arxiv.org/api/query";
const ARXIV_USER_AGENT: &str = "arxiv.py/2.1.3";
const ARXIV_PYTHON_PAGE_SIZE: usize = 100;
const MAX_ARXIV_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArxivSortBy {
    SubmittedDate,
    LastUpdatedDate,
    Relevance,
}

impl ArxivSortBy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SubmittedDate => "submittedDate",
            Self::LastUpdatedDate => "lastUpdatedDate",
            Self::Relevance => "relevance",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArxivSearchRequest {
    pub query: String,
    pub top_n: usize,
    pub sort_by: ArxivSortBy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArxivPaper {
    pub title: String,
    pub authors: Vec<String>,
    pub summary: String,
    pub pdf_url: Option<String>,
    pub entry_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArxivGoResult {
    pub title: String,
    pub authors: Vec<String>,
    pub summary: String,
    pub pdf_url: String,
    pub entry_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArxivGoEnvelope {
    pub results: Vec<ArxivGoResult>,
}

impl ArxivPaper {
    pub fn to_go_result(&self) -> ArxivGoResult {
        ArxivGoResult {
            title: normalize_whitespace(&self.title),
            authors: self.authors.clone(),
            summary: normalize_whitespace(&self.summary),
            pdf_url: self
                .pdf_url
                .clone()
                .unwrap_or_else(|| derive_arxiv_pdf_url(&self.entry_id)),
            entry_id: self.entry_id.clone(),
        }
    }
}

#[async_trait]
pub trait ArxivProvider: Send + Sync {
    async fn search(&self, request: &ArxivSearchRequest) -> Result<Vec<ArxivPaper>>;
}

#[derive(Debug, Clone)]
pub struct ArxivClient {
    client: reqwest::Client,
    python_endpoint: reqwest::Url,
    go_endpoint: reqwest::Url,
}

impl Default for ArxivClient {
    fn default() -> Self {
        Self::new(ARXIV_PYTHON_ENDPOINT, ARXIV_GO_ENDPOINT)
            .expect("fixed arXiv endpoints and HTTP client configuration are valid")
    }
}

impl ArxivClient {
    fn new(python_endpoint: &str, go_endpoint: &str) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(RedirectPolicy::none())
                .timeout(crate::common::cmd_timeout::duration())
                .user_agent(ARXIV_USER_AGENT)
                .build()
                .context("could not build arXiv HTTP client")?,
            python_endpoint: python_endpoint
                .parse()
                .context("invalid arXiv Python endpoint")?,
            go_endpoint: go_endpoint.parse().context("invalid arXiv Go endpoint")?,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_with_endpoints(python_endpoint: &str, go_endpoint: &str) -> Result<Self> {
        Self::new(python_endpoint, go_endpoint)
    }

    async fn get_feed(&self, url: reqwest::Url) -> Result<ParsedArxivFeed> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("arXiv request failed")?;
        let status = response.status();
        let body = read_bounded_body(response).await?;
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&body);
            bail!("arXiv upstream returned HTTP {status}: {detail}");
        }
        parse_arxiv_atom(&body)
    }

    /// Execute the fixed Go tool's distinct public request and result envelope.
    pub async fn search_go(&self, query: &str, max_results: usize) -> Result<ArxivGoEnvelope> {
        if query.is_empty() {
            bail!("arxiv: query is required");
        }
        let max_results = if max_results == 0 { 5 } else { max_results };
        let mut url = self.go_endpoint.clone();
        url.query_pairs_mut()
            .append_pair("search_query", &format!("all:{query}"))
            .append_pair("max_results", &max_results.to_string());
        let feed = self.get_feed(url).await?;
        let results = feed
            .papers
            .into_iter()
            .take(max_results)
            .map(|paper| paper.to_go_result())
            .collect();
        Ok(ArxivGoEnvelope { results })
    }
}

#[async_trait]
impl ArxivProvider for ArxivClient {
    async fn search(&self, request: &ArxivSearchRequest) -> Result<Vec<ArxivPaper>> {
        if request.query.is_empty() {
            bail!("arXiv query is required");
        }
        if request.top_n == 0 {
            bail!("arXiv top_n must be a positive integer");
        }

        let mut papers = Vec::with_capacity(request.top_n.min(ARXIV_PYTHON_PAGE_SIZE));
        let mut start = 0usize;
        loop {
            let mut url = self.python_endpoint.clone();
            url.query_pairs_mut()
                .append_pair("search_query", &request.query)
                .append_pair("id_list", "")
                .append_pair("sortBy", request.sort_by.as_str())
                .append_pair("sortOrder", "descending")
                .append_pair("start", &start.to_string())
                .append_pair("max_results", &ARXIV_PYTHON_PAGE_SIZE.to_string());
            let feed = self.get_feed(url).await?;
            let entry_count = feed.entry_count;
            let total_results = feed.total_results;
            papers.extend(feed.papers);
            if papers.len() >= request.top_n {
                papers.truncate(request.top_n);
                break;
            }
            if entry_count == 0 {
                break;
            }
            start = start.saturating_add(entry_count);
            if total_results.is_some_and(|total| start >= total) {
                break;
            }
        }
        Ok(papers)
    }
}

#[derive(Debug, Default)]
struct ParsedArxivFeed {
    papers: Vec<ArxivPaper>,
    entry_count: usize,
    total_results: Option<usize>,
}

#[derive(Debug, Default)]
struct ArxivEntryBuilder {
    title: String,
    authors: Vec<String>,
    summary: String,
    entry_id: String,
    pdf_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AtomField {
    Title,
    Summary,
    EntryId,
    AuthorName,
    TotalResults,
}

fn parse_arxiv_atom(body: &[u8]) -> Result<ParsedArxivFeed> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(false);
    let mut feed = ParsedArxivFeed::default();
    let mut entry: Option<ArxivEntryBuilder> = None;
    let mut field = None;
    let mut author_name = String::new();
    let mut total_results = String::new();
    let mut saw_feed = false;
    let mut closed_feed = false;

    loop {
        match reader.read_event().context("arxiv: decode atom XML")? {
            Event::Start(event) => match event.local_name().as_ref() {
                b"feed" if entry.is_none() => saw_feed = true,
                b"entry" => {
                    feed.entry_count = feed.entry_count.saturating_add(1);
                    entry = Some(ArxivEntryBuilder::default());
                }
                b"title" if entry.is_some() => field = Some(AtomField::Title),
                b"summary" if entry.is_some() => field = Some(AtomField::Summary),
                b"id" if entry.is_some() => field = Some(AtomField::EntryId),
                b"name" if entry.is_some() => {
                    author_name.clear();
                    field = Some(AtomField::AuthorName);
                }
                b"totalResults" if entry.is_none() => {
                    total_results.clear();
                    field = Some(AtomField::TotalResults);
                }
                b"link" if entry.is_some() => {
                    record_pdf_link(
                        entry.as_mut().expect("entry presence checked"),
                        &event,
                        reader.decoder(),
                    )?;
                }
                _ => {}
            },
            Event::Empty(event) if event.local_name().as_ref() == b"link" && entry.is_some() => {
                record_pdf_link(
                    entry.as_mut().expect("entry presence checked"),
                    &event,
                    reader.decoder(),
                )?;
            }
            Event::Text(event) => {
                let value = event.unescape().context("arxiv: unescape Atom text")?;
                append_atom_text(
                    field,
                    value.as_ref(),
                    entry.as_mut(),
                    &mut author_name,
                    &mut total_results,
                );
            }
            Event::CData(event) => {
                let value = String::from_utf8_lossy(event.as_ref());
                append_atom_text(
                    field,
                    value.as_ref(),
                    entry.as_mut(),
                    &mut author_name,
                    &mut total_results,
                );
            }
            Event::End(event) => match event.local_name().as_ref() {
                b"feed" if entry.is_none() => closed_feed = true,
                b"title" | b"summary" | b"id" => field = None,
                b"name" => {
                    if let Some(entry) = entry.as_mut() {
                        let name = author_name.trim();
                        if !name.is_empty() {
                            entry.authors.push(name.to_owned());
                        }
                    }
                    author_name.clear();
                    field = None;
                }
                b"totalResults" => {
                    feed.total_results = total_results.trim().parse().ok();
                    total_results.clear();
                    field = None;
                }
                b"entry" => {
                    if let Some(entry) = entry.take()
                        && !entry.entry_id.trim().is_empty()
                    {
                        feed.papers.push(ArxivPaper {
                            title: if entry.title.trim().is_empty() {
                                "0".into()
                            } else {
                                normalize_whitespace(&entry.title)
                            },
                            authors: entry.authors,
                            summary: entry.summary.trim().to_owned(),
                            pdf_url: entry.pdf_url,
                            entry_id: entry.entry_id.trim().to_owned(),
                        });
                    }
                    field = None;
                }
                _ => {}
            },
            Event::Eof => {
                if entry.is_some() {
                    bail!("arxiv: decode atom XML: unexpected EOF inside entry");
                }
                if !saw_feed || !closed_feed {
                    bail!("arxiv: decode atom XML: missing or unclosed feed root");
                }
                break;
            }
            _ => {}
        }
    }
    Ok(feed)
}

fn append_atom_text(
    field: Option<AtomField>,
    value: &str,
    entry: Option<&mut ArxivEntryBuilder>,
    author_name: &mut String,
    total_results: &mut String,
) {
    match (field, entry) {
        (Some(AtomField::Title), Some(entry)) => entry.title.push_str(value),
        (Some(AtomField::Summary), Some(entry)) => entry.summary.push_str(value),
        (Some(AtomField::EntryId), Some(entry)) => entry.entry_id.push_str(value),
        (Some(AtomField::AuthorName), Some(_)) => author_name.push_str(value),
        (Some(AtomField::TotalResults), _) => total_results.push_str(value),
        _ => {}
    }
}

fn record_pdf_link(
    entry: &mut ArxivEntryBuilder,
    event: &BytesStart<'_>,
    decoder: quick_xml::encoding::Decoder,
) -> Result<()> {
    let mut href = None;
    let mut rel = None;
    let mut content_type = None;
    let mut title = None;
    for attribute in event.attributes() {
        let attribute = attribute.context("arxiv: decode link attribute")?;
        let value = attribute
            .decode_and_unescape_value(decoder)
            .context("arxiv: unescape link attribute")?
            .into_owned();
        match attribute.key.local_name().as_ref() {
            b"href" => href = Some(value),
            b"rel" => rel = Some(value),
            b"type" => content_type = Some(value),
            b"title" => title = Some(value),
            _ => {}
        }
    }
    if entry.pdf_url.is_none()
        && (title.as_deref() == Some("pdf")
            || (rel.as_deref() == Some("related")
                && content_type.as_deref() == Some("application/pdf")))
    {
        entry.pdf_url = href.filter(|href| !href.is_empty());
    }
    Ok(())
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn derive_arxiv_pdf_url(entry_id: &str) -> String {
    if entry_id.is_empty() {
        return String::new();
    }
    let Ok(mut url) = reqwest::Url::parse(entry_id) else {
        return entry_id.to_owned();
    };
    let path = url.path();
    let path = if let Some(index) = path.rfind("abs") {
        format!("{}pdf{}", &path[..index], &path[index + "abs".len()..])
    } else {
        format!("/pdf{path}")
    };
    url.set_path(&path);
    url.to_string()
}

async fn read_bounded_body(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("could not read arXiv response body")?;
        let remaining = MAX_ARXIV_RESPONSE_BODY.saturating_sub(body.len());
        if chunk.len() > remaining {
            bail!(
                "arXiv response body exceeds {} bytes",
                MAX_ARXIV_RESPONSE_BODY
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
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use std::sync::{Arc, Mutex};

    const FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <opensearch:totalResults>2</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/2501.12345v1</id>
    <title>Retrieval &amp; Generation
      Systems</title>
    <summary>  We present a method
      combining retrieval and generation.  </summary>
    <author><name>Alice Liddell</name></author>
    <author><name>Bob Builder</name></author>
    <link href="http://arxiv.org/pdf/2501.12345v1" rel="related" type="application/pdf" title="pdf"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2409.99999v2</id>
    <title><![CDATA[Single-Author Paper]]></title>
    <summary>Brief.</summary>
    <author><name>Carol Danvers</name></author>
  </entry>
</feed>"#;

    #[derive(Clone, Default)]
    struct Recorded(Arc<Mutex<Vec<(Uri, HeaderMap)>>>);

    async fn feed_handler(
        State(recorded): State<Recorded>,
        uri: Uri,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        recorded.0.lock().unwrap().push((uri, headers));
        (
            [(&axum::http::header::CONTENT_TYPE, "application/atom+xml")],
            FEED,
        )
    }

    async fn status_handler() -> impl IntoResponse {
        (StatusCode::BAD_GATEWAY, "upstream unavailable")
    }

    async fn large_handler() -> Response {
        Response::new(Body::from(vec![b'x'; MAX_ARXIV_RESPONSE_BODY + 1]))
    }

    async fn server() -> (ArxivClient, Recorded, tokio::task::JoinHandle<()>) {
        let recorded = Recorded::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/api/query", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/api/query", get(feed_handler))
            .with_state(recorded.clone());
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (
            ArxivClient::new_with_endpoints(&endpoint, &endpoint).unwrap(),
            recorded,
            handle,
        )
    }

    #[tokio::test]
    async fn python_contract_preserves_query_sort_and_sdk_page_size() {
        let (client, recorded, handle) = server().await;
        let papers = client
            .search(&ArxivSearchRequest {
                query: "ti:retrieval AND au:Lewis".into(),
                top_n: 2,
                sort_by: ArxivSortBy::LastUpdatedDate,
            })
            .await
            .unwrap();
        handle.abort();

        assert_eq!(papers.len(), 2);
        assert_eq!(papers[0].title, "Retrieval & Generation Systems");
        assert_eq!(papers[0].authors, ["Alice Liddell", "Bob Builder"]);
        assert_eq!(
            papers[0].pdf_url.as_deref(),
            Some("http://arxiv.org/pdf/2501.12345v1")
        );
        assert_eq!(papers[1].title, "Single-Author Paper");
        assert_eq!(papers[1].pdf_url, None);

        let requests = recorded.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request_url =
            reqwest::Url::parse(&format!("http://localhost{}", requests[0].0)).unwrap();
        let query: std::collections::HashMap<_, _> =
            request_url.query_pairs().into_owned().collect();
        assert_eq!(query["search_query"], "ti:retrieval AND au:Lewis");
        assert_eq!(query["sortBy"], "lastUpdatedDate");
        assert_eq!(query["sortOrder"], "descending");
        assert_eq!(query["start"], "0");
        assert_eq!(query["max_results"], "100");
        assert_eq!(requests[0].1[reqwest::header::USER_AGENT], ARXIV_USER_AGENT);
    }

    #[tokio::test]
    async fn go_contract_prefixes_all_defaults_five_and_derives_pdf_url() {
        let (client, recorded, handle) = server().await;
        let envelope = client.search_go("transformer", 0).await.unwrap();
        handle.abort();

        assert_eq!(envelope.results.len(), 2);
        assert_eq!(
            envelope.results[1].pdf_url,
            "http://arxiv.org/pdf/2409.99999v2"
        );
        assert_eq!(
            envelope.results[0].summary,
            "We present a method combining retrieval and generation."
        );
        let requests = recorded.0.lock().unwrap();
        let query = requests[0].0.query().unwrap();
        assert!(query.contains("search_query=all%3Atransformer"));
        assert!(query.contains("max_results=5"));
        assert!(!query.contains("sortBy"));
    }

    #[tokio::test]
    async fn http_status_and_body_limit_fail_closed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/status", get(status_handler))
            .route("/large", get(large_handler));
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client =
            ArxivClient::new_with_endpoints(&format!("{base}/status"), &format!("{base}/large"))
                .unwrap();

        let status = client
            .search(&ArxivSearchRequest {
                query: "rust".into(),
                top_n: 1,
                sort_by: ArxivSortBy::Relevance,
            })
            .await
            .unwrap_err();
        assert!(status.to_string().contains("502 Bad Gateway"));

        let large = client.search_go("rust", 1).await.unwrap_err();
        assert!(large.to_string().contains("exceeds 16777216 bytes"));
        handle.abort();
    }

    #[test]
    fn malformed_xml_fails_and_pdf_fallback_matches_go_path() {
        let error = parse_arxiv_atom(b"<feed><entry>").unwrap_err();
        assert!(error.to_string().contains("decode atom XML"));
        let error = parse_arxiv_atom(b"<feed>").unwrap_err();
        assert!(error.to_string().contains("unclosed feed root"));
        assert_eq!(
            derive_arxiv_pdf_url("http://arxiv.org/abs/2501.12345v1"),
            "http://arxiv.org/pdf/2501.12345v1"
        );
        assert_eq!(derive_arxiv_pdf_url("not a url"), "not a url");
    }

    #[tokio::test]
    #[ignore = "requires live access to the public arXiv API"]
    async fn live_arxiv_search() {
        let papers = ArxivClient::default()
            .search(&ArxivSearchRequest {
                query: "rust programming language".into(),
                top_n: 1,
                sort_by: ArxivSortBy::Relevance,
            })
            .await
            .unwrap();
        assert!(!papers.is_empty());
        assert!(!papers[0].entry_id.is_empty());
    }
}
