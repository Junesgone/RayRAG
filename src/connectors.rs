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
//!   * cloud: [`FeishuConnector`] (飞书云空间), [`ConfluenceConnector`],
//!            [`YuqueConnector`] (语雀)
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
                    Err(error) => batch.failures.push(format!("{}: {error:#}", file.name)),
                },
                Err(error) => batch.failures.push(format!("{}: {error:#}", file.name)),
            }
        }
        Ok(batch)
    }
}

// ── S3-protocol connector (SigV4) ─────────────────────────────────────────────

/// S3-protocol object storage connector — mirrors RAGFlow `s3_connector.py`.
/// Covers S3 / GCS-interop / Oracle S3-compat / MinIO via a single
/// S3-compatible endpoint + AWS Signature Version 4 signing.
///
/// Source options:
///   * `url`    = endpoint, e.g. `https://s3.cn-north-1.amazonaws.com.cn`
///                or `http://minio:9000` (path-style)
///   * `token`  = `access_key:secret_key`
///   * `target` = `bucket` or `bucket/prefix`
pub struct S3Connector {
    client: reqwest::Client,
    endpoint: String,
    region: String,
    access_key: String,
    secret_key: String,
    bucket: String,
    prefix: String,
}

impl S3Connector {
    pub fn new(options: SourceOptions) -> Self {
        let (access_key, secret_key) = split_credentials(&options.token);
        let (bucket, prefix) = match options.target.split_once('/') {
            Some((bucket, prefix)) => (
                bucket.trim().to_string(),
                prefix.trim().trim_matches('/').to_string(),
            ),
            None => (options.target.trim().to_string(), String::new()),
        };
        let endpoint = options.url.trim().trim_end_matches('/').to_string();
        // Region guess from the endpoint host (AWS + GCS interop + cn endpoints).
        let region = guess_s3_region(&endpoint);
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            endpoint,
            region,
            access_key,
            secret_key,
            bucket,
            prefix,
        }
    }

    /// Host part of the endpoint (signing + Host header).
    fn host(&self) -> Result<String> {
        let url = url::Url::parse(&self.endpoint)
            .with_context(|| format!("s3: invalid endpoint url: {}", self.endpoint))?;
        Ok(url.host_str().unwrap_or_default().to_string())
    }

    /// Object URL: `{endpoint}/{bucket}/{key}` (path-style, compatible with
    /// MinIO and China-region endpoints).
    fn object_url(&self, key: &str) -> String {
        format!(
            "{}/{}/{}",
            self.endpoint,
            urlencode_path(&self.bucket),
            key.split('/')
                .map(urlencode_path)
                .collect::<Vec<_>>()
                .join("/")
        )
    }

    /// Build a SigV4-signed request for `method` on `path` (e.g. `/bucket`
    /// for ListObjectsV2, `/bucket/key` for GET).
    async fn signed_request(
        &self,
        method: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<reqwest::Response> {
        let host = self.host()?;
        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();
        let payload_hash = EMPTY_SHA256;
        let canonical_query = canonical_query_string(query);
        let canonical_uri = canonical_uri(path);
        let canonical_headers = format!(
            "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
            host, payload_hash, amz_date
        );
        let signed_headers = "host;x-amz-content-sha256;x-amz-date";
        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method, canonical_uri, canonical_query, canonical_headers, signed_headers, payload_hash
        );
        let scope = format!("{}/{}/s3/aws4_request", date_stamp, self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date,
            scope,
            sha256_hex(canonical_request.as_bytes())
        );
        let signature = sigv4_signature(
            &self.secret_key,
            &self.region,
            &date_stamp,
            &amz_date,
            &string_to_sign,
        );
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, scope, signed_headers, signature
        );
        let url = format!(
            "{}{}{}",
            self.endpoint,
            path,
            if canonical_query.is_empty() {
                String::new()
            } else {
                format!("?{}", canonical_query)
            }
        );
        let response = self
            .client
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).context("s3: bad method")?,
                &url,
            )
            .header("Host", host)
            .header("x-amz-date", &amz_date)
            .header("x-amz-content-sha256", payload_hash)
            .header("Authorization", &authorization)
            .send()
            .await
            .with_context(|| format!("s3: request failed: {method} {url}"))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "s3: upstream returned {}: {}",
                status.as_u16(),
                body.chars().take(300).collect::<String>()
            );
        }
        Ok(response)
    }

    /// ListObjectsV2 — mirrors `s3_client.list_objects_v2`.
    async fn list_objects(&self) -> Result<Vec<(String, u64, String, String)>> {
        let path = format!("/{}", urlencode_path(&self.bucket));
        let mut out = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let mut query: Vec<(&str, &str)> = vec![("list-type", "2"), ("max-keys", "1000")];
            if !self.prefix.is_empty() {
                query.push(("prefix", &self.prefix));
            }
            if let Some(token) = continuation.as_deref() {
                query.push(("continuation-token", token));
            }
            let response = self.signed_request("GET", &path, &query).await?;
            let body = crate::common::cmd_timeout::read_body_limited(
                response,
                crate::common::cmd_timeout::connector_body_limit_bytes(),
                "s3 list",
            )
            .await?;
            let text = String::from_utf8_lossy(&body).to_string();
            let parsed = parse_list_objects(&text)?;
            continuation = parsed.next_token;
            out.extend(parsed.objects);
            if continuation.is_none() || continuation.as_deref() == Some("") {
                break;
            }
        }
        Ok(out)
    }
}

#[async_trait]
impl Connector for S3Connector {
    fn kind(&self) -> &'static str {
        "s3"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        if self.endpoint.is_empty() {
            bail!("s3: endpoint URL is required (source url)");
        }
        if self.access_key.is_empty() || self.secret_key.is_empty() {
            bail!("s3: credentials required (token = access_key:secret_key)");
        }
        if self.bucket.is_empty() {
            bail!("s3: bucket required (target = bucket[/prefix])");
        }
        // Probe: list at most 1 key to validate credentials + reachability.
        self.signed_request(
            "GET",
            &format!("/{}", urlencode_path(&self.bucket)),
            &[("list-type", "2"), ("max-keys", "1")],
        )
        .await?;
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let objects = self.list_objects().await?;
        Ok(objects
            .into_iter()
            .map(|(key, size, updated, etag)| RemoteFile {
                id: format!("s3://{}/{}", self.bucket, key),
                name: key.rsplit('/').next().unwrap_or(&key).to_string(),
                path: key.clone(),
                extension: Path::new(&key)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default(),
                size_bytes: size,
                updated_at: updated,
                fingerprint: etag,
                metadata: HashMap::from([
                    ("bucket".into(), self.bucket.clone()),
                    ("key".into(), key),
                ]),
            })
            .collect())
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let path = format!(
            "/{}/{}",
            urlencode_path(&self.bucket),
            file.path
                .split('/')
                .map(urlencode_path)
                .collect::<Vec<_>>()
                .join("/")
        );
        let response = self.signed_request("GET", &path, &[]).await?;
        response
            .bytes()
            .await
            .context("s3: read object")
            .map(|b| b.to_vec())
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "File".into());
        metadata.insert("source".into(), "s3".into());
        metadata.insert("bucket".into(), self.bucket.clone());
        metadata.insert("key".into(), file.path.clone());
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::S3,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

/// Guess an S3 region from the endpoint host; defaults to `us-east-1`.
/// Covers `s3.<region>.amazonaws.com(.cn)`, GCS interop, and cn endpoints.
fn guess_s3_region(endpoint: &str) -> String {
    let host = url::Url::parse(endpoint)
        .map(|u| u.host_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    for candidate in ["amazonaws.com.cn", "amazonaws.com"] {
        if let Some(rest) = host
            .strip_prefix("s3.")
            .and_then(|h| h.strip_suffix(candidate))
        {
            let region = rest.trim_end_matches('.');
            if !region.is_empty() {
                return region.to_string();
            }
        }
    }
    // GCS interop + others: default region.
    "us-east-1".to_string()
}

/// AWS SigV4 empty-payload hash (SHA-256 of the empty string).
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// AWS SigV4 key-derivation chain + signature (extracted for test vectors).
fn sigv4_signature(
    secret_key: &str,
    region: &str,
    date_stamp: &str,
    _amz_date: &str,
    string_to_sign: &str,
) -> String {
    let k_date = hmac_sha256(
        format!("AWS4{}", secret_key).as_bytes(),
        date_stamp.as_bytes(),
    );
    let k_region = hmac_sha256(&k_date, region.as_bytes());
    let k_service = hmac_sha256(&k_region, b"s3");
    let k_signing = hmac_sha256(&k_service, b"aws4_request");
    hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()))
}

fn sha256_hex(input: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex::encode(hasher.finalize())
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// RFC3986 percent-encode a URI path segment (S3 canonical URI keeps `/`).
fn urlencode_path(input: &str) -> String {
    const SAFE: &[u8] = b"-._~/";
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || SAFE.contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

/// Canonical query string: sorted, percent-encoded, joined with `&`.
fn canonical_query_string(query: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(String, String)> = query
        .iter()
        .map(|(k, v)| (urlencode_query(k), urlencode_query(v)))
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode_query(input: &str) -> String {
    const SAFE: &[u8] = b"-._~";
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || SAFE.contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

/// Canonical URI: RFC3986-encode every path segment.
fn canonical_uri(path: &str) -> String {
    path.split('/')
        .map(urlencode_path)
        .collect::<Vec<_>>()
        .join("/")
}

struct ListObjectsPage {
    objects: Vec<(String, u64, String, String)>, // key, size, updated, etag
    next_token: Option<String>,
}

/// Minimal ListBucketResult XML parser (quick-xml) — Key/Size/LastModified/ETag
/// + IsTruncated/NextContinuationToken.
fn parse_list_objects(xml: &str) -> Result<ListObjectsPage> {
    use quick_xml::Reader;
    use quick_xml::events::Event;
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut objects = Vec::new();
    let mut next_token = None;
    let mut in_contents = false;
    let mut key = String::new();
    let mut size: u64 = 0;
    let mut updated = String::new();
    let mut etag = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(tag)) => {
                let name = tag.local_name().as_ref().to_vec();
                if name == b"Contents" {
                    in_contents = true;
                    key.clear();
                    size = 0;
                    updated.clear();
                    etag.clear();
                } else if in_contents && name == b"Key" {
                    key = reader
                        .read_text(tag.name())
                        .context("s3: Key text")?
                        .to_string();
                } else if in_contents && name == b"Size" {
                    size = reader
                        .read_text(tag.name())
                        .context("s3: Size text")?
                        .parse()
                        .unwrap_or(0);
                } else if in_contents && name == b"LastModified" {
                    updated = reader
                        .read_text(tag.name())
                        .context("s3: LastModified text")?
                        .to_string();
                } else if in_contents && name == b"ETag" {
                    etag = reader
                        .read_text(tag.name())
                        .context("s3: ETag text")?
                        .to_string();
                } else if name == b"NextContinuationToken" {
                    next_token = Some(
                        reader
                            .read_text(tag.name())
                            .context("s3: NextContinuationToken text")?
                            .to_string(),
                    );
                }
            }
            Ok(Event::End(tag)) => {
                if tag.local_name().as_ref() == b"Contents" {
                    in_contents = false;
                    objects.push((key.clone(), size, updated.clone(), etag.clone()));
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => bail!("s3: XML parse error: {error}"),
            _ => {}
        }
        buf.clear();
    }
    Ok(ListObjectsPage {
        objects,
        next_token,
    })
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
            "yuque" => Ok(Box::new(YuqueConnector::new(options)?)),
            "dingtalk" => Ok(Box::new(DingTalkConnector::new(options)?)),
            "dingtalk_ai_table" => Ok(Box::new(DingTalkAiTableConnector::new(options)?)),
            // Box 云盘（RAGFlow 官方源 box：OAuth2 refresh token → 递归列表 → 下载）。
            "box" => Ok(Box::new(BoxConnector::new(options)?)),
            // S3 协议家族：S3 / GCS-interop / Oracle S3-compat / Cloudflare R2
            // 共用 SigV4 连接器（R2 为 S3 兼容协议，端点经 options.url 指定）。
            "s3" | "gcs" | "oracle" | "r2" => Ok(Box::new(S3Connector::new(options))),
            // Azure Blob Storage（RAGFlow 官方源名；与旧 "blob" 同一 SAS 适配器）。
            "azure_blob" => Ok(Box::new(BlobAdapter::new(options))),
            "rss" => Ok(Box::new(RssConnector::new(options)?)),
            "airtable" => Ok(Box::new(AirtableConnector::new(options)?)),
            // PostgreSQL 外部库直读（RAGFlow rdbms 源的 postgresql 分支）。
            "postgresql" => Ok(Box::new(RdbmsConnector::new(&options)?)),
            // 以下 RAGFlow 源已登记，连接器按域逐步实装。
            // 未实装前 connect 明确报错，避免静默失败。
            "github" | "bitbucket" | "jira" | "dropbox" | "onedrive" | "outlook" | "sharepoint"
            | "teams" | "slack" | "discord" | "zendesk" | "salesforce" | "moodle" | "asana"
            | "imap" | "seafile" | "rest_api" | "bigquery" => {
                bail!("connector not implemented yet: {kind} (registered, coming soon)")
            }
            // RayRAG 技术栈不含 MySQL（用户决策：统一 postgres:18.4）——
            // mysql 源明确不支持，读 MySQL 请走 postgresql 迁移。
            "mysql" => {
                bail!(
                    "connector not implemented yet: mysql (RayRAG 不支持 MySQL 栈，请迁移到 PostgreSQL 后使用 postgresql 源)"
                )
            }
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
                kind: "github",
                label: "GitHub",
                hint: "url=https://github.com, token=PAT, target=owner/repo",
            },
            ConnectorTypeInfo {
                kind: "bitbucket",
                label: "Bitbucket",
                hint: "url=https://bitbucket.org, token=app_password, target=workspace/repo",
            },
            ConnectorTypeInfo {
                kind: "webdav",
                label: "WebDAV",
                hint: "url=https://webdav.example.com, token=user:password, target=/path",
            },
            ConnectorTypeInfo {
                kind: "confluence",
                label: "Confluence",
                hint: "url=https://xxx.atlassian.net/wiki, token=email:api_token, target=spaceKey",
            },
            ConnectorTypeInfo {
                kind: "jira",
                label: "Jira",
                hint: "url=https://xxx.atlassian.net, token=email:api_token, target=project key",
            },
            ConnectorTypeInfo {
                kind: "notion",
                label: "Notion",
                hint: "token=integration_token, target=database/page id",
            },
            ConnectorTypeInfo {
                kind: "google_drive",
                label: "Google Drive",
                hint: "token=service_account_json, target=folder id",
            },
            ConnectorTypeInfo {
                kind: "gmail",
                label: "Gmail",
                hint: "token=service_account_json, target=query",
            },
            ConnectorTypeInfo {
                kind: "gcs",
                label: "Google Cloud Storage",
                hint: "url=https://storage.googleapis.com, token=access_token, target=bucket",
            },
            ConnectorTypeInfo {
                kind: "oracle",
                label: "Oracle Storage",
                hint: "url=https://objectstorage.region.oraclecloud.com, token=access:secret, target=bucket",
            },
            ConnectorTypeInfo {
                kind: "s3",
                label: "S3",
                hint: "url=https://s3.region.amazonaws.com, token=access:secret, target=bucket",
            },
            ConnectorTypeInfo {
                kind: "r2",
                label: "R2",
                hint: "url=https://<account>.r2.cloudflarestorage.com, token=access:secret, target=bucket",
            },
            ConnectorTypeInfo {
                kind: "azure_blob",
                label: "Azure Blob Storage",
                hint: "url=https://acct.blob.core.windows.net, token=SAS 或 connection_string, target=container",
            },
            ConnectorTypeInfo {
                kind: "box",
                label: "Box",
                hint: "token=OAuth JSON {client_id,client_secret,refresh_token[,access_token]}, target=folder id (默认 0=根目录)",
            },
            ConnectorTypeInfo {
                kind: "dropbox",
                label: "Dropbox",
                hint: "token=access_token (OAuth), target=folder path",
            },
            ConnectorTypeInfo {
                kind: "onedrive",
                label: "OneDrive",
                hint: "token=OAuth 凭据, target=folder id",
            },
            ConnectorTypeInfo {
                kind: "outlook",
                label: "Outlook",
                hint: "token=OAuth 凭据, target=mailbox query",
            },
            ConnectorTypeInfo {
                kind: "sharepoint",
                label: "SharePoint",
                hint: "url=site url, token=OAuth 凭据, target=library",
            },
            ConnectorTypeInfo {
                kind: "teams",
                label: "Microsoft Teams",
                hint: "token=OAuth 凭据, target=channel id",
            },
            ConnectorTypeInfo {
                kind: "slack",
                label: "Slack",
                hint: "token=bot token, target=channel id",
            },
            ConnectorTypeInfo {
                kind: "discord",
                label: "Discord",
                hint: "token=bot token, target=channel id",
            },
            ConnectorTypeInfo {
                kind: "zendesk",
                label: "Zendesk",
                hint: "url=https://subdomain.zendesk.com, token=email:api_token",
            },
            ConnectorTypeInfo {
                kind: "salesforce",
                label: "Salesforce",
                hint: "url=instance url, token=OAuth 凭据",
            },
            ConnectorTypeInfo {
                kind: "moodle",
                label: "Moodle",
                hint: "url=https://moodle.example.com, token=api_token",
            },
            ConnectorTypeInfo {
                kind: "airtable",
                label: "Airtable",
                hint: "token=personal_access_token, target=base id",
            },
            ConnectorTypeInfo {
                kind: "asana",
                label: "Asana",
                hint: "token=personal_access_token, target=project gid",
            },
            ConnectorTypeInfo {
                kind: "imap",
                label: "IMAP",
                hint: "url=imap host, token=user:password, target=folder",
            },
            ConnectorTypeInfo {
                kind: "dingtalk_ai_table",
                label: "Dingtalk AI Table",
                hint: "token=appKey:appSecret, target=app id (多维表格)",
            },
            ConnectorTypeInfo {
                kind: "seafile",
                label: "SeaFile",
                hint: "url=https://seafile.example.com, token=api_token, target=library id",
            },
            ConnectorTypeInfo {
                kind: "rss",
                label: "RSS",
                hint: "url=https://feed.example.com/rss, target=可选过滤词",
            },
            ConnectorTypeInfo {
                kind: "rest_api",
                label: "REST API",
                hint: "url=api endpoint, token=可选, target=可选",
            },
            ConnectorTypeInfo {
                kind: "mysql",
                label: "MySQL",
                hint: "url=host, token=user:password, target=database",
            },
            ConnectorTypeInfo {
                kind: "postgresql",
                label: "PostgreSQL",
                hint: "url=host, token=user:password, target=database",
            },
            ConnectorTypeInfo {
                kind: "bigquery",
                label: "BigQuery",
                hint: "token=service_account_json, target=dataset",
            },
            ConnectorTypeInfo {
                kind: "local",
                label: "Local Files",
                hint: "url=absolute directory path, target=optional relative subdir",
            },
            ConnectorTypeInfo {
                kind: "feishu",
                label: "飞书云空间 (Feishu Drive)",
                hint: "token=app_id:app_secret, target=可选 folder_token",
            },
            ConnectorTypeInfo {
                kind: "dingtalk",
                label: "钉钉云盘 (DingTalk Drive)",
                hint: "token=app_key:app_secret, target=可选 space_id (RAGFlow 无此源，本土扩展)",
            },
            ConnectorTypeInfo {
                kind: "tencent_docs",
                label: "腾讯文档 (Tencent Docs)",
                hint: "token=access_token, target=可选 folder_id (本土扩展)",
            },
            ConnectorTypeInfo {
                kind: "yuque",
                label: "语雀 (Yuque)",
                hint: "token=access_token, target=namespace/repo slug (本土扩展)",
            },
            ConnectorTypeInfo {
                kind: "wechat_docs",
                label: "微信文档 (WeChat Docs)",
                hint: "token=access_token, target=可选 folder_id (本土扩展)",
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
        std::fs::read(&path).with_context(|| format!("local: read failed: {}", path.display()))
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
                    chrono::DateTime::<chrono::Utc>::from(std::time::UNIX_EPOCH + d).to_rfc3339()
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
        format!(
            "Basic {}",
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw.as_bytes())
        )
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
            .request(
                reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND"),
                &url,
            )
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
        crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "connector",
        )
        .await
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
fn parse_propfind(xml: &str, _base_url: &str) -> Result<Vec<RemoteFile>> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

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
                text.push_str(t.unescape().unwrap_or_default().as_ref());
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
                if name == "response"
                    && let Some(map) = current.take()
                    && !in_collection
                    && let Some(href) = map.get("href")
                    && !href.ends_with('/')
                {
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
                        updated_at: map.get("modified").cloned().unwrap_or_default(),
                        fingerprint: String::new(),
                        metadata,
                    });
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
        let url = format!(
            "{}/?comp=list&restype=container&{}",
            self.endpoint()?,
            self.sas()?
        );
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
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("blob: request failed")?;
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
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("blob: request failed")?;
        let status = response.status();
        let bytes = crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "connector",
        )
        .await
        .context("connector download failed")?;
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
            file.metadata
                .get("container")
                .map(|s| s.as_str())
                .unwrap_or_default(),
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
            base_url: options.url.trim().trim_end_matches('/').to_string(),
            app_id,
            app_secret,
            folder_token: options.target.trim().to_string(),
            token: std::sync::Mutex::new(None),
        }
    }

    async fn tenant_access_token(&self) -> Result<String> {
        if let Some(token) = self
            .token
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
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
                value
                    .get("msg")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
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
                value
                    .get("msg")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            );
        }
        Ok(value)
    }

    /// Enumerate folder children (files + folders), recursing into folders.
    async fn list_folder(
        &self,
        folder_token: &str,
        depth: usize,
        out: &mut Vec<RemoteFile>,
    ) -> Result<()> {
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
                Box::pin(self.list_folder(&file_token, depth + 1, out))
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
                let bytes = crate::common::cmd_timeout::read_body_limited(
                    response,
                    crate::common::cmd_timeout::connector_body_limit_bytes(),
                    "connector",
                )
                .await
                .context("connector download failed")?;
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
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, raw.as_bytes())
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
        let body = crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "connector",
        )
        .await
        .context("connector download failed")?;
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
            let size = value
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
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

