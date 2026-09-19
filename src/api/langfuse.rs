//! Tenant-scoped Langfuse observability configuration.
//!
//! Mirrors the fixed RAGFlow v0.26.4 `langfuse_api.py` wire contract:
//! `GET/POST/PUT/DELETE /api/v1/langfuse/api-key` with `secret_key`,
//! `public_key` and `host`. Unlike the Python service, RayRAG validates the
//! credentials against Langfuse's public health/projects endpoints at save
//! time, caches `project_id`/`project_name` from that call, and serves the
//! cached entry on GET instead of making a blocking outbound auth call on
//! every read. The HTTP probe uses short connect/total timeouts so a mainland
//! China deployment without access to the configured host fails fast with the
//! upstream "Invalid Langfuse keys" message rather than hanging the request.

use crate::server::{AppState, AuthContext};
use anyhow::{Context, bail};
use axum::{
    Json,
    extract::{Extension, State},
    response::{IntoResponse, Response},
};
use base64::Engine as _;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

const LANGFUSE_PROBE_CONNECT_SECONDS: u64 = 5;
const LANGFUSE_PROBE_TOTAL_SECONDS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LangfuseEntry {
    pub tenant_id: String,
    pub secret_key: String,
    pub public_key: String,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LangfuseUpdateRequest {
    pub secret_key: String,
    pub public_key: String,
    pub host: String,
    #[serde(default)]
    pub tenant_id: Option<String>,
}

/// Per-tenant Langfuse credentials persisted to an atomic JSON snapshot.
/// The store is tenant-keyed and never exposes another tenant's entry.
pub struct LangfuseStore {
    entries: RwLock<HashMap<String, LangfuseEntry>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl LangfuseStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = PathBuf::from(path);
        crate::persistence::restore_if_missing(&path)?;
        let entries = match crate::persistence::load_json::<Vec<LangfuseEntry>>(&path)? {
            Some(rows) => rows
                .into_iter()
                .map(|row| (row.tenant_id.clone(), row))
                .collect(),
            None => HashMap::new(),
        };
        Ok(Self {
            entries: RwLock::new(entries),
            path: Some(path),
            save_lock: Mutex::new(()),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn get(&self, tenant_id: &str) -> Option<LangfuseEntry> {
        self.entries.read().unwrap().get(tenant_id).cloned()
    }

    pub fn upsert(
        &self,
        tenant_id: &str,
        secret_key: &str,
        public_key: &str,
        host: &str,
        project_id: Option<&str>,
        project_name: Option<&str>,
    ) -> anyhow::Result<LangfuseEntry> {
        let entry = LangfuseEntry {
            tenant_id: tenant_id.to_string(),
            secret_key: secret_key.to_string(),
            public_key: public_key.to_string(),
            host: host.to_string(),
            project_id: project_id.map(str::to_string),
            project_name: project_name.map(str::to_string),
        };
        {
            let mut entries = self.entries.write().unwrap();
            entries.insert(tenant_id.to_string(), entry.clone());
        }
        self.persist()?;
        Ok(entry)
    }

    pub fn delete(&self, tenant_id: &str) -> anyhow::Result<bool> {
        let removed = self.entries.write().unwrap().remove(tenant_id).is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    fn persist(&self) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let _guard = self.save_lock.lock().unwrap();
        let rows: Vec<_> = self.entries.read().unwrap().values().cloned().collect();
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(&rows)?)
            .with_context(|| format!("Failed to persist Langfuse config '{}':", path.display()))
    }
}

/// Langfuse public-API probe. Verifies the credentials via
/// `GET {host}/api/public/health` and reads the first project from
/// `GET {host}/api/public/projects`, both with HTTP Basic auth
/// (`public_key:secret_key`), matching the Python `Langfuse.auth_check()` and
/// `langfuse.api.projects.get()` calls.
pub async fn probe_langfuse(
    host: &str,
    public_key: &str,
    secret_key: &str,
) -> anyhow::Result<(Option<String>, Option<String>)> {
    let host = normalize_langfuse_host(host)?;
    let auth = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{public_key}:{secret_key}"))
    );
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(LANGFUSE_PROBE_CONNECT_SECONDS))
        .timeout(Duration::from_secs(LANGFUSE_PROBE_TOTAL_SECONDS))
        .build()?;

    let health_url = format!("{host}/api/public/health");
    let response = client
        .get(&health_url)
        .header(reqwest::header::AUTHORIZATION, &auth)
        .send()
        .await
        .with_context(|| format!("Failed to reach Langfuse health endpoint {health_url}"))?;
    if !response.status().is_success() {
        bail!("Invalid Langfuse keys");
    }

    let projects_url = format!("{host}/api/public/projects");
    let response = client
        .get(&projects_url)
        .header(reqwest::header::AUTHORIZATION, &auth)
        .send()
        .await
        .with_context(|| format!("Failed to reach Langfuse projects endpoint {projects_url}"))?;
    if !response.status().is_success() {
        return Ok((None, None));
    }
    let payload: serde_json::Value = response.json().await?;
    let first = payload
        .get("data")
        .and_then(serde_json::Value::as_array)
        .and_then(|projects| projects.first())
        .and_then(serde_json::Value::as_object);
    Ok((
        first
            .and_then(|project| project.get("id"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        first
            .and_then(|project| project.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
    ))
}

fn normalize_langfuse_host(host: &str) -> anyhow::Result<String> {
    let host = host.trim().trim_end_matches('/');
    if host.is_empty() {
        bail!("Missing required fields");
    }
    let with_scheme = if host.starts_with("http://") || host.starts_with("https://") {
        host.to_string()
    } else {
        format!("https://{host}")
    };
    let parsed = reqwest::Url::parse(&with_scheme).context("Invalid Langfuse host")?;
    if parsed.host_str().is_none() {
        bail!("Invalid Langfuse host");
    }
    Ok(with_scheme)
}

fn langfuse_error(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 102, "message": message })),
    )
        .into_response()
}

