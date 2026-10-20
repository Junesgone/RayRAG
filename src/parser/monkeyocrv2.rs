//! MonkeyOCRv2 remote PDF parser — RAGFlow v0.27.2
//! `deepdoc/parser/monkeyocrv2_parser.py`.
//!
//! MonkeyOCRv2 is a self-hosted service: `GET /health` reports readiness and
//! `POST /parse` answers with a ZIP archive that carries one native layout
//! JSON per document root plus extracted image artifacts. This module ports
//! the upstream client contract — 512 MiB response / 10 000 ZIP members /
//! 2 GiB uncompressed safety limits, basename-keyed image lookup, root or
//! `jsons/` record discovery with the `all_results.json` fallback — and
//! normalizes the layout records into RayRAG's marker-aware document content,
//! reusing the same `@@page\tleft\tright\ttop\tbottom##` position tags that
//! the SoMark and MinerU parsers emit.
//!
//! Bounded divergence: upstream hands the ZIP's image binaries to the chunker
//! as PIL objects; RayRAG renders `<!--IMAGE_START-->` blocks (alt text +
//! position tag) and lets the existing figure/vision stage consume them.

use crate::Result;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::io::{Cursor, Read};
use std::time::Duration;

/// Upstream default request timeout (1800 seconds).
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
/// Upstream health-probe timeout (10 seconds).
const HEALTH_TIMEOUT_SECS: u64 = 10;
/// Upstream response cap: 512 MiB compressed archive.
pub const MAX_RESPONSE_BYTES: usize = 512 * 1024 * 1024;
/// Upstream ZIP member cap.
pub const MAX_ZIP_MEMBERS: usize = 10_000;
/// Upstream uncompressed size cap: 2 GiB.
pub const MAX_UNCOMPRESSED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const IMAGE_EXTENSIONS: [&str; 4] = [".png", ".jpg", ".jpeg", ".webp"];

/// RAGFlow-compatible MonkeyOCRv2 service configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonkeyOcrV2Config {
    server_url: String,
    request_timeout: Duration,
}

impl MonkeyOcrV2Config {
    pub fn new(server_url: &str) -> Result<Self> {
        let server_url = server_url.trim().trim_end_matches('/').to_owned();
        if server_url.is_empty() {
            anyhow::bail!("MonkeyOCRv2 requires a server URL (MONKEYOCRV2_SERVER_URL)");
        }
        if !server_url.starts_with("http://") && !server_url.starts_with("https://") {
            anyhow::bail!("MONKEYOCRV2_SERVER_URL must start with http:// or https://");
        }
        reqwest::Url::parse(&server_url)
            .map_err(|error| anyhow::anyhow!("Invalid MonkeyOCRv2 server URL: {error}"))?;
        Ok(Self {
            server_url,
            request_timeout: Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        })
    }

    pub fn with_request_timeout(mut self, seconds: u64) -> Self {
        if seconds > 0 {
            self.request_timeout = Duration::from_secs(seconds);
        }
        self
    }

    pub fn server_url(&self) -> &str {
        &self.server_url
    }

    pub fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    /// Resolve the service URL from `MONKEYOCRV2_SERVER_URL` (upstream env).
    pub fn from_env() -> Result<Option<Self>> {
        let url = std::env::var("MONKEYOCRV2_SERVER_URL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        match url {
            Some(url) => Ok(Some(Self::new(&url)?)),
            None => Ok(None),
        }
    }
}

/// MonkeyOCRv2 result normalized for RayRAG's marker-aware parser pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonkeyOcrV2Output {
    pub content: String,
    pub structured: bool,
    pub block_count: usize,
    pub page_count: usize,
}

/// Async MonkeyOCRv2 HTTP client.
#[derive(Clone)]
pub struct MonkeyOcrV2Client {
    config: MonkeyOcrV2Config,
    client: reqwest::Client,
}

impl MonkeyOcrV2Client {
    pub fn new(config: MonkeyOcrV2Config) -> Result<Self> {
        Ok(Self {
            config,
            client: crate::common::cmd_timeout::model_client(),
        })
    }

    pub fn from_env() -> Result<Option<Self>> {
        MonkeyOcrV2Config::from_env()?.map(Self::new).transpose()
    }

    pub fn config(&self) -> &MonkeyOcrV2Config {
        &self.config
    }

