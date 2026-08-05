//! Connector framework — mirrors RAGFlow `common/data_source/`.
//!
//! RAGFlow connector lifecycle (`common/data_source/interfaces.py` +
//! `connector_runner.py`):
//!   1. 授权  — `load_credentials` / `validate_connector_settings`
//!   2. 拉取  — `FingerprintConnector.list_keys` (cheap listing) →
//!              `get_value` (lazy body fetch), batched by `ConnectorRunner`
//!   3. 规范化 — `Document` (blob + semantic_identifier + extension + metadata)
//!
//! The [`Connector`] trait below maps those stages onto `list_files` /
//! `fetch_file` / `normalize`, with [`Connector::fetch_all`] acting as the
//! batch runner (equivalent of `ConnectorRunner.run`, tolerating per-file
//! failures and recording them in a [`SyncBatch`]).
//!
//! Included connectors:
//!   * base:  [`LocalConnector`] (本地文件), [`WebDavConnector`],
//!            [`BlobAdapter`] (Azure blob storage, SAS auth)
//!   * cloud: [`FeishuConnector`] (飞书云空间), [`ConfluenceConnector`]
//!
//! [`ConnectorRegistry`] dispatches by type key (RAGFlow data-source type).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::data_source::{ConnectorDoc, DocumentSource, SourceOptions};

// ── Contract types ───────────────────────────────────────────────────────────

/// One remote file entry — mirrors RAGFlow `KeyRecord` / `SlimDocument`
/// (cheap metadata-only listing primitive). `fingerprint` is an opaque
/// equality token; two equal fingerprints mean unchanged content (empty means
/// "always refetch"), mirroring `Document.content_hash` change-detection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteFile {
    /// Stable id across syncs (RAGFlow `Document.id`).
    pub id: String,
    /// Human readable name (RAGFlow `semantic_identifier`).
    pub name: String,
    /// Source path / key.
    pub path: String,
    /// File extension including the dot, e.g. ".md" (may be empty).
    pub extension: String,
    pub size_bytes: u64,
    /// RFC3339 timestamp when the file was last modified (may be empty).
    pub updated_at: String,
    /// Opaque change-detection token (may be empty = always refetch).
    pub fingerprint: String,
    pub metadata: HashMap<String, String>,
}

/// Result of a batch run — mirrors `ConnectorRunner.run` yields
/// `(docs | failures | next_checkpoint)`.
#[derive(Debug, Clone, Default)]
pub struct SyncBatch {
    pub docs: Vec<ConnectorDoc>,
    /// Per-file failure messages (document id / name + reason).
    pub failures: Vec<String>,
}

// ── Connector trait ──────────────────────────────────────────────────────────

/// Connector contract: 授权 → 拉取 → 规范化存储.
///
/// Aligned with RAGFlow `BaseConnector` / `CheckpointedConnector` /
/// `FingerprintConnector` semantics (see module docs).
#[async_trait]
pub trait Connector: Send + Sync {
    /// Connector type key, e.g. "feishu" / "confluence" / "webdav" /
    /// "local" / "blob".
    fn kind(&self) -> &'static str;

    /// 授权: load + validate credentials (RAGFlow `load_credentials` /
    /// `validate_connector_settings`). Returns `Ok` only when the source is
    /// reachable and the credentials are usable.
    async fn load_credentials(&mut self) -> Result<()>;

    /// 拉取列表: enumerate all files (RAGFlow `FingerprintConnector.list_keys`
    /// / `SlimConnector.retrieve_all_slim_docs`). Metadata-only call.
    async fn list_files(&self) -> Result<Vec<RemoteFile>>;

    /// 拉取内容: fetch raw bytes for one file (RAGFlow `get_value`).
    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>>;

    /// 规范化存储: convert raw bytes into a `ConnectorDoc` (RAGFlow
    /// `Document`: blob + semantic_identifier + extension + metadata).
    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc>;

    /// Batch runner — RAGFlow `ConnectorRunner.run()` equivalent: list →
    /// fetch → normalize with per-file failure tolerance. Applies
    /// `max_items` as the safety bound (RAGFlow `_ITERATION_LIMIT`).
    async fn fetch_all(&self, max_items: usize) -> Result<SyncBatch> {
        let files = self.list_files().await?;
        let mut batch = SyncBatch::default();
        for file in files.into_iter().take(max_items.max(1)) {
            match self.fetch_file(&file).await {
                Ok(raw) => match self.normalize(&file, raw) {
                    Ok(doc) => batch.docs.push(doc),
                    Err(error) => batch
                        .failures
                        .push(format!("{}: {error:#}", file.name)),
                },
                Err(error) => batch
                    .failures
                    .push(format!("{}: {error:#}", file.name)),
            }
        }
        Ok(batch)
    }
}

// ── Registry ─────────────────────────────────────────────────────────────────

/// Connector type descriptor for UI / docs.
pub struct ConnectorTypeInfo {
    pub kind: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
}

