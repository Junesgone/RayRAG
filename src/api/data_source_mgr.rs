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
    /// Who initiated the run: manual / link / schedule / reschedule / connect.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger: Option<String>,
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
    // Replace the previous `config.*` snapshot instead of merging into it: a
    // field the form omitted (or the user cleared) must not survive from an
    // earlier edit and silently keep steering the connector.
    record.extra.retain(|key, _| !key.starts_with("config."));
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
        // The generic key the schema uses for REST-style sources, and the one the connector hints name.
        "config.url",
        "config.credentials.base_url",
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
    // Credentials live under `config.credentials.*`, and the schema names them per source. Reading only
    // GitLab's key left every other schema-driven source with an empty token: the record was created and the
    // connector then had nothing to authenticate with. The app-id/app-secret pair is joined the way the
    // Feishu, DingTalk and GitLab connectors split it (`id:secret`); otherwise the first well-known key wins.
    let app_id = [
        "config.credentials.app_id",
        "config.credentials.app_key",
        "config.credentials.client_id",
    ]
    .iter()
    .map(|key| get(key))
    .find(|value| !value.is_empty());
    let app_secret = get("config.credentials.app_secret");
    // Two-field credentials: the form collects a user and a secret, the connector authenticates with one
    // `user:secret` string. None of these pairs used to be folded, so saving a WebDAV, S3, Confluence or Azure
    // source from the schema-driven form stored an empty token — the save reported success and the source could
    // never connect. Both halves must be present; otherwise the single-key list below gets its chance.
    let pair = [
        ("config.credentials.username", "config.credentials.password"),
        (
            "config.credentials.aws_access_key_id",
            "config.credentials.aws_secret_access_key",
        ),
        (
            "config.credentials.confluence_username",
            "config.credentials.confluence_access_token",
        ),
        (
            "config.credentials.account_name",
            "config.credentials.account_key",
        ),
    ]
    .iter()
    .find_map(|(user_key, secret_key)| {
        let user = get(user_key);
        let secret = get(secret_key);
        if user.is_empty() || secret.is_empty() {
            None
        } else {
            Some(format!("{user}:{secret}"))
        }
    });
    let single = [
        "config.credentials.gitlab_access_token",
        "config.credentials.token",
        "config.credentials.access_token",
        "config.credentials.api_token",
        "config.credentials.api_key",
        "config.credentials.notion_integration_token",
        "config.credentials.secret_key",
        "config.token",
    ]
    .iter()
    .map(|key| get(key))
    .find(|value| !value.is_empty());
    // The app-id pair wins, then the user:secret pairs, then the first single well-known key.
    let token = match (app_id, app_secret.is_empty()) {
        (Some(id), false) => Some(format!("{id}:{app_secret}")),
        _ => pair.or(single),
    };
    if let Some(token) = token {
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
        // A corrupt or old-schema file must never be read as "no data sources": the store
        // would start empty, the page would show nothing, and the next save would persist
        // that emptiness over the real records. The loader keeps the damaged file as a
        // timestamped backup, logs the reason and starts empty.
        let mut sources: HashMap<String, DataSourceRecord> =
            crate::persistence::load_json_or_quarantine(std::path::Path::new(&metadata_path))?;
        // The old 50-file default was a hard truncation for recursive sources.
        // Migrate records that still carry it unless the operator explicitly set
        // a connector batch size. Persist below so the migration is one-shot.
        let mut migrated = false;
        for record in sources.values_mut() {
            if record.max_items == 50 && !record.extra.contains_key("config.batch_size") {
                record.max_items = DEFAULT_DATA_SOURCE_MAX_ITEMS;
                migrated = true;
            }
        }
        let store = Self {
            sources: RwLock::new(sources),
            metadata_path,
            save_lock: Mutex::new(()),
        };
        if migrated {
            store.persist()?;
        }
        Ok(store)
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

    /// Full records for the background refresh scheduler. Unlike `list`,
    /// this includes the stored token and sync logs.
    pub(crate) fn records(&self) -> Vec<DataSourceRecord> {
        self.sources
            .read()
            .map(|sources| sources.values().cloned().collect())
            .unwrap_or_default()
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
        trigger: Option<&str>,
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
                trigger: trigger.map(str::to_string),
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

/// Effectively unlimited file count for a data source. The old default of 50
/// silently truncated recursive WebDAV/S3/local listings; 9999999999999999 is
/// deliberately far above any real NAS tree while still fitting in a JSON
/// integer and `usize` on 64-bit targets.
pub(crate) const DEFAULT_DATA_SOURCE_MAX_ITEMS: usize = 9_999_999_999_999_999;

fn default_max_items() -> usize {
    DEFAULT_DATA_SOURCE_MAX_ITEMS
}

pub async fn list_data_sources(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
) -> Result<Json<serde_json::Value>, DataSourceError> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| data_source_error(StatusCode::NOT_IMPLEMENTED, "data sources disabled"))?;
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
    // Supported kinds: gitlab (legacy) + connector registry kinds. The kind is normalised first so the
    // spellings the UI renders and the canonical registry kinds both resolve.
    let kind = normalize_kind(&body.kind);
    if !is_supported_kind(&kind) {
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
        kind: kind.clone(),
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
    } else {
        // No config was supplied, so the flat fields are all the caller gave.
        //
        // 33 of the 41 kinds reject an incomplete configuration when they are built and state the field
        // themselves, so they are checked by building the connector below. The remaining eight do not, and for
        // those the API checks the fields they actually need — a record missing one would otherwise be stored
        // happily and fail later, at sync time.
        //
        // The message this replaces was the fixed string "name/url/target are required", returned whatever was
        // missing: it named fields the caller had already supplied and rejected a source that needs only a URL.
        let missing: Vec<&str> = deferred_validation_fields(&kind)
            .iter()
            .copied()
            .filter(|field| match *field {
                "url" => record.url.is_empty(),
                "token" => record.token.is_empty(),
                "target" => record.target.is_empty(),
                _ => false,
            })
            .collect();
        if !missing.is_empty() {
            return Err(data_source_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "{} {} required for {kind} when config is omitted",
                    missing.join(" and "),
                    if missing.len() == 1 { "is" } else { "are" }
                ),
            ));
        }
        // Every kind is then built, so the kinds that validate in their constructor report their own reason.
        crate::connectors::ConnectorRegistry::create(&kind, record.to_options()).map_err(|error| {
            let message = error.to_string();
            // Some advertised kinds have no connector in this build at all. Refusing here is deliberate: before
            // this check the record was stored and only failed at sync time, with the same reason but after the
            // user had been told the source was created. Say so plainly instead.
            if message.contains("unsupported connector kind") {
                data_source_error(
                    StatusCode::BAD_REQUEST,
                    format!("{kind}: no connector is available for this source type, so it was not created"),
                )
            } else {
                data_source_error(StatusCode::BAD_REQUEST, message)
            }
        })?;
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
    // The create path refuses a configuration the connector cannot use; an update that clears one of those
    // fields must not be able to produce the same unusable record. Only checked when the request touched a
    // credential field, so renaming a record that predates this rule keeps working.
    if credential_changed {
        let missing: Vec<&str> = deferred_validation_fields(&record.kind)
            .iter()
            .copied()
            .filter(|field| match *field {
                "url" => record.url.is_empty(),
                "token" => record.token.is_empty(),
                "target" => record.target.is_empty(),
                _ => false,
            })
            .collect();
        if !missing.is_empty() {
            return Err(data_source_error(
                StatusCode::BAD_REQUEST,
                format!(
                    "{} {} required for {}",
                    missing.join(" and "),
                    if missing.len() == 1 { "is" } else { "are" },
                    record.kind
                ),
            ));
        }
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

    // A reschedule/schedule request runs the first sync eagerly and leaves the
    // status `schedule` on success (or `fail` on error); the background
    // scheduler introduced later performs subsequent refreshes.
    if should_resume {
        // The response carries the record read back below, so a failed sync must leave a
        // trace here: without it the API answers 200 with a source that is stuck in
        // `running` and no sync-log row explains why.
        if let Err((status, message)) = perform_sync(&state, &auth, &id, None, "reschedule").await {
            tracing::error!(%status, %message, %id, "Rescheduled data-source sync failed");
        }
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
) -> Result<Json<serde_json::Value>, DataSourceError> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| data_source_error(StatusCode::NOT_IMPLEMENTED, "data sources disabled"))?;
    let record = store
        .get(&id)
        .ok_or_else(|| data_source_error(StatusCode::NOT_FOUND, "data source not found"))?;
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
            return Err(data_source_error(
                StatusCode::BAD_REQUEST,
                "page_size must be less than or equal to 100",
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
) -> Result<Json<serde_json::Value>, DataSourceError> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| data_source_error(StatusCode::NOT_IMPLEMENTED, "data sources disabled"))?;
    let removed = store
        .remove(&id)
        .map_err(|error| data_source_error(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()))?;
    Ok(Json(
        serde_json::json!({ "code": 0, "data": { "removed": removed } }),
    ))
}

