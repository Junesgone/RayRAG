//! External data source connectors — mirrors RAGFlow `common/data_source/`.
//!
//! MVP: GitLab connector (token auth). Blob/Box/etc. are follow-ups.
//! The connector fetches external content and converts it into connector
//! documents that can be synced into a knowledge base.

use std::collections::HashMap;

/// RAGFlow-aligned document source enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentSource {
    GitLab,
    Blob,
    /// Local filesystem directory (RAGFlow has no local connector; this is the
    /// offline equivalent of the WebDAV/Blob file connectors).
    Local,
    /// WebDAV server (RAGFlow `DocumentSource.WEBDAV`).
    WebDav,
    /// Feishu / Lark cloud drive (飞书云空间).
    Feishu,
    /// Atlassian Confluence (RAGFlow `DocumentSource.CONFLUENCE`).
    Confluence,
    /// S3-protocol object storage (S3 / GCS-interop / Oracle S3-compat / MinIO;
    /// RAGFlow `DocumentSource.S3`).
    S3,
    /// 语雀 (Yuque) — China-friendly document repo (RAGFlow has no direct
    /// equivalent; local extension for mainland network parity).
    Yuque,
    /// 钉钉云盘 (DingTalk Drive) — mainland cloud disk via OpenAPI.
    DingTalk,
    /// RSS/Atom feed (RAGFlow `rss_connector.py` parity).
    Rss,
    /// Box cloud drive (RAGFlow `box_connector.py` parity).
    Box,
    /// Airtable base rows (RAGFlow `airtable_connector.py` parity).
    AirTable,
}

impl DocumentSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            DocumentSource::GitLab => "gitlab",
            DocumentSource::Blob => "blob",
            DocumentSource::Local => "local",
            DocumentSource::WebDav => "webdav",
            DocumentSource::Feishu => "feishu",
            DocumentSource::Confluence => "confluence",
            DocumentSource::S3 => "s3",
            DocumentSource::Yuque => "yuque",
            DocumentSource::DingTalk => "dingtalk",
            DocumentSource::Rss => "rss",
            DocumentSource::Box => "box",
            DocumentSource::AirTable => "airtable",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "gitlab" => Some(DocumentSource::GitLab),
            "blob" => Some(DocumentSource::Blob),
            "local" => Some(DocumentSource::Local),
            "webdav" => Some(DocumentSource::WebDav),
            "feishu" => Some(DocumentSource::Feishu),
            "confluence" => Some(DocumentSource::Confluence),
            "s3" => Some(DocumentSource::S3),
            "yuque" => Some(DocumentSource::Yuque),
            "dingtalk" => Some(DocumentSource::DingTalk),
            "rss" => Some(DocumentSource::Rss),
            "box" => Some(DocumentSource::Box),
            "airtable" => Some(DocumentSource::AirTable),
            _ => None,
        }
    }
}

/// A connector-produced document (mirrors RAGFlow `Document` fields used by
/// connector runners: blob/semantic_identifier/extension/metadata).
#[derive(Debug, Clone)]
pub struct ConnectorDoc {
    pub id: String,
    pub blob: String,
    pub source: DocumentSource,
    pub semantic_identifier: String,
    pub extension: String,
    pub doc_updated_at: String,
    pub size_bytes: usize,
    pub metadata: HashMap<String, String>,
}

impl ConnectorDoc {
    /// Render a RAGFlow-style text blob (used for preview / ingestion).
    pub fn to_markdown(&self) -> String {
        format!("# {}\n\n{}", self.semantic_identifier, self.blob)
    }
}

/// Shared fetch options for connectors.
#[derive(Debug, Clone, Default)]
pub struct SourceOptions {
    /// Base URL of the source (e.g. https://gitlab.com).
    pub url: String,
    /// Auth token (e.g. GitLab personal access token).
    pub token: String,
    /// Optional project id / owner+repo.
    pub target: String,
    /// Whether to include issues and merge requests (gitlab).
    pub include_issues: bool,
    pub include_merge_requests: bool,
    /// Max files to fetch (safety bound).
    pub max_items: usize,
    /// Free-form credential / option key-values (RAGFlow `options` dict
    /// parity) — connectors with multi-field credentials (rdbms: host/port/
    /// user/password/database/connect_str/query/id_column/timestamp_column/
    /// content_columns) read from here.
    pub extra: std::collections::HashMap<String, String>,
}

pub mod gitlab;
pub use gitlab::GitLabConnector;

pub mod blob;
pub mod form_fields;
pub use blob::BlobConnector;
