//! Data source manager — configures and syncs external connectors
//! (GitLab MVP). Mirrors RAGFlow's connector admin surface.

use crate::data_source::{GitLabConnector, SourceOptions};
use anyhow::Result;
use axum::Json;
use axum::extract::{Extension, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DataSourceSyncLog {
    pub id: String,
    /// RAGFlow `TaskStatus` vocabulary (UNSTART/RUNNING/CANCEL/DONE/FAIL/
    /// SCHEDULE) — the v0.26.4 log table `getSummary` branches on exactly
    /// these values (plus the legacy numeric spellings).
    pub status: String,
    pub message: String,
    pub created_at: String,
    /// RAGFlow `update_date` — display timestamp (newest-first ordering key).
    pub update_date: String,
    /// RAGFlow `task_type` — "sync" or "prune".
    pub task_type: String,
    /// RAGFlow `kb_id` / `kb_name` — the knowledge base a sync wrote into.
    pub kb_id: String,
    pub kb_name: String,
    /// RAGFlow `new_docs_indexed` / `total_docs_indexed` summary counters.
    pub new_docs_indexed: Option<u64>,
    pub total_docs_indexed: Option<u64>,
    /// RAGFlow `docs_removed_from_index` (prune summary).
    pub docs_removed_from_index: Option<u64>,
    /// RAGFlow `error_count` / `error_msg`.
    pub error_count: Option<u64>,
    pub error_msg: Option<String>,
    /// RAGFlow `time_started` — feeds the UI countdown ("Task starts in …").
    pub time_started: Option<String>,
    /// RAGFlow joins `Connector.refresh_freq` / `prune_freq` onto each row.
    pub refresh_freq: Option<u64>,
    pub prune_freq: Option<u64>,
}

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
    /// Whether the connector has passed `load_credentials` (授权) since last
    /// edit. Mirrors RAGFlow's connector enabled/connected state.
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub sync_deleted_files: bool,
    #[serde(default)]
    pub prune_freq: Option<u64>,
    #[serde(default)]
    pub refresh_freq: Option<u64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub status: String,
    /// Flattened nested connector `config.*` fields (dot-path → string).
    #[serde(default)]
    pub extra: HashMap<String, String>,
    #[serde(default)]
    pub sync_logs: Vec<DataSourceSyncLog>,
}

/// RAGFlow stores `TaskStatus` as `"0".."5"`; RayRAG uses the readable
/// lowercase wire forms and accepts both spellings on PATCH.
pub fn normalize_status(value: &serde_json::Value) -> Option<String> {
    let raw = match value {
        serde_json::Value::String(text) => text.as_str(),
        serde_json::Value::Number(number) => {
            return match number.as_u64() {
                Some(0) => Some("unstart".into()),
                Some(1) => Some("running".into()),
                Some(2) => Some("cancel".into()),
                Some(3) => Some("done".into()),
                Some(4) => Some("fail".into()),
                Some(5) => Some("schedule".into()),
                _ => None,
            };
        }
        _ => return None,
    };
    match raw.to_ascii_lowercase().as_str() {
        "0" | "unstart" => Some("unstart".into()),
        "1" | "running" => Some("running".into()),
        "2" | "cancel" => Some("cancel".into()),
        "3" | "done" => Some("done".into()),
        "4" | "fail" => Some("fail".into()),
        "5" | "schedule" => Some("schedule".into()),
        _ => None,
    }
}

/// Flatten a nested connector `config` object into dot-path key-values rooted
/// at `prefix` (the wire `config` object). Scalars keep their string form;
/// arrays/objects are stored as compact JSON.
pub fn flatten_config(value: &serde_json::Value, prefix: &str) -> HashMap<String, String> {
    fn walk(value: &serde_json::Value, prefix: &str, output: &mut HashMap<String, String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    walk(child, &path, output);
                }
            }
            serde_json::Value::Null => {}
            serde_json::Value::String(text) => {
                output.insert(prefix.to_string(), text.clone());
            }
            other => {
                output.insert(prefix.to_string(), other.to_string());
            }
        }
    }
    let mut output = HashMap::new();
    walk(value, prefix, &mut output);
    output
}