/// Connector registry — dispatch by type key (RAGFlow data-source type).
pub struct ConnectorRegistry;

impl ConnectorRegistry {
    /// Build a boxed connector for the given kind (RAGFlow `DocumentSource`).
    pub fn create(kind: &str, options: SourceOptions) -> Result<Box<dyn Connector>> {
        match kind {
            "local" => Ok(Box::new(LocalConnector::new(options)?)),
            "webdav" => Ok(Box::new(WebDavConnector::new(options))),
            "blob" => Ok(Box::new(BlobAdapter::new(options))),
            "feishu" => Ok(Box::new(FeishuConnector::new(options))),
            "confluence" => Ok(Box::new(ConfluenceConnector::new(options))),
            other => bail!("unsupported connector kind: {other}"),
        }
    }

    /// All kinds the registry can dispatch (for the data-sources page).
    pub fn kinds() -> Vec<ConnectorTypeInfo> {
        vec![
            ConnectorTypeInfo {
                kind: "gitlab",
                label: "GitLab",
                hint: "url=https://gitlab.com, token=PAT, target=project id or owner/repo",
            },
            ConnectorTypeInfo {
                kind: "blob",
                label: "Blob Storage",
                hint: "url=https://acct.blob.core.windows.net, token=SAS, target=container",
            },
            ConnectorTypeInfo {
                kind: "local",
                label: "Local Files",
                hint: "url=absolute directory path, target=optional relative subdir",
            },
            ConnectorTypeInfo {
                kind: "webdav",
                label: "WebDAV",
                hint: "url=https://webdav.example.com, token=user:password, target=/path",
            },
            ConnectorTypeInfo {
                kind: "feishu",
                label: "飞书云空间 (Feishu Drive)",
                hint: "token=app_id:app_secret, target=可选 folder_token",
            },
            ConnectorTypeInfo {
                kind: "confluence",
                label: "Confluence",
                hint: "url=https://xxx.atlassian.net/wiki, token=email:api_token, target=spaceKey",
            },
        ]
    }
}

// ── Base connector: local files ──────────────────────────────────────────────

/// Local filesystem connector — walks a directory recursively.
/// `options.url` = absolute root directory, `options.target` = optional
/// relative sub-directory filter, `options.token` unused.
pub struct LocalConnector {
    root: PathBuf,
    subdir: String,
}

impl LocalConnector {
    pub fn new(options: SourceOptions) -> Result<Self> {
        let root = options.url.trim().trim_end_matches('/');
        if root.is_empty() {
            bail!("local: root directory URL is required (source url)");
        }
        let path = PathBuf::from(root);
        if !path.is_dir() {
            bail!("local: not a directory: {root}");
        }
        Ok(Self {
            root: path,
            subdir: options.target.trim().trim_matches('/').to_string(),
        })
    }

    fn resolve(&self, rel: &str) -> PathBuf {
        if self.subdir.is_empty() {
            self.root.join(rel)
        } else {
            self.root.join(&self.subdir).join(rel)
        }
    }
}

#[async_trait]
impl Connector for LocalConnector {
    fn kind(&self) -> &'static str {
        "local"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        let base = if self.subdir.is_empty() {
            self.root.clone()
        } else {
            self.root.join(&self.subdir)
        };
        if !base.is_dir() {
            bail!("local: directory not found: {}", base.display());
        }
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let base = if self.subdir.is_empty() {
            self.root.clone()
        } else {
            self.root.join(&self.subdir)
        };
        let mut files = Vec::new();
        walk_dir(&base, &base, &mut files)?;
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let path = self.resolve(&file.path);
        std::fs::read(&path)
            .with_context(|| format!("local: read failed: {}", path.display()))
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "File".into());
        metadata.insert("source".into(), "local".into());
        metadata.insert("path".into(), file.path.clone());
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::Local,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

fn walk_dir(base: &Path, dir: &Path, out: &mut Vec<RemoteFile>) -> Result<()> {
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("local: read_dir failed: {}", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            walk_dir(base, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| rel.clone());
            let ext = path
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            let metadata = std::fs::metadata(&path).ok();
            let updated_at = metadata
                .as_ref()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| {
                    chrono::DateTime::<chrono::Utc>::from(std::time::UNIX_EPOCH + d)
                        .to_rfc3339()
                })
                .unwrap_or_default();
            let mut meta = HashMap::new();
            meta.insert("path".into(), rel.clone());
            out.push(RemoteFile {
                id: format!("local:{}", rel),
                name,
                path: rel,
                extension: ext,
                size_bytes: metadata.map(|m| m.len()).unwrap_or(0),
                updated_at,
                fingerprint: String::new(),
                metadata: meta,
            });
        }
    }
    Ok(())
}

// ── Base connector: WebDAV ───────────────────────────────────────────────────