    /// Upstream `check_installation()`: `GET /health` with a 10 second
    /// timeout; unreachable or non-success reports `false` instead of raising.
    pub async fn check_installation(&self) -> Result<bool> {
        match self
            .client
            .get(format!("{}/health", self.config.server_url))
            .timeout(Duration::from_secs(HEALTH_TIMEOUT_SECS))
            .send()
            .await
        {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    /// Upload one document and convert the service's ZIP response.
    /// `from_page`/`to_page` mirror upstream `start_page_id`/`end_page_id`
    /// (0 / 99999 by default at the call site).
    pub async fn parse_pdf(
        &self,
        file_name: &str,
        data: &[u8],
        from_page: u32,
        to_page: u32,
    ) -> Result<MonkeyOcrV2Output> {
        if data.is_empty() {
            anyhow::bail!("MonkeyOCRv2 PDF content is empty");
        }
        let part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name.to_owned())
            .mime_str("application/pdf")
            .map_err(|error| anyhow::anyhow!("MonkeyOCRv2 PDF multipart: {error}"))?;
        let form = reqwest::multipart::Form::new()
            .part("files", part)
            .text("start_page_id", from_page.to_string())
            .text("end_page_id", to_page.to_string());

        let response = self
            .client
            .post(format!("{}/parse", self.config.server_url))
            .timeout(self.config.request_timeout)
            .multipart(form)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("MonkeyOCRv2 submit: {error}"))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        // Stream the archive with the upstream 512 MiB cap so a misbehaving
        // service can never grow the process past that bound.
        use futures_util::StreamExt;
        let mut archive_bytes: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|error| anyhow::anyhow!("MonkeyOCRv2 response read: {error}"))?;
            if archive_bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
                anyhow::bail!("MonkeyOCRv2 response exceeds {MAX_RESPONSE_BYTES} bytes");
            }
            archive_bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let snippet = String::from_utf8_lossy(&archive_bytes);
            let snippet: String = snippet.chars().take(4096).collect();
            anyhow::bail!("MonkeyOCRv2 HTTP {status}: {snippet}");
        }
        if !content_type.contains("zip") && !is_zip_archive(&archive_bytes) {
            anyhow::bail!("MonkeyOCRv2 /parse did not return a ZIP archive");
        }
        convert_zip(&archive_bytes)
    }
}

fn is_zip_archive(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04")
        || bytes.starts_with(b"PK\x05\x06")
        || bytes.starts_with(b"PK\x07\x08")
}

