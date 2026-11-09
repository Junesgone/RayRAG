//! RAGFlow ingestion (data-pipeline) logs — `api/apps/restful_apis/dataset_api.py`
//! (`/datasets/<dataset_id>/ingestions`, `/ingestions/<log_id>`,
//! `/ingestions/summary`) and the `PipelineOperationLog` rows behind them.
//!
//! `pages/dataflow-result/index.tsx` reads a log through
//! `GET /datasets/{id}/ingestions/{log_id}` and rebuilds the whole page from the
//! returned `dsl`: `dsl.graph.nodes` becomes the timeline, `dsl.path` the
//! execution order and `components[key].obj.params.outputs` the right-hand
//! parser panel (`output_format` selects which output key is displayed).
//!
//! RayRAG's parsing is a fixed sequence rather than a canvas, so the DSL is
//! synthesized from the stages the run actually executed
//! (`crate::pipeline::PipelineReport`) with their measured elapsed time and
//! produced outputs. The wire contract, the field set and the error strings are
//! upstream's; the component bodies are RayRAG's own report.

use crate::pipeline::PipelineReport;
use crate::server::{AppState, AuthContext};
use anyhow::Context;
use axum::{
    Json,
    extract::{Extension, Path, Query, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

/// One `PipelineOperationLog` row in the upstream field set
/// (`PipelineOperationLogService.get_file_logs_fields`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestionLog {
    pub id: String,
    pub tenant_id: String,
    pub kb_id: String,
    #[serde(default)]
    pub document_id: String,
    #[serde(default)]
    pub document_name: String,
    #[serde(default)]
    pub document_suffix: String,
    #[serde(default)]
    pub document_type: String,
    #[serde(default)]
    pub parser_id: String,
    #[serde(default)]
    pub pipeline_id: String,
    #[serde(default)]
    pub pipeline_title: String,
    #[serde(default)]
    pub source_from: String,
    /// Canvas snapshot the dataflow-result page renders.
    #[serde(default)]
    pub dsl: serde_json::Value,
    pub progress: f32,
    #[serde(default)]
    pub progress_msg: String,
    /// `unstart` | `running` | `done` | `failed` | `cancelled`.
    #[serde(default)]
    pub operation_status: String,
    #[serde(default)]
    pub status: String,
    #[serde(default = "default_task_type")]
    pub task_type: String,
    /// Upstream avatar column: the dataset/document thumbnail carried by the log row.
    #[serde(default)]
    pub avatar: String,
    #[serde(default = "default_log_type")]
    pub log_type: String,
    #[serde(default)]
    pub process_begin_at: u64,
    #[serde(default)]
    pub process_duration: f64,
    pub create_time: u64,
    #[serde(default)]
    pub create_date: String,
    pub update_time: u64,
    #[serde(default)]
    pub update_date: String,
}

fn default_task_type() -> String {
    "dataflow".into()
}

fn default_log_type() -> String {
    "file".into()
}

#[derive(Debug, Serialize, Deserialize)]
struct IngestionSnapshot {
    ingestion_logs: Vec<IngestionLog>,
}

pub struct IngestionLogStore {
    logs: RwLock<Vec<IngestionLog>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl IngestionLogStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = PathBuf::from(path);
        crate::persistence::restore_if_missing(&path)?;
        let logs = if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("Failed to read ingestion logs '{}':", path.display()))?;
            serde_json::from_slice::<IngestionSnapshot>(&bytes)
                .with_context(|| format!("Failed to parse ingestion logs '{}':", path.display()))?
                .ingestion_logs
        } else {
            Vec::new()
        };
        let store = Self {
            logs: RwLock::new(logs),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            logs: RwLock::new(Vec::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    fn persist(&self, logs: &[IngestionLog]) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(
            path,
            &serde_json::to_vec_pretty(&IngestionSnapshot {
                ingestion_logs: logs.to_vec(),
            })?,
        )
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let logs = self.logs.read().unwrap().clone();
        self.persist(&logs)
    }

    pub fn insert(&self, log: IngestionLog) -> anyhow::Result<IngestionLog> {
        let mut guard = self.logs.write().unwrap();
        guard.push(log.clone());
        let snapshot = guard.clone();
        drop(guard);
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(&snapshot)?;
        Ok(log)
    }

    pub fn get(&self, kb_id: &str, log_id: &str) -> Option<IngestionLog> {
        self.logs
            .read()
            .unwrap()
            .iter()
            .find(|log| log.kb_id == kb_id && log.id == log_id)
            .cloned()
    }

    /// Look one log up by id alone (`rerun_agent` resolves the document from the
    /// log before it knows the dataset).
    pub fn find(&self, log_id: &str) -> Option<IngestionLog> {
        self.logs
            .read()
            .unwrap()
            .iter()
            .find(|log| log.id == log_id)
            .cloned()
    }

    pub fn list_for_kb(&self, kb_id: &str) -> Vec<IngestionLog> {
        self.logs
            .read()
            .unwrap()
            .iter()
            .filter(|log| log.kb_id == kb_id)
            .cloned()
            .collect()
    }

    /// Replace the stored DSL of one log (the rerun endpoint rewrites it with
    /// the caller's edited canvas plus the requested `path`).
    pub fn update_dsl(&self, log_id: &str, dsl: serde_json::Value) -> anyhow::Result<bool> {
        let mut guard = self.logs.write().unwrap();
        let mut updated = false;
        for log in guard.iter_mut() {
            if log.id == log_id {
                log.dsl = dsl.clone();
                log.update_time = now_ms();
                log.update_date = now_date();
                updated = true;
            }
        }
        let snapshot = guard.clone();
        drop(guard);
        if updated {
            let _save_guard = self.save_lock.lock().unwrap();
            self.persist(&snapshot)?;
        }
        Ok(updated)
    }
}