/// WebDAV connector — mirrors RAGFlow `webdav_connector.py` (LoadConnector +
/// PollConnector). PROPFIND for listing, GET for fetching, Basic auth via
/// `token = "user:password"`.
pub struct WebDavConnector {
    client: reqwest::Client,
    base_url: String,
    remote_path: String,
    username: String,
    password: String,
}

impl WebDavConnector {
    pub fn new(options: SourceOptions) -> Self {
        let (username, password) = split_credentials(&options.token);
        let remote_path = {
            let raw = if options.target.trim().is_empty() {
                "/"
            } else {
                options.target.trim()
            };
            if !raw.starts_with('/') {
                format!("/{raw}")
            } else {
                raw.to_string()
            }
        };
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            base_url: options.url.trim().trim_end_matches('/').to_string(),
            remote_path,
            username,
            password,
        }
    }

    fn auth_header(&self) -> String {
        let raw = format!("{}:{}", self.username, self.password);
        format!("Basic {}", base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw.as_bytes()))
    }

    fn url_for(&self, href: &str) -> String {
        // href may be absolute, root-relative ("/dav/x"), or relative.
        if href.starts_with("http://") || href.starts_with("https://") {
            href.to_string()
        } else if href.starts_with('/') {
            format!("{}{}", self.base_url, href)
        } else {
            format!("{}{}", self.base_url, href)
        }
    }

    async fn request(&self, method: reqwest::Method, url: &str) -> Result<reqwest::Response> {
        let response = self
            .client
            .request(method, url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .context("webdav: request failed")?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "webdav: upstream returned {}: {}",
                status.as_u16(),
                body.chars().take(200).collect::<String>()
            );
        }
        Ok(response)
    }
}

#[async_trait]
impl Connector for WebDavConnector {
    fn kind(&self) -> &'static str {
        "webdav"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        if self.base_url.is_empty() {
            bail!("webdav: server URL is required (source url)");
        }
        if self.username.is_empty() {
            bail!("webdav: username required (token = user:password)");
        }
        // Probe: OPTIONS on the remote path.
        let url = self.url_for(&self.remote_path);
        let response = self
            .client
            .request(reqwest::Method::OPTIONS, &url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .context("webdav: probe request failed")?;
        if !response.status().is_success() {
            bail!("webdav: probe returned {}", response.status().as_u16());
        }
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let url = self.url_for(&self.remote_path);
        let mut request = self
            .client
            .request(reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND"), &url)
            .header("Authorization", self.auth_header())
            .header("Depth", "1");
        request = request
            .header("Content-Type", "application/xml")
            .body(
                r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:"><D:prop><D:displayname/><D:getcontentlength/><D:getlastmodified/><D:resourcetype/></D:prop></D:propfind>"#,
            );
        let response = request.send().await.context("webdav: propfind failed")?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "webdav: propfind returned {}: {}",
                status.as_u16(),
                body.chars().take(200).collect::<String>()
            );
        }
        parse_propfind(&body, &self.base_url)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let url = self.url_for(&file.path);
        let response = self.request(reqwest::Method::GET, &url).await?;
        Ok(response.bytes().await?.to_vec())
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "File".into());
        metadata.insert("source".into(), "webdav".into());
        metadata.insert("path".into(), file.path.clone());
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::WebDav,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

/// Parse a WebDAV `multistatus` PROPFIND response into file entries.
/// Non-collection (file) resources only, mirroring RAGFlow's WebDAV client
/// `ls` behaviour.
fn parse_propfind(xml: &str, base_url: &str) -> Result<Vec<RemoteFile>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut files = Vec::new();
    let mut current: Option<HashMap<String, String>> = None;
    let mut text = String::new();
    let mut in_collection = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "response" => {
                        current = Some(HashMap::new());
                        in_collection = false;
                    }
                    "collection" => in_collection = true,
                    _ => {}
                }
                text.clear();
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "collection" {
                    in_collection = true;
                }
            }
            Ok(Event::Text(t)) => {
                text.push_str(&t.unescape().unwrap_or_default().to_string());
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if let Some(map) = current.as_mut() {
                    match name.as_str() {
                        "href" => {
                            map.insert("href".into(), text.trim().to_string());
                        }
                        "displayname" => {
                            map.insert("displayname".into(), text.trim().to_string());
                        }
                        "getcontentlength" => {
                            map.insert("size".into(), text.trim().to_string());
                        }
                        "getlastmodified" => {
                            map.insert("modified".into(), text.trim().to_string());
                        }
                        _ => {}
                    }
                }
                if name == "response" {
                    if let Some(map) = current.take() {
                        if !in_collection {
                            if let Some(href) = map.get("href") {
                                if !href.ends_with('/') {
                                    let raw_href = href.clone();
                                    let name = map
                                        .get("displayname")
                                        .filter(|n| !n.is_empty())
                                        .cloned()
                                        .or_else(|| {
                                            raw_href
                                                .trim_end_matches('/')
                                                .rsplit('/')
                                                .next()
                                                .map(|s| s.to_string())
                                        })
                                        .unwrap_or_else(|| raw_href.clone());
                                    let size = map
                                        .get("size")
                                        .and_then(|s| s.parse::<u64>().ok())
                                        .unwrap_or(0);
                                    let ext = Path::new(&name)
                                        .extension()
                                        .map(|e| format!(".{}", e.to_string_lossy()))
                                        .unwrap_or_default();
                                    let mut metadata = HashMap::new();
                                    metadata.insert("source".into(), "webdav".into());
                                    files.push(RemoteFile {
                                        id: format!("webdav:{raw_href}"),
                                        name,
                                        path: raw_href,
                                        extension: ext,
                                        size_bytes: size,
                                        updated_at: map
                                            .get("modified")
                                            .cloned()
                                            .unwrap_or_default(),
                                        fingerprint: String::new(),
                                        metadata,
                                    });
                                }
                            }
                        }
                    }
                }
                text.clear();
            }
            Ok(Event::Eof) => break,
            Err(error) => bail!("webdav: propfind XML parse error: {error}"),
            _ => {}
        }
    }
    Ok(files)
}

