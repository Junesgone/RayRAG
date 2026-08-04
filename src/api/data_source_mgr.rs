//! Data source manager — configures and syncs external connectors
//! (GitLab MVP). Mirrors RAGFlow's connector admin surface.

use crate::data_source::{GitLabConnector, SourceOptions};
use anyhow::{bail, Context, Result};
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSourceRecord {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    pub token: String,
    pub target: String,
    pub include_issues: bool,
    pub include_merge_requests: bool,
    pub max_items: usize,
    pub created_at: String,
}

impl DataSourceRecord {
    pub fn redacted(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "type": self.kind,
            "url": self.url,
            "target": self.target,
            "include_issues": self.include_issues,
            "include_merge_requests": self.include_merge_requests,
            "max_items": self.max_items,
            "created_at": self.created_at,
            "token": if self.token.is_empty() { "" } else { "***" },
        })
    }
}

pub struct DataSourceStore {
    sources: RwLock<HashMap<String, DataSourceRecord>>,
    metadata_path: String,
    save_lock: Mutex<()>,
}

impl DataSourceStore {
    pub fn new(data_dir: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let metadata_path = std::path::Path::new(data_dir)
            .join("data_sources.json")
            .to_string_lossy()
            .to_string();
        crate::persistence::restore_if_missing(std::path::Path::new(&metadata_path))?;
        let sources = if std::path::Path::new(&metadata_path).exists() {
            let content = std::fs::read_to_string(&metadata_path)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self {
            sources: RwLock::new(sources),
            metadata_path,
            save_lock: Mutex::new(()),
        })
    }

    fn persist(&self) -> Result<()> {
        let _guard = self.save_lock.lock().map_err(|_| anyhow::anyhow!("save lock poisoned"))?;
        let sources = self
            .sources
            .read()
            .map_err(|_| anyhow::anyhow!("sources lock poisoned"))?;
        crate::persistence::atomic_write(
            std::path::Path::new(&self.metadata_path),
            &serde_json::to_vec_pretty(&*sources)?,
        )
    }

    pub fn list(&self) -> Vec<serde_json::Value> {
        let sources = self.sources.read().unwrap_or_else(|_| {
            // Poisoned lock: fall back to an empty snapshot rather than panic.
            self.sources.clear_poison();
            self.sources.read().expect("cleared poisoned lock")
        });
        let mut values: Vec<serde_json::Value> =
            sources.values().map(DataSourceRecord::redacted).collect();
        values.sort_by(|a, b| {
            a.get("created_at")
                .and_then(serde_json::Value::as_str)
                .cmp(&b.get("created_at").and_then(serde_json::Value::as_str))
        });
        values
    }

    pub fn get(&self, id: &str) -> Option<DataSourceRecord> {
        self.sources.read().ok()?.get(id).cloned()
    }

    pub fn insert(&self, record: DataSourceRecord) -> Result<()> {
        self.sources
            .write()
            .map_err(|_| anyhow::anyhow!("sources lock poisoned"))?
            .insert(record.id.clone(), record);
        self.persist()
    }

    pub fn remove(&self, id: &str) -> Result<bool> {
        let removed = self
            .sources
            .write()
            .map_err(|_| anyhow::anyhow!("sources lock poisoned"))?
            .remove(id)
            .is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateDataSourceRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub token: String,
    pub target: String,
    #[serde(default)]
    pub include_issues: bool,
    #[serde(default)]
    pub include_merge_requests: bool,
    #[serde(default = "default_max_items")]
    pub max_items: usize,
}

fn default_max_items() -> usize {
    50
}

pub async fn list_data_sources(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    Ok(Json(serde_json::json!({ "code": 0, "data": store.list() })))
}

pub async fn create_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Json(body): Json<CreateDataSourceRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let name = body.name.trim().to_string();
    let url = body.url.trim().to_string();
    let target = body.target.trim().to_string();
    if name.is_empty() || url.is_empty() || target.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name/url/target are required".into()));
    }
    if body.kind != "gitlab" {
        return Err((StatusCode::BAD_REQUEST, "only gitlab is supported".into()));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let record = DataSourceRecord {
        id,
        name,
        kind: body.kind,
        url,
        token: body.token,
        target,
        include_issues: body.include_issues,
        include_merge_requests: body.include_merge_requests,
        max_items: body.max_items.max(1),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    store
        .insert(record.clone())
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(serde_json::json!({ "code": 0, "data": record.redacted() })))
}

pub async fn delete_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let removed = store.remove(&id).map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(serde_json::json!({ "code": 0, "data": { "removed": removed } })))
}

