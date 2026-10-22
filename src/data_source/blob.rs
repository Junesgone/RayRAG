//! Azure Blob connector — mirrors RAGFlow `common/data_source/blob_connector.py`.
//!
//! MVP: SAS-token authentication (no request signing needed). Lists
//! containers (or a specific container) and downloads blobs as
//! connector documents.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;

use super::{ConnectorDoc, DocumentSource, SourceOptions};

/// Minimal Azure Blob REST client (SAS token auth).
pub struct BlobConnector {
    client: reqwest::Client,
}

impl Default for BlobConnector {
    fn default() -> Self {
        Self::new()
    }
}

impl BlobConnector {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .expect("reqwest client"),
        }
    }

    fn endpoint(&self, options: &SourceOptions) -> Result<String> {
        let endpoint = options.url.trim().trim_end_matches('/');
        if endpoint.is_empty() {
            bail!("blob: storage endpoint URL is required (source url)");
        }
        Ok(endpoint.to_string())
    }

    /// SAS token: either the full query (`?sv=...&sig=...`) or bare
    /// (`sv=...&sig=...`). Returns it without a leading `?`.
    fn sas(&self, options: &SourceOptions) -> Result<String> {
        let token = options.token.trim();
        if token.is_empty() {
            bail!("blob: SAS token is required (token)");
        }
        Ok(token.trim_start_matches('?').to_string())
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
        // Bounded read: the connector limit applies to downloaded documents so an
        // oversized or endless upstream object cannot become process memory.
        let bytes = crate::common::cmd_timeout::read_body_limited(
            response,
            crate::common::cmd_timeout::connector_body_limit_bytes(),
            "blob",
        )
        .await?;
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

    /// List container names. When `target` is non-empty it is treated as a
    /// single container (no listing needed).
    async fn list_containers(&self, options: &SourceOptions) -> Result<Vec<String>> {
        let endpoint = self.endpoint(options)?;
        let sas = self.sas(options)?;
        let url = format!("{endpoint}/?comp=list&restype=container&{sas}");
        let xml = self.get_xml(&url).await?;
        Ok(parse_xml_names(&xml, "Container", "Name"))
    }

    /// List blob names inside a container.
    async fn list_blobs(&self, options: &SourceOptions, container: &str) -> Result<Vec<String>> {
        let endpoint = self.endpoint(options)?;
        let sas = self.sas(options)?;
        let url = format!("{endpoint}/{container}?restype=container&comp=list&{sas}");
        let xml = self.get_xml(&url).await?;
        Ok(parse_xml_names(&xml, "Blob", "Name"))
    }

    /// Fetch all blobs of the configured container(s) as connector docs.
    pub async fn fetch(&self, options: &SourceOptions) -> Result<Vec<ConnectorDoc>> {
        let endpoint = self.endpoint(options)?;
        let sas = self.sas(options)?;
        let max = options.max_items.max(1);
        let mut docs = Vec::new();

        let containers: Vec<String> = if options.target.trim().is_empty() {
            self.list_containers(options).await?
        } else {
            options
                .target
                .split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect()
        };

        for container in containers {
            let blobs = self.list_blobs(options, &container).await?;
            for blob in blobs {
                if docs.len() >= max {
                    break;
                }
                let url = format!("{endpoint}/{container}/{blob}?{sas}");
                match self.get_bytes(&url).await {
                    Ok(bytes) => {
                        let name = blob.rsplit('/').next().unwrap_or(&blob).to_string();
                        let ext = std::path::Path::new(&name)
                            .extension()
                            .map(|e| format!(".{}", e.to_string_lossy()))
                            .unwrap_or_default();
                        let mut metadata = HashMap::new();
                        metadata.insert("container".into(), container.clone());
                        metadata.insert("blob".into(), blob.clone());
                        docs.push(ConnectorDoc {
                            id: format!("{endpoint}/{container}/{blob}"),
                            blob: String::from_utf8_lossy(&bytes).to_string(),
                            source: DocumentSource::Blob,
                            semantic_identifier: name,
                            extension: ext,
                            doc_updated_at: String::new(),
                            size_bytes: bytes.len(),
                            metadata,
                        });
                    }
                    Err(error) => {
                        tracing::warn!("blob: skip {container}/{blob}: {error:#}");
                    }
                }
            }
            if docs.len() >= max {
                break;
            }
        }
        Ok(docs)
    }
}

/// Parse `<EnumerationResults><{item_type}><Name>x</Name>...` XML without an
/// XML dependency (RAGFlow's blob listing returns this shape).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_azure_listing() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<EnumerationResults>
  <Blobs><Blob><Name>docs/a.pdf</Name></Blob><Blob><Name>notes.txt</Name></Blob></Blobs>
</EnumerationResults>"#;
        let names = parse_xml_names(xml, "Blob", "Name");
        assert_eq!(names, vec!["docs/a.pdf", "notes.txt"]);
    }

    #[test]
    fn parse_container_listing() {
        let xml = r#"<EnumerationResults><Containers><Container><Name>alpha</Name></Container><Container><Name>beta</Name></Container></Containers></EnumerationResults>"#;
        let names = parse_xml_names(xml, "Container", "Name");
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn source_roundtrip() {
        assert_eq!(DocumentSource::from_str("blob"), Some(DocumentSource::Blob));
        assert_eq!(DocumentSource::Blob.as_str(), "blob");
    }

    #[tokio::test]
    async fn missing_sas_bails() {
        let connector = BlobConnector::new();
        let error = connector
            .fetch(&SourceOptions {
                url: "https://acct.blob.core.windows.net".into(),
                token: String::new(),
                target: "c".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("SAS"));
    }

    #[tokio::test]
    async fn missing_url_bails() {
        let connector = BlobConnector::new();
        let error = connector
            .fetch(&SourceOptions {
                url: String::new(),
                token: "sv=1".into(),
                target: "c".into(),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("endpoint"));
    }
}