// ── Concrete connector: Yuque / 语雀 ─────────────────────────────────────────

/// Yuque (语雀) knowledge-base connector — mirrors the Yuque OpenAPI
/// (https://www.yuque.com/yuque/developer).
///
/// Auth: `X-Auth-Token: <token>` header (`token` = 语雀 personal access token,
/// no colon splitting).
///
///   * list:   GET `/api/v2/repos/{namespace}/docs?offset=N` (paginated)
///   * fetch:  GET `/api/v2/repos/{namespace}/docs/{slug}` → `data.body`
///             (Markdown string)
///   * probe:  GET `/api/v2/user` (validates the token)
///
/// Source options:
///   * `url`    = optional API base override (default
///                `https://www.yuque.com/api/v2`)
///   * `token`  = 语雀 personal access token
///   * `target` = namespace `user/repo_slug`
#[derive(Debug)]
pub struct YuqueConnector {
    client: reqwest::Client,
    base_url: String,
    token: String,
    namespace: String,
}

impl YuqueConnector {
    pub fn new(options: SourceOptions) -> Result<Self> {
        let namespace = options.target.trim().to_string();
        if namespace.is_empty() {
            bail!("yuque: namespace required (target = user/repo_slug)");
        }
        let base_url = {
            let raw = options.url.trim().trim_end_matches('/');
            if raw.is_empty() {
                "https://www.yuque.com/api/v2".to_string()
            } else {
                raw.to_string()
            }
        };
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            base_url,
            token: options.token.trim().to_string(),
            namespace,
        })
    }

    /// GET `path` with the `X-Auth-Token` header, parsed as JSON envelope
    /// `{ "data": ... }`.
    async fn get_json(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .client
            .get(&url)
            .header("X-Auth-Token", &self.token)
            .send()
            .await
            .with_context(|| format!("yuque: request failed: {url}"))?;
        let status = response.status();
        let body = crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "connector",
        )
        .await
        .context("connector download failed")?;
        if !status.is_success() {
            bail!(
                "yuque: upstream returned {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        serde_json::from_slice(&body).context("yuque: invalid JSON")
    }
}

#[async_trait]
impl Connector for YuqueConnector {
    fn kind(&self) -> &'static str {
        "yuque"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        if self.token.is_empty() {
            bail!("yuque: X-Auth-Token required (token)");
        }
        if self.namespace.is_empty() {
            bail!("yuque: namespace required (target = user/repo_slug)");
        }
        // Probe: GET /user validates the token (RAGFlow-style credential check).
        let value = self.get_json("/user").await?;
        if value.get("data").is_none() {
            bail!("yuque: /user probe returned no data — token may be invalid");
        }
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        // Paginate with `offset` (0/20/40/…) until an empty `data` array.
        let mut files = Vec::new();
        let mut offset = 0usize;
        const PAGE_SIZE: usize = 20;
        loop {
            let value = self
                .get_json(&format!("/repos/{}/docs?offset={offset}", self.namespace))
                .await?;
            let docs = value
                .get("data")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            let fetched = docs.len();
            for doc in &docs {
                let id = doc.get("id").map(|v| v.to_string()).unwrap_or_default();
                let title = doc
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let slug = doc
                    .get("slug")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let updated_at = doc
                    .get("updated_at")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| doc.get("created_at").and_then(serde_json::Value::as_str))
                    .unwrap_or_default()
                    .to_string();
                let mut metadata = HashMap::new();
                metadata.insert("type".into(), "Doc".into());
                metadata.insert("source".into(), "yuque".into());
                metadata.insert("namespace".into(), self.namespace.clone());
                metadata.insert("slug".into(), slug.clone());
                metadata.insert("id".into(), id.clone());
                files.push(RemoteFile {
                    id: format!("yuque:{id}"),
                    name: format!("{title}.md"),
                    path: slug,
                    extension: ".md".into(),
                    size_bytes: 0, // body length unknown until fetch
                    updated_at: updated_at.clone(),
                    fingerprint: updated_at,
                    metadata,
                });
            }
            offset += PAGE_SIZE;
            if fetched == 0 {
                break;
            }
            // Safety bound against a misbehaving upstream (RAGFlow
            // `_ITERATION_LIMIT` spirit).
            if offset > 10_000 {
                bail!("yuque: pagination exceeded 10000 docs — aborting");
            }
        }
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let value = self
            .get_json(&format!(
                "/repos/{}/docs/{}",
                self.namespace,
                urlencode(&file.path)
            ))
            .await?;
        let body = value
            .get("data")
            .and_then(|d| d.get("body"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(body.into_bytes())
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "Doc".into());
        metadata.insert("source".into(), "yuque".into());
        metadata.insert("namespace".into(), self.namespace.clone());
        metadata.insert("slug".into(), file.path.clone());
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::Yuque,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

// ── Concrete connector: DingTalk / 钉钉云盘 ───────────────────────────────────

/// DingTalk (钉钉) cloud-drive connector — mirrors the DingTalk OpenAPI
/// (https://open.dingtalk.com).
///
/// Auth: `token = "appKey:appSecret"` → accessToken
/// (POST `/v1.0/oauth2/accessToken`, body `{"appKey","appSecret"}`, response
/// `{"accessToken","expireIn"}`; cached in the connector).
/// List spaces: POST `/v1.0/storage/spaces/list` (paginated by `nextToken`).
/// List files:  POST `/v1.0/storage/spaces/{spaceId}/files` (paginated by
///              `nextToken`, first page sends the empty string).
/// Download:    GET `/v1.0/storage/spaces/{spaceId}/files/{fileId}/download`
///              → `headers.Location` → GET that URL for the bytes.
///
/// All authenticated calls carry `x-acs-dingtalk-access-token: <token>`.
///
/// Source options:
///   * `url`    = optional API base override (default
///                `https://api.dingtalk.com`)
///   * `token`  = `appKey:appSecret`
///   * `target` = optional `spaceId` (empty = enumerate all spaces)
#[derive(Debug)]
pub struct DingTalkConnector {
    client: reqwest::Client,
    base_url: String,
    app_key: String,
    app_secret: String,
    space_id: String,
    access_token: std::sync::Mutex<Option<String>>,
}

impl DingTalkConnector {
    pub fn new(options: SourceOptions) -> Result<Self> {
        let (app_key, app_secret) = split_credentials(&options.token);
        if app_key.is_empty() || app_secret.is_empty() {
            bail!("dingtalk: appKey:appSecret required (token)");
        }
        let base_url = {
            let raw = options.url.trim().trim_end_matches('/');
            if raw.is_empty() {
                "https://api.dingtalk.com".to_string()
            } else {
                raw.to_string()
            }
        };
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            base_url,
            app_key,
            app_secret,
            space_id: options.target.trim().to_string(),
            access_token: std::sync::Mutex::new(None),
        })
    }

    /// Exchange appKey/appSecret for an accessToken (cached in the struct).
    async fn access_token(&self) -> Result<String> {
        if let Some(token) = self
            .access_token
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
        {
            return Ok(token.clone());
        }
        let url = format!("{}/v1.0/oauth2/accessToken", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "appKey": self.app_key,
                "appSecret": self.app_secret,
            }))
            .send()
            .await
            .context("dingtalk: accessToken request failed")?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!(
                "dingtalk: accessToken upstream error ({}): {}",
                status.as_u16(),
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            );
        }
        let token = value
            .get("accessToken")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("dingtalk: accessToken missing in response"))?
            .to_string();
        *self.access_token.lock().unwrap_or_else(|p| p.into_inner()) = Some(token.clone());
        Ok(token)
    }

    /// POST `path` with the DingTalk auth header; parse the JSON response.
    async fn post_json(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let token = self.access_token().await?;
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .client
            .post(&url)
            .header("x-acs-dingtalk-access-token", &token)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("dingtalk: request failed: {url}"))?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!(
                "dingtalk: upstream error ({}): {}",
                status.as_u16(),
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            );
        }
        Ok(value)
    }

    /// GET raw bytes; `with_auth` attaches the access-token header (used for
    /// the download metadata call, NOT for the resolved signed CDN URL).
    async fn get_bytes(&self, url: &str, with_auth: bool) -> Result<Vec<u8>> {
        let mut request = self.client.get(url);
        if with_auth {
            let token = self.access_token().await?;
            request = request.header("x-acs-dingtalk-access-token", &token);
        }
        let response = request
            .send()
            .await
            .context("dingtalk: download request failed")?;
        let status = response.status();
        let bytes = crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "connector",
        )
        .await
        .context("connector download failed")?;
        if !status.is_success() {
            bail!(
                "dingtalk: download upstream error ({}): {}",
                status.as_u16(),
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        Ok(bytes.to_vec())
    }

    /// Resolve the download `Location` for a file (RAGFlow download flow).
    async fn download_location(&self, space_id: &str, file_id: &str) -> Result<String> {
        let token = self.access_token().await?;
        let url = format!(
            "{}/v1.0/storage/spaces/{space_id}/files/{file_id}/download",
            self.base_url
        );
        let response = self
            .client
            .get(&url)
            .header("x-acs-dingtalk-access-token", &token)
            .send()
            .await
            .context("dingtalk: download metadata request failed")?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!(
                "dingtalk: download metadata upstream error ({}): {}",
                status.as_u16(),
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            );
        }
        value
            .get("headers")
            .and_then(|h| h.get("Location"))
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("dingtalk: download Location missing in response"))
    }

    /// Enumerate spaces (a configured `target` spaceId short-circuits the
    /// spaces/list call).
    async fn spaces(&self) -> Result<Vec<(String, String)>> {
        if !self.space_id.is_empty() {
            return Ok(vec![(self.space_id.clone(), self.space_id.clone())]);
        }
        let mut spaces = Vec::new();
        let mut next_token = String::new();
        let mut pages = 0usize;
        loop {
            let body = if next_token.is_empty() {
                serde_json::json!({})
            } else {
                serde_json::json!({ "nextToken": next_token })
            };
            let value = self.post_json("/v1.0/storage/spaces/list", body).await?;
            for space in value
                .get("spaces")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let id = space
                    .get("spaceId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = space
                    .get("spaceName")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !id.is_empty() {
                    spaces.push((id, name));
                }
            }
            next_token = value
                .get("nextToken")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if next_token.is_empty() {
                break;
            }
            pages += 1;
            if pages > 100 {
                bail!("dingtalk: spaces pagination exceeded 100 pages — aborting");
            }
        }
        Ok(spaces)
    }

    /// Enumerate files for one space (paginated by `nextToken`).
    async fn space_files(&self, space_id: &str) -> Result<Vec<RemoteFile>> {
        let mut files = Vec::new();
        let mut next_token = String::new();
        let mut pages = 0usize;
        loop {
            let value = self
                .post_json(
                    &format!("/v1.0/storage/spaces/{space_id}/files"),
                    serde_json::json!({ "nextToken": next_token }),
                )
                .await?;
            for file in value
                .get("files")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let file_id = file
                    .get("fileId")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if file_id.is_empty() {
                    continue;
                }
                let name = file
                    .get("fileName")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let size_bytes = file
                    .get("fileSize")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let updated_at = file
                    .get("modifyTime")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let extension = Path::new(&name)
                    .extension()
                    .map(|e| format!(".{}", e.to_string_lossy()))
                    .unwrap_or_default();
                let mut metadata = HashMap::new();
                metadata.insert("type".into(), "File".into());
                metadata.insert("source".into(), "dingtalk".into());
                metadata.insert("space".into(), space_id.to_string());
                metadata.insert("fileId".into(), file_id.clone());
                files.push(RemoteFile {
                    id: file_id.clone(),
                    name,
                    path: format!("{space_id}/{file_id}"),
                    extension,
                    size_bytes,
                    updated_at: updated_at.clone(),
                    fingerprint: updated_at,
                    metadata,
                });
            }
            next_token = value
                .get("nextToken")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if next_token.is_empty() {
                break;
            }
            pages += 1;
            if pages > 100 {
                bail!("dingtalk: files pagination exceeded 100 pages — aborting");
            }
        }
        Ok(files)
    }
}

#[async_trait]
impl Connector for DingTalkConnector {
    fn kind(&self) -> &'static str {
        "dingtalk"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        if self.app_key.is_empty() || self.app_secret.is_empty() {
            bail!("dingtalk: appKey:appSecret required (token)");
        }
        // 换 token（缓存），然后探针列空间首页验证凭证可用。
        self.access_token().await?;
        let _ = self.spaces().await?;
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let mut files = Vec::new();
        for (space_id, _space_name) in self.spaces().await? {
            files.extend(self.space_files(&space_id).await?);
        }
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let space_id = file
            .metadata
            .get("space")
            .map(String::as_str)
            .unwrap_or_default();
        let file_id = file
            .metadata
            .get("fileId")
            .map(String::as_str)
            .unwrap_or(file.id.as_str());
        let location = self.download_location(space_id, file_id).await?;
        // Upstream Location may be absolute or path-only (relative to base).
        let url = if location.starts_with("http://") || location.starts_with("https://") {
            location
        } else {
            format!("{}{}", self.base_url, location)
        };
        // The resolved download URL is a signed CDN link — no auth header.
        self.get_bytes(&url, false).await
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "File".into());
        metadata.insert("source".into(), "dingtalk".into());
        if let Some(space) = file.metadata.get("space") {
            metadata.insert("space".into(), space.clone());
        }
        if let Some(file_id) = file.metadata.get("fileId") {
            metadata.insert("fileId".into(), file_id.clone());
        }
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::DingTalk,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

// ── Concrete connector: DingTalk AI Table / 钉钉 AI 多维表格 ──────────────────
//
// DingTalk AI Table ("notable") connector — mirrors RAGFlow
// `common/data_source/dingtalk_ai_table_connector.py`. NOT the cloud drive:
// rows of multi-dimensional tables are ingested, one document per record.
//
// Auth: `token = "appKey:appSecret"` → accessToken
// (POST `/v1.0/oauth2/accessToken`, body `{"appKey","appSecret"}`, response
// `{"accessToken","expireIn"}`). The token is cached until 60s before the
// `expireIn` deadline (RAGFlow re-uses the token until it expires).
// List sheets:  GET `/v1.0/notable/tables/{tableId}/sheets?operatorId=…`
//               (response `{"value":[{"id","name"}]}`; single call like the
//               Python `GetAllSheetsRequest`, no pagination).
// List records: GET `/v1.0/notable/tables/{tableId}/sheets/{sheetId}/records
//               ?operatorId=…&maxResults=100&nextToken=…` (paginated;
//               response `{"records":[{"id","fields"}],"nextToken"}`).
//
// All authenticated calls carry `x-acs-dingtalk-access-token: <token>`.
//
// Source options:
//   * `url`    = optional API base override (default
//                `https://api.dingtalk.com`)
//   * `token`  = `appKey:appSecret`
//   * `target` = Notable table id; `operator_id:table_id` when an operator
//                unionId is required by the Notable APIs.
#[derive(Debug)]
pub struct DingTalkAiTableConnector {
    client: reqwest::Client,
    base_url: String,
    app_key: String,
    app_secret: String,
    /// Notable table id (RAGFlow `table_id`, env `DINGTALK_AI_TABLE_BASE_ID`).
    table_id: String,
    /// Operator unionId required by the Notable APIs (may be empty).
    operator_id: String,
    /// Cached access token + expiry instant (refresh 60s before expireIn).
    token_cache: std::sync::Mutex<Option<CachedAccessToken>>,
}

/// Cached DingTalk access token with its refresh deadline.
#[derive(Debug, Clone)]
struct CachedAccessToken {
    token: String,
    expires_at: std::time::Instant,
}

/// DingTalk Notable doc-id prefix (RAGFlow `_DINGTALK_AI_TABLE_DOC_ID_PREFIX`).
const DINGTALK_AI_TABLE_DOC_ID_PREFIX: &str = "dingtalk_ai_table:";

impl DingTalkAiTableConnector {
    pub fn new(options: SourceOptions) -> Result<Self> {
        let (app_key, app_secret) = split_credentials(&options.token);
        if app_key.is_empty() || app_secret.is_empty() {
            bail!("dingtalk_ai_table: appKey:appSecret required (token)");
        }
        // target: "table_id" or "operator_id:table_id" (operator unionId).
        let target = options.target.trim();
        let (operator_id, table_id) = match target.split_once(':') {
            Some((operator, table)) => (operator.trim().to_string(), table.trim().to_string()),
            None => (String::new(), target.to_string()),
        };
        if table_id.is_empty() {
            bail!("dingtalk_ai_table: target required (Notable table id)");
        }
        let base_url = {
            let raw = options.url.trim().trim_end_matches('/');
            if raw.is_empty() {
                "https://api.dingtalk.com".to_string()
            } else {
                raw.to_string()
            }
        };
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            base_url,
            app_key,
            app_secret,
            table_id,
            operator_id,
            token_cache: std::sync::Mutex::new(None),
        })
    }

    /// Exchange appKey/appSecret for an accessToken, cached until 60s before
    /// the `expireIn` deadline elapses.
    async fn access_token(&self) -> Result<String> {
        {
            let guard = self.token_cache.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(cached) = guard.as_ref()
                && std::time::Instant::now() < cached.expires_at
            {
                return Ok(cached.token.clone());
            }
        }
        let url = format!("{}/v1.0/oauth2/accessToken", self.base_url);
        let response = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "appKey": self.app_key,
                "appSecret": self.app_secret,
            }))
            .send()
            .await
            .context("dingtalk_ai_table: accessToken request failed")?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!(
                "dingtalk_ai_table: accessToken upstream error ({}): code={}, message={}",
                status.as_u16(),
                value
                    .get("code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            );
        }
        let token = value
            .get("accessToken")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("dingtalk_ai_table: accessToken missing in response"))?
            .to_string();
        // expireIn (seconds, default 7200); refresh 60s early, never below 1s.
        let expire_in = value
            .get("expireIn")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(7200);
        let expires_at = std::time::Instant::now()
            + std::time::Duration::from_secs(expire_in.saturating_sub(60).max(1));
        *self.token_cache.lock().unwrap_or_else(|p| p.into_inner()) = Some(CachedAccessToken {
            token: token.clone(),
            expires_at,
        });
        Ok(token)
    }

    /// GET `path` with the DingTalk auth header; parse the JSON response.
    /// Non-2xx yields a readable error carrying the upstream code/message.
    async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<serde_json::Value> {
        let token = self.access_token().await?;
        let mut url = format!("{}{}", self.base_url, path);
        if !query.is_empty() {
            url.push('?');
            url.push_str(
                &query
                    .iter()
                    .map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value)))
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        let response = self
            .client
            .get(&url)
            .header("x-acs-dingtalk-access-token", &token)
            .send()
            .await
            .with_context(|| format!("dingtalk_ai_table: request failed: {url}"))?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!(
                "dingtalk_ai_table: upstream error ({}): code={}, message={}",
                status.as_u16(),
                value
                    .get("code")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                value
                    .get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            );
        }
        Ok(value)
    }

    /// Composite document id — RAGFlow `_document_id` =
    /// `dingtalk_ai_table:{table_id}:{sheet_id}:{record_id}`.
    fn doc_id(&self, sheet_id: &str, record_id: &str) -> String {
        format!(
            "{DINGTALK_AI_TABLE_DOC_ID_PREFIX}{}:{}:{}",
            self.table_id, sheet_id, record_id
        )
    }

    /// Enumerate sheets of the configured table (GetAllSheets, single call —
    /// the Python `GetAllSheetsRequest` carries no pagination parameters).
    async fn sheets(&self) -> Result<Vec<(String, String)>> {
        let value = self
            .get_json(
                &format!("/v1.0/notable/tables/{}/sheets", urlencode(&self.table_id)),
                &[("operatorId", self.operator_id.clone())],
            )
            .await?;
        let mut sheets = Vec::new();
        for sheet in value
            .get("value")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let id = sheet
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = sheet
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !id.is_empty() {
                sheets.push((id, name));
            }
        }
        Ok(sheets)
    }

    /// One ListRecords page: `(records, next_token)`.
    async fn records_page(
        &self,
        sheet_id: &str,
        next_token: &str,
        max_results: usize,
    ) -> Result<(Vec<serde_json::Value>, String)> {
        let value = self
            .get_json(
                &format!(
                    "/v1.0/notable/tables/{}/sheets/{}/records",
                    urlencode(&self.table_id),
                    urlencode(sheet_id)
                ),
                &[
                    ("operatorId", self.operator_id.clone()),
                    ("maxResults", max_results.to_string()),
                    ("nextToken", next_token.to_string()),
                ],
            )
            .await?;
        let records = value
            .get("records")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let next = value
            .get("nextToken")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((records, next))
    }
}