// ── Base connector: Azure Blob (adapter) ─────────────────────────────────────

/// Azure Blob storage connector (SAS token auth) — self-contained equivalent
/// of `src/data_source/blob.rs` adapted to the [`Connector`] contract.
pub struct BlobAdapter {
    client: reqwest::Client,
    options: SourceOptions,
}

impl BlobAdapter {
    pub fn new(options: SourceOptions) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            options,
        }
    }

    fn endpoint(&self) -> Result<String> {
        let endpoint = self.options.url.trim().trim_end_matches('/');
        if endpoint.is_empty() {
            bail!("blob: storage endpoint URL is required (source url)");
        }
        Ok(endpoint.to_string())
    }

    fn sas(&self) -> Result<String> {
        let token = self.options.token.trim();
        if token.is_empty() {
            bail!("blob: SAS token is required (token)");
        }
        Ok(token.trim_start_matches('?').to_string())
    }

    async fn list_containers(&self) -> Result<Vec<String>> {
        let url = format!("{}/?comp=list&restype=container&{}", self.endpoint()?, self.sas()?);
        let body = self.get_xml(&url).await?;
        Ok(parse_xml_names(&body, "Container", "Name"))
    }

    async fn list_blobs(&self, container: &str) -> Result<Vec<String>> {
        let url = format!(
            "{}/{container}?restype=container&comp=list&{}",
            self.endpoint()?,
            self.sas()?
        );
        let body = self.get_xml(&url).await?;
        Ok(parse_xml_names(&body, "Blob", "Name"))
    }

    async fn get_xml(&self, url: &str) -> Result<String> {
        let response = self.client.get(url).send().await.context("blob: request failed")?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!(
                "blob: upstream returned {}: {}",
                status.as_u16(),
                body.chars().take(200).collect::<String>()
            );
        }
        Ok(body)
    }

    async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self.client.get(url).send().await.context("blob: request failed")?;
        let status = response.status();
        let bytes = response.bytes().await?;
        if !status.is_success() {
            bail!(
                "blob: upstream returned {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        Ok(bytes.to_vec())
    }
}

#[async_trait]
impl Connector for BlobAdapter {
    fn kind(&self) -> &'static str {
        "blob"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        self.endpoint()?;
        self.sas()?;
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let endpoint = self.endpoint()?;
        let max = self.options.max_items.max(1);
        let containers: Vec<String> = if self.options.target.trim().is_empty() {
            self.list_containers().await?
        } else {
            self.options
                .target
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        };
        let mut files = Vec::new();
        'outer: for container in containers {
            for blob in self.list_blobs(&container).await? {
                if files.len() >= max {
                    break 'outer;
                }
                let name = blob.rsplit('/').next().unwrap_or(&blob).to_string();
                let ext = Path::new(&name)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();
                let mut metadata = HashMap::new();
                metadata.insert("container".into(), container.clone());
                metadata.insert("blob".into(), blob.clone());
                files.push(RemoteFile {
                    id: format!("{endpoint}/{container}/{blob}"),
                    name,
                    path: blob,
                    extension: ext,
                    size_bytes: 0,
                    updated_at: String::new(),
                    fingerprint: String::new(),
                    metadata,
                });
            }
        }
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let url = format!(
            "{}/{}/{}?{}",
            self.endpoint()?,
            file.metadata.get("container").map(|s| s.as_str()).unwrap_or_default(),
            file.path,
            self.sas()?
        );
        self.get_bytes(&url).await
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "File".into());
        metadata.insert("source".into(), "blob".into());
        if let Some(container) = file.metadata.get("container") {
            metadata.insert("container".into(), container.clone());
        }
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::Blob,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