/// Rebuild a nested `config` object from flattened dot-path key-values.
pub fn unflatten_config(extra: &HashMap<String, String>) -> serde_json::Value {
    let mut root = serde_json::Map::new();
    let mut keys: Vec<&String> = extra.keys().collect();
    keys.sort();
    for key in keys {
        let value = extra.get(key).cloned().unwrap_or_default();
        let mut node = &mut root;
        let parts: Vec<&str> = key.split('.').collect();
        for (index, part) in parts.iter().enumerate() {
            if index + 1 == parts.len() {
                node.insert(
                    (*part).to_string(),
                    serde_json::Value::String(value.clone()),
                );
            } else {
                node = node
                    .entry((*part).to_string())
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
                    .as_object_mut()
                    .expect("flattened key does not collide with a scalar");
            }
        }
    }
    serde_json::Value::Object(root)
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
            "connected": self.connected,
            "sync_deleted_files": self.sync_deleted_files,
            "prune_freq": self.prune_freq,
            "refresh_freq": self.refresh_freq,
            "timeout_secs": self.timeout_secs,
            "status": self.status,
            "config": {
                "sync_deleted_files": self.sync_deleted_files,
            },
            "token": if self.token.is_empty() { "" } else { "***" },
        })
    }

    /// Full detail projection for the edit form: keeps the persisted nested
    /// config values (credentials included) so the browser can prefill, while
    /// the flat legacy token stays masked like the list projection.
    pub fn detail_json(&self) -> serde_json::Value {
        let mut detail = self.redacted();
        let mut config = unflatten_config(&self.extra);
        let mut inner = config
            .get_mut("config")
            .and_then(serde_json::Value::as_object_mut)
            .cloned()
            .map(serde_json::Value::Object)
            .unwrap_or_else(|| serde_json::json!({}));
        inner["sync_deleted_files"] = serde_json::json!(self.sync_deleted_files);
        detail["config"] = inner;
        detail
    }

    pub fn to_options(&self) -> SourceOptions {
        SourceOptions {
            url: self.url.clone(),
            token: self.token.clone(),
            target: self.target.clone(),
            include_issues: self.include_issues,
            include_merge_requests: self.include_merge_requests,
            max_items: self.max_items,
            extra: self.extra.clone(),
        }
    }
}