pub async fn sync_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Extension(auth): Extension<crate::server::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let record = store
        .get(&id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "data source not found".into()))?;
    if record.kind != "gitlab" {
        return Err((StatusCode::BAD_REQUEST, "only gitlab sync is supported".into()));
    }
    let options = SourceOptions {
        url: record.url,
        token: record.token,
        target: record.target,
        include_issues: record.include_issues,
        include_merge_requests: record.include_merge_requests,
        max_items: record.max_items,
    };
    let connector = GitLabConnector::new();
    let docs = connector
        .fetch(&options)
        .await
        .map_err(|error| (StatusCode::BAD_GATEWAY, format!("sync failed: {error:#}")))?;

    // Optional ingestion into a knowledge base: ?kb_id=<dataset id>.
    // Each connector doc is written as a text file and routed through the
    // standard upload pipeline (parse -> chunk -> embed -> index).
    let mut ingested = 0_usize;
    let mut skipped = Vec::new();
    if let Some(kb_id) = query.get("kb_id").filter(|value| !value.is_empty()) {
        if !crate::server::kb_manageable(&state, kb_id, &auth) {
            return Err((StatusCode::NOT_FOUND, "knowledge base not found".into()));
        }
        let upload_dir = std::path::Path::new(&state.static_dir).join("../uploads");
        std::fs::create_dir_all(&upload_dir)
            .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
        for doc in &docs {
            let doc_id = uuid::Uuid::new_v4().to_string();
            let ext = if doc.extension.is_empty() {
                ".md"
            } else {
                &doc.extension
            };
            let file_name = format!("{doc_id}{ext}");
            let path = upload_dir.join(&file_name);
            if std::fs::write(&path, doc.blob.as_bytes()).is_err() {
                skipped.push(doc.semantic_identifier.clone());
                continue;
            }
            let upload = crate::server::PersistedUpload {
                name: doc.semantic_identifier.clone(),
                storage_name: file_name,
                path,
                size: doc.blob.len(),
                content_hash: crate::server::server_xxh3_hash(doc.blob.as_bytes()),
                cleanup_on_drop: false,
            };
            match crate::server::register_document_upload(
                state.clone(),
                &auth.user_id,
                kb_id,
                doc_id,
                upload,
            ) {
                Ok(_) => ingested += 1,
                Err(error) => {
                    tracing::warn!("data source: ingest {} failed: {error:#}", doc.semantic_identifier);
                    skipped.push(doc.semantic_identifier.clone());
                }
            }
        }
    }

    let preview: Vec<serde_json::Value> = docs
        .iter()
        .take(10)
        .map(|doc| {
            serde_json::json!({
                "id": doc.id,
                "title": doc.semantic_identifier,
                "extension": doc.extension,
                "size_bytes": doc.size_bytes,
                "type": doc.metadata.get("type"),
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "code": 0,
        "data": { "count": docs.len(), "ingested": ingested, "skipped": skipped, "preview": preview }
    })))
}

// ---- tests ----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip_and_redaction() {
        let dir = std::env::temp_dir().join(format!("rayrag-ds-test-{}", uuid::Uuid::new_v4()));
        let store = DataSourceStore::new(dir.to_str().unwrap()).unwrap();
        let record = DataSourceRecord {
            id: "1".into(),
            name: "gitlab-main".into(),
            kind: "gitlab".into(),
            url: "https://gitlab.com".into(),
            token: "secret-token".into(),
            target: "1".into(),
            include_issues: true,
            include_merge_requests: false,
            max_items: 10,
            created_at: "2026-01-01T00:00:00Z".into(),
        };
        store.insert(record).unwrap();
        let redacted = store.list();
        assert_eq!(redacted.len(), 1);
        assert_eq!(redacted[0]["token"], "***");
        assert_eq!(store.get("1").unwrap().token, "secret-token");
        assert!(store.remove("1").unwrap());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reload_after_restart() {
        let dir = std::env::temp_dir().join(format!("rayrag-ds-test-{}", uuid::Uuid::new_v4()));
        {
            let store = DataSourceStore::new(dir.to_str().unwrap()).unwrap();
            store
                .insert(DataSourceRecord {
                    id: "2".into(),
                    name: "n".into(),
                    kind: "gitlab".into(),
                    url: "https://gitlab.com".into(),
                    token: "t".into(),
                    target: "2".into(),
                    include_issues: false,
                    include_merge_requests: false,
                    max_items: 5,
                    created_at: "2026-01-01T00:00:00Z".into(),
                })
                .unwrap();
        }
        let store = DataSourceStore::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(store.list().len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn create_request_defaults() {
        let body: CreateDataSourceRequest = serde_json::from_str(
            r#"{"name":"n","type":"gitlab","url":"https://gitlab.com","target":"1"}"#,
        )
        .unwrap();
        assert_eq!(body.max_items, 50);
        assert!(!body.include_issues);
    }
}