/// GET `/api/v1/langfuse/api-key` — the stored tenant entry, including the
/// project identity cached during the last successful save.
pub async fn get_langfuse_api_key(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    match state.langfuse.get(&auth.user_id) {
        Some(entry) => Json(serde_json::json!({ "code": 0, "data": entry })).into_response(),
        None => Json(serde_json::json!({
            "code": 0,
            "message": "Have not record any Langfuse keys."
        }))
        .into_response(),
    }
}

/// POST/PUT `/api/v1/langfuse/api-key` — validate, probe and upsert the
/// tenant's Langfuse credentials.
pub async fn set_langfuse_api_key(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(update): Json<LangfuseUpdateRequest>,
) -> Response {
    let tenant_id = update.tenant_id.as_deref().unwrap_or(&auth.user_id);
    if tenant_id != auth.user_id && !auth.is_admin {
        return langfuse_error("Tenant administrator access required");
    }
    let secret_key = update.secret_key.trim();
    let public_key = update.public_key.trim();
    let host = update.host.trim();
    if secret_key.is_empty() || public_key.is_empty() || host.is_empty() {
        return langfuse_error("Missing required fields");
    }
    let (project_id, project_name) = match probe_langfuse(host, public_key, secret_key).await {
        Ok(project) => project,
        Err(_) => return langfuse_error("Invalid Langfuse keys"),
    };
    match state.langfuse.upsert(
        tenant_id,
        secret_key,
        public_key,
        host,
        project_id.as_deref(),
        project_name.as_deref(),
    ) {
        Ok(entry) => Json(serde_json::json!({ "code": 0, "data": entry })).into_response(),
        Err(error) => langfuse_error(&error.to_string()),
    }
}

/// DELETE `/api/v1/langfuse/api-key` — remove the tenant's stored entry.
pub async fn delete_langfuse_api_key(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    match state.langfuse.delete(&auth.user_id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => Json(serde_json::json!({
            "code": 0,
            "message": "Have not record any Langfuse keys."
        }))
        .into_response(),
        Err(error) => langfuse_error(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_persists_per_tenant_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("langfuse.json")
            .to_string_lossy()
            .into_owned();
        let store = LangfuseStore::new(&path).unwrap();
        store
            .upsert(
                "tenant-a",
                "sk-a",
                "pk-a",
                "https://cloud.langfuse.com",
                Some("p-1"),
                Some("Project 1"),
            )
            .unwrap();
        store
            .upsert(
                "tenant-b",
                "sk-b",
                "pk-b",
                "https://lf.example.com",
                None,
                None,
            )
            .unwrap();
        assert!(store.get("tenant-a").unwrap().project_id.as_deref() == Some("p-1"));
        assert!(store.get("tenant-b").unwrap().host == "https://lf.example.com");

        let reopened = LangfuseStore::new(&path).unwrap();
        assert_eq!(reopened.get("tenant-a").unwrap().secret_key, "sk-a");
        assert_eq!(reopened.get("tenant-b").unwrap().public_key, "pk-b");

        assert!(reopened.delete("tenant-a").unwrap());
        assert!(!reopened.delete("tenant-a").unwrap());
        let reopened = LangfuseStore::new(&path).unwrap();
        assert!(reopened.get("tenant-a").is_none());
        assert!(reopened.get("tenant-b").is_some());
    }

    #[test]
    fn host_normalization_adds_https_and_rejects_invalid() {
        assert_eq!(
            normalize_langfuse_host("https://cloud.langfuse.com/").unwrap(),
            "https://cloud.langfuse.com"
        );
        assert_eq!(
            normalize_langfuse_host("lf.example.com").unwrap(),
            "https://lf.example.com"
        );
        assert!(normalize_langfuse_host("").is_err());
        assert!(normalize_langfuse_host("://bad").is_err());
    }

    #[tokio::test]
    async fn probe_reads_health_and_first_project() {
        let app = axum::Router::new()
            .route(
                "/api/public/health",
                axum::routing::get(|| async { (StatusCode::OK, r#"{"status":"ok"}"#) }),
            )
            .route(
                "/api/public/projects",
                axum::routing::get(|| async {
                    (
                        StatusCode::OK,
                        r#"{"data":[{"id":"p-1","name":"Audit Project"}]}"#,
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (project_id, project_name) =
            probe_langfuse(&format!("http://{address}"), "pk-audit", "sk-audit")
                .await
                .unwrap();
        server.abort();
        assert_eq!(project_id.as_deref(), Some("p-1"));
        assert_eq!(project_name.as_deref(), Some("Audit Project"));
    }

    #[tokio::test]
    async fn probe_rejects_invalid_credentials() {
        let app = axum::Router::new().route(
            "/api/public/health",
            axum::routing::get(|| async { StatusCode::UNAUTHORIZED }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = probe_langfuse(&format!("http://{address}"), "pk-bad", "sk-bad").await;
        server.abort();
        assert!(
            result.is_err(),
            "non-2xx health probe must fail closed with an error"
        );
    }
}