impl Default for IngestionLogStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

fn now_date() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Turn one parse run into the DSL the dataflow-result page consumes:
/// `graph.nodes` (timeline, `data.name` is the node title), `graph.edges`,
/// `components[key] = {obj: {component_name, params: {outputs}}, upstream,
/// downstream}` and `path` (execution order).
pub fn build_parse_dsl(report: &PipelineReport, file_name: &str) -> serde_json::Value {
    let mut nodes: Vec<serde_json::Value> = Vec::new();
    let mut edges: Vec<serde_json::Value> = Vec::new();
    let mut components = serde_json::Map::new();
    let mut path: Vec<serde_json::Value> = Vec::new();
    let mut previous: Option<&str> = None;
    for stage in &report.stages {
        nodes.push(serde_json::json!({
            "id": stage.id,
            "type": format!("{}Node", stage.component),
            "data": {"name": stage.title},
            "position": {"x": 0, "y": 0},
        }));
        if let Some(source) = previous {
            edges.push(serde_json::json!({"source": source, "target": stage.id}));
        }
        let downstream: Vec<serde_json::Value> = report
            .stages
            .iter()
            .skip_while(|candidate| candidate.id != stage.id)
            .skip(1)
            .take(1)
            .map(|candidate| serde_json::Value::String(candidate.id.to_string()))
            .collect();
        components.insert(
            stage.id.to_string(),
            serde_json::json!({
                "obj": {
                    "component_name": stage.component,
                    "params": {
                        "outputs": stage.outputs,
                        "_elapsed_time": {
                            "type": "number",
                            "value": stage.elapsed_seconds
                        },
                    },
                },
                "upstream": previous.map(|id| vec![id]).unwrap_or_default(),
                "downstream": downstream,
            }),
        );
        path.push(serde_json::Value::String(stage.id.to_string()));
        previous = Some(stage.id);
    }
    serde_json::json!({
        "path": path,
        "graph": {"nodes": nodes, "edges": edges},
        "components": components,
        "globals": {"sys.filename": file_name},
    })
}