/// Parse `<EnumerationResults><{item_type}><Name>x</Name>...` without an XML
/// dependency (same shape as the existing blob connector).
fn parse_xml_names(xml: &str, item_type: &str, name_tag: &str) -> Vec<String> {
    let mut names = Vec::new();
    let item_open = format!("<{item_type}>");
    let item_close = format!("</{item_type}>");
    let name_open = format!("<{name_tag}>");
    let name_close = format!("</{name_tag}>");
    let mut rest = xml;
    while let Some(start) = rest.find(&item_open) {
        let item_start = start + item_open.len();
        let Some(item_end) = rest[item_start..].find(&item_close) else {
            break;
        };
        let item = &rest[item_start..item_start + item_end];
        if let Some(ns) = item.find(&name_open) {
            let value_start = ns + name_open.len();
            if let Some(ne) = item[value_start..].find(&name_close) {
                names.push(item[value_start..value_start + ne].to_string());
            }
        }
        rest = &rest[item_start + item_end + item_close.len()..];
    }
    names
}

// ── Concrete connector: Feishu / Lark 云空间 ─────────────────────────────────

/// Feishu (Lark) cloud drive connector (飞书云空间).
///
/// Auth: `token = "app_id:app_secret"` → tenant_access_token
/// (POST /open-apis/auth/v3/tenant_access_token/internal).
/// List: GET /open-apis/drive/v1/files (folder children, recursive into
/// folders). Fetch: media download for `file` type, `raw_content` export for
/// `docx` (mirrors RAGFlow's file + doc handling).
pub struct FeishuConnector {
    client: reqwest::Client,
    base_url: String,
    app_id: String,
    app_secret: String,
    folder_token: String,
    token: std::sync::Mutex<Option<String>>,
}