/// Fold the well-known nested config keys back onto the flat legacy record
/// fields the existing connector dispatch reads (GitLab URL/target/token and
/// the include flags), while keeping the full dot-path map in `extra`.
fn apply_config_mapping(record: &mut DataSourceRecord, config: &serde_json::Value) {
    let map = flatten_config(config, "config");
    for (key, value) in map {
        if key == "config.sync_deleted_files" {
            continue;
        }
        record.extra.insert(key, value);
    }
    if let Some(flag) = config.get("sync_deleted_files").and_then(|v| v.as_bool()) {
        record.sync_deleted_files = flag;
    }
    let get = |key: &str| record.extra.get(key).cloned().unwrap_or_default();
    let url = [
        "config.gitlab_url",
        "config.base_url",
        "config.feed_url",
        "config.seafile_url",
        "config.moodle_url",
        "config.wiki_base",
        "config.credentials.instance_url",
    ]
    .iter()
    .map(|key| get(key))
    .find(|value| !value.is_empty());
    if let Some(url) = url {
        record.url = url;
    }
    let owner = get("config.project_owner");
    let project = get("config.project_name");
    if !owner.is_empty() && !project.is_empty() {
        record.target = format!("{owner}/{project}");
    } else {
        let target = [
            "config.remote_path",
            "config.folder_id",
            "config.base_id",
            "config.table_name_or_id",
            "config.bucket_name",
        ]
        .iter()
        .map(|key| get(key))
        .find(|value| !value.is_empty());
        if let Some(target) = target {
            record.target = target;
        }
    }
    let token = get("config.credentials.gitlab_access_token");
    if !token.is_empty() {
        record.token = token;
    }
    record.include_merge_requests = get("config.include_mrs") == "true";
    record.include_issues = get("config.include_issues") == "true";
    if let Ok(batch) = get("config.batch_size").parse::<usize>() {
        record.max_items = batch.max(1);
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
        let _guard = self
            .save_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("save lock poisoned"))?;
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

    /// Append a fully-specified log entry (newest first) and persist.
    /// Mirrors RAGFlow `SyncLogsService.list_sync_tasks` row shape so the
    /// v0.26.4 log table (5 columns + summary + countdown) renders directly.
    pub fn append_log_full(&self, id: &str, log: DataSourceSyncLog) -> Result<()> {
        let mut sources = self
            .sources
            .write()
            .map_err(|_| anyhow::anyhow!("sources lock poisoned"))?;
        let Some(record) = sources.get_mut(id) else {
            return Ok(());
        };
        record.sync_logs.insert(0, log);
        record.sync_logs.truncate(100);
        drop(sources);
        self.persist()
    }

    /// Append a minimal sync/connect log entry (newest first) and persist.
    pub fn append_log(&self, id: &str, status: &str, message: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.append_log_full(
            id,
            DataSourceSyncLog {
                id: uuid::Uuid::new_v4().to_string(),
                status: status.to_string(),
                message: message.to_string(),
                created_at: now.clone(),
                update_date: now,
                ..Default::default()
            },
        )
    }

    /// Append a rich sync log row (RAGFlow `SyncLogs` column set): status,
    /// task type, kb id/name, ingest counters, error info and the connector
    /// refresh/prune frequencies used by the UI countdown.
    pub fn append_sync_log(
        &self,
        id: &str,
        record: &DataSourceRecord,
        status: &str,
        task_type: &str,
        message: &str,
        new_docs: Option<u64>,
        total_docs: Option<u64>,
        removed_docs: Option<u64>,
        error_count: Option<u64>,
        error_msg: Option<String>,
        kb_id: Option<&str>,
        kb_name: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let kb_id = kb_id
            .filter(|kb| !kb.is_empty())
            .unwrap_or_default()
            .to_string();
        let kb_name = kb_name.unwrap_or_default().to_string();
        self.append_log_full(
            id,
            DataSourceSyncLog {
                id: uuid::Uuid::new_v4().to_string(),
                status: status.to_string(),
                message: message.to_string(),
                created_at: now.clone(),
                update_date: now.clone(),
                task_type: task_type.to_string(),
                kb_id,
                kb_name,
                new_docs_indexed: new_docs,
                total_docs_indexed: total_docs,
                docs_removed_from_index: removed_docs,
                error_count,
                error_msg,
                time_started: Some(now),
                refresh_freq: record.refresh_freq,
                prune_freq: record.prune_freq,
            },
        )
    }

    /// Transition the connector status (unstart/running/cancel/done/fail/
    /// schedule) and persist.
    pub fn set_status(&self, id: &str, status: &str) -> Result<()> {
        let mut sources = self
            .sources
            .write()
            .map_err(|_| anyhow::anyhow!("sources lock poisoned"))?;
        let Some(record) = sources.get_mut(id) else {
            return Ok(());
        };
        record.status = status.to_string();
        drop(sources);
        self.persist()
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateDataSourceRequest {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// Optional for schema-driven creates whose URL derives from `config`.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: String,
    /// Optional for schema-driven creates whose target derives from `config`.
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub include_issues: bool,
    #[serde(default)]
    pub include_merge_requests: bool,
    #[serde(default = "default_max_items")]
    pub max_items: usize,
    /// Nested connector config (upstream create wire shape).
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub sync_deleted_files: Option<bool>,
    #[serde(default)]
    pub prune_freq: Option<u64>,
    #[serde(default)]
    pub refresh_freq: Option<u64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
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

/// The error shape the data-source endpoints answer with.
///
/// These handlers used to return plain `(status, string)`, so a browser reading the body
/// as JSON hit a parse error instead of the message — the one thing an operator needs
/// when a connector is refused. RAGFlow's own endpoints answer `{code, message}`.
pub type DataSourceError = (StatusCode, Json<serde_json::Value>);

/// Build a connector error with the RAGFlow body shape.
pub fn data_source_error(status: StatusCode, message: impl Into<String>) -> DataSourceError {
    let message = message.into();
    (
        status,
        Json(serde_json::json!({ "code": status.as_u16(), "message": message })),
    )
}

pub async fn create_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Json(body): Json<CreateDataSourceRequest>,
) -> Result<Json<serde_json::Value>, DataSourceError> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| data_source_error(StatusCode::NOT_IMPLEMENTED, "data sources disabled"))?;
    let name = body.name.trim().to_string();
    let url = body.url.unwrap_or_default().trim().to_string();
    let target = body.target.unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return Err(data_source_error(
            StatusCode::BAD_REQUEST,
            "name is required",
        ));
    }
    // Supported kinds: gitlab (legacy) + connector registry kinds.
    if !is_supported_kind(&body.kind) {
        return Err(data_source_error(
            StatusCode::BAD_REQUEST,
            format!(
                "unsupported data source type: {} (supported: {})",
                body.kind,
                supported_kinds()
            ),
        ));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let mut record = DataSourceRecord {
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
        connected: false,
        sync_deleted_files: body.sync_deleted_files.unwrap_or(false),
        prune_freq: Some(body.prune_freq.unwrap_or(5)),
        refresh_freq: Some(body.refresh_freq.unwrap_or(5)),
        timeout_secs: Some(body.timeout_secs.unwrap_or(1740)),
        status: "unstart".into(),
        extra: HashMap::new(),
        sync_logs: Vec::new(),
    };
    if let Some(config) = body.config {
        // The connector constants validate across fields (`customValidate` upstream). The
        // dialog runs the same rules, but a client that skips the JavaScript — or an API
        // caller — must not be able to store a configuration the connector cannot use.
        let failures = crate::data_source::form_fields::validate_config(&record.kind, &config);
        if let Some((_, message)) = failures.first() {
            return Err(data_source_error(StatusCode::BAD_REQUEST, message));
        }
        apply_config_mapping(&mut record, &config);
    } else if record.url.is_empty() || record.target.is_empty() {
        return Err(data_source_error(
            StatusCode::BAD_REQUEST,
            "name/url/target are required",
        ));
    }
    store
        .insert(record.clone())
        .map_err(|error| data_source_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(
        serde_json::json!({ "code": 0, "data": record.redacted() }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct UpdateDataSourceRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub sync_deleted_files: Option<bool>,
    #[serde(default)]
    pub prune_freq: Option<u64>,
    #[serde(default)]
    pub refresh_freq: Option<u64>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Nested connector config (upstream `config` wire shape).
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    /// `cancel`/`schedule`/`unstart`/… — string or legacy numeric.
    #[serde(default)]
    pub status: Option<serde_json::Value>,
    /// Cancel then reschedule the connector (upstream `reschedule: true`).
    #[serde(default)]
    pub reschedule: Option<bool>,
}

/// GET `/api/v1/datasources/{id}` — redacted detail plus the sync log.
pub async fn get_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, DataSourceError> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| data_source_error(StatusCode::NOT_IMPLEMENTED, "data sources disabled"))?;
    let record = store
        .get(&id)
        .ok_or_else(|| data_source_error(StatusCode::NOT_FOUND, "data source not found"))?;
    let mut detail = record.detail_json();
    detail["sync_logs"] = serde_json::to_value(&record.sync_logs).unwrap_or_default();
    Ok(Json(serde_json::json!({ "code": 0, "data": detail })))
}

/// PUT/PATCH `/api/v1/data_sources/{id}` — update the editable connector
/// settings, apply the nested `config`, and honor the upstream
/// `reschedule` / `status` task transitions. Credential changes reset the
/// connected flag like upstream.
pub async fn update_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Extension(auth): Extension<crate::server::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<UpdateDataSourceRequest>,
) -> Result<Json<serde_json::Value>, DataSourceError> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| data_source_error(StatusCode::NOT_IMPLEMENTED, "data sources disabled"))?;
    let mut record = store
        .get(&id)
        .ok_or_else(|| data_source_error(StatusCode::NOT_FOUND, "data source not found"))?;
    let credential_changed = body.url.is_some()
        || body.token.is_some()
        || body.target.is_some()
        || body.config.is_some();
    if let Some(name) = body.name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err(data_source_error(
                StatusCode::BAD_REQUEST,
                "name is required",
            ));
        }
        record.name = name;
    }
    if let Some(url) = body.url {
        let url = url.trim().to_string();
        if url.is_empty() {
            return Err(data_source_error(
                StatusCode::BAD_REQUEST,
                "url is required",
            ));
        }
        record.url = url;
    }
    if let Some(token) = body.token {
        record.token = token;
    }
    if let Some(target) = body.target {
        record.target = target.trim().to_string();
    }
    if let Some(value) = body.sync_deleted_files {
        record.sync_deleted_files = value;
    }
    if let Some(value) = body.prune_freq {
        record.prune_freq = Some(value);
    }
    if let Some(value) = body.refresh_freq {
        record.refresh_freq = Some(value);
    }
    if let Some(value) = body.timeout_secs {
        record.timeout_secs = Some(value);
    }
    if let Some(config) = body.config {
        let failures = crate::data_source::form_fields::validate_config(&record.kind, &config);
        if let Some((_, message)) = failures.first() {
            return Err(data_source_error(StatusCode::BAD_REQUEST, message));
        }
        apply_config_mapping(&mut record, &config);
    }
    if credential_changed {
        record.connected = false;
    }
    let requested_status = match body.status.as_ref() {
        Some(value) => Some(
            normalize_status(value)
                .ok_or(data_source_error(StatusCode::BAD_REQUEST, "invalid status"))?,
        ),
        None => None,
    };
    let should_resume =
        body.reschedule == Some(true) || requested_status.as_deref() == Some("schedule");
    if let Some(status) = requested_status {
        record.status = status;
    }
    store
        .insert(record.clone())
        .map_err(|error| data_source_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;

    // RayRAG has no background connector scheduler: a reschedule/schedule
    // request runs the first sync eagerly and leaves the status `schedule` on
    // success (or `fail` on error) — a documented deviation from upstream's
    // async task queue.
    if should_resume {
        let _ = perform_sync(&state, &auth, &id, None).await;
        let updated = store
            .get(&id)
            .ok_or_else(|| data_source_error(StatusCode::NOT_FOUND, "data source not found"))?;
        let mut detail = updated.detail_json();
        detail["sync_logs"] = serde_json::to_value(&updated.sync_logs).unwrap_or_default();
        return Ok(Json(serde_json::json!({ "code": 0, "data": detail })));
    }

    let mut detail = record.detail_json();
    detail["sync_logs"] = serde_json::to_value(&record.sync_logs).unwrap_or_default();
    Ok(Json(serde_json::json!({ "code": 0, "data": detail })))
}