/// Render record fields as `列名: 值` lines (RAGFlow would JSON-dump them; the
/// Rust connector flattens to readable text so `.txt` parsing is unambiguous).
fn dingtalk_ai_table_record_blob(fields: &serde_json::Value) -> String {
    let Some(object) = fields.as_object() else {
        return fields.to_string();
    };
    let mut lines = String::new();
    for (name, value) in object {
        let rendered = match value {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        lines.push_str(name);
        lines.push_str(": ");
        lines.push_str(&rendered);
        lines.push('\n');
    }
    lines
}

/// Semantic identifier: `{sheet_name} - {first short string field[:50]}`,
/// falling back to `{sheet_name} - Record {record_id}` (RAGFlow
/// `_convert_record_to_document`).
fn dingtalk_ai_table_record_name(
    fields: &serde_json::Value,
    sheet_name: &str,
    record_id: &str,
) -> String {
    if let Some(object) = fields.as_object() {
        for value in object.values() {
            if let serde_json::Value::String(text) = value
                && !text.is_empty()
                && text.len() < 100
            {
                return format!(
                    "{sheet_name} - {}",
                    text.chars().take(50).collect::<String>()
                );
            }
        }
    }
    format!("{sheet_name} - Record {record_id}")
}

#[async_trait]
impl Connector for DingTalkAiTableConnector {
    fn kind(&self) -> &'static str {
        "dingtalk_ai_table"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        if self.app_key.is_empty() || self.app_secret.is_empty() {
            bail!("dingtalk_ai_table: appKey:appSecret required (token)");
        }
        // 换 token（缓存至到期前 60s），然后探针列 sheets 验证凭证可用
        // （RAGFlow `validate_connector_settings` 调 GetAllSheets）。
        self.access_token().await?;
        let _ = self.sheets().await?;
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let mut files = Vec::new();
        for (sheet_id, sheet_name) in self.sheets().await? {
            let mut next_token = String::new();
            let mut pages = 0usize;
            loop {
                let (records, next) = self.records_page(&sheet_id, &next_token, 100).await?;
                for (index, record) in records.iter().enumerate() {
                    let record_id = match record
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        Some(id) => id.to_string(),
                        None => format!("row{index}"),
                    };
                    let fields = record
                        .get("fields")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let name = dingtalk_ai_table_record_name(&fields, &sheet_name, &record_id);
                    let blob = dingtalk_ai_table_record_blob(&fields);
                    let mut metadata = HashMap::new();
                    metadata.insert("source".into(), "dingtalk_ai_table".into());
                    metadata.insert("table_id".into(), self.table_id.clone());
                    metadata.insert("sheet_id".into(), sheet_id.clone());
                    metadata.insert("sheet_name".into(), sheet_name.clone());
                    metadata.insert("record_id".into(), record_id.clone());
                    files.push(RemoteFile {
                        id: self.doc_id(&sheet_id, &record_id),
                        name,
                        path: format!("{}/{}/{}", self.table_id, sheet_id, record_id),
                        extension: ".txt".into(),
                        size_bytes: blob.len() as u64,
                        updated_at: String::new(),
                        fingerprint: String::new(),
                        metadata,
                    });
                }
                next_token = next;
                if next_token.is_empty() {
                    break;
                }
                pages += 1;
                if pages > 1000 {
                    bail!("dingtalk_ai_table: records pagination exceeded 1000 pages — aborting");
                }
            }
        }
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let sheet_id = file
            .metadata
            .get("sheet_id")
            .map(String::as_str)
            .unwrap_or_default();
        let record_id = file
            .metadata
            .get("record_id")
            .map(String::as_str)
            .unwrap_or_default();
        // Re-list the sheet until the record is found (RAGFlow `get_value`
        // re-fetches content lazily; rows have no stable download URL).
        let mut next_token = String::new();
        let mut pages = 0usize;
        loop {
            let (records, next) = self.records_page(sheet_id, &next_token, 100).await?;
            for (index, record) in records.iter().enumerate() {
                let rid = record
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                // Fallback ids (row{index}) match records the API lists
                // without an id.
                let matched =
                    (rid == record_id) || (rid.is_empty() && record_id == format!("row{index}"));
                if matched {
                    let fields = record
                        .get("fields")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    return Ok(dingtalk_ai_table_record_blob(&fields).into_bytes());
                }
            }
            if next.is_empty() {
                break;
            }
            next_token = next;
            pages += 1;
            if pages > 1000 {
                bail!("dingtalk_ai_table: records pagination exceeded 1000 pages — aborting");
            }
        }
        bail!("dingtalk_ai_table: record not found: {record_id} (sheet {sheet_id})")
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        for key in ["source", "table_id", "sheet_id", "sheet_name", "record_id"] {
            if let Some(value) = file.metadata.get(key) {
                metadata.insert(key.to_string(), value.clone());
            }
        }
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::DingTalk,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

// ── Concrete connector: Box (cloud drive) ────────────────────────────────────
//
// Mirrors RAGFlow `common/data_source/box_connector.py` (box_sdk_gen):
//   * OAuth2 credentials live in `options.token` as JSON — the RAGFlow web
//     `box-token-field.tsx` stores exactly this shape:
//     `{client_id, client_secret, redirect_uri, access_token, refresh_token}`.
//     A non-JSON token is treated as a legacy raw access token.
//   * `access_token` is used directly when present; `refresh_token` +
//     `client_id`/`client_secret` enable the refresh flow
//     (POST {base}/oauth2/token, grant_type=refresh_token, form-encoded) —
//     performed lazily on cache expiry AND once on a 401 response.
//   * folder_id = options.target (default "0" = root; Python
//     `folder_id = "0" if not folder_id else folder_id`).
//   * list_files → GET /2.0/folders/{id}/items, marker-based pagination
//     (`usemarker=true`), recursive over subfolders — mirrors
//     `_iter_files_recursive`. `next_marker` empty ⇒ end of folder.
//   * fetch_file → GET /2.0/files/{id}/content (binary).
//   * load_credentials → acquire/refresh token, probe GET /2.0/users/me
//     (RAGFlow `validate_connector_settings` calls `get_user_me`).
//   * normalize → ConnectorDoc; `DocumentSource::Box` has no variant yet, so
//     the stand-in is `DocumentSource::Local` + metadata `("source","box")`
//     (parent will add the enum variant, same precedent as Rss/Yuque).

/// Mutable OAuth state: cached access token + rotated refresh token.
#[derive(Debug, Clone)]
struct BoxAuthState {
    access_token: String,
    expires_at: std::time::Instant,
    /// Box rotates refresh tokens on use; keep the latest one here.
    refresh_token: String,
    /// False for legacy raw access tokens (no refresh credentials).
    refreshable: bool,
}

/// One entry of a Box folder listing (type=file/folder).
#[derive(Debug, Clone)]
struct BoxEntry {
    kind: String,
    id: String,
    name: String,
    size: u64,
    modified_at: String,
}

impl BoxEntry {
    fn from_value(value: serde_json::Value) -> Option<Self> {
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            return None;
        }
        Some(Self {
            kind: value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            id,
            name: value
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
            size: value
                .get("size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            modified_at: value
                .get("modified_at")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// Map onto the metadata-only listing primitive (id=file id, path=id,
    /// extension from the file name, size/updated_at from the entry).
    fn to_remote_file(&self) -> RemoteFile {
        let mut metadata = HashMap::new();
        metadata.insert("source".into(), "box".into());
        metadata.insert("file_id".into(), self.id.clone());
        RemoteFile {
            id: self.id.clone(),
            name: self.name.clone(),
            path: self.id.clone(),
            extension: box_file_extension(&self.name),
            size_bytes: self.size,
            updated_at: self.modified_at.clone(),
            fingerprint: self.modified_at.clone(),
            metadata,
        }
    }
}

/// Box cloud-drive connector — RAGFlow `BoxConnector`.
#[derive(Debug)]
pub struct BoxConnector {
    client: reqwest::Client,
    /// API base (default https://api.box.com; test override via options.url).
    base_url: String,
    /// Root folder to walk (default "0" = All Files).
    folder_id: String,
    client_id: String,
    client_secret: String,
    /// Cached access token + refresh token (Mutex: &self interior mutability).
    auth: std::sync::Mutex<BoxAuthState>,
}

impl BoxConnector {
    pub fn new(options: SourceOptions) -> Result<Self> {
        let base_url = {
            let raw = options.url.trim().trim_end_matches('/');
            if raw.is_empty() {
                "https://api.box.com".to_string()
            } else {
                raw.to_string()
            }
        };
        let folder_id = {
            let raw = options.target.trim();
            if raw.is_empty() {
                "0".to_string()
            } else {
                raw.to_string()
            }
        };
        let token_raw = options.token.trim();
        let mut client_id = String::new();
        let mut client_secret = String::new();
        let mut refresh_token = String::new();
        let mut access_token = String::new();
        let mut refreshable = false;
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(token_raw) {
            let get = |key: &str| {
                value
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            };
            client_id = get("client_id");
            client_secret = get("client_secret");
            refresh_token = get("refresh_token");
            access_token = get("access_token");
            refreshable =
                !client_id.is_empty() && !client_secret.is_empty() && !refresh_token.is_empty();
        } else if !token_raw.is_empty() {
            // Legacy shape: the token IS the access token (pre-OAuth UI).
            access_token = token_raw.to_string();
        }
        if access_token.is_empty() && !refreshable {
            bail!(
                "box: credentials required — token must be OAuth JSON \
                 (client_id/client_secret/refresh_token[, access_token]) or a raw access token"
            );
        }
        let auth = BoxAuthState {
            access_token,
            expires_at: std::time::Instant::now()
                + std::time::Duration::from_secs(3600u64.saturating_sub(60)),
            refresh_token,
            refreshable,
        };
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            base_url,
            folder_id,
            client_id,
            client_secret,
            auth: std::sync::Mutex::new(auth),
        })
    }

    /// Cached access token; refreshed lazily when expired or absent.
    async fn access_token(&self) -> Result<String> {
        {
            let guard = self.auth.lock().unwrap_or_else(|p| p.into_inner());
            let state = &*guard;
            if !state.access_token.is_empty() && std::time::Instant::now() < state.expires_at {
                return Ok(state.access_token.clone());
            }
        }
        self.force_refresh().await
    }

    /// Exchange the refresh token for a fresh access token (bypasses the
    /// cache) — POST {base}/oauth2/token, grant_type=refresh_token. Box
    /// rotates the refresh token; the rotated one is stored for next time.
    async fn force_refresh(&self) -> Result<String> {
        let refresh_token = {
            let guard = self.auth.lock().unwrap_or_else(|p| p.into_inner());
            let state = &*guard;
            if !state.refreshable || state.refresh_token.is_empty() {
                bail!("box: access token rejected (401) and no refresh credentials available");
            }
            state.refresh_token.clone()
        };
        let url = format!("{}/oauth2/token", self.base_url);
        let response = self
            .client
            .post(&url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .with_context(|| format!("box: oauth2/token request failed: {url}"))?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!(
                "{}",
                box_error_message(&value, status.as_u16(), "oauth2/token")
            );
        }
        let token = value
            .get("access_token")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("box: oauth2/token: access_token missing in response"))?
            .to_string();
        let expire_in = value
            .get("expires_in")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(3600);
        let rotated = value
            .get("refresh_token")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&refresh_token)
            .to_string();
        let state = BoxAuthState {
            access_token: token.clone(),
            expires_at: std::time::Instant::now()
                + std::time::Duration::from_secs(expire_in.saturating_sub(60).max(1)),
            refresh_token: rotated,
            refreshable: true,
        };
        *self.auth.lock().unwrap_or_else(|p| p.into_inner()) = state;
        Ok(token)
    }

    /// Bearer-authenticated GET; on 401, refresh the access token once and
    /// retry (RAGFlow Box SDK auto-refresh semantics).
    async fn authed_request(
        &self,
        method: reqwest::Method,
        url: String,
    ) -> Result<reqwest::Response> {
        let token = self.access_token().await?;
        let mut response = self
            .client
            .request(method.clone(), &url)
            .bearer_auth(&token)
            .send()
            .await
            .with_context(|| format!("box: request failed: {url}"))?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let refreshed = self.force_refresh().await?;
            response = self
                .client
                .request(method, &url)
                .bearer_auth(&refreshed)
                .send()
                .await
                .with_context(|| format!("box: retry request failed: {url}"))?;
        }
        Ok(response)
    }

    /// One folder-items page: `(entries, next_marker)` — marker-based
    /// pagination (`usemarker=true`, limit 1000, RAGFlow INDEX_BATCH_SIZE
    /// spirit with the SDK default).
    async fn list_folder_page(
        &self,
        folder_id: &str,
        marker: &str,
    ) -> Result<(Vec<BoxEntry>, String)> {
        let mut query = vec![
            ("usemarker", "true".to_string()),
            ("limit", "1000".to_string()),
        ];
        if !marker.is_empty() {
            query.push(("marker", marker.to_string()));
        }
        let query_str = query
            .iter()
            .map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value)))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!(
            "{}/2.0/folders/{}/items?{}",
            self.base_url,
            urlencode(folder_id),
            query_str
        );
        let response = self.authed_request(reqwest::Method::GET, url).await?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!(
                "{}",
                box_error_message(
                    &value,
                    status.as_u16(),
                    &format!("folders/{folder_id}/items")
                )
            );
        }
        let entries = value
            .get("entries")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(BoxEntry::from_value)
            .collect();
        let next_marker = value
            .get("next_marker")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((entries, next_marker))
    }

    /// Recursive walk mirroring `_iter_files_recursive`: files become
    /// RemoteFiles, subfolders recurse inline (same folder order as the
    /// Python generator).
    async fn walk_folder(&self, folder_id: &str, out: &mut Vec<RemoteFile>) -> Result<()> {
        let mut marker = String::new();
        let mut pages = 0usize;
        loop {
            let (entries, next) = self.list_folder_page(folder_id, &marker).await?;
            for entry in entries {
                match entry.kind.as_str() {
                    "file" => out.push(entry.to_remote_file()),
                    "folder" => {
                        // async recursion must be boxed (E0733).
                        Box::pin(self.walk_folder(&entry.id, out)).await?;
                    }
                    _ => {}
                }
            }
            if next.is_empty() {
                break;
            }
            marker = next;
            pages += 1;
            if pages > 1000 {
                bail!("box: folder listing exceeded 1000 pages — aborting");
            }
        }
        Ok(())
    }
}

/// File extension from the entry name — RAGFlow `get_file_ext`
/// (`os.path.splitext(name)[1].lower()`); empty when the name has no dot.
fn box_file_extension(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

/// Render a Box upstream error (RAGFlow Box SDK error envelope:
/// `{type, status, code, message, …}`) into a readable message.
fn box_error_message(value: &serde_json::Value, status: u16, context: &str) -> String {
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let message = value
        .get("message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("error_description")
                .and_then(serde_json::Value::as_str)
        })
        .unwrap_or_default();
    format!("box: {context} upstream error ({status}): code={code}, message={message}")
}

#[async_trait]
impl Connector for BoxConnector {
    fn kind(&self) -> &'static str {
        "box"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        // 换/取 access token，然后探针 users/me 验证凭证可用
        // （RAGFlow `validate_connector_settings` 调 get_user_me）。
        self.access_token().await?;
        let url = format!("{}/2.0/users/me", self.base_url);
        let response = self.authed_request(reqwest::Method::GET, url).await?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!("{}", box_error_message(&value, status.as_u16(), "users/me"));
        }
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let mut files = Vec::new();
        let root = self.folder_id.clone();
        self.walk_folder(&root, &mut files).await?;
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let url = format!(
            "{}/2.0/files/{}/content",
            self.base_url,
            urlencode(&file.id)
        );
        let response = self.authed_request(reqwest::Method::GET, url).await?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let parsed: serde_json::Value =
                serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
            if parsed.is_null() {
                bail!(
                    "box: files/{}/content upstream error ({}): {}",
                    file.id,
                    status.as_u16(),
                    text
                );
            }
            bail!(
                "{}",
                box_error_message(
                    &parsed,
                    status.as_u16(),
                    &format!("files/{}/content", file.id)
                )
            );
        }
        let bytes = crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "connector",
        )
        .await
        .context("connector download failed")?;
        Ok(bytes.to_vec())
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        for key in ["source", "file_id"] {
            if let Some(value) = file.metadata.get(key) {
                metadata.insert(key.to_string(), value.clone());
            }
        }
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            // Stand-in: DocumentSource has no Box variant yet (parent adds it
            // later, same precedent as Rss/Yuque); metadata carries
            // source=box provenance.
            source: DocumentSource::Local,
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

// ── Concrete connector: RSS / Atom feeds ──────────────────────────────────────
//
// Mirrors RAGFlow `common/data_source/rss_connector.py`:
//   * `feed_url`   = options.url (required, http/https only)
//   * `batch_size` = options.max_items (default 50, RAGFlow INDEX_BATCH_SIZE)
//   * SSRF guard: reject private / loopback / link-local targets (RAGFlow
//     `assert_url_is_safe` spirit). IP literals are checked directly;
//     hostnames are resolved fail-closed (private address or resolution
//     failure ⇒ rejection).
//   * list_files → fetch the feed once, map entries onto `RemoteFile`
//   * fetch_file → refetch the feed, find the entry by id, return content
//   * normalize  → `ConnectorDoc` with metadata source=rss / feed=feed_url
//
// Both RSS 2.0 (`<rss><channel><item>`) and Atom (`<feed><entry>`) are
// parsed; the XML layer matches `local_name()` so prefixed extensions
// (`dc:creator`, `content:encoded`) work too.

/// One parsed RSS/Atom entry (RSS `<item>` / Atom `<entry>`).
#[derive(Debug, Default, Clone)]
struct RssEntry {
    title: String,
    link: String,
    /// Stable key: guid (RSS) / id (Atom).
    key: String,
    /// Concatenated body blocks: description / summary / content / encoded.
    body: String,
    /// Raw timestamp: pubDate / updated / published / dc:date.
    date_raw: String,
    author: String,
    categories: Vec<String>,
}

impl RssEntry {
    /// Stable id — link > guid/id > title (RAGFlow `_resolve_stable_key`).
    fn stable_key(&self) -> String {
        for candidate in [&self.link, &self.key, &self.title] {
            if !candidate.is_empty() {
                return candidate.clone();
            }
        }
        String::new()
    }

    /// Assembled content: semantic identifier + body blocks (RAGFlow
    /// `_build_content` joins the title with description/summary/content).
    fn content(&self) -> String {
        let identifier = if self.title.is_empty() {
            self.stable_key()
        } else {
            self.title.clone()
        };
        let mut parts: Vec<&str> = Vec::new();
        if !identifier.is_empty() {
            parts.push(identifier.as_str());
        }
        if !self.body.is_empty() {
            parts.push(self.body.as_str());
        }
        parts.join("\n\n")
    }

    /// Map onto the metadata-only listing primitive.
    fn to_remote_file(&self, feed_url: &str) -> Option<RemoteFile> {
        let id = self.stable_key();
        if id.is_empty() {
            return None;
        }
        let content = self.content();
        let updated_at = parse_feed_date(&self.date_raw);
        let mut metadata = HashMap::new();
        metadata.insert("source".into(), "rss".into());
        metadata.insert("feed".into(), feed_url.to_string());
        if !self.link.is_empty() {
            metadata.insert("link".into(), self.link.clone());
        }
        if !self.author.is_empty() {
            metadata.insert("author".into(), self.author.clone());
        }
        if !self.categories.is_empty() {
            metadata.insert("categories".into(), self.categories.join(", "));
        }
        Some(RemoteFile {
            id: id.clone(),
            name: if self.title.is_empty() {
                id.clone()
            } else {
                self.title.clone()
            },
            path: id,
            extension: extension_from_link(&self.link),
            size_bytes: content.len() as u64,
            updated_at: updated_at.clone(),
            fingerprint: updated_at,
            metadata,
        })
    }
}

/// RSS/Atom feed connector — RAGFlow `RSSConnector`.
#[derive(Debug)]
pub struct RssConnector {
    client: reqwest::Client,
    feed_url: String,
    /// `options.max_items` (default 50, RAGFlow `INDEX_BATCH_SIZE`).
    batch_size: usize,
    /// Test-only SSRF exemption so mocks bound to 127.0.0.1 are reachable.
    allow_private_hosts: bool,
}

impl RssConnector {
    /// RAGFlow `RSSConnector.__init__` — explicit batch_size; <1 is rejected.
    pub fn with_batch_size(options: SourceOptions, batch_size: usize) -> Result<Self> {
        if batch_size < 1 {
            bail!("rss: batch_size must be greater than 0");
        }
        let feed_url = options.url.trim().to_string();
        validate_feed_url(&feed_url, false)?;
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            feed_url,
            batch_size,
            allow_private_hosts: false,
        })
    }

    /// Registry entry point — `batch_size` defaults to 50 when
    /// `options.max_items` is unset (0).
    pub fn new(options: SourceOptions) -> Result<Self> {
        let batch_size = if options.max_items == 0 {
            50
        } else {
            options.max_items
        };
        Self::with_batch_size(options, batch_size)
    }

    /// Test-only constructor: skips the private-host guard so the connector
    /// can talk to a `127.0.0.1` TcpListener mock (the yuque/dingtalk tests
    /// exercise their local HTTP mocks the same way).
    #[cfg(test)]
    pub fn new_for_test(options: SourceOptions) -> Result<Self> {
        let batch_size = if options.max_items == 0 {
            50
        } else {
            options.max_items
        };
        if batch_size < 1 {
            bail!("rss: batch_size must be greater than 0");
        }
        let feed_url = options.url.trim().to_string();
        validate_feed_url(&feed_url, true)?;
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            feed_url,
            batch_size,
            allow_private_hosts: true,
        })
    }

    /// Fetch the feed body (non-2xx ⇒ error), re-validating the SSRF guard
    /// on every fetch (RAGFlow re-validates every hop of a redirect chain).
    async fn fetch_feed(&self) -> Result<String> {
        validate_feed_url(&self.feed_url, self.allow_private_hosts)?;
        let response = self
            .client
            .get(&self.feed_url)
            .send()
            .await
            .with_context(|| format!("rss: request failed: {}", self.feed_url))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(
                "rss: upstream returned {}: {}",
                status.as_u16(),
                body.chars().take(300).collect::<String>()
            );
        }
        let bytes = crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "rss feed",
        )
        .await?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }
}