/// Record one finished parse run. Called by the document-parse task with the
/// report the pipeline produced.
#[allow(clippy::too_many_arguments)]
pub fn record_parse_run(
    state: &AppState,
    tenant_id: &str,
    kb_id: &str,
    document_id: &str,
    document_name: &str,
    parser_id: &str,
    report: &PipelineReport,
    operation_status: &str,
    progress_msg: &str,
) -> anyhow::Result<IngestionLog> {
    let now = now_ms();
    let suffix = std::path::Path::new(document_name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let log = IngestionLog {
        id: uuid::Uuid::new_v4().to_string(),
        tenant_id: tenant_id.to_string(),
        kb_id: kb_id.to_string(),
        document_id: document_id.to_string(),
        document_name: document_name.to_string(),
        document_suffix: suffix.clone(),
        document_type: suffix,
        parser_id: parser_id.to_string(),
        pipeline_id: String::new(),
        pipeline_title: String::new(),
        source_from: "upload".into(),
        dsl: build_parse_dsl(report, document_name),
        progress: 1.0,
        progress_msg: progress_msg.to_string(),
        operation_status: operation_status.to_string(),
        status: "1".into(),
        task_type: default_task_type(),
        avatar: String::new(),
        log_type: default_log_type(),
        process_begin_at: now,
        process_duration: report.total_seconds(),
        create_time: now,
        create_date: now_date(),
        update_time: now,
        update_date: now_date(),
    };
    state.ingestion_logs.insert(log)
}

/// Query string of `list_ingestion_logs`. Upstream reads `page`/`page_size`
/// with `0` defaults, `orderby=create_time`, `desc=true`,
/// `log_type=dataset`, repeated `operation_status` values and the two date
/// bounds.
#[derive(Debug, Default, Deserialize)]
pub struct IngestionLogQuery {
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub page_size: Option<String>,
    #[serde(default)]
    pub orderby: Option<String>,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub operation_status: Option<String>,
    #[serde(default)]
    pub create_date_from: Option<String>,
    #[serde(default)]
    pub create_date_to: Option<String>,
    #[serde(default)]
    pub log_type: Option<String>,
    #[serde(default)]
    pub keywords: Option<String>,
}

/// Upstream PipelineOperationLog attributes accepted as an orderby value.
const ORDERBY_FIELDS: [&str; 25] = [
    "id",
    "document_id",
    "tenant_id",
    "kb_id",
    "pipeline_id",
    "pipeline_title",
    "parser_id",
    "document_name",
    "document_suffix",
    "document_type",
    "source_from",
    "progress",
    "progress_msg",
    "process_begin_at",
    "process_duration",
    "dsl",
    "task_type",
    "operation_status",
    "avatar",
    "status",
    "create_time",
    "create_date",
    "update_time",
    "update_date",
    "log_type",
];

/// Upstream validate_rest_api_page: unparseable or < 1 silently falls back to page 1.
fn rest_api_page(raw: Option<&str>) -> usize {
    raw.and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|value| *value >= 1)
        .map(|value| value as usize)
        .unwrap_or(1)
}

/// Upstream validate_rest_api_page_size: unparseable or < 1 falls back to 30 and a
/// value above the public maximum raises the argument error the route reports verbatim.
fn rest_api_page_size(raw: Option<&str>) -> Result<usize, String> {
    match raw.and_then(|value| value.trim().parse::<i64>().ok()) {
        None => Ok(30),
        Some(value) if value < 1 => Ok(30),
        Some(value) if value > 100 => Err("page_size must be less than or equal to 100".into()),
        Some(value) => Ok(value as usize),
    }
}

/// Upstream `get_data_error_result(message=…)` — HTTP 200 with `RetCode.DATA_ERROR`
/// (business code 102) and no data key.
fn data_error(message: impl AsRef<str>) -> Response {
    let message = message.as_ref();
    Json(serde_json::json!({ "code": 102, "message": message })).into_response()
}