/// Convert the native JSON layout records in a `/parse` ZIP archive into
/// marker-aware content (mirrors `MonkeyOCRv2Parser._convert_zip`).
fn convert_zip(archive_bytes: &[u8]) -> Result<MonkeyOcrV2Output> {
    let mut archive = zip::ZipArchive::new(Cursor::new(archive_bytes))
        .map_err(|error| anyhow::anyhow!("Invalid MonkeyOCRv2 ZIP: {error}"))?;
    if archive.len() > MAX_ZIP_MEMBERS {
        anyhow::bail!("MonkeyOCRv2 ZIP exceeds {MAX_ZIP_MEMBERS} members");
    }

    let mut names: Vec<String> = Vec::new();
    let mut total_uncompressed: u64 = 0;
    let mut image_data: HashMap<String, Vec<u8>> = HashMap::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| anyhow::anyhow!("Invalid MonkeyOCRv2 ZIP entry: {error}"))?;
        let name = entry.name().to_owned();
        total_uncompressed = total_uncompressed.saturating_add(entry.size());
        if total_uncompressed > MAX_UNCOMPRESSED_BYTES {
            anyhow::bail!("MonkeyOCRv2 ZIP exceeds {MAX_UNCOMPRESSED_BYTES} uncompressed bytes");
        }
        if is_image_name(&name) {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|error| anyhow::anyhow!("MonkeyOCRv2 ZIP image read: {error}"))?;
            if let Some(base) = base_name(&name) {
                image_data.insert(base, bytes);
            }
        }
        names.push(name);
    }

    let roots = discover_roots(&names);
    if roots.is_empty() {
        anyhow::bail!("MonkeyOCRv2 ZIP carries no layout records");
    }

    let mut sections: Vec<String> = Vec::new();
    let mut pages: BTreeSet<i64> = BTreeSet::new();
    for root in roots {
        let records = collect_records(&mut archive, &names, &root);
        for document in records {
            let documents = match document {
                Value::Array(items) => items,
                other => vec![other],
            };
            for document in documents {
                if !document.is_object() {
                    continue;
                }
                let Some(layouts) = document.get("layouts").and_then(Value::as_array) else {
                    continue;
                };
                for layout in layouts {
                    let Some(raw_text) = layout.get("content").and_then(Value::as_str) else {
                        continue;
                    };
                    let text = raw_text.trim();
                    if text.is_empty() {
                        continue;
                    }
                    let Some(page_number) = layout.get("page_num").and_then(int_like) else {
                        continue;
                    };
                    let page = page_number.saturating_sub(1).max(0);
                    let Some(bbox) = layout.get("bbox").and_then(Value::as_array) else {
                        continue;
                    };
                    if bbox.len() != 4 {
                        continue;
                    }
                    let Some(coordinates) =
                        bbox.iter().map(float_like).collect::<Option<Vec<f64>>>()
                    else {
                        continue;
                    };
                    // Upstream tag order: page, x0, x1, y0, y1.
                    let tag = format!(
                        "@@{}\t{}\t{}\t{}\t{}##",
                        page + 1,
                        coordinates[0],
                        coordinates[2],
                        coordinates[1],
                        coordinates[3]
                    );
                    let label = layout
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if matches!(label.as_str(), "picture" | "figure" | "image") {
                        let Some(path) = markdown_image_path(text) else {
                            continue;
                        };
                        let Some(base) = base_name(path) else {
                            continue;
                        };
                        if !image_data.contains_key(&base) {
                            continue;
                        }
                        let caption = markdown_image_alt(text).unwrap_or_default();
                        let label_text = if caption.is_empty() {
                            format!("image {}", sections.len() + 1)
                        } else {
                            caption
                        };
                        sections.push(format!(
                            "<!--IMAGE_START-->\n{label_text}{tag}\n<!--IMAGE_END-->"
                        ));
                        pages.insert(page);
                        continue;
                    }
                    if label == "table" {
                        sections.push(format!("<!--TABLE_START-->\n{text}{tag}\n<!--TABLE_END-->"));
                    } else {
                        sections.push(format!("{text}{tag}"));
                    }
                    pages.insert(page);
                }
            }
        }
    }

    if sections.is_empty() {
        anyhow::bail!("MonkeyOCRv2 returned no usable blocks");
    }
    Ok(MonkeyOcrV2Output {
        block_count: sections.len(),
        page_count: pages.len(),
        content: sections.join("\n\n"),
        structured: true,
    })
}

/// Roots are the first path segment of every `.md`, canonical
/// `{root}/{root}.json`, `jsons/*.json` and `all_results.json` member.
fn discover_roots(names: &[String]) -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    for name in names {
        let Some((root, _rest)) = name.split_once('/') else {
            continue;
        };
        let canonical = format!("{root}/{root}.json");
        if name.ends_with(".md")
            || name.ends_with("/all_results.json")
            || name == &canonical
            || (name.ends_with(".json") && name.contains("/jsons/"))
        {
            roots.insert(root.to_owned());
        }
    }
    roots
}

/// Prefer the canonical root JSON; otherwise every `jsons/*.json`; only when
/// both are absent fall back to `all_results.json` (a dict counts as one
/// document).
fn collect_records(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    names: &[String],
    root: &str,
) -> Vec<Value> {
    let canonical = format!("{root}/{root}.json");
    let mut candidates: Vec<&str> = Vec::new();
    if names.iter().any(|name| name == &canonical) {
        candidates.push(&canonical);
    } else {
        let prefix = format!("{root}/jsons/");
        candidates.extend(
            names
                .iter()
                .filter(|name| name.starts_with(&prefix) && name.ends_with(".json"))
                .map(String::as_str),
        );
    }
    let mut records: Vec<Value> = Vec::new();
    for candidate in candidates {
        if let Some(bytes) = read_zip_entry(archive, candidate)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            records.push(value);
        }
    }
    if records.is_empty() {
        let summary = format!("{root}/all_results.json");
        if let Some(bytes) = read_zip_entry(archive, &summary)
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
        {
            records = match value {
                Value::Array(items) => items,
                other => vec![other],
            };
        }
    }
    records
}