pub async fn sync_data_source(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Extension(auth): Extension<crate::server::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, DataSourceError> {
    let kb_id = query.get("kb_id").cloned();
    if state
        .data_sources
        .as_ref()
        .and_then(|store| store.get(&id))
        .is_some_and(|record| record.status == "running")
    {
        return Err(data_source_error(
            StatusCode::CONFLICT,
            "data source is already syncing",
        ));
    }
    let background = query
        .get("async")
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
    if background {
        // Large WebDAV/S3 trees can take minutes. The UI can start the sync and
        // poll the source status/logs instead of holding one HTTP request open.
        if state
            .data_sources
            .as_ref()
            .ok_or_else(|| data_source_error(StatusCode::NOT_IMPLEMENTED, "data sources disabled"))?
            .get(&id)
            .is_none()
        {
            return Err(data_source_error(
                StatusCode::NOT_FOUND,
                "data source not found",
            ));
        }
        let state = state.clone();
        let auth = auth.clone();
        let id = id.clone();
        let kb_id = kb_id.clone();
        tokio::spawn(async move {
            if let Err((_, message)) =
                perform_sync(&state, &auth, &id, kb_id.as_deref(), "manual").await
            {
                tracing::warn!(source = %id, %message, "background data-source sync failed");
                if let Some(store) = state.data_sources.as_ref() {
                    let _ = store.set_status(&id, "fail");
                }
            }
        });
        return Ok(Json(serde_json::json!({
            "code": 0,
            "data": { "async": true, "status": "running" },
        })));
    }
    let result = perform_sync(&state, &auth, &id, kb_id.as_deref(), "manual")
        .await
        .map_err(|(status, message)| data_source_error(status, message))?;
    Ok(Json(result))
}

/// Shared connector sync core used by both POST `/sync` and the PATCH
/// reschedule/schedule flow. Status transitions are persisted so the
/// browser-side Save/Stop/Resume state machine stays truthful.
pub(crate) async fn perform_sync(
    state: &std::sync::Arc<crate::server::AppState>,
    auth: &crate::server::AuthContext,
    id: &str,
    kb_id: Option<&str>,
    trigger: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let store = state
        .data_sources
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "data sources disabled".into()))?;
    let record = store
        .get(id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "data source not found".into()))?;
    if let Err(error) = store.set_status(id, "running") {
        tracing::warn!(%error, %id, "Failed to persist the running sync status");
    }
    let options = record.to_options();
    let source_prefix = format!("{}:", record.id);
    let known_versions: HashMap<String, String> = kb_id
        .filter(|kb| !kb.is_empty())
        .map(|kb| {
            state
                .docs
                .list(kb)
                .into_iter()
                .filter(|doc| doc.run != "FAILED")
                .filter_map(|doc| {
                    let source_uri = doc.source_uri.as_deref()?;
                    let key = source_uri.strip_prefix(&source_prefix)?;
                    let version = doc.source_version.clone()?;
                    Some((key.to_string(), version))
                })
                .collect()
        })
        .unwrap_or_default();
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
                let source_keys = docs.iter().map(|doc| doc.id.clone()).collect();
                Ok(crate::connectors::SyncBatch {
                    docs,
                    failures: Vec::new(),
                    skipped: Vec::new(),
                    source_keys,
                    // The legacy GitLab connector does not expose a complete
                    // listing contract, so never prune from it.
                    truncated: true,
                    unchanged_source_keys: Vec::new(),
                    unchanged_files: Vec::new(),
                })
            }
            // Connector registry dispatch (RAGFlow data-source type → Connector).
            kind => {
                let connector = crate::connectors::ConnectorRegistry::create(kind, options)
                    .map_err(|error| (StatusCode::BAD_REQUEST, error.to_string()))?;
                let timeout_seconds = record.timeout_secs.unwrap_or(1_740).clamp(1, 7_200);
                match tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_seconds),
                    connector.fetch_all_with_known(
                        record.max_items,
                        crate::connectors::connector_batch_limit_bytes(),
                        &known_versions,
                    ),
                )
                .await
                {
                    Ok(result) => result.map_err(|error| {
                        (StatusCode::BAD_GATEWAY, format!("sync failed: {error:#}"))
                    }),
                    Err(_) => Err((
                        StatusCode::GATEWAY_TIMEOUT,
                        format!(
                            "sync timed out after {timeout_seconds}s; raise the data source timeout, lower max_items, or narrow the target"
                        ),
                    )),
                }
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
                Some(trigger),
            );
            return Err(error);
        }
    };
    let source_keys = batch.source_keys;
    let truncated_listing = batch.truncated;
    let skipped_unchanged = batch.unchanged_source_keys.len();
    let unchanged_files = batch.unchanged_files;
    let docs = batch.docs;
    let total_docs = docs.len() + skipped_unchanged;
    let mut connector_failures = batch.failures;
    // Files the connector refused before download (unsupported extension,
    // oversized, images disabled). RAGFlow logs these as warnings; keep them
    // in the same visible "unsupported" bucket as binary documents.
    let mut skipped_unsupported: Vec<String> = batch.skipped;

    // Optional ingestion into a knowledge base: ?kb_id=<dataset id>.
    // Each connector doc is written as a text file and routed through the
    // standard upload pipeline (parse -> chunk -> embed -> index).
    let mut ingested = 0_usize;
    let mut unchanged = skipped_unchanged;
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
            let Some(ext) = connector_storage_extension(doc) else {
                let declared = if doc.extension.trim().is_empty() {
                    "no suffix"
                } else {
                    doc.extension.trim()
                };
                skipped_unsupported.push(format!(
                    "{} ({declared}, binary content; convert it to PDF/DOCX/TXT or exclude it)",
                    doc.semantic_identifier
                ));
                continue;
            };
            let file_name = format!("{doc_id}{ext}");
            let path = upload_dir.join(&file_name);
            // Binary sources (PDF/Office/images) must be persisted byte-for-byte;
            // the lossy UTF-8 blob is only a preview/legacy text channel.
            let bytes = doc.bytes();
            let content_hash = crate::server::server_xxh3_hash(bytes);
            let source_parent = connector_source_parent(doc);
            let source_uri = format!("{}:{}", record.id, doc.id);
            let existing_docs = state.docs.list(kb_id);
            let same_source = existing_docs
                .iter()
                .find(|existing| existing.source_uri.as_deref() == Some(source_uri.as_str()));
            let same_location = existing_docs.iter().find(|existing| {
                existing.source_uri.is_none()
                    && existing.name == doc.semantic_identifier
                    && existing.parent_path.as_deref() == source_parent.as_deref()
            });
            if let Some(existing) = same_source.or(same_location) {
                if existing.content_hash == content_hash && existing.run != "FAILED" {
                    unchanged += 1;
                    continue;
                }
                // Content changed (or the old parse failed): replace the old
                // document so the knowledge base does not accumulate (1) copies.
                if let Err(error) = crate::server::delete_document_data(state, existing) {
                    tracing::warn!(
                        "data source: could not replace {}: {error:#}",
                        doc.semantic_identifier
                    );
                    connector_failures.push(format!(
                        "{}: could not replace changed document: {error:#}",
                        doc.semantic_identifier
                    ));
                    skipped.push(doc.semantic_identifier.clone());
                    continue;
                }
            } else {
                // Legacy rows have no parent_path/source_uri; keep the old
                // name+hash check so a re-sync does not duplicate them.
                let legacy_unchanged = existing_docs.iter().any(|existing| {
                    existing.parent_path.is_none()
                        && existing.source_uri.is_none()
                        && existing.name == doc.semantic_identifier
                        && existing.content_hash == content_hash
                        && existing.run != "FAILED"
                });
                if legacy_unchanged {
                    unchanged += 1;
                    continue;
                }
            }
            if std::fs::write(&path, bytes).is_err() {
                skipped.push(doc.semantic_identifier.clone());
                continue;
            }
            let upload = crate::server::PersistedUpload {
                name: doc.semantic_identifier.clone(),
                storage_name: file_name,
                path,
                size: bytes.len(),
                content_hash,
                cleanup_on_drop: false,
            };
            match crate::server::register_document_upload_with_options(
                state.clone(),
                &auth.user_id,
                kb_id,
                doc_id,
                upload,
                None,
                source_parent,
                Some(source_uri),
                doc.metadata.get("source_version").cloned(),
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

    // Remove documents whose remote files disappeared from a complete listing.
    let mut removed = 0_usize;
    if record.sync_deleted_files {
        if let Some(kb_id) = kb_id.filter(|value| !value.is_empty()) {
            if truncated_listing {
                connector_failures.push(
                    "sync deleted files skipped: the connector listing was truncated by max_items or the batch byte limit".into(),
                );
            } else {
                let prefix = format!("{}:", record.id);
                let seen: std::collections::HashSet<String> = source_keys
                    .iter()
                    .map(|key| format!("{prefix}{key}"))
                    .collect();
                let stale: Vec<_> = state
                    .docs
                    .list(kb_id)
                    .into_iter()
                    .filter(|existing| {
                        existing
                            .source_uri
                            .as_deref()
                            .is_some_and(|uri| uri.starts_with(&prefix) && !seen.contains(uri))
                    })
                    .collect();
                for existing in stale {
                    match crate::server::delete_document_data(state, &existing) {
                        Ok(()) => removed += 1,
                        Err(error) => {
                            tracing::warn!(
                                "data source: could not prune {}: {error:#}",
                                existing.name
                            );
                            connector_failures.push(format!(
                                "{}: could not prune deleted source file: {error:#}",
                                existing.name
                            ));
                        }
                    }
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
        .chain(
            unchanged_files
                .iter()
                .take(10_usize.saturating_sub(docs.len().min(10)))
                .map(|file| {
                    serde_json::json!({
                        "id": file.id,
                        "title": file.name,
                        "extension": file.extension,
                        "size_bytes": file.size_bytes,
                        "type": file.metadata.get("type"),
                    })
                }),
        )
        .collect();
    let kb_name = kb_id.and_then(|kb| state.kbs.get(kb).map(|kb| kb.name.clone()));
    let truncated_note = if truncated_listing {
        "; listing truncated by max_items or the batch byte budget (deleted-file pruning skipped)"
    } else {
        ""
    };
    let unsupported_note = if skipped_unsupported.is_empty() {
        String::new()
    } else {
        format!(
            "; skipped {} unsupported file(s): {}",
            skipped_unsupported.len(),
            skipped_unsupported
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        )
    };
    let parse_note = if ingested > 0 {
        "; parsing continues in the background, open the dataset to see per-document results"
    } else {
        ""
    };
    let failed = skipped.len() + connector_failures.len();
    let mut detail = String::new();
    for failure in connector_failures.iter().take(10) {
        detail.push_str(failure);
        detail.push('\n');
    }
    for unsupported in skipped_unsupported.iter().take(10) {
        detail.push_str("unsupported: ");
        detail.push_str(unsupported);
        detail.push('\n');
    }
    let detail = (!detail.trim().is_empty()).then(|| detail.trim().to_string());
    let _ = store.append_sync_log(
        id,
        &record,
        "DONE",
        "sync",
        &format!(
            "Synced {} docs (ingested {ingested}, unchanged {unchanged}, skipped {}, unsupported {}, removed {removed}){truncated_note}{unsupported_note}{parse_note}",
            total_docs,
            skipped.len(),
            skipped_unsupported.len(),
        ),
        Some(ingested as u64),
        Some(total_docs as u64),
        Some(removed as u64),
        Some(failed as u64),
        detail,
        kb_id,
        kb_name.as_deref(),
        Some(trigger),
    );
    let _ = store.set_status(id, "schedule");
    Ok(serde_json::json!({
        "code": 0,
        "data": {
            "count": total_docs,
            "ingested": ingested,
            "unchanged": unchanged,
            "removed": removed,
            "skipped": skipped,
            "unsupported": skipped_unsupported,
            "failures": connector_failures,
            "truncated": truncated_listing,
            "preview": preview
        }
    }))
}

/// Link a data source to a dataset and synchronise it immediately.
///
/// Mirrors upstream `Connector2KbService.link_connectors(..., run_immediately=True)`:
/// linking a source into a dataset is what triggers the first sync. The request
/// that saved the dataset must not wait for the connector (nor be failed by it),
/// so the sync runs detached and only logs its failure. The existence check is
/// repeated here because the link may have been removed between the PUT and this
/// task; a missing store or source is a quiet no-op.
pub(crate) fn schedule_link_sync(
    state: std::sync::Arc<crate::server::AppState>,
    auth: crate::server::AuthContext,
    connector_id: String,
    kb_id: String,
) {
    tokio::spawn(async move {
        let Some(store) = state.data_sources.as_ref() else {
            return;
        };
        if store.get(&connector_id).is_none() {
            return;
        }
        if let Err((status, message)) =
            perform_sync(&state, &auth, &connector_id, Some(&kb_id), "link").await
        {
            tracing::warn!(
                %connector_id,
                %kb_id,
                status = status.as_u16(),
                %message,
                "linked data source sync failed"
            );
        }
    });
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
            "connect",
            &format!("connect failed: {error:#}"),
            None,
            None,
            None,
            Some(1),
            Some(format!("connect failed: {error:#}")),
            None,
            None,
            Some("connect"),
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
        "connect",
        "Connection verified",
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some("connect"),
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

/// Canonicalises a data source kind.
///
/// The UI renders one card per entry of its own schema catalogue, whose keys are upper-cased
/// (`SALESFORCE`, `GOOGLE_CLOUD_STORAGE`, `OCI_STORAGE`), while the connector registry and this API speak
/// lower-case kinds (`salesforce`, `gcs`, `oracle`). Every card on the data-source page therefore posted a
/// type the API rejected, so the whole add flow failed. Normalising here — one place, used for both the
/// validation and the stored record — fixes the page without a second naming convention on the client.
pub(crate) fn normalize_kind(raw: &str) -> String {
    let lowered = raw.trim().to_ascii_lowercase();
    match lowered.as_str() {
        // The UI's display names for services the registry names differently.
        "google_cloud_storage" => "gcs".to_string(),
        "oci_storage" => "oracle".to_string(),
        _ => lowered,
    }
}

/// Fields a connector needs but does **not** check in its constructor.
///
/// Measured, not guessed: of the 41 registry kinds, 33 reject an empty configuration when they are built and
/// report the field themselves; these eight accept one and would only fail later, at sync time — the silent
/// failure this API must prevent. Each entry mirrors the connector's own guard:
/// `s3`/`gcs`/`oracle`/`r2` need an endpoint, credentials and a bucket; `azure_blob` an endpoint and a SAS
/// token; `webdav` a server URL and credentials; `confluence` an instance URL, a space key and a token;
/// `feishu` a base URL and `app_id:app_secret`.
fn deferred_validation_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "s3" | "gcs" | "oracle" | "r2" => &["url", "token", "target"],
        "azure_blob" | "webdav" | "feishu" => &["url", "token"],
        "confluence" => &["url", "target", "token"],
        _ => &[],
    }
}

/// Whether the background scheduler should refresh this source now. A source
/// with no sync log falls back to its creation time; disabled (0/None) or
/// unparseable timestamps never trigger an automatic sync.
pub(crate) fn refresh_due(record: &DataSourceRecord, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Some(frequency) = record.refresh_freq.filter(|value| *value > 0) else {
        return false;
    };
    let last = record
        .sync_logs
        .first()
        .and_then(|log| log.time_started.as_deref())
        .and_then(|started| chrono::DateTime::parse_from_rfc3339(started).ok())
        .or_else(|| chrono::DateTime::parse_from_rfc3339(&record.created_at).ok())
        .map(|time| time.with_timezone(&chrono::Utc));
    match last {
        Some(time) => {
            now.signed_duration_since(time) >= chrono::Duration::minutes(frequency as i64)
        }
        None => false,
    }
}

pub(crate) fn is_supported_kind(kind: &str) -> bool {
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

/// Extension used for a connector document. Remote listings sometimes carry a
/// display name without a suffix; fall back to the file magic before defaulting
/// text documents to Markdown, otherwise a PDF would be ingested as text.
fn connector_doc_extension(doc: &crate::data_source::ConnectorDoc) -> String {
    if !doc.extension.is_empty() {
        return doc.extension.clone();
    }
    if let Some(extension) = sniff_binary_extension(doc.bytes()) {
        return extension.into();
    }
    ".md".into()
}

/// Binary signatures we can recognise without trusting the remote suffix.
fn sniff_binary_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"%PDF-") {
        return Some(".pdf");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(".png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(".jpg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(".gif");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(".webp");
    }
    if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        return Some(".tiff");
    }
    if bytes.starts_with(b"BM") {
        return Some(".bmp");
    }
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WAVE" {
        return Some(".wav");
    }
    if bytes.starts_with(b"OggS") {
        return Some(".ogg");
    }
    if bytes.starts_with(b"fLaC") {
        return Some(".flac");
    }
    if bytes.starts_with(b"ID3") {
        return Some(".mp3");
    }
    None
}

/// True when a sample looks like UTF-8 text rather than an opaque binary.
/// NUL bytes are the strongest binary signal; a truncated multi-byte character
/// at the sample boundary is tolerated.
fn looks_like_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }
    // Container/executable signatures that do not necessarily contain a NUL
    // in their first bytes. Treating an OLE `.doc`, ZIP/Office archive or
    // executable as UTF-8 text would hand the parser a corrupted `.md`.
    const BINARY_MAGIC: &[&[u8]] = &[
        b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1", // OLE2 (.doc/.xls/.ppt)
        b"PK\x03\x04",                       // ZIP (.docx/.xlsx/.pptx/.epub)
        b"PK\x05\x06",                       // empty ZIP
        b"\x1F\x8B",                         // gzip
        b"7z\xBC\xAF\x27\x1C",               // 7z
        b"Rar!\x1A\x07",                     // RAR
        b"\x7FELF",                          // ELF executable
        b"MZ",                               // PE executable
        b"\xCA\xFE\xBA\xBE",                 // Mach-O / Java class
    ];
    if BINARY_MAGIC.iter().any(|magic| bytes.starts_with(magic)) {
        return false;
    }
    let sample = &bytes[..bytes.len().min(64 * 1024)];
    if sample.contains(&0) {
        return false;
    }
    let lossy = String::from_utf8_lossy(sample);
    let replacements = lossy.chars().filter(|ch| *ch == '\u{fffd}').count();
    replacements * 100 <= sample.len().max(1)
}