/// `get_error_argument_result`: HTTP 200 + code 101.
fn argument_error(message: impl AsRef<str>) -> Response {
    let message = message.as_ref();
    Json(serde_json::json!({ "code": 101, "message": message })).into_response()
}

/// `get_result`: `{code:0, data}`, no message key on success.
fn ok(data: serde_json::Value) -> Response {
    Json(serde_json::json!({ "code": 0, "data": data })).into_response()
}

fn dataset_log_json(log: &IngestionLog) -> serde_json::Value {
    serde_json::json!({
        "id": log.id,
        "tenant_id": log.tenant_id,
        "kb_id": log.kb_id,
        "progress": log.progress,
        "progress_msg": log.progress_msg,
        "process_begin_at": log.process_begin_at,
        "process_duration": log.process_duration,
        "task_type": log.task_type,
        "operation_status": log.operation_status,
        "avatar": log.avatar,
        "status": log.status,
        "create_time": log.create_time,
        "create_date": log.create_date,
        "update_time": log.update_time,
        "update_date": log.update_date,
    })
}

fn file_log_json(log: &IngestionLog) -> serde_json::Value {
    serde_json::json!({
        "id": log.id,
        "document_id": log.document_id,
        "tenant_id": log.tenant_id,
        "kb_id": log.kb_id,
        "pipeline_id": log.pipeline_id,
        "pipeline_title": log.pipeline_title,
        "parser_id": log.parser_id,
        "document_name": log.document_name,
        "document_suffix": log.document_suffix,
        "document_type": log.document_type,
        "source_from": log.source_from,
        "progress": log.progress,
        "progress_msg": log.progress_msg,
        "process_begin_at": log.process_begin_at,
        "process_duration": log.process_duration,
        "dsl": log.dsl,
        "task_type": log.task_type,
        "operation_status": log.operation_status,
        "avatar": log.avatar,
        "status": log.status,
        "create_time": log.create_time,
        "create_date": log.create_date,
        "update_time": log.update_time,
        "update_date": log.update_date,
    })
}

fn kb_accessible(state: &AppState, auth: &AuthContext, kb_id: &str) -> bool {
    state
        .kbs
        .can_read(kb_id, &auth.user_id, auth.is_admin, |tenant_id, user_id| {
            state.tenants.is_member(tenant_id, user_id)
        })
}