fn read_zip_entry(archive: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let mut entry = archive.by_name(name).ok()?;
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

fn is_image_name(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    IMAGE_EXTENSIONS
        .iter()
        .any(|extension| lowered.ends_with(extension))
}

fn base_name(path: &str) -> Option<String> {
    path.rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Extract the link target of the first `![alt](path)` markdown image.
fn markdown_image_path(text: &str) -> Option<&str> {
    let start = text.find("![")?;
    let alt_end = text[start + 2..].find(']')? + start + 2;
    let rest = text.get(alt_end + 1..)?;
    if !rest.starts_with('(') {
        return None;
    }
    let path_end = rest.find(')')?;
    Some(rest[1..path_end].trim().trim_matches(['\'', '"']))
}

/// Extract the alt text of the first `![alt](path)` markdown image.
fn markdown_image_alt(text: &str) -> Option<String> {
    let start = text.find("![")?;
    let alt_end = text[start + 2..].find(']')? + start + 2;
    Some(text[start + 2..alt_end].trim().to_owned())
}

/// Python `int(value)` for the JSON shapes `page_num` can take.
fn int_like(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|f| f.trunc() as i64)),
        Value::String(text) => text.trim().parse::<f64>().ok().map(|f| f.trunc() as i64),
        _ => None,
    }
}