/// GET `/api/v1/datasources/{id}/logs?page=&page_size=` — the newest-first
/// sync log with RAGFlow `SyncLogsService.list_sync_tasks` semantics: page
/// size capped at 100 (400 above it, mirroring
/// `validate_rest_api_page_size`), `{total, logs}` envelope, rows ordered by
/// `update_time DESC` (RayRAG keeps rows newest-first).
pub async fn get_data_source_logs(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
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
    let page: u64 = query
        .get("page")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let page_size: u64 = match query
        .get("page_size")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(15)
    {
        size if size == 0 => 15,
        size if size > 100 => {
            return Err((
                StatusCode::BAD_REQUEST,
                "page_size must be less than or equal to 100".into(),
            ));
        }
        size => size,
    };
    let total = record.sync_logs.len() as u64;
    let start = ((page - 1) * page_size) as usize;
    let logs: Vec<_> = if start >= record.sync_logs.len() {
        Vec::new()
    } else {
        record
            .sync_logs
            .iter()
            .skip(start)
            .take(page_size as usize)
            .cloned()
            .collect()
    };
    Ok(Json(
        serde_json::json!({ "code": 0, "data": { "total": total, "logs": logs } }),
    ))
}

pub async fn delete_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let removed = store
        .remove(&id)
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(
        serde_json::json!({ "code": 0, "data": { "removed": removed } }),
    ))
}