/// Extension used when writing the connector document for the parser. A known
/// suffix wins; otherwise magic, then text sniffing decide. `None` means the
/// file cannot be parsed and should be reported instead of ingested as a
/// bogus `.md` document (for example an extensionless NIfTI `.sample` file).
fn connector_storage_extension(doc: &crate::data_source::ConnectorDoc) -> Option<String> {
    let declared = doc.extension.trim();
    if !declared.is_empty() {
        let probe = format!("probe{declared}");
        if crate::parser::mime_from_extension(&probe).is_some() {
            return Some(declared.to_string());
        }
    }
    if let Some(extension) = sniff_binary_extension(doc.bytes()) {
        return Some(extension.to_string());
    }
    if looks_like_text(doc.bytes()) {
        return Some(".md".into());
    }
    None
}

/// Source folder used for document identity and UI location. Connector docs
/// carry a path/key/blob in metadata; using its parent folder lets a changed
/// file replace the previous document without deleting a same-named file from
/// another remote folder.
fn connector_source_parent(doc: &crate::data_source::ConnectorDoc) -> Option<String> {
    let raw = doc
        .metadata
        .get("path")
        .or_else(|| doc.metadata.get("key"))
        .or_else(|| doc.metadata.get("blob"))?
        .trim()
        .trim_matches('/');
    let (parent, _) = raw.rsplit_once('/')?;
    (!parent.is_empty()).then(|| parent.to_string())
}