/// Python `float(value)` for bbox coordinates (numbers or numeric strings).
fn float_like(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        extract::Multipart,
        http::{StatusCode, header},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn layout(content: &str, label: &str, page: Value, bbox: Value) -> Value {
        serde_json::json!({
            "content": content,
            "label": label,
            "page_num": page,
            "bbox": bbox,
        })
    }

    #[test]
    fn config_requires_http_server_url() {
        assert!(MonkeyOcrV2Config::new("").is_err());
        assert!(MonkeyOcrV2Config::new("ftp://example.com").is_err());
        let config = MonkeyOcrV2Config::new("http://127.0.0.1:8081/").unwrap();
        assert_eq!(config.server_url(), "http://127.0.0.1:8081");
        assert_eq!(config.request_timeout(), Duration::from_secs(1800));
        assert_eq!(
            MonkeyOcrV2Config::new("https://ocr.example.com")
                .unwrap()
                .with_request_timeout(60)
                .request_timeout(),
            Duration::from_secs(60)
        );
    }

    #[test]
    fn converts_canonical_root_records_into_marker_content() {
        let records = serde_json::json!({
            "layouts": [
                layout("Hello world", "text", serde_json::json!(1), serde_json::json!([1, 2, 3, 4])),
                layout("| a |", "table", serde_json::json!(2), serde_json::json!([5, 6, 7, 8])),
                layout("![diagram](doc/images/fig.png)", "picture", serde_json::json!(2), serde_json::json!([9, 10, 11, 12])),
                layout("dropped", "text", serde_json::json!(3), serde_json::json!([1, 2])),
            ]
        });
        let zip = build_zip(&[
            ("doc/doc.json", records.to_string().as_bytes()),
            ("doc/images/fig.png", b"not-a-real-png"),
        ]);
        let output = convert_zip(&zip).unwrap();
        assert!(output.content.contains("Hello world@@1\t1\t3\t2\t4##"));
        assert!(
            output
                .content
                .contains("<!--TABLE_START-->\n| a |@@2\t5\t7\t6\t8##\n<!--TABLE_END-->")
        );
        assert!(
            output
                .content
                .contains("<!--IMAGE_START-->\ndiagram@@2\t9\t11\t10\t12##\n<!--IMAGE_END-->")
        );
        assert!(!output.content.contains("dropped"));
        assert_eq!(output.block_count, 3);
        assert_eq!(output.page_count, 2);
        assert!(output.structured);
    }

    #[test]
    fn picture_without_image_is_skipped() {
        let records = serde_json::json!({
            "layouts": [
                layout("![diagram](doc/images/missing.png)", "figure", serde_json::json!(1), serde_json::json!([1, 2, 3, 4])),
            ]
        });
        let zip = build_zip(&[("doc/doc.json", records.to_string().as_bytes())]);
        assert!(convert_zip(&zip).is_err());
    }

    #[test]
    fn string_page_numbers_and_jsons_fallback_are_supported() {
        let first = serde_json::json!({
            "layouts": [
                layout("from jsons", "text", serde_json::json!("3"), serde_json::json!([1.5, 2, 3, 4])),
            ]
        });
        let second = serde_json::json!({
            "layouts": [
                layout("second doc", "text", serde_json::json!(1), serde_json::json!([0, 0, 0, 0])),
            ]
        });
        let zip = build_zip(&[
            ("a/jsons/one.json", first.to_string().as_bytes()),
            ("a/jsons/two.json", second.to_string().as_bytes()),
        ]);
        let output = convert_zip(&zip).unwrap();
        assert!(output.content.contains("from jsons@@3\t1.5\t3\t2\t4##"));
        assert!(output.content.contains("second doc@@1\t0\t0\t0\t0##"));
        assert_eq!(output.page_count, 2);
    }

    #[test]
    fn all_results_fallback_accepts_summary_dict_and_broken_json() {
        let summary = serde_json::json!({
            "layouts": [
                layout("summary text", "text", serde_json::json!(1), serde_json::json!([1, 1, 2, 2])),
            ]
        });
        let zip = build_zip(&[
            ("b/broken.json", b"{not json"),
            ("b/all_results.json", summary.to_string().as_bytes()),
        ]);
        let output = convert_zip(&zip).unwrap();
        assert!(output.content.contains("summary text@@1\t1\t2\t1\t2##"));
    }

    #[test]
    fn rejects_invalid_zip_and_empty_layouts() {
        let error = convert_zip(b"definitely not a zip")
            .unwrap_err()
            .to_string();
        assert!(error.contains("Invalid MonkeyOCRv2 ZIP"), "{error}");
        let empty = serde_json::json!({"layouts": []});
        let zip = build_zip(&[("doc/doc.json", empty.to_string().as_bytes())]);
        let error = convert_zip(&zip).unwrap_err().to_string();
        assert!(error.contains("no usable blocks"), "{error}");
    }

    #[tokio::test]
    async fn client_parses_zip_from_mock_service() {
        let records = serde_json::json!({
            "layouts": [
                layout("Mocked text", "text", serde_json::json!(1), serde_json::json!([1, 1, 2, 2])),
            ]
        });
        let archive = build_zip(&[("doc/doc.json", records.to_string().as_bytes())]);
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let state = seen.clone();
        let app = Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route(
                "/parse",
                post(move |mut multipart: Multipart| {
                    let state = state.clone();
                    async move {
                        while let Some(field) = multipart.next_field().await.unwrap() {
                            let name = field.name().unwrap_or_default().to_owned();
                            let value = field.text().await.unwrap_or_default();
                            state.lock().unwrap().push(format!("{name}={value}"));
                        }
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(header::CONTENT_TYPE, "application/zip")
                            .body(Body::from(archive))
                            .unwrap()
                            .into_response()
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client =
            MonkeyOcrV2Client::new(MonkeyOcrV2Config::new(&format!("http://{address}")).unwrap())
                .unwrap();
        assert!(client.check_installation().await.unwrap());
        let output = client
            .parse_pdf("sample.pdf", b"%PDF-1.4 mock", 0, 99999)
            .await
            .unwrap();
        server.abort();
        assert!(output.content.contains("Mocked text@@1\t1\t2\t1\t2##"));
        let fields = seen.lock().unwrap().clone();
        assert!(fields.iter().any(|field| field == "start_page_id=0"));
        assert!(fields.iter().any(|field| field == "end_page_id=99999"));
        // The multipart file field carries its filename as a separate part.
        assert!(
            fields.iter().any(|field| field == "files=")
                || fields.iter().any(|field| field.starts_with("files=")),
            "{fields:?}"
        );
    }

    #[tokio::test]
    async fn client_rejects_json_response() {
        let app = Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route(
                "/parse",
                post(|| async {
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"detail":"nope"}"#))
                        .unwrap()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client =
            MonkeyOcrV2Client::new(MonkeyOcrV2Config::new(&format!("http://{address}")).unwrap())
                .unwrap();
        let error = client
            .parse_pdf("sample.pdf", b"%PDF", 0, 99999)
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("did not return a ZIP archive"), "{error}");
    }

    #[tokio::test]
    async fn client_reports_http_errors_with_body_snippet() {
        let app = Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route(
                "/parse",
                post(|| async {
                    Response::builder()
                        .status(StatusCode::BAD_GATEWAY)
                        .body(Body::from("upstream exploded"))
                        .unwrap()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client =
            MonkeyOcrV2Client::new(MonkeyOcrV2Config::new(&format!("http://{address}")).unwrap())
                .unwrap();
        let error = client
            .parse_pdf("sample.pdf", b"%PDF", 0, 99999)
            .await
            .unwrap_err()
            .to_string();
        server.abort();
        assert!(error.contains("502"), "{error}");
        assert!(error.contains("upstream exploded"), "{error}");
    }
}