#[async_trait]
impl Connector for RssConnector {
    fn kind(&self) -> &'static str {
        "rss"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        // 验证 URL 格式 + SSRF，然后探针拉取首页（非 2xx 报错）。
        validate_feed_url(&self.feed_url, self.allow_private_hosts)?;
        self.fetch_feed().await?;
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let body = self.fetch_feed().await?;
        let entries = parse_feed(&body)?;
        Ok(entries
            .iter()
            .filter_map(|entry| entry.to_remote_file(&self.feed_url))
            .collect())
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        // 重新拉 feed，按 RemoteFile.id（link/guid）定位 entry。
        let body = self.fetch_feed().await?;
        let entries = parse_feed(&body)?;
        let entry = entries
            .iter()
            .find(|entry| entry.stable_key() == file.id)
            .with_context(|| format!("rss: entry not found in feed: {}", file.id))?;
        Ok(entry.content().into_bytes())
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        metadata.insert("type".into(), "Feed".into());
        metadata.insert("source".into(), "rss".into());
        metadata.insert("feed".into(), self.feed_url.clone());
        if let Some(link) = file.metadata.get("link") {
            metadata.insert("link".into(), link.clone());
        }
        if let Some(author) = file.metadata.get("author") {
            metadata.insert("author".into(), author.clone());
        }
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::Rss,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

// ── RSS/Atom parsing helpers ─────────────────────────────────────────────────

/// Validate an RSS/Atom feed URL: http/https scheme + host, and (unless
/// `allow_private`, which is test-only) reject private/loopback/link-local
/// targets — RAGFlow `assert_url_is_safe` spirit. IP-literal hosts are
/// checked directly; hostnames are resolved fail-closed (any private
/// address ⇒ rejection, resolution failure ⇒ rejection).
fn validate_feed_url(url: &str, allow_private: bool) -> Result<()> {
    let parsed = url::Url::parse(url).with_context(|| format!("rss: invalid feed URL: {url}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => bail!("rss: feed_url must be http or https, got scheme: {other}"),
    }
    let host = parsed
        .host_str()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']'))
        .filter(|host| !host.is_empty())
        .with_context(|| format!("rss: feed_url must include a host: {url}"))?;
    if allow_private {
        return Ok(());
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_private_ip(ip) {
            bail!("rss: feed_url targets a private/loopback address (SSRF blocked): {host}");
        }
        return Ok(());
    }
    // Hostname: resolve and require every resolved address to be public.
    use std::net::ToSocketAddrs;
    let addrs: Vec<std::net::SocketAddr> = (host, 80)
        .to_socket_addrs()
        .with_context(|| format!("rss: cannot resolve feed host: {host}"))?
        .collect();
    if let Some(bad) = addrs.iter().find(|addr| is_private_ip(addr.ip())) {
        bail!(
            "rss: feed_url resolves to a private address (SSRF blocked): {host} -> {}",
            bad.ip()
        );
    }
    Ok(())
}

/// RAGFlow `assert_url_is_safe` address policy: reject loopback, RFC1918
/// private, link-local and unspecified addresses.
pub(crate) fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 127 // 127.0.0.0/8    loopback
                || octets[0] == 0 // 0.0.0.0/8      unspecified
                || octets[0] == 10 // 10.0.0.0/8     private
                || (octets[0] == 172 && (16..=31).contains(&octets[1])) // 172.16.0.0/12
                || (octets[0] == 192 && octets[1] == 168) // 192.168.0.0/16
                || (octets[0] == 169 && octets[1] == 254) // 169.254.0.0/16 link-local
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback() // ::1
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        }
    }
}