/// `GET /api/v1/datasets/{dataset_id}/ingestions` — `{total, logs}`.
pub async fn list_ingestion_logs(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
    Query(query): Query<IngestionLogQuery>,
) -> Response {
    if dataset_id.is_empty() {
        return data_error("Lack of \"Dataset ID\"");
    }
    // Upstream list_ingestion_logs / get_ingestion_log answer the bare string
    // "no authorization" here (the summary route uses the user-facing sentence).
    if !kb_accessible(&state, &auth, &dataset_id) {
        return data_error("no authorization");
    }
    let log_type = query
        .log_type
        .clone()
        .unwrap_or_else(|| "dataset".to_string());
    if log_type != "dataset" && log_type != "file" {
        return data_error("Invalid \"log_type\", expected \"dataset\" or \"file\"");
    }
    let page = rest_api_page(query.page.as_deref());
    let page_size = match rest_api_page_size(query.page_size.as_deref()) {
        Ok(size) => size,
        Err(message) => return argument_error(message),
    };
    let desc = query
        .desc
        .as_deref()
        .is_none_or(|value| !value.eq_ignore_ascii_case("false"));
    let orderby = query
        .orderby
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "create_time".to_string());
    // PipelineOperationLogService.getter_by raises an AttributeError for an unknown
    // attribute, which the route's generic handler turns into "Internal server error".
    if !ORDERBY_FIELDS.contains(&orderby.as_str()) {
        return data_error("Internal server error");
    }
    let statuses: Vec<String> = query
        .operation_status
        .as_deref()
        .map(|value| {
            value
                .split(',')
                .map(|item| item.trim().to_lowercase())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let keywords = query
        .keywords
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase);
    let mut logs: Vec<IngestionLog> = state
        .ingestion_logs
        .list_for_kb(&dataset_id)
        .into_iter()
        .filter(|log| log.log_type == log_type)
        .filter(|log| statuses.is_empty() || statuses.contains(&log.operation_status))
        .filter(|log| {
            keywords
                .as_ref()
                .is_none_or(|keywords| log.document_name.to_lowercase().contains(keywords))
        })
        .filter(|log| {
            query
                .create_date_from
                .as_deref()
                .filter(|value| !value.is_empty())
                .is_none_or(|from| log.create_date.as_str() >= from)
        })
        .filter(|log| {
            query
                .create_date_to
                .as_deref()
                .filter(|value| !value.is_empty())
                .is_none_or(|to| log.create_date.as_str() <= to)
        })
        .collect();
    logs.sort_by(|left, right| {
        let ordering = match orderby.as_str() {
            "update_time" => left.update_time.cmp(&right.update_time),
            "process_begin_at" => left.process_begin_at.cmp(&right.process_begin_at),
            "process_duration" => left
                .process_duration
                .partial_cmp(&right.process_duration)
                .unwrap_or(std::cmp::Ordering::Equal),
            "progress" => left
                .progress
                .partial_cmp(&right.progress)
                .unwrap_or(std::cmp::Ordering::Equal),
            "document_name" => left.document_name.cmp(&right.document_name),
            "document_suffix" => left.document_suffix.cmp(&right.document_suffix),
            "document_type" => left.document_type.cmp(&right.document_type),
            "parser_id" => left.parser_id.cmp(&right.parser_id),
            "pipeline_id" => left.pipeline_id.cmp(&right.pipeline_id),
            "pipeline_title" => left.pipeline_title.cmp(&right.pipeline_title),
            "source_from" => left.source_from.cmp(&right.source_from),
            "task_type" => left.task_type.cmp(&right.task_type),
            "operation_status" => left.operation_status.cmp(&right.operation_status),
            "status" => left.status.cmp(&right.status),
            "progress_msg" => left.progress_msg.cmp(&right.progress_msg),
            "create_date" => left.create_date.cmp(&right.create_date),
            "update_date" => left.update_date.cmp(&right.update_date),
            "tenant_id" => left.tenant_id.cmp(&right.tenant_id),
            "kb_id" => left.kb_id.cmp(&right.kb_id),
            "document_id" => left.document_id.cmp(&right.document_id),
            "avatar" => left.avatar.cmp(&right.avatar),
            _ => left.create_time.cmp(&right.create_time),
        };
        if desc { ordering.reverse() } else { ordering }
    });
    let total = logs.len();
    let start = page.saturating_sub(1) * page_size;
    let rows: Vec<serde_json::Value> = logs
        .into_iter()
        .skip(start)
        .take(page_size)
        .map(|log| {
            if log_type == "dataset" {
                dataset_log_json(&log)
            } else {
                file_log_json(&log)
            }
        })
        .collect();
    ok(serde_json::json!({ "total": total, "logs": rows }))
}

/// `GET /api/v1/datasets/{dataset_id}/ingestions/{log_id}` — the full file-level field
/// set (a superset of the dataset-level one), including the `dsl` the dataflow-result
/// page renders.
pub async fn get_ingestion_log(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((dataset_id, log_id)): Path<(String, String)>,
) -> Response {
    if dataset_id.is_empty() {
        return data_error("Lack of \"Dataset ID\"");
    }
    if !kb_accessible(&state, &auth, &dataset_id) {
        return data_error("no authorization");
    }
    match state.ingestion_logs.get(&dataset_id, &log_id) {
        Some(log) => {
            let mut row = file_log_json(&log);
            row["dsl"] = log.dsl.clone();
            ok(row)
        }
        None => data_error("Log not found"),
    }
}

