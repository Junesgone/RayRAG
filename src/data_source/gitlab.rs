//! GitLab connector — mirrors RAGFlow `common/data_source/gitlab_connector.py`.
//!
//! Fetches project files, issues and merge requests via the GitLab REST API
//! using a personal access token, then converts them into `ConnectorDoc`s.

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use std::collections::HashMap;

use super::{ConnectorDoc, DocumentSource, SourceOptions};

/// Minimal percent-encoding for path segments (same approach as the
/// SearXNG connector; keeps the crate free of an extra dependency).
fn urlencoding(input: &str) -> String {
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

/// Minimal GitLab REST client (token auth).
pub struct GitLabConnector {
    client: reqwest::Client,
}

impl Default for GitLabConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl GitLabConnector {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
        }
    }

    fn base(&self, options: &SourceOptions) -> Result<String> {
        let base = options.url.trim().trim_end_matches('/');
        if base.is_empty() {
            bail!("gitlab: instance URL is required (source url)");
        }
        Ok(base.to_string())
    }

    async fn get(&self, options: &SourceOptions, path: &str) -> Result<Value> {
        let base = self.base(options)?;
        let url = format!("{base}{path}");
        let mut request = self.client.get(&url);
        if !options.token.is_empty() {
            request = request.header("PRIVATE-TOKEN", &options.token);
        }
        let response = request.send().await.context("gitlab: request failed")?;
        let status = response.status();
        // Bounded read (see `blob`): downloaded documents obey the connector limit.
        let body = crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "gitlab",
        )
        .await?;
        if !status.is_success() {
            bail!(
                "gitlab: upstream returned {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
                    .chars()
                    .take(200)
                    .collect::<String>()
            );
        }
        serde_json::from_slice(&body).with_context(|| format!("gitlab: invalid JSON from {path}"))
    }

    /// Resolve project id from `target` (numeric id, or `owner/repo` path).
    async fn resolve_project_id(&self, options: &SourceOptions) -> Result<i64> {
        let target = options.target.trim().trim_matches('/').to_string();
        if target.is_empty() {
            bail!("gitlab: project id or owner/repo is required (target)");
        }
        if let Ok(id) = target.parse::<i64>() {
            return Ok(id);
        }
        // owner/repo form: GET /projects/{urlencoded path}
        let encoded = urlencoding(&target);
        let value = self
            .get(options, &format!("/api/v4/projects/{encoded}"))
            .await?;
        value
            .get("id")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("gitlab: project not found: {target}"))
    }

    async fn default_branch(&self, options: &SourceOptions, project_id: i64) -> Result<String> {
        let value = self
            .get(options, &format!("/api/v4/projects/{project_id}"))
            .await?;
        Ok(value
            .get("default_branch")
            .and_then(Value::as_str)
            .unwrap_or("main")
            .to_string())
    }

    /// Fetch repository tree (recursive).
    async fn list_tree(
        &self,
        options: &SourceOptions,
        project_id: i64,
        branch: &str,
    ) -> Result<Vec<Value>> {
        let mut all = Vec::new();
        let mut page = 1u32;
        loop {
            let value = self
                .get(
                    options,
                    &format!(
                        "/api/v4/projects/{project_id}/repository/tree?ref={branch}&recursive=true&per_page=100&page={page}"
                    ),
                )
                .await?;
            let items = value.as_array().cloned().unwrap_or_default();
            if items.is_empty() {
                break;
            }
            let fetched = items.len();
            all.extend(items);
            if all.len() >= options.max_items.max(1) || fetched < 100 {
                break;
            }
            page += 1;
        }
        Ok(all)
    }

    async fn fetch_file_raw(
        &self,
        options: &SourceOptions,
        project_id: i64,
        path: &str,
        branch: &str,
    ) -> Result<Vec<u8>> {
        let encoded_path = urlencoding(path);
        let value = self
            .get(
                options,
                &format!(
                    "/api/v4/projects/{project_id}/repository/files/{encoded_path}/raw?ref={branch}"
                ),
            )
            .await?;
        match value {
            Value::String(text) => Ok(text.into_bytes()),
            other => Ok(serde_json::to_vec(&other)?),
        }
    }

    async fn list_issues(&self, options: &SourceOptions, project_id: i64) -> Result<Vec<Value>> {
        let value = self
            .get(
                options,
                &format!("/api/v4/projects/{project_id}/issues?state=all&per_page=100&scope=all"),
            )
            .await?;
        Ok(value.as_array().cloned().unwrap_or_default())
    }

    async fn list_merge_requests(
        &self,
        options: &SourceOptions,
        project_id: i64,
    ) -> Result<Vec<Value>> {
        let value = self
            .get(
                options,
                &format!(
                    "/api/v4/projects/{project_id}/merge_requests?state=all&per_page=100&scope=all"
                ),
            )
            .await?;
        Ok(value.as_array().cloned().unwrap_or_default())
    }

    /// Fetch all selectable content of a project as connector documents.
    pub async fn fetch(&self, options: &SourceOptions) -> Result<Vec<ConnectorDoc>> {
        let project_id = self.resolve_project_id(options).await?;
        let branch = self.default_branch(options, project_id).await?;
        let base = self.base(options)?;
        let mut docs = Vec::new();
        let max = options.max_items.max(1);

        // Files (code) — mirrors _convert_code_to_document.
        let tree = self.list_tree(options, project_id, &branch).await?;
        for item in tree {
            if item.get("type").and_then(Value::as_str) != Some("blob") {
                continue;
            }
            let path = item
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if path.is_empty() || docs.len() >= max {
                continue;
            }
            let ext = std::path::Path::new(&path)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            match self
                .fetch_file_raw(options, project_id, &path, &branch)
                .await
            {
                Ok(bytes) => {
                    let blob = String::from_utf8_lossy(&bytes).to_string();
                    let mut metadata = HashMap::new();
                    metadata.insert("type".into(), "CodeFile".into());
                    metadata.insert("path".into(), path.clone());
                    docs.push(ConnectorDoc {
                        id: format!("{base}/{path}"),
                        blob,
                        source: DocumentSource::GitLab,
                        semantic_identifier: path,
                        extension: ext,
                        doc_updated_at: String::new(),
                        size_bytes: bytes.len(),
                        metadata,
                    });
                }
                Err(error) => {
                    tracing::warn!("gitlab: skip file {path}: {error:#}");
                }
            }
        }

        // Issues — mirrors _convert_issue_to_document.
        if options.include_issues {
            for issue in self.list_issues(options, project_id).await? {
                if docs.len() >= max {
                    break;
                }
                let title = issue
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let web_url = issue
                    .get("web_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let description = issue
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let description_for_doc = description.clone();
                let state = issue
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("opened")
                    .to_string();
                let updated_at = issue
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut metadata = HashMap::new();
                metadata.insert("state".into(), state);
                metadata.insert("type".into(), "Issue".into());
                metadata.insert("web_url".into(), web_url.clone());
                docs.push(ConnectorDoc {
                    id: if web_url.is_empty() {
                        format!("issue-{title}")
                    } else {
                        web_url
                    },
                    blob: description_for_doc.clone(),
                    source: DocumentSource::GitLab,
                    semantic_identifier: title,
                    extension: ".md".into(),
                    doc_updated_at: updated_at,
                    size_bytes: description_for_doc.len(),
                    metadata,
                });
            }
        }

        // Merge requests — mirrors _convert_mr_to_document.
        if options.include_merge_requests {
            for mr in self.list_merge_requests(options, project_id).await? {
                if docs.len() >= max {
                    break;
                }
                let title = mr
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let web_url = mr
                    .get("web_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let description = mr
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let description_for_doc = description.clone();
                let state = mr
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("opened")
                    .to_string();
                let updated_at = mr
                    .get("updated_at")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let mut metadata = HashMap::new();
                metadata.insert("state".into(), state);
                metadata.insert("type".into(), "MergeRequest".into());
                metadata.insert("web_url".into(), web_url.clone());
                docs.push(ConnectorDoc {
                    id: if web_url.is_empty() {
                        format!("mr-{title}")
                    } else {
                        web_url
                    },
                    blob: description_for_doc.clone(),
                    source: DocumentSource::GitLab,
                    semantic_identifier: title,
                    extension: ".md".into(),
                    doc_updated_at: updated_at,
                    size_bytes: description_for_doc.len(),
                    metadata,
                });
            }
        }

        Ok(docs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_roundtrip() {
        assert_eq!(
            DocumentSource::from_str("gitlab"),
            Some(DocumentSource::GitLab)
        );
        assert_eq!(DocumentSource::GitLab.as_str(), "gitlab");
        assert_eq!(DocumentSource::from_str("blob"), Some(DocumentSource::Blob));
        assert_eq!(DocumentSource::from_str("unknown"), None);
    }

    #[test]
    fn doc_to_markdown_renders_identifier() {
        let doc = ConnectorDoc {
            id: "https://gitlab.com/x/y".into(),
            blob: "hello".into(),
            source: DocumentSource::GitLab,
            semantic_identifier: "readme.md".into(),
            extension: ".md".into(),
            doc_updated_at: String::new(),
            size_bytes: 5,
            metadata: HashMap::new(),
        };
        assert!(doc.to_markdown().contains("# readme.md"));
        assert!(doc.to_markdown().contains("hello"));
    }

    #[tokio::test]
    async fn missing_url_bails() {
        let connector = GitLabConnector::new();
        let error = connector
            .fetch(&SourceOptions {
                url: String::new(),
                token: "t".into(),
                target: "1".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("instance URL"));
    }

    #[tokio::test]
    async fn missing_target_bails() {
        let connector = GitLabConnector::new();
        let error = connector
            .fetch(&SourceOptions {
                url: "https://gitlab.com".into(),
                token: "t".into(),
                target: String::new(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("project id"));
    }
}