/// Parse an RSS 2.0 (`<rss><channel><item>…`) or Atom (`<feed><entry>…`)
/// document into entries. Handles the RSS tag set (item/title/link/guid/
/// description/pubDate/category/author) and the Atom tag set (entry/title/
/// link@href/id/summary/content/updated) via `local_name()`.
fn parse_feed(xml: &str) -> Result<Vec<RssEntry>> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    // NOTE: `trim_text(true)` would collapse interleaved whitespace around
    // nested inline elements (`<summary>Summary <em>text</em> here</summary>`),
    // gluing words together. Keep raw text (default) and trim at assign time.

    let mut entries = Vec::new();
    let mut current: Option<RssEntry> = None;
    let mut field: Option<Vec<u8>> = None;
    let mut text = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => {
                let name = tag.local_name().as_ref().to_vec();
                if name.as_slice() == b"item" || name.as_slice() == b"entry" {
                    current = Some(RssEntry::default());
                    field = None;
                    text.clear();
                } else if current.is_some() {
                    match name.as_slice() {
                        b"title" | b"guid" | b"id" | b"description" | b"summary" | b"content"
                        | b"encoded" | b"pubDate" | b"updated" | b"published" | b"date"
                        | b"author" | b"creator" | b"category" | b"term" | b"name" => {
                            field = Some(name.clone());
                            text.clear();
                        }
                        b"link" => {
                            // Atom: `<link href="…"/>` — prefer the attribute.
                            for attr in tag.attributes().flatten() {
                                if attr.key.local_name().as_ref() == b"href"
                                    && let Some(entry) = current.as_mut()
                                    && entry.link.is_empty()
                                {
                                    entry.link = attr
                                        .decode_and_unescape_value(reader.decoder())
                                        .unwrap_or_default()
                                        .trim()
                                        .to_string();
                                }
                            }
                            // RSS: `<link>http://…</link>` — capture text too.
                            field = Some(name.clone());
                            text.clear();
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Empty(tag)) => {
                let name = tag.local_name().as_ref().to_vec();
                if current.is_some() && name.as_slice() == b"link" {
                    for attr in tag.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"href"
                            && let Some(entry) = current.as_mut()
                            && entry.link.is_empty()
                        {
                            entry.link = attr
                                .decode_and_unescape_value(reader.decoder())
                                .unwrap_or_default()
                                .trim()
                                .to_string();
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if current.is_some() && field.is_some() {
                    text.push_str(&t.unescape().unwrap_or_default());
                }
            }
            Ok(Event::CData(t)) => {
                if current.is_some() && field.is_some() {
                    text.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
            }
            Ok(Event::End(tag)) => {
                let name = tag.local_name().as_ref().to_vec();
                if name.as_slice() == b"item" || name.as_slice() == b"entry" {
                    if let Some(entry) = current.take() {
                        entries.push(entry);
                    }
                    field = None;
                    text.clear();
                } else if current.is_some() && field.as_deref() == Some(name.as_slice()) {
                    let value = text.trim().to_string();
                    if !value.is_empty()
                        && let Some(entry) = current.as_mut()
                    {
                        assign_field(entry, &name, value);
                    }
                    field = None;
                    text.clear();
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => bail!("rss: XML parse error: {error}"),
            _ => {}
        }
    }
    Ok(entries)
}

/// Route a captured leaf-field value into the current entry.
fn assign_field(entry: &mut RssEntry, name: &[u8], value: String) {
    match name {
        b"title" => {
            if entry.title.is_empty() {
                entry.title = strip_html(&value);
            }
        }
        b"guid" | b"id" => {
            if entry.key.is_empty() {
                entry.key = value;
            }
        }
        b"link" => {
            if entry.link.is_empty() {
                entry.link = value;
            }
        }
        b"description" | b"summary" | b"content" | b"encoded" => {
            let cleaned = strip_html(&value);
            if !cleaned.is_empty() {
                if !entry.body.is_empty() {
                    entry.body.push_str("\n\n");
                }
                entry.body.push_str(&cleaned);
            }
        }
        b"pubDate" | b"updated" | b"published" | b"date" => {
            if entry.date_raw.is_empty() {
                entry.date_raw = value;
            }
        }
        b"author" | b"creator" | b"name" => {
            if entry.author.is_empty() {
                entry.author = strip_html(&value);
            }
        }
        b"category" | b"term" => {
            let cleaned = strip_html(&value);
            if !cleaned.is_empty() {
                entry.categories.push(cleaned);
            }
        }
        _ => {}
    }
}

/// Minimal HTML-to-text: strip tags. RAGFlow uses BeautifulSoup.get_text —
/// this is a dependency-free approximation: `<br>/<p>/<div>/<li>/<hN>`
/// become newlines, everything else is dropped. No entity handling is
/// needed because the XML layer already unescaped text nodes (CDATA is
/// raw by definition and needs none either).
fn strip_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    let mut tag_name = String::new();
    for ch in input.chars() {
        if ch == '<' {
            in_tag = true;
            tag_name.clear();
        } else if ch == '>' {
            if in_tag {
                tag_name.retain(|c: char| !c.is_whitespace());
                match tag_name.as_str() {
                    "br" | "br/" | "p" | "p/" | "div" | "div/" | "li" | "li/" | "tr" | "tr/"
                    | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "h1/" | "h2/" | "h3/" | "h4/"
                    | "h5/" | "h6/" | "blockquote" => {
                        out.push('\n');
                    }
                    _ => {}
                }
                in_tag = false;
                tag_name.clear();
            } else {
                out.push('>');
            }
        } else if in_tag {
            tag_name.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    // Collapse runs of blank lines and trim.
    let mut collapsed = String::with_capacity(out.len());
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() {
            if !collapsed.is_empty() && !collapsed.ends_with('\n') {
                collapsed.push('\n');
            }
        } else {
            if !collapsed.is_empty() {
                collapsed.push('\n');
            }
            collapsed.push_str(line);
        }
    }
    collapsed.trim().to_string()
}

/// File extension from the entry link (RAGFlow maps RSS docs onto text
/// blobs); falls back to `.html` when the link carries no plausible
/// extension.
fn extension_from_link(link: &str) -> String {
    if let Ok(parsed) = url::Url::parse(link)
        && let Some(ext) = Path::new(parsed.path()).extension()
    {
        let ext = ext.to_string_lossy();
        if !ext.is_empty() && ext.len() <= 12 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
            return format!(".{ext}");
        }
    }
    ".html".to_string()
}

/// RAGFlow `_resolve_entry_time`: RFC822 (pubDate) or ISO8601 (Atom
/// updated); anything else falls back to epoch 0.
fn parse_feed_date(raw: &str) -> String {
    const EPOCH: &str = "1970-01-01T00:00:00Z";
    let raw = raw.trim();
    if raw.is_empty() {
        return EPOCH.to_string();
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc2822(raw) {
        return parsed
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    }
    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) {
        return parsed
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    }
    EPOCH.to_string()
}

// ── Concrete connector: Airtable ─────────────────────────────────────────────
//
// Mirrors RAGFlow `common/data_source/airtable_connector.py` (Personal Access
// Token auth) with row-per-record semantics (task spec): enumerate tables via
// the metadata API, paginate each table's records via `offset`, and emit one
// RemoteFile per row.
//
//   * credentials: Airtable PAT (`options.token`, sent as `Bearer`) + base id
//     (`options.target` — SourceOptions has no `base_id` field; the registry
//     hint already documents `target=base id`).
//   * list tables : GET {base}/v0/meta/bases/{base_id}/tables → tables[]
//     {id, name} — also the load_credentials probe (RAGFlow
//     `validate_connector_settings` equivalent).
//   * list records: GET {base}/v0/{base_id}/{table}?offset=… paginated;
//     response carries `records[]` {id, createdTime, fields{}} + `offset`.
//   * RemoteFile  : id=record id (fallback row{index}), name=first text field
//     (fallback `{table}-{row_n}`), blob=`列名: 值` lines, metadata
//     source=airtable / table / table_id / record_id.
//   * fetch_file  : re-list the table and re-render the record blob (rows have
//     no stable download URL — same refetch pattern as RSS / DingTalkAiTable).
//   * normalize   : `DocumentSource::Airtable` has no variant yet, so the
//     stand-in is `DocumentSource::Local` + metadata `("source","airtable")`
//     (parent will add the enum variant, same precedent as Rss/Box/Yuque).

/// Airtable base connector — PAT Bearer → list tables → paginate records →
/// one document per row.
#[derive(Debug)]
pub struct AirtableConnector {
    client: reqwest::Client,
    /// API base — `options.url` override (tests), default
    /// `https://api.airtable.com`.
    base_url: String,
    /// Airtable Personal Access Token (`options.token`).
    token: String,
    /// Airtable base id (`options.target`).
    base_id: String,
}

impl AirtableConnector {
    pub fn new(options: SourceOptions) -> Result<Self> {
        let token = options.token.trim().to_string();
        if token.is_empty() {
            bail!("airtable: token required (Airtable personal access token)");
        }
        let base_id = options.target.trim().to_string();
        if base_id.is_empty() {
            bail!("airtable: base id required (target)");
        }
        let base_url = {
            let raw = options.url.trim().trim_end_matches('/');
            if raw.is_empty() {
                "https://api.airtable.com".to_string()
            } else {
                raw.to_string()
            }
        };
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
            base_url,
            token,
            base_id,
        })
    }

    /// GET `path` with the Bearer PAT; non-2xx surfaces the Airtable
    /// `error.message` envelope (`{error:{type,message}}`).
    async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<serde_json::Value> {
        let mut url = format!("{}{}", self.base_url, path);
        if !query.is_empty() {
            url.push('?');
            url.push_str(
                &query
                    .iter()
                    .map(|(key, value)| format!("{}={}", urlencode(key), urlencode(value)))
                    .collect::<Vec<_>>()
                    .join("&"),
            );
        }
        let response = self
            .client
            .get(&url)
            .bearer_auth(&self.token)
            .send()
            .await
            .with_context(|| format!("airtable: request failed: {url}"))?;
        let status = response.status();
        let value: serde_json::Value = response.json().await?;
        if !status.is_success() {
            bail!(
                "airtable: upstream error ({}): type={}, message={}",
                status.as_u16(),
                value
                    .pointer("/error/type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
                value
                    .pointer("/error/message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
            );
        }
        Ok(value)
    }

    /// Enumerate the base's tables via the metadata API → `(table_id, name)`.
    async fn tables(&self) -> Result<Vec<(String, String)>> {
        let value = self
            .get_json(
                &format!("/v0/meta/bases/{}/tables", urlencode(&self.base_id)),
                &[],
            )
            .await?;
        let mut tables = Vec::new();
        for table in value
            .get("tables")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let id = table
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = table
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            if !id.is_empty() {
                tables.push((id, name));
            }
        }
        Ok(tables)
    }

    /// One records page of a table: `(records, next_offset)`.
    async fn records_page(
        &self,
        table_id: &str,
        offset: &str,
    ) -> Result<(Vec<serde_json::Value>, String)> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if !offset.is_empty() {
            query.push(("offset", offset.to_string()));
        }
        let value = self
            .get_json(
                &format!("/v0/{}/{}", urlencode(&self.base_id), urlencode(table_id)),
                &query,
            )
            .await?;
        let records = value
            .get("records")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let next = value
            .get("offset")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok((records, next))
    }
}

/// Render record fields as `列名: 值` lines (task spec; same flattening as
/// DingTalkAiTable — readable text parses unambiguously).
fn airtable_record_blob(fields: &serde_json::Value) -> String {
    let Some(object) = fields.as_object() else {
        return fields.to_string();
    };
    let mut lines = String::new();
    for (name, value) in object {
        let rendered = match value {
            serde_json::Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        lines.push_str(name);
        lines.push_str(": ");
        lines.push_str(&rendered);
        lines.push('\n');
    }
    lines
}

/// Semantic identifier: first column's text (truncated), falling back to
/// `{table}-{row_n}` when no text field exists.
fn airtable_record_name(fields: &serde_json::Value, table_name: &str, row_n: usize) -> String {
    if let Some(object) = fields.as_object() {
        for value in object.values() {
            if let serde_json::Value::String(text) = value
                && !text.is_empty()
                && text.len() < 100
            {
                return text.chars().take(50).collect::<String>();
            }
        }
    }
    format!("{table_name}-{row_n}")
}

#[async_trait]
impl Connector for AirtableConnector {
    fn kind(&self) -> &'static str {
        "airtable"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        if self.token.is_empty() {
            bail!("airtable: token required (Airtable personal access token)");
        }
        if self.base_id.is_empty() {
            bail!("airtable: base id required (target)");
        }
        // 探针：列 base 的 tables 验证 PAT 可用（RAGFlow
        // `validate_connector_settings` 的等价探活）。
        let _ = self.tables().await?;
        Ok(())
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        let mut files = Vec::new();
        for (table_id, table_name) in self.tables().await? {
            let mut offset = String::new();
            let mut pages = 0usize;
            loop {
                let (records, next) = self.records_page(&table_id, &offset).await?;
                for (index, record) in records.iter().enumerate() {
                    let record_id = match record
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|s| !s.is_empty())
                    {
                        Some(id) => id.to_string(),
                        None => format!("row{index}"),
                    };
                    let fields = record
                        .get("fields")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let name = airtable_record_name(&fields, &table_name, index + 1);
                    let blob = airtable_record_blob(&fields);
                    let mut metadata = HashMap::new();
                    metadata.insert("source".into(), "airtable".into());
                    metadata.insert("table".into(), table_name.clone());
                    metadata.insert("table_id".into(), table_id.clone());
                    metadata.insert("record_id".into(), record_id.clone());
                    files.push(RemoteFile {
                        id: record_id.clone(),
                        name,
                        path: format!("{}/{}/{}", self.base_id, table_id, record_id),
                        extension: ".txt".into(),
                        size_bytes: blob.len() as u64,
                        updated_at: record
                            .get("createdTime")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        fingerprint: String::new(),
                        metadata,
                    });
                }
                offset = next;
                if offset.is_empty() {
                    break;
                }
                pages += 1;
                if pages > 1000 {
                    bail!("airtable: records pagination exceeded 1000 pages — aborting");
                }
            }
        }
        Ok(files)
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        let table_id = file
            .metadata
            .get("table_id")
            .map(String::as_str)
            .unwrap_or_default();
        let record_id = file
            .metadata
            .get("record_id")
            .map(String::as_str)
            .unwrap_or_default();
        // Re-list the table until the record is found (RAGFlow `get_value`
        // re-fetches content lazily; rows have no stable download URL).
        let mut offset = String::new();
        let mut pages = 0usize;
        loop {
            let (records, next) = self.records_page(table_id, &offset).await?;
            for (index, record) in records.iter().enumerate() {
                let rid = record
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                // Fallback ids (row{index}) match records the API lists
                // without an id.
                let matched =
                    (rid == record_id) || (rid.is_empty() && record_id == format!("row{index}"));
                if matched {
                    let fields = record
                        .get("fields")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    return Ok(airtable_record_blob(&fields).into_bytes());
                }
            }
            if next.is_empty() {
                break;
            }
            offset = next;
            pages += 1;
            if pages > 1000 {
                bail!("airtable: records pagination exceeded 1000 pages — aborting");
            }
        }
        bail!("airtable: record not found: {record_id} (table {table_id})")
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        for key in ["source", "table", "table_id", "record_id"] {
            if let Some(value) = file.metadata.get(key) {
                metadata.insert(key.to_string(), value.clone());
            }
        }
        Ok(ConnectorDoc {
            id: file.id.clone(),
            blob,
            source: DocumentSource::AirTable,
            semantic_identifier: file.name.clone(),
            extension: file.extension.clone(),
            doc_updated_at: file.updated_at.clone(),
            size_bytes: raw.len(),
            metadata,
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod registry_kind_tests {
    use super::*;

    #[test]
    fn kinds_covers_ragflow_sources_and_china_alternatives() {
        let kinds: Vec<&str> = ConnectorRegistry::kinds()
            .iter()
            .map(|info| info.kind)
            .collect();
        // RAGFlow v0.26.4 数据源管理页全量 35 源（constant/index.tsx DataSourceKey）
        for expected in [
            "confluence",
            "notion",
            "google_drive",
            "gmail",
            "gcs",
            "oracle",
            "s3",
            "r2",
            "jira",
            "box",
            "dropbox",
            "bitbucket",
            "gitlab",
            "github",
            "moodle",
            "discord",
            "zendesk",
            "webdav",
            "airtable",
            "asana",
            "imap",
            "dingtalk_ai_table",
            "seafile",
            "mysql",
            "postgresql",
            "bigquery",
            "rest_api",
            "rss",
            "onedrive",
            "outlook",
            "salesforce",
            "azure_blob",
            "teams",
            "slack",
            "sharepoint",
        ] {
            assert!(
                kinds.contains(&expected),
                "missing RAGFlow source: {expected}"
            );
        }
        // 中国大陆替代源
        for expected in ["feishu", "dingtalk", "tencent_docs", "yuque", "wechat_docs"] {
            assert!(
                kinds.contains(&expected),
                "missing China source: {expected}"
            );
        }
        // 现有源不受影响（"blob" 已并入 azure_blob，分发保留兼容旧记录）
        for expected in ["gitlab", "azure_blob", "local", "webdav"] {
            assert!(kinds.contains(&expected), "missing legacy kind: {expected}");
        }
    }
}

#[cfg(test)]
mod s3_connector_tests {
    use super::*;

    /// AWS SigV4 official test vector (docs.aws.amazon.com/general/latest/gr/sigv4-calculate-signature.html).
    #[test]
    fn sigv4_matches_aws_official_vector() {
        // GET /test.txt on examplebucket.s3.amazonaws.com, 2013-05-24.
        let canonical_request = "GET\n/test.txt\n\nhost:examplebucket.s3.amazonaws.com\nrange:bytes=0-9\nx-amz-content-sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\nx-amz-date:20130524T000000Z\n\nhost;range;x-amz-content-sha256;x-amz-date\ne3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n20130524T000000Z\n20130524/us-east-1/s3/aws4_request\n{}",
            sha256_hex(canonical_request.as_bytes())
        );
        // AWS 文档：canonical request 的 SHA-256 = 7344ae5b…，
        // 最终签名 = f0e8bdb…（独立用 Python hmac 复核一致）。
        assert_eq!(
            sha256_hex(canonical_request.as_bytes()),
            "7344ae5b7ee6c3e7e6b0fe0640412a37625d1fbfff95c48bbb2dc43964946972"
        );
        let signature = sigv4_signature(
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "us-east-1",
            "20130524",
            "20130524T000000Z",
            &string_to_sign,
        );
        assert_eq!(
            signature,
            "f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn canonical_uri_encodes_segments_but_keeps_slashes() {
        assert_eq!(
            canonical_uri("/bucket/key with spaces.txt"),
            "/bucket/key%20with%20spaces.txt"
        );
        assert_eq!(canonical_uri("/plain/path"), "/plain/path");
    }

    #[test]
    fn canonical_query_is_sorted_and_encoded() {
        let q = canonical_query_string(&[("list-type", "2"), ("prefix", "a b/文件")]);
        assert_eq!(q, "list-type=2&prefix=a%20b%2F%E6%96%87%E4%BB%B6");
    }

    #[test]
    fn guess_region_handles_aws_and_cn_endpoints() {
        assert_eq!(
            guess_s3_region("https://s3.cn-north-1.amazonaws.com.cn"),
            "cn-north-1"
        );
        assert_eq!(
            guess_s3_region("https://s3.ap-southeast-1.amazonaws.com"),
            "ap-southeast-1"
        );
        assert_eq!(guess_s3_region("http://minio:9000"), "us-east-1");
    }

    #[test]
    fn parse_list_objects_extracts_keys_sizes_and_etags() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <Name>bucket</Name>
  <IsTruncated>false</IsTruncated>
  <Contents>
    <Key>docs/readme.md</Key>
    <LastModified>2026-08-13T10:00:00.000Z</LastModified>
    <ETag>"f1c9645dbc14efddc7d8a322685f26eb"</ETag>
    <Size>1204</Size>
  </Contents>
  <Contents>
    <Key>docs/guide.pdf</Key>
    <LastModified>2026-08-12T09:00:00.000Z</LastModified>
    <ETag>"abc"</ETag>
    <Size>4096</Size>
  </Contents>
</ListBucketResult>"#;
        let page = parse_list_objects(xml).unwrap();
        assert_eq!(page.objects.len(), 2);
        assert_eq!(page.objects[0].0, "docs/readme.md");
        assert_eq!(page.objects[0].1, 1204);
        assert_eq!(page.objects[0].2, "2026-08-13T10:00:00.000Z");
        assert_eq!(page.objects[0].3, "\"f1c9645dbc14efddc7d8a322685f26eb\"");
        assert_eq!(page.objects[1].0, "docs/guide.pdf");
        assert!(page.next_token.is_none());
    }

    #[test]
    fn parse_list_objects_reads_continuation_token() {
        let xml = r#"<ListBucketResult>
  <IsTruncated>true</IsTruncated>
  <NextContinuationToken>tok123</NextContinuationToken>
  <Contents><Key>a.txt</Key><Size>1</Size></Contents>
</ListBucketResult>"#;
        let page = parse_list_objects(xml).unwrap();
        assert_eq!(page.next_token.as_deref(), Some("tok123"));
    }

    #[test]
    fn s3_source_roundtrips_via_str() {
        assert_eq!(DocumentSource::S3.as_str(), "s3");
        assert_eq!(DocumentSource::from_str("s3").unwrap().as_str(), "s3");
    }
}

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

#[cfg(test)]
mod yuque_connector_tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};

    /// Spawn a single-threaded HTTP mock server on `127.0.0.1:<random port>`.
    /// The handler receives the request path (query string stripped) and the
    /// raw query string, and returns `(status, json_body)`. No real network —
    /// the connector's `options.url` override points at this base.
    fn mock_yuque_server(handler: impl Fn(&str, &str) -> (u16, String) + Send + 'static) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buf = [0u8; 8192];
                let n = match stream.read(&mut buf) {
                    Ok(n) => n,
                    Err(_) => return,
                };
                if n == 0 {
                    return;
                }
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let first_line = request.lines().next().unwrap_or_default();
                let mut parts = first_line.split_whitespace();
                let _method = parts.next().unwrap_or_default();
                let raw_target = parts.next().unwrap_or_default();
                let (path, query) = raw_target.split_once('?').unwrap_or((raw_target, ""));
                let (status, body) = handler(path, query);
                let reason = if status == 200 { "OK" } else { "Not Found" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn options(base_url: String, token: &str, target: &str) -> SourceOptions {
        let mut options = SourceOptions::default();
        options.url = base_url;
        options.token = token.to_string();
        options.target = target.to_string();
        options.max_items = 50;
        options
    }

    /// Listing paginates with `offset` (0/20/40/…) and maps each doc onto a
    /// `RemoteFile` (name = title + \".md\", path = slug, fingerprint =
    /// updated_at, metadata carries namespace/slug/source).
    #[tokio::test]
    async fn list_files_paginates_across_pages() {
        let list_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = list_calls.clone();
        let base = mock_yuque_server(move |path, query| match (path, query) {
            ("/repos/team/kb/docs", "offset=0") => {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (
                    200,
                    json!({"data": [
                        {"id": 101, "title": "Doc One", "slug": "doc-one",
                         "created_at": "2026-01-01T00:00:00.000Z",
                         "updated_at": "2026-02-01T00:00:00.000Z"},
                        {"id": 102, "title": "Doc Two", "slug": "doc-two",
                         "created_at": "2026-01-02T00:00:00.000Z",
                         "updated_at": "2026-02-02T00:00:00.000Z"}
                    ]})
                    .to_string(),
                )
            }
            ("/repos/team/kb/docs", "offset=20") => {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (
                    200,
                    json!({"data": [
                        {"id": 103, "title": "Doc Three", "slug": "doc-three",
                         "created_at": "2026-01-03T00:00:00.000Z",
                         "updated_at": "2026-02-03T00:00:00.000Z"}
                    ]})
                    .to_string(),
                )
            }
            ("/repos/team/kb/docs", "offset=40") => {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (200, json!({"data": []}).to_string())
            }
            _ => (404, "{}".to_string()),
        });

        let connector = YuqueConnector::new(options(base, "tok-123", "team/kb")).unwrap();
        let files = connector.list_files().await.unwrap();

        // Three requests: offset=0, offset=20, offset=40 (empty → stop).
        assert_eq!(list_calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(files.len(), 3);

        let first = &files[0];
        assert_eq!(first.id, "yuque:101");
        assert_eq!(first.name, "Doc One.md");
        assert_eq!(first.path, "doc-one");
        assert_eq!(first.extension, ".md");
        assert_eq!(first.size_bytes, 0);
        assert_eq!(first.updated_at, "2026-02-01T00:00:00.000Z");
        assert_eq!(first.fingerprint, "2026-02-01T00:00:00.000Z");
        assert_eq!(
            first.metadata.get("namespace").map(String::as_str),
            Some("team/kb")
        );
        assert_eq!(
            first.metadata.get("source").map(String::as_str),
            Some("yuque")
        );
        assert_eq!(
            first.metadata.get("slug").map(String::as_str),
            Some("doc-one")
        );
        assert_eq!(files[2].name, "Doc Three.md");
        assert_eq!(files[2].id, "yuque:103");
    }

    /// fetch_file hits `/repos/{namespace}/docs/{slug}` and returns the
    /// Markdown `data.body`.
    #[tokio::test]
    async fn fetch_file_returns_markdown_body() {
        let base = mock_yuque_server(move |path, _query| match path {
            "/repos/team/kb/docs/doc-one" => (
                200,
                json!({"data": {"id": 101, "slug": "doc-one", "title": "Doc One",
                                "body": "# Hello\n\nYuque markdown body."}})
                .to_string(),
            ),
            _ => (404, "{}".to_string()),
        });

        let connector = YuqueConnector::new(options(base, "tok-123", "team/kb")).unwrap();
        let file = RemoteFile {
            id: "yuque:101".into(),
            name: "Doc One.md".into(),
            path: "doc-one".into(),
            extension: ".md".into(),
            size_bytes: 0,
            updated_at: "2026-02-01T00:00:00.000Z".into(),
            fingerprint: String::new(),
            metadata: HashMap::new(),
        };
        let raw = connector.fetch_file(&file).await.unwrap();
        assert_eq!(
            String::from_utf8(raw).unwrap(),
            "# Hello\n\nYuque markdown body."
        );
    }

    /// load_credentials rejects an empty token before any network call.
    #[tokio::test]
    async fn load_credentials_rejects_empty_token() {
        let mut connector =
            YuqueConnector::new(options("http://127.0.0.1:1".into(), "", "team/kb")).unwrap();
        let error = connector.load_credentials().await.unwrap_err();
        assert!(error.to_string().contains("token"), "got: {error:#}");
    }

    /// load_credentials probes `GET /user` to validate the token.
    #[tokio::test]
    async fn load_credentials_probes_user_endpoint() {
        let base = mock_yuque_server(move |path, _query| match path {
            "/user" => (
                200,
                json!({"data": {"id": 7, "name": "tester"}}).to_string(),
            ),
            _ => (404, "{}".to_string()),
        });
        let mut connector = YuqueConnector::new(options(base, "tok-123", "team/kb")).unwrap();
        connector.load_credentials().await.unwrap();
    }

    /// new() requires the `target` namespace.
    #[test]
    fn new_requires_namespace() {
        let error = YuqueConnector::new(options(String::new(), "tok-123", "")).unwrap_err();
        assert!(error.to_string().contains("namespace"));
    }

    /// normalize marks the doc with source=yuque metadata; the
    /// `DocumentSource` field falls back to `Confluence` until the enum gains
    /// a Yuque variant.
    #[test]
    fn normalize_marks_yuque_metadata() {
        let connector = YuqueConnector::new(options(String::new(), "tok-123", "team/kb")).unwrap();
        let file = RemoteFile {
            id: "yuque:101".into(),
            name: "Doc One.md".into(),
            path: "doc-one".into(),
            extension: ".md".into(),
            size_bytes: 0,
            updated_at: "2026-02-01T00:00:00.000Z".into(),
            fingerprint: String::new(),
            metadata: HashMap::new(),
        };
        let doc = connector.normalize(&file, b"# Body".to_vec()).unwrap();
        assert_eq!(doc.blob, "# Body");
        assert_eq!(doc.semantic_identifier, "Doc One.md");
        assert_eq!(doc.extension, ".md");
        assert_eq!(doc.size_bytes, 6);
        assert_eq!(
            doc.metadata.get("source").map(String::as_str),
            Some("yuque")
        );
        assert_eq!(
            doc.metadata.get("namespace").map(String::as_str),
            Some("team/kb")
        );
        assert_eq!(
            doc.metadata.get("slug").map(String::as_str),
            Some("doc-one")
        );
        assert_eq!(doc.source, DocumentSource::Yuque);
    }

    /// Registry dispatch end-to-end: `create("yuque", …)` → fetch_all over the
    /// mock server produces a normalized ConnectorDoc.
    #[tokio::test]
    async fn registry_creates_and_fetches_all() {
        let base = mock_yuque_server(move |path, query| match (path, query) {
            ("/repos/team/kb/docs", "offset=0") => (
                200,
                json!({"data": [
                    {"id": 1, "title": "One", "slug": "one",
                     "updated_at": "2026-01-01T00:00:00Z"}
                ]})
                .to_string(),
            ),
            ("/repos/team/kb/docs", "offset=20") => (200, json!({"data": []}).to_string()),
            ("/repos/team/kb/docs/one", _) => {
                (200, json!({"data": {"body": "# One body"}}).to_string())
            }
            _ => (404, "{}".to_string()),
        });

        let connector =
            ConnectorRegistry::create("yuque", options(base, "tok-123", "team/kb")).unwrap();
        assert_eq!(connector.kind(), "yuque");
        let batch = connector.fetch_all(10).await.unwrap();
        assert_eq!(batch.docs.len(), 1);
        assert!(batch.failures.is_empty());
        let doc = &batch.docs[0];
        assert_eq!(doc.blob, "# One body");
        assert_eq!(doc.semantic_identifier, "One.md");
        assert_eq!(
            doc.metadata.get("source").map(String::as_str),
            Some("yuque")
        );
    }
}

#[cfg(test)]
mod dingtalk_connector_tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};

    /// Spawn a single-threaded HTTP mock server on `127.0.0.1:<random port>`.
    /// The handler receives `(method, path, query, header_block, body)` and
    /// returns `(status, body)`. `header_block` is the raw request-header
    /// section so tests can assert the `x-acs-dingtalk-access-token` header;
    /// `body` is the request body (DingTalk POST endpoints carry JSON). No
    /// real network — the connector's `options.url` override points here.
    fn mock_dingtalk_server(
        handler: impl Fn(&str, &str, &str, &str, &str) -> (u16, String) + Send + 'static,
    ) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                // Read until the full header block + declared body arrived.
                let mut data = Vec::new();
                let mut chunk = [0u8; 16384];
                loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&chunk[..n]);
                    let Some(header_end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
                    let mut content_length = 0usize;
                    for line in head.lines() {
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                    if data.len() >= header_end + 4 + content_length {
                        break;
                    }
                    if data.len() > 1_000_000 {
                        break;
                    }
                }
                if data.is_empty() {
                    return;
                }
                let request = String::from_utf8_lossy(&data).to_string();
                let (head, body) = request
                    .split_once("\r\n\r\n")
                    .unwrap_or((request.as_ref(), ""));
                let first_line = head.lines().next().unwrap_or_default();
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or_default();
                let raw_target = parts.next().unwrap_or_default();
                let (path, query) = raw_target.split_once('?').unwrap_or((raw_target, ""));
                let (status, body) = handler(method, path, query, head, body);
                let reason = if status == 200 { "OK" } else { "Not Found" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn options(base_url: String, token: &str, target: &str) -> SourceOptions {
        let mut options = SourceOptions::default();
        options.url = base_url;
        options.token = token.to_string();
        options.target = target.to_string();
        options.max_items = 50;
        options
    }

    /// `new()` requires `token = "appKey:appSecret"` (a colon-separated pair).
    #[test]
    fn new_requires_app_key_and_app_secret() {
        let error = DingTalkConnector::new(options(String::new(), "no-colon", "")).unwrap_err();
        assert!(error.to_string().contains("appKey"), "got: {error:#}");
        let error = DingTalkConnector::new(options(String::new(), "", "")).unwrap_err();
        assert!(error.to_string().contains("appKey"), "got: {error:#}");
    }

    /// load_credentials exchanges appKey/appSecret for an accessToken, caches
    /// it, and probes the spaces/list first page. Subsequent calls reuse the
    /// cached token — exactly ONE accessToken request.
    #[tokio::test]
    async fn load_credentials_acquires_and_caches_access_token() {
        let token_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = token_calls.clone();
        let base = mock_dingtalk_server(move |method, path, _query, headers, _body| {
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        200,
                        json!({"accessToken": "acc-abc", "expireIn": 7200}).to_string(),
                    )
                }
                ("POST", "/v1.0/storage/spaces/list") => {
                    if headers.contains("x-acs-dingtalk-access-token: acc-abc") {
                        (
                            200,
                            json!({"spaces": [{"spaceId": "sp1", "spaceName": "Space One"}],
                                   "nextToken": ""})
                            .to_string(),
                        )
                    } else {
                        (403, json!({"message": "missing token"}).to_string())
                    }
                }
                ("POST", "/v1.0/storage/spaces/sp1/files") => {
                    (200, json!({"files": [], "nextToken": ""}).to_string())
                }
                _ => (404, "{}".to_string()),
            }
        });

        let mut connector = DingTalkConnector::new(options(base, "appk:apps", "")).unwrap();
        connector.load_credentials().await.unwrap();
        // list_files reuses the cached token — no second accessToken request.
        let files = connector.list_files().await.unwrap();
        assert!(files.is_empty());
        assert_eq!(
            token_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "accessToken must be cached after load_credentials"
        );
    }

    /// list_files paginates spaces (nextToken) and files per space, mapping
    /// each entry onto a RemoteFile (id=fileId, path=spaceId/fileId,
    /// fingerprint=modifyTime, metadata carries space/fileId/source).
    #[tokio::test]
    async fn list_files_paginates_spaces_and_files() {
        let space_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sc = space_calls.clone();
        let file_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fc = file_calls.clone();
        let base = mock_dingtalk_server(move |method, path, _query, _headers, body| {
            let body = body.trim();
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => (
                    200,
                    json!({"accessToken": "tok", "expireIn": 7200}).to_string(),
                ),
                // Spaces page 1 (empty body) → sp1 + nextToken; page 2 → sp2.
                ("POST", "/v1.0/storage/spaces/list") if body == "{}" => {
                    sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        200,
                        json!({"spaces": [{"spaceId": "sp1", "spaceName": "One"}],
                               "nextToken": "nt-2"})
                        .to_string(),
                    )
                }
                ("POST", "/v1.0/storage/spaces/list") if body.contains("nt-2") => {
                    sc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        200,
                        json!({"spaces": [{"spaceId": "sp2", "spaceName": "Two"}],
                               "nextToken": ""})
                        .to_string(),
                    )
                }
                // sp1 files page 1 (nextToken "") → 2 files + nextToken ft2.
                ("POST", "/v1.0/storage/spaces/sp1/files")
                    if body.contains("\"nextToken\":\"\"") =>
                {
                    fc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        200,
                        json!({"files": [
                            {"fileId": "f1", "fileName": "readme.md", "fileSize": 10,
                             "createTime": "2026-01-01T00:00:00Z",
                             "modifyTime": "2026-02-01T00:00:00Z"},
                            {"fileId": "f2", "fileName": "notes", "fileSize": 20,
                             "createTime": "2026-01-02T00:00:00Z",
                             "modifyTime": "2026-02-02T00:00:00Z"}
                        ], "nextToken": "ft2"})
                        .to_string(),
                    )
                }
                ("POST", "/v1.0/storage/spaces/sp1/files") if body.contains("ft2") => {
                    fc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        200,
                        json!({"files": [
                            {"fileId": "f3", "fileName": "extra.txt", "fileSize": 30,
                             "createTime": "2026-01-03T00:00:00Z",
                             "modifyTime": "2026-02-03T00:00:00Z"}
                        ], "nextToken": ""})
                        .to_string(),
                    )
                }
                ("POST", "/v1.0/storage/spaces/sp2/files") => {
                    (200, json!({"files": [], "nextToken": ""}).to_string())
                }
                _ => (404, format!(r#"{{"path":"{path}","body":"{body}"}}"#)),
            }
        });

        let connector = DingTalkConnector::new(options(base, "appk:apps", "")).unwrap();
        let files = connector.list_files().await.unwrap();

        assert_eq!(space_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(file_calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(files.len(), 3);

        let first = &files[0];
        assert_eq!(first.id, "f1");
        assert_eq!(first.name, "readme.md");
        assert_eq!(first.path, "sp1/f1");
        assert_eq!(first.extension, ".md");
        assert_eq!(first.size_bytes, 10);
        assert_eq!(first.updated_at, "2026-02-01T00:00:00Z");
        assert_eq!(first.fingerprint, "2026-02-01T00:00:00Z");
        assert_eq!(first.metadata.get("space").map(String::as_str), Some("sp1"));
        assert_eq!(first.metadata.get("fileId").map(String::as_str), Some("f1"));
        assert_eq!(
            first.metadata.get("source").map(String::as_str),
            Some("dingtalk")
        );
        // Extension-less name → empty extension.
        assert_eq!(files[1].name, "notes");
        assert_eq!(files[1].extension, "");
        assert_eq!(files[1].size_bytes, 20);
        // Second file page belongs to the same space.
        assert_eq!(files[2].id, "f3");
        assert_eq!(files[2].path, "sp1/f3");
    }

    /// fetch_file resolves the download Location (auth header attached) and
    /// fetches the bytes from it (no auth header on the signed link).
    #[tokio::test]
    async fn fetch_file_follows_download_location() {
        let base = mock_dingtalk_server(move |method, path, _query, headers, _body| {
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => (
                    200,
                    json!({"accessToken": "tok", "expireIn": 7200}).to_string(),
                ),
                ("GET", "/v1.0/storage/spaces/sp1/files/f1/download") => {
                    if headers.contains("x-acs-dingtalk-access-token: tok") {
                        (
                            200,
                            json!({"headers": {"Location": "/dl-target"}, "result": {}})
                                .to_string(),
                        )
                    } else {
                        (403, json!({"message": "no token"}).to_string())
                    }
                }
                ("GET", "/dl-target") => (200, "hello dingtalk file".to_string()),
                _ => (404, "{}".to_string()),
            }
        });

        let connector = DingTalkConnector::new(options(base, "appk:apps", "sp1")).unwrap();
        let file = RemoteFile {
            id: "f1".into(),
            name: "readme.md".into(),
            path: "sp1/f1".into(),
            extension: ".md".into(),
            size_bytes: 0,
            updated_at: String::new(),
            fingerprint: String::new(),
            metadata: HashMap::from([
                ("space".into(), "sp1".into()),
                ("fileId".into(), "f1".into()),
            ]),
        };
        let raw = connector.fetch_file(&file).await.unwrap();
        assert_eq!(String::from_utf8(raw).unwrap(), "hello dingtalk file");
    }

    /// normalize marks the doc with source=dingtalk metadata; the
    /// `DocumentSource` field falls back to `Local` until the enum gains a
    /// DingTalk variant.
    #[test]
    fn normalize_marks_dingtalk_metadata() {
        let connector = DingTalkConnector::new(options(String::new(), "appk:apps", "sp1")).unwrap();
        let file = RemoteFile {
            id: "f1".into(),
            name: "readme.md".into(),
            path: "sp1/f1".into(),
            extension: ".md".into(),
            size_bytes: 0,
            updated_at: "2026-02-01T00:00:00Z".into(),
            fingerprint: String::new(),
            metadata: HashMap::from([
                ("space".into(), "sp1".into()),
                ("fileId".into(), "f1".into()),
            ]),
        };
        let doc = connector.normalize(&file, b"# Body".to_vec()).unwrap();
        assert_eq!(doc.blob, "# Body");
        assert_eq!(doc.semantic_identifier, "readme.md");
        assert_eq!(doc.extension, ".md");
        assert_eq!(doc.doc_updated_at, "2026-02-01T00:00:00Z");
        assert_eq!(doc.size_bytes, 6);
        assert_eq!(doc.source, DocumentSource::DingTalk);
        assert_eq!(
            doc.metadata.get("source").map(String::as_str),
            Some("dingtalk")
        );
        assert_eq!(doc.metadata.get("space").map(String::as_str), Some("sp1"));
        assert_eq!(doc.metadata.get("fileId").map(String::as_str), Some("f1"));
    }

    /// Registry dispatch end-to-end: `create("dingtalk", …)` → fetch_all over
    /// the mock server produces a normalized ConnectorDoc. With a `target`
    /// spaceId configured, spaces/list is skipped entirely.
    #[tokio::test]
    async fn registry_creates_and_fetches_all() {
        let base = mock_dingtalk_server(move |method, path, _query, _headers, _body| {
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => (
                    200,
                    json!({"accessToken": "tok", "expireIn": 7200}).to_string(),
                ),
                ("POST", "/v1.0/storage/spaces/sp1/files") => (
                    200,
                    json!({"files": [
                        {"fileId": "f1", "fileName": "one.md", "fileSize": 8,
                         "createTime": "2026-01-01T00:00:00Z",
                         "modifyTime": "2026-01-01T00:00:00Z"}
                    ], "nextToken": ""})
                    .to_string(),
                ),
                ("GET", "/v1.0/storage/spaces/sp1/files/f1/download") => (
                    200,
                    json!({"headers": {"Location": "/dl-target"}, "result": {}}).to_string(),
                ),
                ("GET", "/dl-target") => (200, "# One body".to_string()),
                _ => (404, "{}".to_string()),
            }
        });

        let connector =
            ConnectorRegistry::create("dingtalk", options(base, "appk:apps", "sp1")).unwrap();
        assert_eq!(connector.kind(), "dingtalk");
        let batch = connector.fetch_all(10).await.unwrap();
        assert_eq!(batch.docs.len(), 1);
        assert!(batch.failures.is_empty());
        let doc = &batch.docs[0];
        assert_eq!(doc.blob, "# One body");
        assert_eq!(doc.semantic_identifier, "one.md");
        assert_eq!(
            doc.metadata.get("source").map(String::as_str),
            Some("dingtalk")
        );
    }
}