/// `GET /api/v1/datasets/{dataset_id}/ingestions/summary` —
/// `{doc_num, chunk_num, token_num, status}` where `status` counts the
/// documents in each `TaskStatus` bucket (`get_parsing_status_by_kb_ids`).
pub async fn get_ingestion_summary(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(dataset_id): Path<String>,
) -> Response {
    if dataset_id.is_empty() {
        return data_error("Lack of \"Dataset ID\"");
    }
    if !kb_accessible(&state, &auth, &dataset_id) {
        return data_error(format!(
            "User '{}' lacks permission for dataset '{}'",
            auth.user_id, dataset_id
        ));
    }
    let Some(kb) = state.kbs.get(&dataset_id) else {
        return data_error("Invalid Dataset ID");
    };
    let mut status = serde_json::json!({
        "unstart_count": 0,
        "running_count": 0,
        "cancel_count": 0,
        "done_count": 0,
        "fail_count": 0,
    });
    for doc in state.docs.list(&dataset_id) {
        let bucket = match doc.run.as_str() {
            "UNSTARTED" | "0" => "unstart_count",
            "RUNNING" | "1" => "running_count",
            "CANCEL" | "2" => "cancel_count",
            "DONE" | "3" => "done_count",
            "FAILED" | "4" => "fail_count",
            _ => "unstart_count",
        };
        let current = status[bucket].as_i64().unwrap_or_default();
        status[bucket] = serde_json::json!(current + 1);
    }
    let token_num: usize = state
        .engine
        .read()
        .unwrap()
        .to_vec()
        .iter()
        .filter(|chunk| chunk.metadata.get("kb_id").map(String::as_str) == Some(&dataset_id))
        .map(|chunk| chunk.token_count)
        .sum();
    ok(serde_json::json!({
        "doc_num": kb.doc_count,
        "chunk_num": kb.chunk_count,
        "token_num": token_num,
        "status": status,
    }))
}

/// `POST /api/v1/agents/rerun` — `agent_api.py::rerun_agent`. The body carries
/// the edited canvas (`dsl`) and the component to re-run
/// (`component_id`); upstream clears the document's chunks and queues the
/// dataflow again. RayRAG stores the edited DSL on the log and re-parses the
/// document, which is what the flow does for a file-level log.
#[derive(Debug, Deserialize)]
pub struct RerunRequest {
    pub id: String,
    #[serde(default)]
    pub dsl: serde_json::Value,
    #[serde(default)]
    pub component_id: String,
}