// ---- tests ----

#[cfg(test)]
mod tests {
    use super::*;

    fn refresh_record(created_at: &str, refresh_freq: Option<u64>) -> DataSourceRecord {
        DataSourceRecord {
            id: "refresh-test".into(),
            name: "refresh".into(),
            kind: "webdav".into(),
            url: "http://127.0.0.1".into(),
            token: String::new(),
            target: String::new(),
            include_issues: false,
            include_merge_requests: false,
            max_items: 10,
            created_at: created_at.into(),
            connected: true,
            sync_deleted_files: false,
            prune_freq: None,
            refresh_freq,
            timeout_secs: None,
            status: "schedule".into(),
            extra: HashMap::new(),
            sync_logs: Vec::new(),
        }
    }

    #[test]
    fn legacy_fifty_file_cap_is_migrated_to_effectively_unlimited() {
        assert_eq!(default_max_items(), DEFAULT_DATA_SOURCE_MAX_ITEMS);
        let dir = std::env::temp_dir().join(format!("rayrag-ds-cap-{}", uuid::Uuid::new_v4()));
        let store = DataSourceStore::new(dir.to_str().unwrap()).unwrap();
        let mut record = refresh_record("2026-11-20T23:00:00Z", Some(5));
        record.id = "legacy-cap".into();
        record.max_items = 50;
        store.insert(record.clone()).unwrap();
        let reloaded = DataSourceStore::new(dir.to_str().unwrap()).unwrap();
        assert_eq!(
            reloaded.get("legacy-cap").unwrap().max_items,
            DEFAULT_DATA_SOURCE_MAX_ITEMS
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn refresh_schedule_honours_the_interval_and_disabled_values() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-21T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        assert!(refresh_due(
            &refresh_record("2026-11-20T23:00:00Z", Some(5)),
            now
        ));
        assert!(!refresh_due(
            &refresh_record("2026-11-20T23:58:00Z", Some(5)),
            now
        ));
        assert!(!refresh_due(
            &refresh_record("2026-11-20T23:00:00Z", None),
            now
        ));
        assert!(!refresh_due(
            &refresh_record("2026-11-20T23:00:00Z", Some(0)),
            now
        ));
        let mut record = refresh_record("2026-11-20T20:00:00Z", Some(5));
        record.sync_logs.push(DataSourceSyncLog {
            status: "DONE".into(),
            time_started: Some("2026-11-20T23:59:00Z".into()),
            ..Default::default()
        });
        assert!(
            !refresh_due(&record, now),
            "a recent log delays the refresh"
        );
    }

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
        record
            .extra
            .insert("config.max_file_size_mb".into(), "5".into());
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
        assert!(
            !record.extra.contains_key("config.max_file_size_mb"),
            "a field omitted from the new config must not survive the update"
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

    /// Every two-field credential the schema collects has to become the one string its connector expects, or a
    /// schema-driven save stores an empty token and the source silently never connects.
    #[test]
    fn config_mapping_joins_each_username_password_pair() {
        let mapped = |config: serde_json::Value| {
            let mut record = DataSourceRecord {
                id: "t".into(),
                name: "t".into(),
                kind: "webdav".into(),
                url: String::new(),
                token: String::new(),
                target: String::new(),
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
                extra: Default::default(),
                sync_logs: Vec::new(),
            };
            apply_config_mapping(&mut record, &config);
            record.token
        };
        for (user_key, secret_key, user, secret, expected) in [
            ("username", "password", "alice", "secret", "alice:secret"),
            (
                "aws_access_key_id",
                "aws_secret_access_key",
                "AKIA",
                "shh",
                "AKIA:shh",
            ),
            (
                "confluence_username",
                "confluence_access_token",
                "me@example.com",
                "tok",
                "me@example.com:tok",
            ),
            ("account_name", "account_key", "acct", "key", "acct:key"),
        ] {
            let config =
                serde_json::json!({ "credentials": { user_key: user, secret_key: secret } });
            assert_eq!(
                mapped(config),
                expected,
                "{user_key}/{secret_key} must be joined as {expected}"
            );
        }
        // Half a pair is not a credential: the single-key list gets its chance instead of a fabricated "user:".
        let half =
            serde_json::json!({ "credentials": { "username": "alice", "token": "explicit" } });
        assert_eq!(mapped(half), "explicit");
        let neither = serde_json::json!({ "credentials": { "username": "alice" } });
        assert_eq!(mapped(neither), "");
    }

    #[test]
    fn flat_create_names_the_missing_field_for_kinds_that_need_one() {
        assert_eq!(
            deferred_validation_fields("s3"),
            &["url", "token", "target"]
        );
        assert_eq!(
            deferred_validation_fields("confluence"),
            &["url", "target", "token"]
        );
        // Kinds that validate in their constructor need no entry here.
        assert!(deferred_validation_fields("rss").is_empty());
        assert!(deferred_validation_fields("notion").is_empty());
    }

    /// Pins the measurement the API's split relies on: exactly these kinds accept an empty configuration when
    /// built. If a connector loses or gains constructor validation this fails, and the table above must be
    /// revisited — the alternative is a record that only fails at sync time.
    #[test]
    fn only_the_listed_kinds_skip_constructor_validation() {
        let mut lax: Vec<&str> = Vec::new();
        let mut unbuildable: Vec<&str> = Vec::new();
        for info in crate::connectors::ConnectorRegistry::kinds() {
            let options = crate::data_source::SourceOptions {
                url: String::new(),
                token: String::new(),
                target: String::new(),
                include_issues: false,
                include_merge_requests: false,
                max_items: 50,
                extra: Default::default(),
            };
            match crate::connectors::ConnectorRegistry::create(info.kind, options) {
                Ok(_) => lax.push(info.kind),
                // "unsupported connector kind" means there is no connector, not that the configuration is
                // incomplete; counting it as strict made the earlier measurement wrong.
                Err(error) if error.to_string().contains("unsupported connector kind") => {
                    unbuildable.push(info.kind)
                }
                Err(_) => {}
            }
        }
        lax.sort_unstable();
        unbuildable.sort_unstable();
        assert_eq!(
            unbuildable,
            vec![
                "gitlab",
                "gmail",
                "google_drive",
                "notion",
                "tencent_docs",
                "wechat_docs"
            ],
            "the set of advertised kinds without a connector changed; the create path refuses these on purpose, \
             so the count and the user-facing message must be revisited"
        );
        assert_eq!(
            lax,
            vec![
                "azure_blob",
                "confluence",
                "feishu",
                "gcs",
                "oracle",
                "r2",
                "s3",
                "webdav"
            ],
            "the set of connectors that accept an empty configuration changed: \
             deferred_validation_fields must be updated to match"
        );
    }

    #[test]
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
                None,
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
                None,
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
        assert!(json.get("trigger").is_none());
        assert!(json.get("time_started").is_some());
        assert!(json.get("error_count").is_some());
        std::fs::remove_dir_all(dir).ok();
    }

    fn connector_doc_for_extension(
        raw: &[u8],
        extension: &str,
    ) -> crate::data_source::ConnectorDoc {
        crate::data_source::ConnectorDoc {
            id: "webdav:/x".into(),
            blob: String::from_utf8_lossy(raw).to_string(),
            raw_bytes: Some(raw.to_vec()),
            source: crate::data_source::DocumentSource::WebDav,
            semantic_identifier: "x".into(),
            extension: extension.into(),
            doc_updated_at: String::new(),
            size_bytes: raw.len(),
            metadata: Default::default(),
        }
    }

    #[test]
    fn connector_source_parent_uses_the_remote_folder_not_the_file() {
        let mut doc = crate::data_source::ConnectorDoc {
            id: "webdav:/files/sub/b.pdf".into(),
            blob: String::new(),
            raw_bytes: Some(Vec::new()),
            source: crate::data_source::DocumentSource::WebDav,
            semantic_identifier: "b.pdf".into(),
            extension: ".pdf".into(),
            doc_updated_at: String::new(),
            size_bytes: 0,
            metadata: Default::default(),
        };
        doc.metadata
            .insert("path".into(), "/files/sub/b.pdf".into());
        assert_eq!(connector_source_parent(&doc).as_deref(), Some("files/sub"));
        doc.metadata.insert("path".into(), "/b.pdf".into());
        assert_eq!(connector_source_parent(&doc), None);
        doc.metadata.remove("path");
        doc.metadata.insert("key".into(), "bucket/a/b.txt".into());
        assert_eq!(connector_source_parent(&doc).as_deref(), Some("bucket/a"));
    }

    #[test]
    fn connector_doc_extension_sniffs_binary_magic_and_keeps_explicit_suffix() {
        assert_eq!(
            connector_doc_extension(&connector_doc_for_extension(b"%PDF-1.7\n...", "")),
            ".pdf"
        );
        assert_eq!(
            connector_doc_extension(&connector_doc_for_extension(b"\x89PNG\r\n\x1a\n....", "")),
            ".png"
        );
        assert_eq!(
            connector_doc_extension(&connector_doc_for_extension(b"plain text", "")),
            ".md"
        );
        assert_eq!(
            connector_doc_extension(&connector_doc_for_extension(b"%PDF-1.7", ".data")),
            ".data",
            "an explicit remote extension wins"
        );
    }

    #[test]
    fn connector_storage_extension_reports_unparseable_binary_instead_of_faking_markdown() {
        assert_eq!(
            connector_storage_extension(&connector_doc_for_extension(b"%PDF-1.7", "")),
            Some(".pdf".into())
        );
        assert_eq!(
            connector_storage_extension(&connector_doc_for_extension(b"hello world", "")),
            Some(".md".into())
        );
        assert_eq!(
            connector_storage_extension(&connector_doc_for_extension(
                b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1legacy",
                ".doc"
            )),
            None,
            "a legacy .doc must not be stored as text"
        );
        assert_eq!(
            connector_storage_extension(&connector_doc_for_extension(&[0, 1, 2, 3, 4], "")),
            None,
            "opaque binary without a suffix is not a Markdown document"
        );
        assert_eq!(
            connector_storage_extension(&connector_doc_for_extension(b"PK\x03\x04zip", "")),
            None,
            "a ZIP container without a suffix cannot be guessed safely"
        );
    }

    #[test]
    fn looks_like_text_rejects_known_binary_containers() {
        assert!(looks_like_text(b"plain UTF-8 text\n"));
        assert!(!looks_like_text(b"\x7FELF\x02\x01\x01"));
        assert!(!looks_like_text(b"PK\x03\x04"));
        assert!(!looks_like_text(&[0x61, 0x00, 0x62]));
    }
}