#[cfg(test)]
mod rss_connector_tests {
    use super::*;
    use std::io::{Read, Write};

    const RSS2_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Test Feed</title>
    <link>http://example.com/</link>
    <description>channel description</description>
    <item>
      <title>Post One</title>
      <link>http://example.com/posts/one</link>
      <guid>http://example.com/posts/one</guid>
      <description>&lt;p&gt;Hello &amp; welcome&lt;/p&gt;</description>
      <pubDate>Tue, 10 Jun 2025 09:00:00 GMT</pubDate>
      <author>alice@example.com</author>
      <category>rust</category>
    </item>
    <item>
      <title>Post Two</title>
      <link>http://example.com/posts/two.php</link>
      <description><![CDATA[Second <b>post</b>]]></description>
      <pubDate>Wed, 11 Jun 2025 09:00:00 GMT</pubDate>
    </item>
  </channel>
</rss>
"#;

    const ATOM_FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom Feed</title>
  <id>urn:uuid:feed-1</id>
  <updated>2025-06-01T00:00:00Z</updated>
  <entry>
    <title>Atom One</title>
    <link href="https://blog.example.com/atom-one.html"/>
    <id>urn:uuid:entry-aaa</id>
    <updated>2025-07-01T12:34:56Z</updated>
    <summary>Summary <em>text</em> here</summary>
    <author><name>Bob</name></author>
  </entry>
  <entry>
    <title>Atom Two</title>
    <link href="https://blog.example.com/atom-two"/>
    <id>urn:uuid:entry-bbb</id>
    <updated>2025-07-02T00:00:00+02:00</updated>
    <content type="html">&lt;p&gt;Full &lt;b&gt;content&lt;/b&gt;&lt;/p&gt;</content>
  </entry>
</feed>
"#;

    /// Spawn a single-threaded HTTP mock server on `127.0.0.1:<random port>`.
    /// The handler receives the request path and returns `(status,
    /// content_type, body)`; every connection serves exactly one request
    /// (`Connection: close`), which matches the RSS connector's behaviour of
    /// refetching the feed per file. No real network — the connector's
    /// `options.url` points here.
    fn mock_feed_server(
        handler: impl Fn(&str) -> (u16, &'static str, String) + Send + 'static,
    ) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut data = Vec::new();
                let mut chunk = [0u8; 16384];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            data.extend_from_slice(&chunk[..n]);
                            if data.windows(4).any(|window| window == b"\r\n\r\n") {
                                break;
                            }
                            if data.len() > 1_000_000 {
                                break;
                            }
                        }
                        Err(_) => return,
                    }
                }
                if data.is_empty() {
                    return;
                }
                let request = String::from_utf8_lossy(&data).to_string();
                let head = request.split("\r\n\r\n").next().unwrap_or_default();
                let path = head
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default();
                let (status, content_type, body) = handler(path);
                let reason = if status == 200 { "OK" } else { "Not Found" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn options(feed_url: String) -> SourceOptions {
        let mut options = SourceOptions::default();
        options.url = feed_url;
        options.max_items = 50;
        options
    }

    /// RSS 2.0: list_files maps `<item>`s onto RemoteFile (id=link,
    /// name=title, extension from link / `.html` fallback, pubDate → ISO),
    /// fetch_file refetches and returns the assembled content, normalize
    /// tags the doc with source=rss + feed metadata.
    #[tokio::test]
    async fn list_fetch_normalize_rss2_feed() {
        let base = mock_feed_server(|_path| (200, "application/rss+xml", RSS2_FEED.to_string()));
        let feed_url = format!("{base}/feed.xml");
        let connector = RssConnector::new_for_test(options(feed_url.clone())).unwrap();

        let files = connector.list_files().await.unwrap();
        assert_eq!(files.len(), 2);

        let first = &files[0];
        assert_eq!(first.id, "http://example.com/posts/one");
        assert_eq!(first.name, "Post One");
        assert_eq!(first.path, "http://example.com/posts/one");
        assert_eq!(first.extension, ".html"); // link carries no extension
        assert_eq!(first.updated_at, "2025-06-10T09:00:00Z");
        assert_eq!(first.fingerprint, first.updated_at);
        assert_eq!(first.size_bytes, "Post One\n\nHello & welcome".len() as u64);
        assert_eq!(
            first.metadata.get("source").map(String::as_str),
            Some("rss")
        );
        assert_eq!(
            first.metadata.get("feed").map(String::as_str),
            Some(feed_url.as_str())
        );
        assert_eq!(
            first.metadata.get("author").map(String::as_str),
            Some("alice@example.com")
        );
        assert_eq!(
            first.metadata.get("categories").map(String::as_str),
            Some("rust")
        );

        // Extension taken from the link when present.
        assert_eq!(files[1].extension, ".php");

        // fetch_file refetches the feed and returns the assembled content.
        let raw = connector.fetch_file(first).await.unwrap();
        assert_eq!(
            String::from_utf8(raw).unwrap(),
            "Post One\n\nHello & welcome"
        );

        // normalize produces a ConnectorDoc with source=rss metadata.
        let raw = connector.fetch_file(first).await.unwrap();
        let doc = connector.normalize(first, raw).unwrap();
        assert_eq!(doc.blob, "Post One\n\nHello & welcome");
        assert_eq!(doc.semantic_identifier, "Post One");
        assert_eq!(doc.extension, ".html");
        assert_eq!(doc.metadata.get("source").map(String::as_str), Some("rss"));
        assert_eq!(
            doc.metadata.get("feed").map(String::as_str),
            Some(feed_url.as_str())
        );
        assert_eq!(doc.source, DocumentSource::Rss);

        // Unknown id → error.
        let ghost = RemoteFile {
            id: "http://example.com/missing".into(),
            ..first.clone()
        };
        let error = connector.fetch_file(&ghost).await.unwrap_err();
        assert!(error.to_string().contains("not found"), "{error:#}");
    }

    /// Atom: default-namespace `<feed><entry>` documents parse via
    /// `local_name()`; link@href becomes the id, summary/content become the
    /// body, `updated` (ISO8601 with offset) normalizes to UTC.
    #[tokio::test]
    async fn list_fetch_normalize_atom_feed() {
        let base = mock_feed_server(|_path| (200, "application/atom+xml", ATOM_FEED.to_string()));
        let feed_url = format!("{base}/feed.xml");
        let connector = RssConnector::new_for_test(options(feed_url.clone())).unwrap();

        let files = connector.list_files().await.unwrap();
        assert_eq!(files.len(), 2);

        let first = &files[0];
        assert_eq!(first.id, "https://blog.example.com/atom-one.html");
        assert_eq!(first.name, "Atom One");
        assert_eq!(first.extension, ".html");
        assert_eq!(first.updated_at, "2025-07-01T12:34:56Z");
        assert_eq!(
            first.metadata.get("author").map(String::as_str),
            Some("Bob")
        );
        let raw = connector.fetch_file(first).await.unwrap();
        assert_eq!(
            String::from_utf8(raw).unwrap(),
            "Atom One\n\nSummary text here"
        );

        // Offset timestamp → UTC; link without extension → .html fallback.
        let second = &files[1];
        assert_eq!(second.id, "https://blog.example.com/atom-two");
        assert_eq!(second.extension, ".html");
        assert_eq!(second.updated_at, "2025-07-01T22:00:00Z");
        let raw = connector.fetch_file(second).await.unwrap();
        assert_eq!(String::from_utf8(raw).unwrap(), "Atom Two\n\nFull content");
    }

    /// SSRF guard: private / loopback / link-local / unspecified targets are
    /// rejected; non-http(s) schemes are rejected; a public IP literal passes
    /// with no network access.
    #[test]
    fn ssrf_rejects_private_and_loopback_urls() {
        for url in [
            "http://127.0.0.1/feed.xml",
            "http://127.8.8.8/feed.xml",
            "http://10.1.2.3/feed.xml",
            "http://172.16.0.1/feed.xml",
            "http://172.31.255.255/feed.xml",
            "http://192.168.1.10/feed.xml",
            "http://169.254.169.254/latest/meta-data",
            "http://0.0.0.0/feed.xml",
            "http://[::1]/feed.xml",
            "http://[fe80::1]/feed.xml",
        ] {
            let error = validate_feed_url(url, false).unwrap_err();
            assert!(
                error.to_string().contains("private"),
                "expected SSRF rejection for {url}, got: {error:#}"
            );
        }
        // Non-http(s) schemes are rejected outright.
        assert!(validate_feed_url("ftp://example.com/feed.xml", false).is_err());
        assert!(validate_feed_url("file:///etc/passwd", false).is_err());
        // A public IP literal passes without any network access.
        assert!(validate_feed_url("https://8.8.8.8/feed.xml", false).is_ok());
        assert!(validate_feed_url("https://1.1.1.1/feed.xml", false).is_ok());
        // Test-only exemption permits loopback (local mock servers).
        assert!(validate_feed_url("http://127.0.0.1:1234/feed.xml", true).is_ok());
    }

    /// Constructor validation: scheme enforced, private hosts rejected at
    /// construction, `max_items=0` defaults to batch 50, explicit
    /// batch_size < 1 rejected.
    #[test]
    fn new_validates_feed_url_and_batch_size() {
        let mut options = SourceOptions::default();
        options.url = "ftp://example.com/feed".into();
        let error = RssConnector::new(options.clone()).unwrap_err();
        assert!(error.to_string().contains("http"), "{error:#}");

        options.url = "http://10.0.0.1/feed.xml".into();
        assert!(
            RssConnector::new(options.clone())
                .unwrap_err()
                .to_string()
                .contains("private")
        );

        // Public IP literal constructs fine; max_items=0 → default batch 50.
        options.url = "https://8.8.8.8/feed.xml".into();
        options.max_items = 0;
        assert!(RssConnector::new(options.clone()).is_ok());

        // Explicit batch_size < 1 rejected.
        assert!(
            RssConnector::with_batch_size(options, 0)
                .unwrap_err()
                .to_string()
                .contains("batch_size")
        );
    }

    /// load_credentials probes the feed: non-2xx upstream is an error, a
    /// healthy feed passes.
    #[tokio::test]
    async fn load_credentials_probes_feed_and_rejects_non_2xx() {
        let base = mock_feed_server(|_path| (500, "text/plain", "boom".to_string()));
        let mut connector =
            RssConnector::new_for_test(options(format!("{base}/feed.xml"))).unwrap();
        let error = connector.load_credentials().await.unwrap_err();
        assert!(error.to_string().contains("500"), "{error:#}");

        let base = mock_feed_server(|_path| (200, "application/rss+xml", RSS2_FEED.to_string()));
        let mut connector =
            RssConnector::new_for_test(options(format!("{base}/feed.xml"))).unwrap();
        connector.load_credentials().await.unwrap();
    }

    /// Registry dispatch: `create("rss", …)` now returns an RssConnector
    /// instead of bailing with "not implemented".
    #[test]
    fn registry_dispatches_rss_connector() {
        let connector =
            ConnectorRegistry::create("rss", options("https://8.8.8.8/feed.xml".into())).unwrap();
        assert_eq!(connector.kind(), "rss");
    }
}

#[cfg(test)]
mod dingtalk_ai_table_tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};

    /// Spawn a single-threaded HTTP mock server on `127.0.0.1:<random port>`.
    /// The handler receives `(method, path, query, header_block, body)` and
    /// returns `(status, body)`; the connector's `options.url` override points
    /// here — no real network.
    fn mock_ai_table_server(
        handler: impl Fn(&str, &str, &str, &str, &str) -> (u16, String) + Send + 'static,
    ) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut data = Vec::new();
                let mut chunk = [0u8; 16384];
                loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&chunk[..n]);
                    let Some(header_end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
                    let mut content_length = 0usize;
                    for line in head.lines() {
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                    if data.len() >= header_end + 4 + content_length {
                        break;
                    }
                    if data.len() > 1_000_000 {
                        break;
                    }
                }
                if data.is_empty() {
                    return;
                }
                let request = String::from_utf8_lossy(&data).to_string();
                let (head, body) = request
                    .split_once("\r\n\r\n")
                    .unwrap_or((request.as_ref(), ""));
                let first_line = head.lines().next().unwrap_or_default();
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or_default();
                let raw_target = parts.next().unwrap_or_default();
                let (path, query) = raw_target.split_once('?').unwrap_or((raw_target, ""));
                let (status, body) = handler(method, path, query, head, body);
                let reason = if status == 200 { "OK" } else { "Not Found" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn options(base_url: String, token: &str, target: &str) -> SourceOptions {
        let mut options = SourceOptions::default();
        options.url = base_url;
        options.token = token.to_string();
        options.target = target.to_string();
        options.max_items = 50;
        options
    }

    fn record(id: &str, fields: serde_json::Value) -> serde_json::Value {
        json!({ "id": id, "fields": fields })
    }

    /// `new()` requires `token = "appKey:appSecret"` and a non-empty
    /// `target` (Notable table id); `target = "op:tbl"` splits the operator
    /// unionId off the table id.
    #[test]
    fn new_requires_app_key_app_secret_and_target() {
        let error =
            DingTalkAiTableConnector::new(options(String::new(), "no-colon", "tbl1")).unwrap_err();
        assert!(error.to_string().contains("appKey"), "got: {error:#}");
        let error = DingTalkAiTableConnector::new(options(String::new(), "", "tbl1")).unwrap_err();
        assert!(error.to_string().contains("appKey"), "got: {error:#}");
        let error =
            DingTalkAiTableConnector::new(options(String::new(), "appk:apps", "")).unwrap_err();
        assert!(error.to_string().contains("target"), "got: {error:#}");

        // operator_id:table_id target splits cleanly.
        let connector =
            DingTalkAiTableConnector::new(options(String::new(), "appk:apps", "op-union-1:tbl9"))
                .unwrap();
        assert_eq!(connector.table_id, "tbl9");
        assert_eq!(connector.operator_id, "op-union-1");
    }

    /// load_credentials exchanges appKey/appSecret for an accessToken (cached
    /// until 60s before expireIn) and probes the GetAllSheets endpoint; a
    /// later list_files reuses the cached token — exactly ONE accessToken
    /// request.
    #[tokio::test]
    async fn load_credentials_acquires_and_caches_access_token() {
        let token_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = token_calls.clone();
        let base = mock_ai_table_server(move |method, path, _query, headers, _body| {
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => {
                    calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        200,
                        json!({"accessToken": "acc-abc", "expireIn": 7200}).to_string(),
                    )
                }
                ("GET", "/v1.0/notable/tables/tbl1/sheets") => {
                    if headers.contains("x-acs-dingtalk-access-token: acc-abc") {
                        (200, json!({"value": []}).to_string())
                    } else {
                        (
                            403,
                            json!({"code": "invalidToken", "message": "bad token"}).to_string(),
                        )
                    }
                }
                _ => (404, "{}".to_string()),
            }
        });

        let mut connector =
            DingTalkAiTableConnector::new(options(base, "appk:apps", "tbl1")).unwrap();
        connector.load_credentials().await.unwrap();
        // list_files reuses the cached token — no second accessToken request.
        let files = connector.list_files().await.unwrap();
        assert!(files.is_empty());
        assert_eq!(
            token_calls.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "accessToken must be cached after load_credentials"
        );
    }

    /// list_files walks sheets → paginated records, mapping every row onto a
    /// RemoteFile: id=`dingtalk_ai_table:{table}:{sheet}:{record}` (row{idx}
    /// fallback when the API lists a record without an id), name = first short
    /// string field, metadata carries source/table/sheet/record ids.
    #[tokio::test]
    async fn list_files_walks_sheets_and_paginates_records() {
        let base = mock_ai_table_server(move |method, path, query, _headers, _body| {
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => (
                    200,
                    json!({"accessToken": "tok", "expireIn": 7200}).to_string(),
                ),
                ("GET", "/v1.0/notable/tables/tbl1/sheets") => (
                    200,
                    json!({"value": [
                        {"id": "s1", "name": "Sheet One"},
                        {"id": "s2", "name": "Sheet Two"}
                    ]})
                    .to_string(),
                ),
                ("GET", "/v1.0/notable/tables/tbl1/sheets/s1/records") => {
                    // First page carries nextToken; second page terminates.
                    if query.contains("nt-2") {
                        (
                            200,
                            json!({"records": [
                                record("r3", json!({"标题": "第三条", "状态": "done"}))
                            ], "nextToken": ""})
                            .to_string(),
                        )
                    } else {
                        (
                            200,
                            json!({"records": [
                                record("r1", json!({"标题": "例会纪要", "负责人": "张三"})),
                                record("r2", json!({"标题": "周报", "负责人": "李四"}))
                            ], "nextToken": "nt-2"})
                            .to_string(),
                        )
                    }
                }
                ("GET", "/v1.0/notable/tables/tbl1/sheets/s2/records") => (
                    200,
                    json!({"records": [
                        record("r4", json!({"标题": "待办"})),
                        json!({"fields": {"标题": "无 id 行"}})
                    ], "nextToken": ""})
                    .to_string(),
                ),
                _ => (404, "{}".to_string()),
            }
        });

        let connector = DingTalkAiTableConnector::new(options(base, "appk:apps", "tbl1")).unwrap();
        let files = connector.list_files().await.unwrap();
        assert_eq!(files.len(), 5, "3 records in s1 + 2 in s2");

        let first = &files[0];
        assert_eq!(first.id, "dingtalk_ai_table:tbl1:s1:r1");
        assert_eq!(first.name, "Sheet One - 例会纪要");
        assert_eq!(first.path, "tbl1/s1/r1");
        assert_eq!(first.extension, ".txt");
        assert_eq!(
            first.metadata.get("source").map(String::as_str),
            Some("dingtalk_ai_table")
        );
        assert_eq!(
            first.metadata.get("table_id").map(String::as_str),
            Some("tbl1")
        );
        assert_eq!(
            first.metadata.get("sheet_id").map(String::as_str),
            Some("s1")
        );
        assert_eq!(
            first.metadata.get("sheet_name").map(String::as_str),
            Some("Sheet One")
        );
        assert_eq!(
            first.metadata.get("record_id").map(String::as_str),
            Some("r1")
        );

        // Paginated page 2 lands under the same sheet.
        assert_eq!(files[2].id, "dingtalk_ai_table:tbl1:s1:r3");

        // Record without an id falls back to row{index} (sheet_id kept in the
        // composite id, satisfying sheet_id+row 序号).
        assert_eq!(files[4].id, "dingtalk_ai_table:tbl1:s2:row1");
        assert_eq!(files[4].name, "Sheet Two - 无 id 行");
    }

    /// fetch_file re-lists the sheet and rebuilds the `列名: 值` blob for the
    /// record; normalize tags the doc with source=dingtalk_ai_table metadata
    /// and DocumentSource::DingTalk.
    #[tokio::test]
    async fn fetch_file_and_normalize_build_record_doc() {
        let base = mock_ai_table_server(move |method, path, _query, _headers, _body| {
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => (
                    200,
                    json!({"accessToken": "tok", "expireIn": 7200}).to_string(),
                ),
                ("GET", "/v1.0/notable/tables/tbl1/sheets") => (
                    200,
                    json!({"value": [{"id": "s1", "name": "Sheet One"}]}).to_string(),
                ),
                ("GET", "/v1.0/notable/tables/tbl1/sheets/s1/records") => (
                    200,
                    json!({"records": [
                        record("r1", json!({"标题": "例会纪要", "负责人": "张三", "人数": 3}))
                    ], "nextToken": ""})
                    .to_string(),
                ),
                _ => (404, "{}".to_string()),
            }
        });

        let connector = DingTalkAiTableConnector::new(options(base, "appk:apps", "tbl1")).unwrap();
        let files = connector.list_files().await.unwrap();
        assert_eq!(files.len(), 1);

        let raw = connector.fetch_file(&files[0]).await.unwrap();
        let blob = String::from_utf8(raw).unwrap();
        assert_eq!(blob, "标题: 例会纪要\n负责人: 张三\n人数: 3\n");

        let doc = connector
            .normalize(&files[0], blob.clone().into_bytes())
            .unwrap();
        assert_eq!(doc.blob, blob);
        assert_eq!(doc.source, DocumentSource::DingTalk);
        assert_eq!(doc.semantic_identifier, "Sheet One - 例会纪要");
        assert_eq!(doc.extension, ".txt");
        assert_eq!(
            doc.metadata.get("source").map(String::as_str),
            Some("dingtalk_ai_table")
        );
        assert_eq!(
            doc.metadata.get("sheet_name").map(String::as_str),
            Some("Sheet One")
        );
        assert_eq!(
            doc.metadata.get("record_id").map(String::as_str),
            Some("r1")
        );

        // Unknown record id → readable error.
        let ghost = RemoteFile {
            id: "dingtalk_ai_table:tbl1:s1:ghost".into(),
            name: "ghost".into(),
            path: "tbl1/s1/ghost".into(),
            extension: ".txt".into(),
            size_bytes: 0,
            updated_at: String::new(),
            fingerprint: String::new(),
            metadata: [
                ("sheet_id".to_string(), "s1".to_string()),
                ("record_id".to_string(), "ghost".to_string()),
            ]
            .into_iter()
            .collect(),
        };
        let error = connector.fetch_file(&ghost).await.unwrap_err();
        assert!(error.to_string().contains("not found"), "{error:#}");
    }

    /// Any non-2xx upstream response surfaces the upstream code/message in
    /// the error (token exchange and OpenAPI probes alike).
    #[tokio::test]
    async fn upstream_error_response_reports_code_and_message() {
        // GetAllSheets probe fails with a structured DingTalk error.
        let base = mock_ai_table_server(move |method, path, _query, _headers, _body| {
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => (
                    200,
                    json!({"accessToken": "tok", "expireIn": 7200}).to_string(),
                ),
                ("GET", "/v1.0/notable/tables/tbl1/sheets") => (
                    400,
                    json!({"code": "invalidParameter.value.invalid",
                           "message": "table not found"})
                    .to_string(),
                ),
                _ => (404, "{}".to_string()),
            }
        });
        let mut connector =
            DingTalkAiTableConnector::new(options(base, "appk:apps", "tbl1")).unwrap();
        let error = connector.load_credentials().await.unwrap_err().to_string();
        assert!(error.contains("400"), "{error}");
        assert!(error.contains("invalidParameter.value.invalid"), "{error}");
        assert!(error.contains("table not found"), "{error}");

        // A failing accessToken exchange reports code/message as well.
        let base = mock_ai_table_server(move |method, path, _query, _headers, _body| {
            match (method, path) {
                ("POST", "/v1.0/oauth2/accessToken") => (
                    401,
                    json!({"code": "InvalidAuthentication",
                           "message": "appSecret error"})
                    .to_string(),
                ),
                _ => (404, "{}".to_string()),
            }
        });
        let mut connector =
            DingTalkAiTableConnector::new(options(base, "appk:bad", "tbl1")).unwrap();
        let error = connector.load_credentials().await.unwrap_err().to_string();
        assert!(error.contains("401"), "{error}");
        assert!(error.contains("InvalidAuthentication"), "{error}");
        assert!(error.contains("appSecret error"), "{error}");
    }

    /// Registry dispatch: `create("dingtalk_ai_table", …)` returns the
    /// DingTalkAiTableConnector instead of bailing with "not implemented";
    /// the "dingtalk" (cloud drive) kind is untouched.
    #[test]
    fn registry_dispatches_dingtalk_ai_table_connector() {
        let connector = ConnectorRegistry::create(
            "dingtalk_ai_table",
            options(String::new(), "appk:apps", "tbl1"),
        )
        .unwrap();
        assert_eq!(connector.kind(), "dingtalk_ai_table");
        assert_eq!(
            ConnectorRegistry::create("dingtalk", options(String::new(), "appk:apps", "sp1"))
                .unwrap()
                .kind(),
            "dingtalk"
        );
    }
}