pub async fn sync_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Extension(auth): Extension<crate::server::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let kb_id = query.get("kb_id").cloned();
    let result = perform_sync(&state, &auth, &id, kb_id.as_deref()).await?;
    Ok(Json(result))
}

/// Shared connector sync core used by both POST `/sync` and the PATCH
/// reschedule/schedule flow. Status transitions are persisted so the
/// browser-side Save/Stop/Resume state machine stays truthful.
async fn perform_sync(
    state: &std::sync::Arc<crate::server::AppState>,
    auth: &crate::server::AuthContext,
    id: &str,
    kb_id: Option<&str>,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let record = store
        .get(id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "data source not found".into()))?;
    let _ = store.set_status(id, "running");
    let options = record.to_options();
    let connector_result: std::result::Result<crate::connectors::SyncBatch, (StatusCode, String)> =
        match record.kind.as_str() {
            // Legacy path: GitLab connector lives in src/data_source/.
            "gitlab" => {
                let docs = GitLabConnector::new()
                    .fetch(&options)
                    .await
                    .map_err(|error| {
                        (StatusCode::BAD_GATEWAY, format!("sync failed: {error:#}"))
                    })?;
                Ok(crate::connectors::SyncBatch {
                    docs,
                    failures: Vec::new(),
                })
            }
            // Connector registry dispatch (RAGFlow data-source type → Connector).
            kind => {
                let connector = crate::connectors::ConnectorRegistry::create(kind, options)
                    .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
                connector
                    .fetch_all(record.max_items)
                    .await
                    .map_err(|error| (StatusCode::BAD_GATEWAY, format!("sync failed: {error:#}")))
            }
        };
    let batch = match connector_result {
        Ok(batch) => batch,
        Err(error) => {
            let _ = store.set_status(id, "fail");
            let kb_name = kb_id.and_then(|kb| state.kbs.get(kb).map(|kb| kb.name.clone()));
            let _ = store.append_sync_log(
                id,
                &record,
                "FAIL",
                "sync",
                &error.1,
                None,
                None,
                None,
                Some(1),
                Some(error.1.clone()),
                kb_id,
                kb_name.as_deref(),
            );
            return Err(error);
        }
    };
    let docs = batch.docs;
    let connector_failures = batch.failures;

    // Optional ingestion into a knowledge base: ?kb_id=<dataset id>.
    // Each connector doc is written as a text file and routed through the
    // standard upload pipeline (parse -> chunk -> embed -> index).
    let mut ingested = 0_usize;
    let mut skipped = Vec::new();
    if let Some(kb_id) = kb_id.filter(|value| !value.is_empty()) {
        if !crate::server::kb_manageable(state, kb_id, auth) {
            let _ = store.set_status(id, "fail");
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
                    tracing::warn!(
                        "data source: ingest {} failed: {error:#}",
                        doc.semantic_identifier
                    );
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
    let kb_name = kb_id.and_then(|kb| state.kbs.get(kb).map(|kb| kb.name.clone()));
    let _ = store.append_sync_log(
        id,
        &record,
        "DONE",
        "sync",
        &format!(
            "Synced {} docs (ingested {ingested}, skipped {})",
            docs.len(),
            skipped.len()
        ),
        Some(ingested as u64),
        Some(docs.len() as u64),
        None,
        Some(connector_failures.len() as u64),
        None,
        kb_id,
        kb_name.as_deref(),
    );
    let _ = store.set_status(id, "schedule");
    Ok(serde_json::json!({
        "code": 0,
        "data": { "count": docs.len(), "ingested": ingested, "skipped": skipped, "failures": connector_failures, "preview": preview }
    }))
}

// ── Connector lifecycle endpoints (连接/断开/拉取列表) ──────────────────────

/// POST /api/v1/data_sources/{id}/connect — 授权: run `load_credentials`
/// against the source and mark the data source connected on success.
/// Mirrors RAGFlow's "测试连接" (validate connector settings).
pub async fn connect_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let mut record = store
        .get(&id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "data source not found".into()))?;
    let options = record.to_options();
    let mut connector = crate::connectors::ConnectorRegistry::create(&record.kind, options)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    if let Err(error) = connector.load_credentials().await {
        let _ = store.append_sync_log(
            &id,
            &record,
            "FAIL",
            "sync",
            &format!("connect failed: {error:#}"),
            None,
            None,
            None,
            Some(1),
            Some(format!("connect failed: {error:#}")),
            None,
            None,
        );
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("connect failed: {error:#}"),
        ));
    }
    record.connected = true;
    store
        .insert(record.clone())
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    let _ = store.append_sync_log(
        &id,
        &record,
        "DONE",
        "sync",
        "Connection verified",
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    Ok(Json(
        serde_json::json!({ "code": 0, "data": record.redacted() }),
    ))
}