pub async fn rerun_agent(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<RerunRequest>,
) -> Response {
    let Some(log) = state.ingestion_logs.find(&body.id) else {
        return data_error("Document not found.");
    };
    if !kb_accessible(&state, &auth, &log.kb_id) {
        return data_error("Document not found.");
    }
    let Some(document) = state.docs.get(&log.document_id) else {
        return data_error("Document not found.");
    };
    if document.progress > 0.0 && document.progress < 1.0 {
        return data_error(format!("`{}` is processing...", document.name));
    }
    // Upstream rewrites the log's DSL with `dsl["path"] = [component_id]`.
    let mut dsl = body.dsl.clone();
    if !body.component_id.is_empty() {
        dsl["path"] = serde_json::json!([body.component_id]);
    }
    if dsl.is_null() {
        dsl = log.dsl.clone();
    }
    if let Err(error) = state.ingestion_logs.update_dsl(&log.id, dsl) {
        return data_error(error.to_string());
    }
    // Re-queue the document exactly like `/datasets/{kb}/documents/{doc}/reparse`.
    match crate::api::document::enqueue_document_processing(
        state,
        &auth.user_id,
        document,
        crate::api::features::TASK_PRIORITY_HIGH,
    ) {
        Ok(task_id) => {
            Json(serde_json::json!({ "code": 0, "data": true, "task_id": task_id })).into_response()
        }
        Err(error) => data_error(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{PipelineReport, StageReport};

    fn report() -> PipelineReport {
        PipelineReport {
            stages: vec![
                StageReport {
                    id: "parser",
                    component: "Parser",
                    title: "Parser",
                    elapsed_seconds: 1.5,
                    outputs: serde_json::json!({
                        "output_format": {"type": "string", "value": "text"},
                        "text": {"type": "string", "value": "hello"}
                    }),
                },
                StageReport {
                    id: "tokenChunker",
                    component: "TokenChunker",
                    title: "Token chunker",
                    elapsed_seconds: 0.25,
                    outputs: serde_json::json!({
                        "output_format": {"type": "string", "value": "chunks"},
                        "chunks": {"type": "array", "value": [{"content": "hello"}]},
                        "_elapsed_time": {"type": "number", "value": 0.25}
                    }),
                },
            ],
        }
    }

    #[test]
    fn parse_dsl_matches_the_dataflow_result_contract() {
        let dsl = build_parse_dsl(&report(), "report.pdf");
        // `pages/dataflow-result/hooks.ts::useTimelineDataFlow` walks
        // `dsl.components` and resolves each key against `dsl.graph.nodes`.
        assert_eq!(dsl["path"], serde_json::json!(["parser", "tokenChunker"]));
        let nodes = dsl["graph"]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0]["id"], "parser");
        assert_eq!(nodes[0]["data"]["name"], "Parser");
        assert_eq!(nodes[1]["id"], "tokenChunker");
        assert_eq!(
            dsl["graph"]["edges"][0],
            serde_json::json!({"source": "parser", "target": "tokenChunker"})
        );
        assert_eq!(
            dsl["components"]["tokenChunker"]["obj"]["component_name"],
            "TokenChunker"
        );
        assert_eq!(
            dsl["components"]["tokenChunker"]["obj"]["params"]["outputs"]["output_format"]["value"],
            "chunks"
        );
        assert_eq!(
            dsl["components"]["tokenChunker"]["obj"]["params"]["_elapsed_time"]["value"],
            0.25
        );
        assert_eq!(dsl["globals"]["sys.filename"], "report.pdf");
        assert_eq!(report().total_seconds(), 1.75);
        assert_eq!(report().stage("parser").unwrap().elapsed_seconds, 1.5);
        assert!(report().stage("missing").is_none());
    }

    #[test]
    fn logs_are_scoped_to_their_dataset_and_dsl_updates_persist() {
        let store = IngestionLogStore::in_memory();
        let mut log = IngestionLog {
            id: "log-1".into(),
            tenant_id: "tenant-a".into(),
            kb_id: "kb-1".into(),
            document_id: "doc-1".into(),
            document_name: "report.pdf".into(),
            document_suffix: "pdf".into(),
            document_type: "pdf".into(),
            parser_id: "naive".into(),
            pipeline_id: String::new(),
            pipeline_title: String::new(),
            source_from: "upload".into(),
            dsl: build_parse_dsl(&report(), "report.pdf"),
            progress: 1.0,
            progress_msg: String::new(),
            operation_status: "done".into(),
            status: "1".into(),
            task_type: "dataflow".into(),
            avatar: String::new(),
            log_type: "file".into(),
            process_begin_at: 1,
            process_duration: 1.75,
            create_time: 1,
            create_date: "2026-01-01 00:00:00".into(),
            update_time: 1,
            update_date: "2026-01-01 00:00:00".into(),
        };
        store.insert(log.clone()).unwrap();
        assert!(store.get("kb-1", "log-1").is_some());
        assert!(store.get("kb-2", "log-1").is_none());
        assert_eq!(store.list_for_kb("kb-1").len(), 1);
        assert!(
            store
                .update_dsl("log-1", serde_json::json!({"path": ["parser"]}))
                .unwrap()
        );
        assert_eq!(
            store.get("kb-1", "log-1").unwrap().dsl["path"],
            serde_json::json!(["parser"])
        );
        log.kb_id = "kb-2".into();
        store.insert(log).unwrap();
        assert_eq!(store.list_for_kb("kb-2").len(), 1);
    }
}