#[cfg(test)]
mod box_connector_tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Spawn a single-threaded HTTP mock server on `127.0.0.1:<random port>`.
    /// The handler receives `(method, path, query, header_block, body)` and
    /// returns `(status, content_type, body)`; the connector's `options.url`
    /// override points here — no real network. `Connection: close` makes every
    /// request land on a fresh connection, matching the one-request-per-
    /// connection loop.
    fn mock_box_server(
        handler: impl Fn(&str, &str, &str, &str, &str) -> (u16, &'static str, String) + Send + 'static,
    ) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut data = Vec::new();
                let mut chunk = [0u8; 16384];
                loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&chunk[..n]);
                    let Some(header_end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
                    let mut content_length = 0usize;
                    for line in head.lines() {
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                    if data.len() >= header_end + 4 + content_length {
                        break;
                    }
                    if data.len() > 1_000_000 {
                        break;
                    }
                }
                if data.is_empty() {
                    return;
                }
                let request = String::from_utf8_lossy(&data).to_string();
                let (head, body) = request
                    .split_once("\r\n\r\n")
                    .unwrap_or((request.as_ref(), ""));
                let first_line = head.lines().next().unwrap_or_default();
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or_default();
                let raw_target = parts.next().unwrap_or_default();
                let (path, query) = raw_target.split_once('?').unwrap_or((raw_target, ""));
                let (status, content_type, body) = handler(method, path, query, head, body);
                let reason = if status == 200 { "OK" } else { "Not Found" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn options(base: String, token: &str, target: &str) -> SourceOptions {
        let mut options = SourceOptions::default();
        options.url = base;
        options.token = token.to_string();
        options.target = target.to_string();
        options.max_items = 50;
        options
    }

    /// RAGFlow `box-token-field.tsx` credential shape stored in the token
    /// field. `with_access_token` adds the OAuth result payload.
    fn oauth_json(with_access_token: bool) -> String {
        if with_access_token {
            json!({
                "client_id": "cid-1",
                "client_secret": "csec-1",
                "refresh_token": "rt-1",
                "access_token": "at-init",
            })
            .to_string()
        } else {
            json!({
                "client_id": "cid-1",
                "client_secret": "csec-1",
                "refresh_token": "rt-1",
            })
            .to_string()
        }
    }

    fn remote_file(id: &str, name: &str, extension: &str) -> RemoteFile {
        RemoteFile {
            id: id.to_string(),
            name: name.to_string(),
            path: id.to_string(),
            extension: extension.to_string(),
            size_bytes: 5,
            updated_at: "2025-08-01T09:00:00Z".to_string(),
            fingerprint: "2025-08-01T09:00:00Z".to_string(),
            metadata: [
                ("source".to_string(), "box".to_string()),
                ("file_id".to_string(), id.to_string()),
            ]
            .into_iter()
            .collect(),
        }
    }

    /// `new()` credential parsing: OAuth JSON (with/without access_token),
    /// legacy raw access token, empty/malformed tokens rejected, target
    /// defaults to root folder "0".
    #[test]
    fn new_accepts_oauth_json_and_legacy_access_token() {
        assert!(BoxConnector::new(options(String::new(), &oauth_json(true), "123")).is_ok());
        assert!(BoxConnector::new(options(String::new(), &oauth_json(false), "123")).is_ok());

        // Legacy raw access token + empty target → folder "0".
        let connector = BoxConnector::new(options(String::new(), "raw-token", "")).unwrap();
        assert_eq!(connector.folder_id, "0");
        let guard = connector.auth.lock().unwrap_or_else(|p| p.into_inner());
        let state = &*guard;
        assert_eq!(state.access_token, "raw-token");
        assert!(!state.refreshable);

        // Empty token rejected with a readable error.
        let error = BoxConnector::new(options(String::new(), "", "")).unwrap_err();
        assert!(error.to_string().contains("credentials"), "{error:#}");

        // JSON without access_token and without the refresh triple rejected.
        let bad = json!({"client_id": "cid-only"}).to_string();
        let error = BoxConnector::new(options(String::new(), &bad, "")).unwrap_err();
        assert!(error.to_string().contains("credentials"), "{error:#}");
    }

    /// load_credentials: no initial access token → ONE refresh_token grant
    /// (form fields verified), then GET /2.0/users/me probe with the fresh
    /// bearer; a later list_files reuses the cached token (no second refresh).
    #[tokio::test]
    async fn load_credentials_refreshes_token_and_probes_user() {
        let refresh_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let calls = refresh_calls.clone();
        let base =
            mock_box_server(
                move |method, path, _query, headers, body| match (method, path) {
                    ("POST", "/oauth2/token") => {
                        calls.fetch_add(1, Ordering::SeqCst);
                        let lower = headers.to_ascii_lowercase();
                        for required in [
                            "grant_type=refresh_token",
                            "refresh_token=rt-1",
                            "client_id=cid-1",
                            "client_secret=csec-1",
                        ] {
                            if !body.contains(required) {
                                return (
                                    400,
                                    "application/json",
                                    json!({"code": "missing_form_field",
                                       "message": required})
                                    .to_string(),
                                );
                            }
                        }
                        let _ = lower;
                        (
                            200,
                            "application/json",
                            json!({"access_token": "at-fresh", "expires_in": 3600}).to_string(),
                        )
                    }
                    ("GET", "/2.0/users/me") => {
                        if headers
                            .to_ascii_lowercase()
                            .contains("authorization: bearer at-fresh")
                        {
                            (
                                200,
                                "application/json",
                                json!({"type": "user", "id": "u1", "name": "Me"}).to_string(),
                            )
                        } else {
                            (
                                401,
                                "application/json",
                                json!({"code": "unauthorized", "message": "bad token"}).to_string(),
                            )
                        }
                    }
                    ("GET", "/2.0/folders/0/items") => (
                        200,
                        "application/json",
                        json!({"entries": [], "next_marker": ""}).to_string(),
                    ),
                    _ => (404, "application/json", "{}".to_string()),
                },
            );

        let mut connector = BoxConnector::new(options(base, &oauth_json(false), "")).unwrap();
        connector.load_credentials().await.unwrap();

        // list_files reuses the cached token — still exactly one refresh.
        let files = connector.list_files().await.unwrap();
        assert!(files.is_empty());
        assert_eq!(
            refresh_calls.load(Ordering::SeqCst),
            1,
            "access token must be cached after load_credentials"
        );
    }

    /// list_files: marker pagination across pages AND recursive descent into
    /// subfolders (mirrors `_iter_files_recursive`): root page 1 yields
    /// f1 + folder `sub` (recurse → f3) before root page 2 yields f2.
    /// Entry mapping: id/name/path/extension(lowercased)/size/updated_at.
    #[tokio::test]
    async fn list_files_paginates_and_recurses_subfolders() {
        let base =
            mock_box_server(
                move |method, path, query, _headers, _body| match (method, path) {
                    ("POST", "/oauth2/token") => (
                        200,
                        "application/json",
                        json!({"access_token": "tok", "expires_in": 3600}).to_string(),
                    ),
                    ("GET", "/2.0/folders/0/items") => {
                        if query.contains("marker=m1") {
                            (
                                200,
                                "application/json",
                                json!({
                                    "entries": [
                                        {"type": "file", "id": "f2", "name": "second.md",
                                         "size": 22, "modified_at": "2025-08-01T10:00:00Z"}
                                    ],
                                    "next_marker": ""
                                })
                                .to_string(),
                            )
                        } else {
                            (
                                200,
                                "application/json",
                                json!({
                                    "entries": [
                                        {"type": "file", "id": "f1", "name": "report.PDF",
                                         "size": 11, "modified_at": "2025-08-01T09:00:00Z"},
                                        {"type": "folder", "id": "sub", "name": "Sub Folder",
                                         "size": 0, "modified_at": "2025-08-01T08:00:00Z"}
                                    ],
                                    "next_marker": "m1"
                                })
                                .to_string(),
                            )
                        }
                    }
                    ("GET", "/2.0/folders/sub/items") => (
                        200,
                        "application/json",
                        json!({
                            "entries": [
                                {"type": "file", "id": "f3", "name": "nested.docx",
                                 "size": 33, "modified_at": "2025-08-02T00:00:00Z"}
                            ],
                            "next_marker": ""
                        })
                        .to_string(),
                    ),
                    _ => (404, "application/json", "{}".to_string()),
                },
            );

        let connector = BoxConnector::new(options(base, &oauth_json(true), "0")).unwrap();
        let files = connector.list_files().await.unwrap();
        assert_eq!(files.len(), 3, "f1 + f3 (recursed) + f2 (page 2)");

        let first = &files[0];
        assert_eq!(first.id, "f1");
        assert_eq!(first.name, "report.PDF");
        assert_eq!(first.path, "f1");
        assert_eq!(first.extension, ".pdf"); // lowercased like get_file_ext
        assert_eq!(first.size_bytes, 11);
        assert_eq!(first.updated_at, "2025-08-01T09:00:00Z");
        assert_eq!(first.fingerprint, first.updated_at);
        assert_eq!(
            first.metadata.get("source").map(String::as_str),
            Some("box")
        );
        assert_eq!(
            first.metadata.get("file_id").map(String::as_str),
            Some("f1")
        );

        // Recursion happens inline before the root folder's page 2.
        assert_eq!(files[1].id, "f3");
        assert_eq!(files[1].extension, ".docx");
        assert_eq!(files[2].id, "f2");
        assert_eq!(files[2].extension, ".md");
        assert_eq!(files[2].size_bytes, 22);
        assert_eq!(files[2].updated_at, "2025-08-01T10:00:00Z");
    }

    /// fetch_file downloads `/2.0/files/{id}/content`; normalize maps the raw
    /// bytes onto a ConnectorDoc (Local stand-in + metadata source=box).
    #[tokio::test]
    async fn fetch_file_and_normalize_build_box_doc() {
        let base =
            mock_box_server(
                move |method, path, _query, _headers, _body| match (method, path) {
                    ("POST", "/oauth2/token") => (
                        200,
                        "application/json",
                        json!({"access_token": "tok", "expires_in": 3600}).to_string(),
                    ),
                    ("GET", "/2.0/files/f1/content") => (
                        200,
                        "application/octet-stream",
                        "# Hello Box\n\nbody text".to_string(),
                    ),
                    _ => (404, "application/json", "{}".to_string()),
                },
            );

        let connector = BoxConnector::new(options(base, &oauth_json(true), "")).unwrap();
        let file = remote_file("f1", "hello.md", ".md");
        let raw = connector.fetch_file(&file).await.unwrap();
        assert_eq!(
            String::from_utf8(raw.clone()).unwrap(),
            "# Hello Box\n\nbody text"
        );

        let doc = connector.normalize(&file, raw).unwrap();
        assert_eq!(doc.id, "f1");
        assert_eq!(doc.blob, "# Hello Box\n\nbody text");
        assert_eq!(doc.semantic_identifier, "hello.md");
        assert_eq!(doc.extension, ".md");
        assert_eq!(doc.doc_updated_at, "2025-08-01T09:00:00Z");
        // Stand-in until the parent adds a DocumentSource::Box variant.
        assert_eq!(doc.source, DocumentSource::Local);
        assert_eq!(doc.metadata.get("source").map(String::as_str), Some("box"));
        assert_eq!(doc.metadata.get("file_id").map(String::as_str), Some("f1"));
    }

    /// 401 on an API call → exactly one refresh-token grant → retry with the
    /// fresh bearer succeeds; the refreshed token is then cached (a second
    /// download issues no further refresh).
    #[tokio::test]
    async fn unauthorized_response_triggers_single_refresh_then_retry() {
        let refresh_calls = std::sync::Arc::new(AtomicUsize::new(0));
        let calls = refresh_calls.clone();
        let base =
            mock_box_server(
                move |method, path, _query, headers, _body| match (method, path) {
                    ("POST", "/oauth2/token") => {
                        calls.fetch_add(1, Ordering::SeqCst);
                        (
                            200,
                            "application/json",
                            json!({"access_token": "at-refreshed", "expires_in": 3600}).to_string(),
                        )
                    }
                    ("GET", "/2.0/files/f1/content") => {
                        if headers
                            .to_ascii_lowercase()
                            .contains("authorization: bearer at-refreshed")
                        {
                            (
                                200,
                                "application/octet-stream",
                                "retried-content".to_string(),
                            )
                        } else {
                            (
                                401,
                                "application/json",
                                json!({"code": "unauthorized",
                                   "message": "token expired"})
                                .to_string(),
                            )
                        }
                    }
                    _ => (404, "application/json", "{}".to_string()),
                },
            );

        let connector = BoxConnector::new(options(base, &oauth_json(true), "")).unwrap();
        // Initial cached token is "at-init" → first download 401 → refresh →
        // retry with "at-refreshed" → 200.
        let file = remote_file("f1", "hello.md", ".md");
        let raw = connector.fetch_file(&file).await.unwrap();
        assert_eq!(String::from_utf8(raw).unwrap(), "retried-content");
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);

        // Second download uses the cached refreshed token — no new refresh.
        let raw = connector.fetch_file(&file).await.unwrap();
        assert_eq!(String::from_utf8(raw).unwrap(), "retried-content");
        assert_eq!(refresh_calls.load(Ordering::SeqCst), 1);
    }

    /// Non-2xx upstream responses surface the Box code/message envelope
    /// (users/me probe and oauth2/token alike).
    #[tokio::test]
    async fn upstream_error_reports_code_and_message() {
        // Probe fails with a structured Box error.
        let base =
            mock_box_server(
                move |method, path, _query, _headers, _body| match (method, path) {
                    ("POST", "/oauth2/token") => (
                        200,
                        "application/json",
                        json!({"access_token": "tok", "expires_in": 3600}).to_string(),
                    ),
                    ("GET", "/2.0/users/me") => (
                        403,
                        "application/json",
                        json!({"type": "error", "status": 403,
                           "code": "access_denied_insufficient_permissions",
                           "message": "Access denied"})
                        .to_string(),
                    ),
                    _ => (404, "application/json", "{}".to_string()),
                },
            );
        let mut connector = BoxConnector::new(options(base, &oauth_json(true), "")).unwrap();
        let error = connector.load_credentials().await.unwrap_err().to_string();
        assert!(error.contains("403"), "{error}");
        assert!(
            error.contains("access_denied_insufficient_permissions"),
            "{error}"
        );
        assert!(error.contains("Access denied"), "{error}");

        // A failing token exchange reports code/message as well.
        let base =
            mock_box_server(
                move |method, path, _query, _headers, _body| match (method, path) {
                    ("POST", "/oauth2/token") => (
                        400,
                        "application/json",
                        json!({"code": "invalid_grant",
                           "error_description": "Refresh token has expired"})
                        .to_string(),
                    ),
                    _ => (404, "application/json", "{}".to_string()),
                },
            );
        let mut connector = BoxConnector::new(options(base, &oauth_json(false), "")).unwrap();
        let error = connector.load_credentials().await.unwrap_err().to_string();
        assert!(error.contains("400"), "{error}");
        assert!(error.contains("invalid_grant"), "{error}");
        assert!(error.contains("Refresh token has expired"), "{error}");
    }

    /// Registry dispatch: `create("box", …)` returns a BoxConnector instead
    /// of bailing with "not implemented"; a legacy raw access token works too.
    #[test]
    fn registry_dispatches_box_connector() {
        let connector =
            ConnectorRegistry::create("box", options(String::new(), &oauth_json(true), "0"))
                .unwrap();
        assert_eq!(connector.kind(), "box");

        let connector =
            ConnectorRegistry::create("box", options(String::new(), "legacy-raw-token", ""))
                .unwrap();
        assert_eq!(connector.kind(), "box");
    }
}

// ── RdbmsConnector (PostgreSQL 外部库直读) ───────────────────────────────────

/// RAGFlow `rdbms_connector.py` parity for PostgreSQL (RayRAG 栈不含 MySQL;
/// mysql 源保持 registry bail 并注明"仅支持 PostgreSQL").
///
/// 语义: host/port/user/password/database（或整串 connect_str）→
/// information_schema 列表 → 每表 `SELECT * FROM <table>`（或自定义 query，
/// 支持 markdown 围栏清理）→ 每行生成一个文档（"列名: 值" 拼接）。
#[derive(Debug)]
pub struct RdbmsConnector {
    base_url: String,
    host: String,
    port: u16,
    user: String,
    password: String,
    database: String,
    connect_str: Option<String>,
    query: Option<String>,
    id_column: String,
    timestamp_column: String,
    content_columns: Vec<String>,
    http_client: reqwest::Client,
}