/// POST /api/v1/data_sources/{id}/disconnect — mark the data source
/// disconnected (RAGFlow disables the connector without deleting it).
pub async fn disconnect_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let mut record = store
        .get(&id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "data source not found".into()))?;
    record.connected = false;
    store
        .insert(record.clone())
        .map_err(|error| (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(
        serde_json::json!({ "code": 0, "data": record.redacted() }),
    ))
}

/// POST /api/v1/data_sources/{id}/files — 拉取列表: run `list_files` and
/// return the remote file listing (RAGFlow slim-doc listing).
pub async fn list_data_source_files(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let record = store
        .get(&id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "data source not found".into()))?;
    let options = record.to_options();
    let connector = crate::connectors::ConnectorRegistry::create(&record.kind, options)
        .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
    let files = connector
        .list_files()
        .await
        .map_err(|error| (StatusCode::BAD_GATEWAY, format!("list failed: {error:#}")))?;
    let preview: Vec<serde_json::Value> = files
        .iter()
        .take(record.max_items.max(1))
        .map(|file| {
            serde_json::json!({
                "id": file.id,
                "name": file.name,
                "path": file.path,
                "extension": file.extension,
                "size_bytes": file.size_bytes,
                "updated_at": file.updated_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({
        "code": 0,
        "data": { "count": files.len(), "files": preview }
    })))
}

fn is_supported_kind(kind: &str) -> bool {
    if kind == "gitlab" {
        return true;
    }
    crate::connectors::ConnectorRegistry::kinds()
        .iter()
        .any(|info| info.kind == kind)
}

fn supported_kinds() -> String {
    let mut kinds: Vec<String> = vec!["gitlab".into()];
    for info in crate::connectors::ConnectorRegistry::kinds() {
        kinds.push(info.kind.to_string());
    }
    kinds.join("/")
}

// ---- tests ----

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_normalization_accepts_upstream_wire_forms() {
        assert_eq!(
            normalize_status(&serde_json::json!("cancel")).as_deref(),
            Some("cancel")
        );
        assert_eq!(
            normalize_status(&serde_json::json!("SCHEDULE")).as_deref(),
            Some("schedule")
        );
        assert_eq!(
            normalize_status(&serde_json::json!(2)).as_deref(),
            Some("cancel")
        );
        assert_eq!(
            normalize_status(&serde_json::json!("5")).as_deref(),
            Some("schedule")
        );
        assert!(normalize_status(&serde_json::json!("bogus")).is_none());
        assert!(normalize_status(&serde_json::json!(99)).is_none());
    }

    #[test]
    fn config_flatten_and_unflatten_roundtrip_nested_credentials() {
        let config = serde_json::json!({
            "credentials": {"client_id": "cid", "client_secret": "secret"},
            "batch_size": 2,
            "include_mrs": true,
            "empty": null,
            "server_ids": ["a", "b"],
        });
        let flat = flatten_config(&config, "config");
        assert_eq!(flat.get("config.credentials.client_id").unwrap(), "cid");
        assert_eq!(flat.get("config.batch_size").unwrap(), "2");
        assert_eq!(flat.get("config.include_mrs").unwrap(), "true");
        assert_eq!(flat.get("config.server_ids").unwrap(), "[\"a\",\"b\"]");
        assert!(!flat.contains_key("config.empty"));

        let rebuilt = unflatten_config(&flat);
        assert_eq!(
            rebuilt["config"]["credentials"]["client_id"],
            serde_json::json!("cid")
        );
        assert_eq!(rebuilt["config"]["batch_size"], serde_json::json!("2"));
    }

    #[test]
    fn config_mapping_folds_gitlab_fields_onto_the_flat_record() {
        let mut record = DataSourceRecord {
            id: "g1".into(),
            name: "gitlab".into(),
            kind: "gitlab".into(),
            url: String::new(),
            token: String::new(),
            target: String::new(),
            include_issues: false,
            include_merge_requests: false,
            max_items: 5,
            created_at: String::new(),
            connected: false,
            sync_deleted_files: false,
            prune_freq: None,
            refresh_freq: None,
            timeout_secs: None,
            status: "unstart".into(),
            extra: HashMap::new(),
            sync_logs: Vec::new(),
        };
        apply_config_mapping(
            &mut record,
            &serde_json::json!({
                "project_owner": "owner",
                "project_name": "repo",
                "gitlab_url": "https://gitlab.example.com",
                "include_mrs": true,
                "include_issues": true,
                "credentials": {"gitlab_access_token": "tok"},
                "sync_deleted_files": true,
            }),
        );
        assert_eq!(record.url, "https://gitlab.example.com");
        assert_eq!(record.target, "owner/repo");
        assert_eq!(record.token, "tok");
        assert!(record.include_merge_requests);
        assert!(record.include_issues);
        assert!(record.sync_deleted_files);
        assert_eq!(
            record.extra.get("config.project_owner").map(String::as_str),
            Some("owner")
        );
        assert!(!record.extra.contains_key("config.sync_deleted_files"));
    }

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
            connected: false,
            sync_deleted_files: false,
            prune_freq: None,
            refresh_freq: None,
            timeout_secs: None,
            status: "unstart".into(),
            extra: HashMap::new(),
            sync_logs: Vec::new(),
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
                    connected: false,
                    sync_deleted_files: false,
                    prune_freq: None,
                    refresh_freq: None,
                    timeout_secs: None,
                    status: "unstart".into(),
                    extra: HashMap::new(),
                    sync_logs: Vec::new(),
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

    #[test]
    fn registry_dispatch_accepts_all_supported_kinds() {
        // create_data_source's kind gate mirrors the connector registry.
        for kind in [
            "gitlab",
            "azure_blob",
            "local",
            "webdav",
            "feishu",
            "confluence",
            "r2",
            "dingtalk_ai_table",
            "postgresql",
            "rss",
        ] {
            assert!(is_supported_kind(kind), "kind {kind} should be supported");
        }
        assert!(!is_supported_kind("nope"));
        assert!(supported_kinds().contains("feishu"));
    }

    #[test]
    fn record_roundtrip_preserves_connected_flag() {
        let record = DataSourceRecord {
            id: "c1".into(),
            name: "feishu-main".into(),
            kind: "feishu".into(),
            url: "https://open.feishu.cn".into(),
            token: "app:secret".into(),
            target: String::new(),
            include_issues: false,
            include_merge_requests: false,
            max_items: 10,
            created_at: "2026-01-01T00:00:00Z".into(),
            connected: true,
            sync_deleted_files: false,
            prune_freq: None,
            refresh_freq: None,
            timeout_secs: None,
            status: "unstart".into(),
            extra: HashMap::new(),
            sync_logs: Vec::new(),
        };
        let json = serde_json::to_string(&record).unwrap();
        let back: DataSourceRecord = serde_json::from_str(&json).unwrap();
        assert!(back.connected);
        let redacted = back.redacted();
        assert_eq!(redacted["connected"], true);
    }

    #[test]
    fn sync_log_row_carries_ragflow_column_set_and_counts() {
        let dir = std::env::temp_dir().join(format!("rayrag-ds-log-{}", uuid::Uuid::new_v4()));
        let store = DataSourceStore::new(dir.to_str().unwrap()).unwrap();
        let record = DataSourceRecord {
            id: "1".into(),
            name: "gitlab-main".into(),
            kind: "gitlab".into(),
            url: "https://gitlab.com".into(),
            token: "t".into(),
            target: "1".into(),
            include_issues: false,
            include_merge_requests: false,
            max_items: 10,
            created_at: "2026-01-01T00:00:00Z".into(),
            connected: true,
            sync_deleted_files: true,
            prune_freq: Some(60),
            refresh_freq: Some(30),
            timeout_secs: None,
            status: "done".into(),
            extra: HashMap::new(),
            sync_logs: Vec::new(),
        };
        store.insert(record.clone()).unwrap();
        store
            .append_sync_log(
                "1",
                &record,
                "DONE",
                "sync",
                "Synced 3 docs (ingested 2, skipped 1)",
                Some(2),
                Some(3),
                None,
                Some(1),
                None,
                Some("kb1"),
                Some("Finance"),
            )
            .unwrap();
        store
            .append_sync_log(
                "1",
                &record,
                "FAIL",
                "sync",
                "boom",
                None,
                None,
                None,
                Some(1),
                Some("boom".into()),
                Some("kb1"),
                Some("Finance"),
            )
            .unwrap();
        let record = store.get("1").unwrap();
        assert_eq!(record.sync_logs.len(), 2);
        let latest = &record.sync_logs[0];
        assert_eq!(latest.status, "FAIL");
        assert_eq!(latest.task_type, "sync");
        assert_eq!(latest.kb_id, "kb1");
        assert_eq!(latest.kb_name, "Finance");
        assert_eq!(latest.error_msg.as_deref(), Some("boom"));
        assert_eq!(latest.refresh_freq, Some(30));
        assert_eq!(latest.prune_freq, Some(60));
        assert!(latest.time_started.is_some());
        let done = &record.sync_logs[1];
        assert_eq!(done.status, "DONE");
        assert_eq!(done.new_docs_indexed, Some(2));
        assert_eq!(done.total_docs_indexed, Some(3));
        assert_eq!(done.docs_removed_from_index, None);
        assert_eq!(done.error_count, Some(1));
        let json = serde_json::to_value(&latest).unwrap();
        assert!(json.get("docs_removed_from_index").is_some());
        assert!(json.get("time_started").is_some());
        assert!(json.get("error_count").is_some());
        std::fs::remove_dir_all(dir).ok();
    }
}