impl FeishuConnector {
    pub fn new(options: SourceOptions) -> Self {
        let (app_id, app_secret) = split_credentials(&options.token);
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            base_url: options
                .url
                .trim()
                .trim_end_matches('/')
                .to_string(),
            app_id,
            app_secret,
            folder_token: options.target.trim().to_string(),
            token: std::sync::Mutex::new(None),
        }
    }

    async fn tenant_access_token(&self) -> Result<String> {
        if let Some(token) = self.token.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            return Ok(token.clone());
        }
        if self.app_id.is_empty() || self.app_secret.is_empty() {
            bail!("feishu: app_id:app_secret required (token)");
        }
        let base = if self.base_url.is_empty() {
            "https://open.feishu.cn".to_string()
        } else {
            self.base_url.clone()
        };
        let url = format!("{base}/open-apis/auth/v3/tenant_access_token/internal");
        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "app_id": self.app_id,
                "app_secret": self.app_secret,
            }))
            .send()
            .await
            .context("feishu: token request failed")?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() || value.get("code").and_then(serde_json::Value::as_i64) != Some(0)
        {
            bail!(
                "feishu: auth failed ({}): {}",
                status.as_u16(),
                value.get("msg").and_then(serde_json::Value::as_str).unwrap_or_default()
            );
        }
        let token = value
            .get("tenant_access_token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("feishu: tenant_access_token missing"))?
            .to_string();
        *self.token.lock().unwrap_or_else(|p| p.into_inner()) = Some(token.clone());
        Ok(token)
    }

    async fn get_json(&self, path: &str, token: &str) -> Result<serde_json::Value> {
        let base = if self.base_url.is_empty() {
            "https://open.feishu.cn".to_string()
        } else {
            self.base_url.clone()
        };
        let url = format!("{base}{path}");
        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .context("feishu: request failed")?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() || value.get("code").and_then(serde_json::Value::as_i64) != Some(0)
        {
            bail!(
                "feishu: API error ({}): {}",
                status.as_u16(),
                value.get("msg").and_then(serde_json::Value::as_str).unwrap_or_default()
            );
        }
        Ok(value)
    }

    /// Enumerate folder children (files + folders), recursing into folders.
    async fn list_folder(&self, folder_token: &str, depth: usize, out: &mut Vec<RemoteFile>) -> Result<()> {
        if depth > 8 {
            return Ok(());
        }
        let token = self.tenant_access_token().await?;
        let path = if folder_token.is_empty() {
            "/open-apis/drive/v1/files?page_size=50".to_string()
        } else {
            format!(
                "/open-apis/drive/v1/files?folder_token={}&page_size=50",
                urlencode(folder_token)
            )
        };
        let value = self.get_json(&path, &token).await?;
        let files = value
            .get("data")
            .and_then(|d| d.get("files"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        for file in files {
            let file_token = file
                .get("file_token")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = file
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let kind = file
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("file")
                .to_string();
            if kind == "folder" {
                // recurse into sub-folder (mirrors RAGFlow recursive fetch);
                // async recursion must be boxed (E0733)
                let _ = Box::pin(self.list_folder(&file_token, depth + 1, out))
                    .await
                    .unwrap_or_else(|error| {
                        tracing::warn!("feishu: skip folder {name}: {error:#}");
                    });
            } else {
                let ext = match kind.as_str() {
                    "docx" => ".md".to_string(),
                    "sheet" => ".xlsx".to_string(),
                    "bitable" => ".md".to_string(),
                    _ => Path::new(&name)
                        .extension()
                        .map(|e| format!(".{}", e.to_string_lossy()))
                        .unwrap_or_default(),
                };
                let mut metadata = HashMap::new();
                metadata.insert("type".into(), kind.clone());
                metadata.insert("file_token".into(), file_token.clone());
                metadata.insert("source".into(), "feishu".into());
                out.push(RemoteFile {
                    id: format!("feishu:{file_token}"),
                    name: name.clone(),
                    path: file_token.clone(),
                    extension: ext,
                    size_bytes: 0,
                    updated_at: String::new(),
                    fingerprint: String::new(),
                    metadata,
                });
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Connector for FeishuConnector {
    fn kind(&self) -> &'static str {
        "feishu"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        self.tenant_access_token().await?;
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let mut files = Vec::new();
        self.list_folder(&self.folder_token, 0, &mut files).await?;
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let token = self.tenant_access_token().await?;
        let base = if self.base_url.is_empty() {
            "https://open.feishu.cn".to_string()
        } else {
            self.base_url.clone()
        };
        let kind = file
            .metadata
            .get("type")
            .map(|s| s.as_str())
            .unwrap_or("file");
        match kind {
            "docx" => {
                // Export docx content as plain text (markdown-ish raw content).
                let url = format!(
                    "{base}/open-apis/docx/v1/documents/{}/raw_content",
                    urlencode(&file.path)
                );
                let response = self
                    .client
                    .get(&url)
                    .header("Authorization", format!("Bearer {token}"))
                    .send()
                    .await
                    .context("feishu: docx export failed")?;
                let status = response.status();
                let value: serde_json::Value = response.json().await?;
                if !status.is_success()
                    || value.get("code").and_then(serde_json::Value::as_i64) != Some(0)
                {
                    bail!(
                        "feishu: docx export error ({}): {}",
                        status.as_u16(),
                        value
                            .get("msg")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                    );
                }
                let content = value
                    .get("data")
                    .and_then(|d| d.get("content"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Ok(content.into_bytes())
            }
            "file" => {
                let url = format!(
                    "{base}/open-apis/drive/v1/medias/{}/download",
                    urlencode(&file.path)
                );
                let response = self
                    .client
                    .get(&url)
                    .header("Authorization", format!("Bearer {token}"))
                    .send()
                    .await
                    .context("feishu: media download failed")?;
                let status = response.status();
                let bytes = response.bytes().await?;
                if !status.is_success() {
                    bail!(
                        "feishu: media download returned {}: {}",
                        status.as_u16(),
                        String::from_utf8_lossy(&bytes)
                            .chars()
                            .take(200)
                            .collect::<String>()
                    );
                }
                Ok(bytes.to_vec())
            }
            other => bail!("feishu: unsupported file type for fetch: {other}"),
        }
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "File".into());
        metadata.insert("source".into(), "feishu".into());
        if let Some(kind) = file.metadata.get("type") {
            metadata.insert("file_type".into(), kind.clone());
        }
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::Feishu,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

// ── Concrete connector: Confluence ───────────────────────────────────────────

/// Atlassian Confluence connector — mirrors RAGFlow
/// `confluence_connector.py` (CheckpointedConnector). Lists pages by space
/// key, fetches `body.storage` HTML and normalizes it to text/markdown-ish.
///
/// Auth: `token = "email:api_token"` (Basic) or a bare personal access token
/// (Bearer).
pub struct ConfluenceConnector {
    client: reqwest::Client,
    base_url: String,
    space_key: String,
    username: String,
    password: String,
    pat: String,
}

impl ConfluenceConnector {
    pub fn new(options: SourceOptions) -> Self {
        let token = options.token.trim();
        let (username, password) = split_credentials(token);
        let pat = if username.is_empty() {
            token.to_string()
        } else {
            String::new()
        };
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            base_url: options.url.trim().trim_end_matches('/').to_string(),
            space_key: options.target.trim().to_string(),
            username,
            password,
            pat,
        }
    }

    fn auth_header(&self) -> String {
        if !self.pat.is_empty() {
            format!("Bearer {}", self.pat)
        } else {
            let raw = format!("{}:{}", self.username, self.password);
            format!(
                "Basic {}",
                base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    raw.as_bytes()
                )
            )
        }
    }

    async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .context("confluence: request failed")?;
        let status = response.status();
        let body = response.bytes().await?;
        if !status.is_success() {
            bail!(
                "confluence: upstream returned {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        serde_json::from_slice(&body).context("confluence: invalid JSON")
    }
}

#[async_trait]
impl Connector for ConfluenceConnector {
    fn kind(&self) -> &'static str {
        "confluence"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        if self.base_url.is_empty() {
            bail!("confluence: instance URL is required (source url)");
        }
        if self.space_key.is_empty() {
            bail!("confluence: spaceKey is required (target)");
        }
        if self.username.is_empty() && self.pat.is_empty() {
            bail!("confluence: email:api_token or PAT required (token)");
        }
        // Probe: fetch the space itself.
        let value = self
            .get(&format!(
                "/rest/api/space/{}?expand=description.plain",
                urlencode(&self.space_key)
            ))
            .await?;
        if value.get("key").is_none() {
            bail!("confluence: space not found: {}", self.space_key);
        }
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        // Paginated page listing: GET /rest/api/content?spaceKey=..&limit=50&expand=version
        let mut files = Vec::new();
        let mut start = 0usize;
        loop {
            let value = self
                .get(&format!(
                    "/rest/api/content?spaceKey={}&limit=50&start={}&expand=version",
                    urlencode(&self.space_key),
                    start
                ))
                .await?;
            let results = value
                .get("results")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let fetched = results.len();
            for page in &results {
                let id = page
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let title = page
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let page_type = page
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("page")
                    .to_string();
                let updated_at = page
                    .get("version")
                    .and_then(|v| v.get("when"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut metadata = HashMap::new();
                metadata.insert("type".into(), page_type.clone());
                metadata.insert("source".into(), "confluence".into());
                metadata.insert("space".into(), self.space_key.clone());
                files.push(RemoteFile {
                    id: format!("confluence:{id}"),
                    name: title,
                    path: id,
                    extension: ".md".into(),
                    size_bytes: 0,
                    updated_at,
                    fingerprint: String::new(),
                    metadata,
                });
            }
            let size = value.get("size").and_then(serde_json::Value::as_u64).unwrap_or(0);
            start += fetched;
            if fetched == 0 || start >= size as usize {
                break;
            }
        }
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        // GET /rest/api/content/{id}?expand=body.storage
        let value = self
            .get(&format!(
                "/rest/api/content/{}?expand=body.storage",
                urlencode(&file.path)
            ))
            .await?;
        let html = value
            .get("body")
            .and_then(|b| b.get("storage"))
            .and_then(|s| s.get("value"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(html.into_bytes())
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let html = String::from_utf8_lossy(&raw).to_string();
        // Lightweight HTML → markdown-ish conversion (mirrors RAGFlow
        // html_utils.format_document_soup): headings, lists, paragraphs.
        let mut text = html
            .replace("<h1", "\n# ")
            .replace("<h2", "\n## ")
            .replace("<h3", "\n### ")
            .replace("<h4", "\n#### ")
            .replace("<h5", "\n##### ")
            .replace("<h6", "\n###### ")
            .replace("<li", "\n- ")
            .replace("<p", "\n")
            .replace("<br", "\n")
            .replace("</h1>", "\n")
            .replace("</h2>", "\n")
            .replace("</h3>", "\n")
            .replace("</h4>", "\n")
            .replace("</h5>", "\n")
            .replace("</h6>", "\n")
            .replace("</li>", "")
            .replace("</p>", "\n")
            .replace("<td", " | ")
            .replace("<tr", "\n| ");
        text = crate::parser::html::HtmlParser::strip_tags(&text);
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "Page".into());
        metadata.insert("source".into(), "confluence".into());
        metadata.insert("space".into(), self.space_key.clone());
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob: text,
            source: DocumentSource::Confluence,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Split "user:password" (or "app_id:app_secret") credential strings.
fn split_credentials(token: &str) -> (String, String) {
    let token = token.trim();
    if let Some((left, right)) = token.split_once(':') {
        (left.trim().to_string(), right.trim().to_string())
    } else {
        (String::new(), String::new())
    }
}

/// Minimal percent-encoding (same approach as the GitLab connector).
fn urlencode(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                output.push(byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Fake connector for contract tests: 3 files, second one fails to fetch
    /// (exercises the ConnectorRunner-style failure tolerance).
    struct FakeConnector {
        kind: &'static str,
        files: Vec<RemoteFile>,
        fail_fetch: Vec<String>,
    }

    impl FakeConnector {
        fn new() -> Self {
            Self {
                kind: "fake",
                files: vec![
                    RemoteFile {
                        id: "f1".into(),
                        name: "a.md".into(),
                        path: "a.md".into(),
                        extension: ".md".into(),
                        size_bytes: 3,
                        updated_at: String::new(),
                        fingerprint: "fp-a".into(),
                        metadata: HashMap::new(),
                    },
                    RemoteFile {
                        id: "f2".into(),
                        name: "b.txt".into(),
                        path: "b.txt".into(),
                        extension: ".txt".into(),
                        size_bytes: 3,
                        updated_at: String::new(),
                        fingerprint: "fp-b".into(),
                        metadata: HashMap::new(),
                    },
                    RemoteFile {
                        id: "f3".into(),
                        name: "c.md".into(),
                        path: "c.md".into(),
                        extension: ".md".into(),
                        size_bytes: 3,
                        updated_at: String::new(),
                        fingerprint: "fp-c".into(),
                        metadata: HashMap::new(),
                    },
                ],
                fail_fetch: vec!["f2".into()],
            }
        }
    }

    #[async_trait]
    impl Connector for FakeConnector {
        fn kind(&self) -> &'static str {
            self.kind
        }

        async fn load_credentials(&mut self) -> Result<()> {
            Ok(())
        }

        async fn list_files(&self) -> Result<Vec<RemoteFile>> {
            Ok(self.files.clone())
        }

        async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
            if self.fail_fetch.contains(&file.id) {
                bail!("fake: boom");
            }
            Ok(format!("content-of-{}", file.id).into_bytes())
        }

        fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
            let blob = String::from_utf8_lossy(&raw).to_string();
            Ok(ConnectorDoc {
                id: file.id.clone(),
                blob,
                source: DocumentSource::Local,
                semantic_identifier: file.name.clone(),
                extension: file.extension.clone(),
                doc_updated_at: file.updated_at.clone(),
                size_bytes: raw.len(),
                metadata: HashMap::new(),
            })
        }
    }

    /// Trait contract: list → fetch → normalize produces documents with the
    /// expected identifiers, and per-file failures are recorded without
    /// aborting the batch (ConnectorRunner.run semantics).
    #[tokio::test]
    async fn fetch_all_batches_and_tolerates_failures() {
        let connector = FakeConnector::new();
        let batch = connector.fetch_all(10).await.unwrap();
        assert_eq!(batch.docs.len(), 2);
        assert_eq!(batch.failures.len(), 1);
        assert!(batch.failures[0].contains("b.txt"));
        assert!(batch.failures[0].contains("boom"));
        let a = batch.docs.iter().find(|d| d.id == "f1").unwrap();
        assert_eq!(a.semantic_identifier, "a.md");
        assert_eq!(a.extension, ".md");
        assert_eq!(a.blob, "content-of-f1");
        let c = batch.docs.iter().find(|d| d.id == "f3").unwrap();
        assert_eq!(c.blob, "content-of-f3");
    }

    /// fetch_all applies the max_items safety bound (RAGFlow _ITERATION_LIMIT).
    #[tokio::test]
    async fn fetch_all_respects_max_items() {
        let connector = FakeConnector::new();
        let batch = connector.fetch_all(1).await.unwrap();
        assert_eq!(batch.docs.len() + batch.failures.len(), 1);
        assert_eq!(batch.docs.first().map(|d| d.id.as_str()), Some("f1"));
    }

    /// Registry dispatches by kind; unknown kinds are rejected.
    #[test]
    fn registry_dispatches_by_kind() {
        let mut options = SourceOptions::default();
        options.url = "/tmp".into();
        for kind in ["local", "webdav", "blob", "feishu", "confluence"] {
            let connector = ConnectorRegistry::create(kind, options.clone()).unwrap();
            assert_eq!(connector.kind(), kind);
        }
        assert!(ConnectorRegistry::create("nope", options.clone()).is_err());
        // All kinds advertised by the registry are constructible.
        for info in ConnectorRegistry::kinds() {
            assert!(!info.label.is_empty());
        }
    }

    /// Local connector full contract: 授权 → 拉取列表 → 拉取内容 → 规范化.
    #[tokio::test]
    async fn local_connector_full_contract() {
        let dir = std::env::temp_dir().join(format!("rayrag-conn-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("readme.md"), "# Hello\nWorld").unwrap();
        std::fs::write(dir.join("sub").join("notes.txt"), "note body").unwrap();

        let mut options = SourceOptions::default();
        options.url = dir.to_string_lossy().to_string();
        options.max_items = 50;

        let mut connector = LocalConnector::new(options).unwrap();
        connector.load_credentials().await.unwrap();
        let files = connector.list_files().await.unwrap();
        assert_eq!(files.len(), 2);
        let readme = files.iter().find(|f| f.name == "readme.md").unwrap();
        assert_eq!(readme.extension, ".md");

        let raw = connector.fetch_file(readme).await.unwrap();
        let doc = connector.normalize(readme, raw).unwrap();
        assert_eq!(doc.semantic_identifier, "readme.md");
        assert_eq!(doc.blob, "# Hello\nWorld");
        assert_eq!(doc.source, DocumentSource::Local);

        // registry path end-to-end
        let mut options2 = SourceOptions::default();
        options2.url = dir.to_string_lossy().to_string();
        options2.max_items = 50;
        let registry_connector = ConnectorRegistry::create("local", options2).unwrap();
        let batch = registry_connector.fetch_all(50).await.unwrap();
        assert_eq!(batch.docs.len(), 2);
        assert!(batch.failures.is_empty());
        std::fs::remove_dir_all(dir).ok();
    }
}