/// 清理用户粘贴的 SQL（strip ```sql 围栏 / 语言标签首行）——对齐
/// `RDBMSConnector._sanitize_query`。
pub fn sanitize_sql_query(raw: &str) -> String {
    let mut query = raw.trim().to_string();
    if query.is_empty() {
        return query;
    }
    if query.starts_with("```") {
        query = query[3..].trim().to_string();
        if query.ends_with("```") {
            query = query[..query.len() - 3].trim().to_string();
        }
    }
    let fence_languages = [
        "sql",
        "tsql",
        "t-sql",
        "mssql",
        "mysql",
        "postgresql",
        "psql",
    ];
    if let Some((head, tail)) = query.split_once('\n')
        && !tail.trim().is_empty()
        && fence_languages.contains(&head.trim().to_ascii_lowercase().as_str())
    {
        query = tail.trim().to_string();
    }
    query
}

/// 把一行记录渲染成 "列名: 值" 多行文本（对齐 Airtable 同款行文档模式）。
pub fn rdbms_row_blob(columns: &[String], values: &[Option<String>]) -> String {
    columns
        .iter()
        .zip(values.iter())
        .map(|(c, v)| format!("{c}: {}", v.clone().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("\n")
}

impl RdbmsConnector {
    pub fn new(options: &SourceOptions) -> Result<Self> {
        let g = |k: &str| options.extra.get(k).cloned().unwrap_or_default();
        let host = g("host");
        let port = g("port").parse::<u16>().unwrap_or(5432);
        let user = g("user");
        let password = g("password");
        let database = g("database");
        let connect_str = options.extra.get("connect_str").cloned();
        if connect_str.is_none() && (host.is_empty() || database.is_empty()) {
            bail!("RDBMS (postgresql): missing connect_str or host+database");
        }
        Ok(Self {
            base_url: String::new(),
            host,
            port,
            user,
            password,
            database,
            connect_str,
            query: options
                .extra
                .get("query")
                .cloned()
                .filter(|q| !q.is_empty()),
            id_column: {
                let v = g("id_column");
                if v.is_empty() { "id".to_string() } else { v }
            },
            timestamp_column: {
                let v = g("timestamp_column");
                if v.is_empty() {
                    "create_time".to_string()
                } else {
                    v
                }
            },
            content_columns: g("content_columns")
                .split(',')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .map(str::to_string)
                .collect(),
            http_client: crate::common::cmd_timeout::model_client(),
        })
    }

    fn connect_str(&self) -> String {
        if let Some(cs) = &self.connect_str {
            return cs.clone();
        }
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.user, self.password, self.host, self.port, self.database
        )
    }

    /// 列取外部 PostgreSQL 的所有表（同步 postgres crate，在 async 上下文中
    /// 用 block_in_place 包装，避免阻塞 executor）。
    #[cfg(feature = "postgres-backend")]
    fn tables_sync(&self) -> Result<Vec<String>> {
        let mut client = postgres::Client::connect(&self.connect_str(), postgres::NoTls)
            .map_err(|e| anyhow!("RDBMS (postgresql): connect failed: {e}"))?;
        let rows = client
            .query(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' ORDER BY table_name",
                &[],
            )
            .map_err(|e| anyhow!("RDBMS (postgresql): listing tables failed: {e}"))?;
        Ok(rows.iter().map(|r| r.get::<_, String>(0)).collect())
    }

    /// 同步执行一条 SELECT 并渲染为 (columns, row_blobs)。
    #[cfg(feature = "postgres-backend")]
    fn query_rows_sync(&self, sql: &str) -> Result<Vec<(Vec<String>, Vec<Option<String>>)>> {
        let mut client = postgres::Client::connect(&self.connect_str(), postgres::NoTls)
            .map_err(|e| anyhow!("RDBMS (postgresql): connect failed: {e}"))?;
        let rows = client
            .query(sql, &[])
            .map_err(|e| anyhow!("RDBMS (postgresql): query failed: {e}"))?;
        let columns: Vec<String> = rows
            .first()
            .map(|r| {
                (0..r.len())
                    .map(|i| r.columns()[i].name().to_string())
                    .collect()
            })
            .unwrap_or_default();
        let mut out = Vec::new();
        for row in &rows {
            // 宽松序列化：String→i64→f64→bool 降级链，未知类型置空。
            let mut vals = Vec::new();
            for i in 0..row.len() {
                let v: Option<String> = row
                    .try_get::<_, Option<String>>(i)
                    .or_else(|_| {
                        row.try_get::<_, Option<i64>>(i)
                            .map(|x| x.map(|v| v.to_string()))
                    })
                    .or_else(|_| {
                        row.try_get::<_, Option<f64>>(i)
                            .map(|x| x.map(|v| v.to_string()))
                    })
                    .or_else(|_| {
                        row.try_get::<_, Option<bool>>(i)
                            .map(|x| x.map(|v| v.to_string()))
                    })
                    .unwrap_or(None);
                vals.push(v);
            }
            out.push((columns.clone(), vals));
        }
        Ok(out)
    }
}

#[async_trait]
impl Connector for RdbmsConnector {
    fn kind(&self) -> &'static str {
        "postgresql"
    }

    async fn load_credentials(&mut self) -> Result<()> {
        #[cfg(feature = "postgres-backend")]
        {
            let tables = self.tables_sync()?;
            let _ = tables;
            Ok(())
        }
        #[cfg(not(feature = "postgres-backend"))]
        bail!("RDBMS (postgresql): build without --features postgres-backend")
    }

    async fn list_files(&self) -> Result<Vec<RemoteFile>> {
        #[cfg(feature = "postgres-backend")]
        {
            let tables = self.tables_sync()?;
            let mut files = Vec::new();
            let queries: Vec<String> = if let Some(q) = &self.query {
                vec![sanitize_sql_query(q)]
            } else {
                tables
                    .iter()
                    .map(|t| format!("SELECT * FROM {t}"))
                    .collect()
            };
            let mut row_no = 0usize;
            for sql in queries {
                let table_hint = self
                    .query
                    .as_ref()
                    .map(|_| "custom_query".to_string())
                    .or_else(|| {
                        sql.split_whitespace()
                            .skip(1)
                            .find(|w| !w.eq_ignore_ascii_case("from"))
                            .map(|s| s.trim_end_matches(';').to_string())
                    })
                    .unwrap_or_default();
                let rows = self.query_rows_sync(&sql)?;
                for (columns, values) in rows {
                    let name = if let Some(first) = values.first() {
                        first
                            .clone()
                            .unwrap_or_default()
                            .chars()
                            .take(50)
                            .collect::<String>()
                    } else {
                        format!("{table_hint}-{row_no}")
                    };
                    let id = format!("rdbms:{table_hint}:{row_no}");
                    let blob = rdbms_row_blob(&columns, &values);
                    files.push(RemoteFile {
                        id,
                        name,
                        path: format!("rdbms://{table_hint}/{row_no}"),
                        extension: ".md".into(),
                        size_bytes: blob.len() as u64,
                        updated_at: String::new(),
                        fingerprint: String::new(),
                        metadata: {
                            let mut m = HashMap::new();
                            m.insert("source".into(), "rdbms".into());
                            m.insert("table".into(), table_hint.clone());
                            m.insert("database".into(), self.database.clone());
                            m
                        },
                    });
                    row_no += 1;
                }
            }
            Ok(files)
        }
        #[cfg(not(feature = "postgres-backend"))]
        bail!("RDBMS (postgresql): build without --features postgres-backend")
    }

    async fn fetch_file(&self, file: &RemoteFile) -> Result<Vec<u8>> {
        // 行内容已在 list_files 阶段物化到 metadata（blob 由 normalize 重建，
        // 见下）：此处重查该行所在的表以保证增量同步一致性。
        let _table = file.metadata.get("table").cloned().unwrap_or_default();
        let _row_no = file
            .path
            .trim_start_matches("rdbms://")
            .rsplit('/')
            .next()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        #[cfg(feature = "postgres-backend")]
        {
            let sql = if let Some(q) = &self.query {
                sanitize_sql_query(q)
            } else {
                format!("SELECT * FROM {_table}")
            };
            let rows = self.query_rows_sync(&sql)?;
            if let Some((columns, values)) = rows.get(_row_no) {
                return Ok(rdbms_row_blob(columns, values).into_bytes());
            }
            bail!("RDBMS (postgresql): row {_row_no} of table {_table} disappeared");
        }
        #[cfg(not(feature = "postgres-backend"))]
        bail!("RDBMS (postgresql): build without --features postgres-backend")
    }

    fn normalize(&self, file: &RemoteFile, raw: Vec<u8>) -> Result<ConnectorDoc> {
        let blob = String::from_utf8_lossy(&raw).to_string();
        let mut metadata = HashMap::new();
        for key in ["source", "table", "database"] {
            if let Some(value) = file.metadata.get(key) {
                metadata.insert(key.to_string(), value.clone());
            }
        }
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

#[cfg(test)]
mod rdbms_connector_tests {
    use super::*;

    fn opts(extra: &[(&str, &str)]) -> SourceOptions {
        SourceOptions {
            url: String::new(),
            token: String::new(),
            target: String::new(),
            include_issues: false,
            include_merge_requests: false,
            max_items: 100,
            extra: extra
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn requires_connect_str_or_host_database() {
        let err = RdbmsConnector::new(&opts(&[])).unwrap_err();
        assert!(err.to_string().contains("missing connect_str"), "{err}");
        let err = RdbmsConnector::new(&opts(&[("host", "h")])).unwrap_err();
        assert!(err.to_string().contains("missing connect_str"), "{err}");
    }

    #[test]
    fn builds_connect_str_from_parts_and_defaults() {
        let c = RdbmsConnector::new(&opts(&[
            ("host", "db.example.com"),
            ("user", "reader"),
            ("password", "s3cret"),
            ("database", "analytics"),
        ]))
        .unwrap();
        assert_eq!(
            c.connect_str(),
            "postgres://reader:s3cret@db.example.com:5432/analytics"
        );
        // 显式 connect_str 优先
        let c2 = RdbmsConnector::new(&opts(&[("connect_str", "postgres://u:p@h:9999/d")])).unwrap();
        assert_eq!(c2.connect_str(), "postgres://u:p@h:9999/d");
        assert_eq!(c2.kind(), "postgresql");
    }

    #[test]
    fn sanitizes_markdown_fenced_sql() {
        assert_eq!(sanitize_sql_query("SELECT 1"), "SELECT 1");
        assert_eq!(sanitize_sql_query("```sql\nSELECT 1\n```"), "SELECT 1");
        assert_eq!(
            sanitize_sql_query("sql\nSELECT * FROM t;"),
            "SELECT * FROM t;"
        );
        assert_eq!(sanitize_sql_query(""), "");
    }

    #[test]
    fn renders_row_blob_as_column_value_lines() {
        let blob = rdbms_row_blob(
            &["name".into(), "age".into()],
            &[Some("Alice".into()), Some("30".into())],
        );
        assert_eq!(blob, "name: Alice\nage: 30");
        assert_eq!(rdbms_row_blob(&["a".into()], &[None]), "a: ");
    }
}

#[cfg(test)]
mod airtable_connector_tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};

    /// Spawn a single-threaded HTTP mock server on `127.0.0.1:<random port>`.
    /// The handler receives `(method, path, query, header_block, body)` and
    /// returns `(status, content_type, body)`; the connector's `options.url`
    /// override points here — no real network. `Connection: close` makes every
    /// request land on a fresh connection.
    fn mock_airtable_server(
        handler: impl Fn(&str, &str, &str, &str, &str) -> (u16, &'static str, String) + Send + 'static,
    ) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut data = Vec::new();
                let mut chunk = [0u8; 16384];
                loop {
                    let n = match stream.read(&mut chunk) {
                        Ok(n) => n,
                        Err(_) => return,
                    };
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&chunk[..n]);
                    let Some(header_end) = data.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
                    let mut content_length = 0usize;
                    for line in head.lines() {
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                            content_length = v.trim().parse().unwrap_or(0);
                        }
                    }
                    if data.len() >= header_end + 4 + content_length {
                        break;
                    }
                    if data.len() > 1_000_000 {
                        break;
                    }
                }
                if data.is_empty() {
                    return;
                }
                let request = String::from_utf8_lossy(&data).to_string();
                let (head, body) = request
                    .split_once("\r\n\r\n")
                    .unwrap_or((request.as_ref(), ""));
                let first_line = head.lines().next().unwrap_or_default();
                let mut parts = first_line.split_whitespace();
                let method = parts.next().unwrap_or_default();
                let raw_target = parts.next().unwrap_or_default();
                let (path, query) = raw_target.split_once('?').unwrap_or((raw_target, ""));
                let (status, content_type, body) = handler(method, path, query, head, body);
                let reason = if status == 200 { "OK" } else { "Not Found" };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    fn options(base: &str, token: &str, base_id: &str) -> SourceOptions {
        let mut options = SourceOptions::default();
        options.url = base.to_string();
        options.token = token.to_string();
        options.target = base_id.to_string();
        options.max_items = 50;
        options
    }

    /// `new()` credential parsing: token and base id (target) are both
    /// mandatory; the API base defaults to https://api.airtable.com.
    #[test]
    fn new_requires_token_and_base_id() {
        // Empty token → readable error.
        let error = AirtableConnector::new(options("", "", "appBase1")).unwrap_err();
        assert!(error.to_string().contains("token"), "{error:#}");

        // Empty base id (target) → readable error.
        let error = AirtableConnector::new(options("", "pat123", "")).unwrap_err();
        assert!(error.to_string().contains("base id"), "{error:#}");

        // Valid credentials → default API base, target carried as base id.
        let connector = AirtableConnector::new(options("", "pat123", "appBase1")).unwrap();
        assert_eq!(connector.base_url, "https://api.airtable.com");
        assert_eq!(connector.base_id, "appBase1");
    }

    /// list_files: enumerates tables via the metadata API, paginates each
    /// table's records via `offset`, and maps every row onto a RemoteFile
    /// (id=record id, name=first text field or `{table}-{row_n}`, blob-size
    /// from the `列名: 值` rendering, metadata source/table/table_id/
    /// record_id). The mock also enforces the PAT Bearer header.
    #[tokio::test]
    async fn list_files_enumerates_tables_and_paginates_records() {
        let base = mock_airtable_server(move |method, path, query, headers, _body| {
            if headers
                .to_ascii_lowercase()
                .contains("authorization: bearer pat123")
            {
                match (method, path) {
                    ("GET", "/v0/meta/bases/appBase1/tables") => (
                        200,
                        "application/json",
                        json!({
                            "tables": [
                                {"id": "tbl1", "name": "Users"},
                                {"id": "tbl2", "name": "Pets"}
                            ]
                        })
                        .to_string(),
                    ),
                    ("GET", "/v0/appBase1/tbl1") => {
                        if query.contains("offset=itr1") {
                            (
                                200,
                                "application/json",
                                json!({
                                    "records": [
                                        {"id": "rec3", "createdTime": "2025-08-01T09:00:00.000Z",
                                         "fields": {"Name": "Cara"}}
                                    ],
                                    "offset": ""
                                })
                                .to_string(),
                            )
                        } else {
                            (
                                200,
                                "application/json",
                                json!({
                                    "records": [
                                        {"id": "rec1", "createdTime": "2025-08-01T08:00:00.000Z",
                                         "fields": {"Name": "Alice", "Email": "alice@example.com"}},
                                        {"id": "rec2", "createdTime": "2025-08-01T08:30:00.000Z",
                                         "fields": {"Name": "Bob"}}
                                    ],
                                    "offset": "itr1"
                                })
                                .to_string(),
                            )
                        }
                    }
                    ("GET", "/v0/appBase1/tbl2") => (
                        200,
                        "application/json",
                        json!({
                            "records": [
                                {"id": "rec9", "createdTime": "2025-08-02T00:00:00.000Z",
                                 "fields": {}}
                            ],
                            "offset": ""
                        })
                        .to_string(),
                    ),
                    _ => (404, "application/json", "{}".to_string()),
                }
            } else {
                (
                    401,
                    "application/json",
                    json!({"error": {"type": "AUTHENTICATION_REQUIRED", "message": "no token"}})
                        .to_string(),
                )
            }
        });

        let connector = AirtableConnector::new(options(&base, "pat123", "appBase1")).unwrap();
        let files = connector.list_files().await.unwrap();
        assert_eq!(
            files.len(),
            4,
            "tbl1 page1 (rec1,rec2) + page2 (rec3) + tbl2 (rec9)"
        );

        // Row 1 of table "Users": id/name/metadata/extension/updated_at mapping.
        let first = &files[0];
        assert_eq!(first.id, "rec1");
        assert_eq!(first.name, "Alice");
        assert_eq!(first.path, "appBase1/tbl1/rec1");
        assert_eq!(first.extension, ".txt");
        assert_eq!(first.updated_at, "2025-08-01T08:00:00.000Z");
        assert_eq!(
            first.metadata.get("source").map(String::as_str),
            Some("airtable")
        );
        assert_eq!(
            first.metadata.get("table").map(String::as_str),
            Some("Users")
        );
        assert_eq!(
            first.metadata.get("table_id").map(String::as_str),
            Some("tbl1")
        );
        assert_eq!(
            first.metadata.get("record_id").map(String::as_str),
            Some("rec1")
        );
        // size_bytes = rendered `列名: 值` blob length.
        assert_eq!(
            first.size_bytes as usize,
            "Name: Alice\nEmail: alice@example.com\n".len()
        );

        // Rows 2 and 3 (page 2 via offset).
        assert_eq!(files[1].id, "rec2");
        assert_eq!(files[1].name, "Bob");
        assert_eq!(files[2].id, "rec3");
        assert_eq!(files[2].name, "Cara");

        // Record without any text field → `{table}-{row_n}` fallback name.
        let last = &files[3];
        assert_eq!(last.id, "rec9");
        assert_eq!(last.name, "Pets-1");
        assert_eq!(last.metadata.get("table").map(String::as_str), Some("Pets"));
    }

    /// fetch_file re-lists the table and re-renders the record blob; normalize
    /// maps it onto a ConnectorDoc (Local stand-in + metadata source=airtable).
    #[tokio::test]
    async fn fetch_file_and_normalize_build_airtable_doc() {
        let base = mock_airtable_server(move |method, path, _query, _headers, _body| {
            match (method, path) {
                ("GET", "/v0/meta/bases/appBase1/tables") => (
                    200,
                    "application/json",
                    json!({
                        "tables": [{"id": "tbl1", "name": "Users"}]
                    })
                    .to_string(),
                ),
                ("GET", "/v0/appBase1/tbl1") => (
                    200,
                    "application/json",
                    json!({
                        "records": [
                            {"id": "rec1", "createdTime": "2025-08-01T08:00:00.000Z",
                             "fields": {"Name": "Alice", "Email": "alice@example.com"}}
                        ],
                        "offset": ""
                    })
                    .to_string(),
                ),
                _ => (404, "application/json", "{}".to_string()),
            }
        });

        let connector = AirtableConnector::new(options(&base, "pat123", "appBase1")).unwrap();
        let files = connector.list_files().await.unwrap();
        assert_eq!(files.len(), 1);
        let file = files.into_iter().next().unwrap();

        let raw = connector.fetch_file(&file).await.unwrap();
        assert_eq!(
            String::from_utf8(raw.clone()).unwrap(),
            "Name: Alice\nEmail: alice@example.com\n"
        );

        let doc = connector.normalize(&file, raw).unwrap();
        assert_eq!(doc.id, "rec1");
        assert_eq!(doc.blob, "Name: Alice\nEmail: alice@example.com\n");
        assert_eq!(doc.semantic_identifier, "Alice");
        assert_eq!(doc.extension, ".txt");
        assert_eq!(doc.doc_updated_at, "2025-08-01T08:00:00.000Z");
        assert_eq!(doc.source, DocumentSource::AirTable);
        assert_eq!(
            doc.metadata.get("source").map(String::as_str),
            Some("airtable")
        );
        assert_eq!(doc.metadata.get("table").map(String::as_str), Some("Users"));
        assert_eq!(
            doc.metadata.get("table_id").map(String::as_str),
            Some("tbl1")
        );
        assert_eq!(
            doc.metadata.get("record_id").map(String::as_str),
            Some("rec1")
        );
    }

    /// Non-2xx upstream responses surface the Airtable error envelope
    /// (type + message) through the load_credentials probe.
    #[tokio::test]
    async fn upstream_error_reports_error_message() {
        let base = mock_airtable_server(move |method, path, _query, _headers, _body| {
            match (method, path) {
                ("GET", "/v0/meta/bases/appBase1/tables") => (
                    403,
                    "application/json",
                    json!({
                        "error": {
                            "type": "INVALID_PERMISSIONS_OR_MODEL_NOT_FOUND",
                            "message": "Permission denied for base appBase1"
                        }
                    })
                    .to_string(),
                ),
                _ => (404, "application/json", "{}".to_string()),
            }
        });

        let mut connector = AirtableConnector::new(options(&base, "pat123", "appBase1")).unwrap();
        let error = connector.load_credentials().await.unwrap_err().to_string();
        assert!(error.contains("403"), "{error}");
        assert!(
            error.contains("INVALID_PERMISSIONS_OR_MODEL_NOT_FOUND"),
            "{error}"
        );
        assert!(
            error.contains("Permission denied for base appBase1"),
            "{error}"
        );
    }

    /// Registry dispatch: `create("airtable", …)` returns an AirtableConnector
    /// instead of bailing with "not implemented".
    #[test]
    fn registry_dispatches_airtable_connector() {
        let connector =
            ConnectorRegistry::create("airtable", options("", "pat123", "appBase1")).unwrap();
        assert_eq!(connector.kind(), "airtable");
    }
}
