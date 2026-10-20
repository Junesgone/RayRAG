//! Agent run traces — RAGFlow `api/apps/restful_apis/agent_api.py::get_agent_logs`
//! (1002) and its beta sibling `bot_api.py::agent_bot_logs` (277).
//!
//! Upstream stores one Redis value per assistant message under the key
//! `f"{agent_id}-{message_id}-logs"`: a JSON array of `ITraceData`
//! (`{component_id, trace: [{progress, message, datetime, timestamp,
//! elapsed_time}]}`). `pages/agent/log-sheet/workflow-timeline.tsx` reads it
//! through `useFetchMessageTrace` and polls every 3 s while a run is streaming
//! (that is what the shared embed's "Thinking" button uses, via
//! `fetchSharedTrace` → `/agentbots/{shared_id}/logs/{message_id}`).
//!
//! RayRAG keeps the same key/value contract in its own atomic-JSON store and
//! fills it from the canvas lifecycle events (`node_started` / `node_finished`)
//! that the workflow already emits for the SSE adapter, so a trace is written
//! incrementally while the run streams and is complete when it ends.

use crate::server::{AppState, AuthContext};
use axum::{
    Json,
    extract::{Extension, Path, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

/// `ITraceData` (`interfaces/database/agent.ts`).
///
/// `trace` is heterogeneous exactly like upstream's: progress samples written by
/// the pipeline callback (`{progress, message, datetime, timestamp,
/// elapsed_time}`) and `tool_use_callback` records
/// (`{path, tool_name, arguments, result, elapsed_time}`) share the same array —
/// `pages/agent/log-sheet/tool-timeline-item.tsx` reads the records that carry a
/// `tool_name`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceComponent {
    pub component_id: String,
    #[serde(default)]
    pub trace: Vec<serde_json::Value>,
}

/// One progress sample (`Pipeline.callback`); tool records are built by
/// `crate::agent::ToolUseTrace`.
pub fn progress_sample(
    message: impl Into<String>,
    datetime: impl Into<String>,
    timestamp: i64,
    elapsed_time: f64,
) -> serde_json::Value {
    serde_json::json!({
        "progress": 1.0,
        "message": message.into(),
        "datetime": datetime.into(),
        "timestamp": timestamp,
        "elapsed_time": elapsed_time,
    })
}

#[derive(Debug, Serialize, Deserialize)]
struct AgentTraceSnapshot {
    traces: HashMap<String, Vec<TraceComponent>>,
}

/// `f"{agent_id}-{message_id}-logs"` — the upstream Redis key.
pub fn trace_key(agent_id: &str, message_id: &str) -> String {
    format!("{agent_id}-{message_id}-logs")
}

pub struct AgentTraceStore {
    traces: RwLock<HashMap<String, Vec<TraceComponent>>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl AgentTraceStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = PathBuf::from(path);
        crate::persistence::restore_if_missing(&path)?;
        let traces = if path.exists() {
            let bytes = std::fs::read(&path)?;
            serde_json::from_slice::<AgentTraceSnapshot>(&bytes)
                .map(|snapshot| snapshot.traces)
                .unwrap_or_default()
        } else {
            HashMap::new()
        };
        let store = Self {
            traces: RwLock::new(traces),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            traces: RwLock::new(HashMap::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    fn persist(&self, traces: &HashMap<String, Vec<TraceComponent>>) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(
            path,
            &serde_json::to_vec_pretty(&AgentTraceSnapshot {
                traces: traces.clone(),
            })?,
        )
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let traces = self.traces.read().unwrap().clone();
        self.persist(&traces)
    }

    pub fn get(&self, agent_id: &str, message_id: &str) -> Option<Vec<TraceComponent>> {
        self.traces
            .read()
            .unwrap()
            .get(&trace_key(agent_id, message_id))
            .cloned()
    }

    pub fn record(
        &self,
        agent_id: &str,
        message_id: &str,
        trace: Vec<TraceComponent>,
    ) -> anyhow::Result<()> {
        let key = trace_key(agent_id, message_id);
        let snapshot = {
            let mut guard = self.traces.write().unwrap();
            guard.insert(key, trace);
            guard.clone()
        };
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(&snapshot)
    }
}

impl Default for AgentTraceStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// Collects the canvas lifecycle events into upstream's `ITraceData` array.
///
/// The workflow emits `node_started` when a component begins and `node_finished`
/// with its `elapsed_time`, `outputs` and `error`; each finished node appends one
/// trace sample for that component. The collected array is persisted on every
/// event so `GET .../logs/{message_id}` answers while the run is still streaming
/// — exactly the polling behaviour of upstream's log sheet.
pub struct AgentTraceCollector<'a> {
    store: Arc<AgentTraceStore>,
    agent_id: String,
    /// `None` for non-streaming runs, whose assistant message id is only known
    /// after the exchange has been appended.
    message_id: Mutex<Option<String>>,
    components: Mutex<Vec<TraceComponent>>,
    /// The SSE adapter, forwarded to unchanged when this run streams.
    inner: Option<&'a dyn crate::agent::WorkflowEventObserver>,
}

impl<'a> AgentTraceCollector<'a> {
    pub fn new(
        store: Arc<AgentTraceStore>,
        agent_id: impl Into<String>,
        message_id: Option<String>,
        inner: Option<&'a dyn crate::agent::WorkflowEventObserver>,
    ) -> Self {
        Self {
            store,
            agent_id: agent_id.into(),
            message_id: Mutex::new(message_id),
            components: Mutex::new(Vec::new()),
            inner,
        }
    }

    /// Publish the collected trace once the assistant message id is known
    /// (the non-streaming path).
    pub fn finish(&self, message_id: &str) -> anyhow::Result<()> {
        {
            let mut guard = self.message_id.lock().unwrap();
            if guard.is_none() {
                *guard = Some(message_id.to_string());
            }
        }
        let components = self.components.lock().unwrap().clone();
        self.store.record(&self.agent_id, message_id, components)
    }

    pub fn components(&self) -> Vec<TraceComponent> {
        self.components.lock().unwrap().clone()
    }

    fn persist(&self) {
        let message_id = self.message_id.lock().unwrap().clone();
        let Some(message_id) = message_id else {
            return;
        };
        let components = self.components.lock().unwrap().clone();
        if let Err(error) = self.store.record(&self.agent_id, &message_id, components) {
            tracing::warn!(%error, "Failed to persist agent trace");
        }
    }
}

impl crate::agent::WorkflowEventObserver for AgentTraceCollector<'_> {
    fn emit(&self, event: crate::agent::WorkflowLifecycleEvent) {
        if let Some(inner) = self.inner {
            inner.emit(event.clone());
        }
        let Some(component_id) = event
            .data
            .get("component_id")
            .and_then(serde_json::Value::as_str)
        else {
            return;
        };
        match event.event {
            "node_started" => {
                let mut components = self.components.lock().unwrap();
                if !components
                    .iter()
                    .any(|component| component.component_id == component_id)
                {
                    components.push(TraceComponent {
                        component_id: component_id.to_string(),
                        trace: Vec::new(),
                    });
                }
            }
            "node_finished" => {
                let elapsed = event
                    .data
                    .get("elapsed_time")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or_default();
                let created_at = event
                    .data
                    .get("created_at")
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or_default();
                let message = event
                    .data
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        event
                            .data
                            .get("component_name")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| component_id.to_string());
                let timestamp = created_at as i64;
                let datetime = chrono::DateTime::from_timestamp(timestamp, 0)
                    .map(|value| {
                        value
                            .with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                            .to_string()
                    })
                    .unwrap_or_default();
                {
                    let mut components = self.components.lock().unwrap();
                    // A resumed run can finish a node whose `node_started` event
                    // predates the resume, so the component is created on demand.
                    if !components
                        .iter()
                        .any(|component| component.component_id == component_id)
                    {
                        components.push(TraceComponent {
                            component_id: component_id.to_string(),
                            trace: Vec::new(),
                        });
                    }
                    if let Some(component) = components
                        .iter_mut()
                        .find(|component| component.component_id == component_id)
                    {
                        component
                            .trace
                            .push(progress_sample(message, datetime, timestamp, elapsed));
                        // `tool_use_callback` records ride in the node outputs
                        // and join the same array, which is what
                        // `tool-timeline-item.tsx` renders.
                        if let Some(tools) = event
                            .data
                            .get("outputs")
                            .and_then(|outputs| outputs.get("tool_trace"))
                            .and_then(serde_json::Value::as_array)
                        {
                            component.trace.extend(tools.iter().cloned());
                        }
                    }
                }
                self.persist();
            }
            _ => {}
        }
    }
}

/// `get_json_result(data=[...])`, or `get_json_result(data={})` when the run has
/// no trace yet (upstream returns an empty object, not an empty array).
fn trace_response(trace: Option<Vec<TraceComponent>>) -> Response {
    let data = match trace {
        Some(trace) => serde_json::to_value(trace).unwrap_or(serde_json::Value::Array(Vec::new())),
        None => serde_json::json!({}),
    };
    Json(serde_json::json!({ "code": 0, "data": data })).into_response()
}

/// `GET /api/v1/agents/{agent_id}/logs/{message_id}` — `get_agent_logs`.
pub async fn get_agent_logs(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((agent_id, message_id)): Path<(String, String)>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &agent_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        return (
            axum::http::StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "code": 404, "message": "Agent not found" })),
        )
            .into_response();
    }
    trace_response(state.agent_traces.get(&agent_id, &message_id))
}

/// `GET /api/v1/agentbots/{shared_id}/logs/{message_id}` — the beta-token
/// sibling. `shared_id` is the canvas id in the share URL; authentication comes
/// from the `APIToken.beta` bearer (resolved by the request middleware).
pub async fn agent_bot_logs(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((shared_id, message_id)): Path<(String, String)>,
) -> Response {
    if state
        .agents
        .get_accessible(
            &shared_id,
            &auth.user_id,
            auth.is_admin,
            |tenant_id, user_id| state.tenants.is_member(tenant_id, user_id),
        )
        .is_none()
    {
        // Upstream answers a data error with the client-supplied identifier.
        return Json(serde_json::json!({
            "code": 102,
            "message": format!("Can't find agent by ID: {shared_id}"),
            "data": null,
        }))
        .into_response();
    }
    trace_response(state.agent_traces.get(&shared_id, &message_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{WorkflowEventObserver, WorkflowLifecycleEvent};

    fn new_store() -> Arc<AgentTraceStore> {
        Arc::new(AgentTraceStore::in_memory())
    }

    #[test]
    fn trace_key_matches_the_upstream_redis_key() {
        assert_eq!(trace_key("agent-1", "msg-1"), "agent-1-msg-1-logs");
    }

    #[test]
    fn collector_groups_events_per_component_and_persists_progressively() {
        let store = new_store();
        let collector =
            AgentTraceCollector::new(store.clone(), "agent-1", Some("msg-1".to_string()), None);
        collector.emit(WorkflowLifecycleEvent {
            event: "node_started",
            data: serde_json::json!({"component_id": "begin", "component_name": "Begin"}),
        });
        collector.emit(WorkflowLifecycleEvent {
            event: "node_finished",
            data: serde_json::json!({
                "component_id": "begin",
                "component_name": "Begin",
                "elapsed_time": 0.5,
                "created_at": 1_700_086_400.0
            }),
        });
        // A second sample for the same component extends its trace, like
        // upstream's repeated pipeline callbacks.
        collector.emit(WorkflowLifecycleEvent {
            event: "node_finished",
            data: serde_json::json!({
                "component_id": "begin",
                "component_name": "Begin",
                "elapsed_time": 0.25,
                "created_at": 1_700_086_401.0
            }),
        });
        collector.emit(WorkflowLifecycleEvent {
            event: "node_started",
            data: serde_json::json!({"component_id": "retrieval", "component_name": "Retrieval"}),
        });
        collector.emit(WorkflowLifecycleEvent {
            event: "node_finished",
            data: serde_json::json!({
                "component_id": "retrieval",
                "component_name": "Retrieval",
                "elapsed_time": 1.5,
                "created_at": 1_700_086_402.0
            }),
        });
        // The store already answers while the run is streaming.
        let mid = store.get("agent-1", "msg-1").expect("trace persisted");
        assert_eq!(mid.len(), 2);
        assert_eq!(mid[0].component_id, "begin");
        assert_eq!(mid[0].trace.len(), 2);
        assert_eq!(mid[0].trace[0]["elapsed_time"], 0.5);
        assert_eq!(mid[0].trace[0]["message"], "Begin");
        assert_eq!(mid[0].trace[0]["progress"], 1.0);
        assert_eq!(mid[1].component_id, "retrieval");
        assert_eq!(mid[1].trace.len(), 1);

        // A failure is recorded as the sample's message.
        collector.emit(WorkflowLifecycleEvent {
            event: "node_finished",
            data: serde_json::json!({
                "component_id": "retrieval",
                "component_name": "Retrieval",
                "error": "boom",
                "elapsed_time": 0.1,
                "created_at": 1_700_086_403.0
            }),
        });
        let trace = store.get("agent-1", "msg-1").unwrap();
        assert_eq!(trace[1].trace[1]["message"], "boom");

        // `tool_use_callback` records in the node outputs join the same trace
        // array (`tool-timeline-item.tsx` reads the entries carrying tool_name).
        collector.emit(WorkflowLifecycleEvent {
            event: "node_finished",
            data: serde_json::json!({
                "component_id": "retrieval",
                "component_name": "Retrieval",
                "elapsed_time": 0.3,
                "created_at": 1_700_086_405.0,
                "outputs": {
                    "tool_trace": [{
                        "path": "retrieval-->google",
                        "tool_name": "google",
                        "arguments": {"q": "rust"},
                        "result": "ok",
                        "elapsed_time": 0.42
                    }]
                }
            }),
        });
        let trace = store.get("agent-1", "msg-1").unwrap();
        let retrieval = trace
            .iter()
            .find(|component| component.component_id == "retrieval")
            .unwrap();
        let tool = retrieval
            .trace
            .iter()
            .find(|entry| entry.get("tool_name").is_some())
            .expect("tool record merged");
        assert_eq!(tool["tool_name"], "google");
        assert_eq!(tool["arguments"]["q"], "rust");
        assert_eq!(tool["result"], "ok");
        assert_eq!(tool["elapsed_time"], 0.42);

        // Non-streaming runs publish once the assistant message id is known.
        let late_store = new_store();
        let late: AgentTraceCollector<'_> =
            AgentTraceCollector::new(late_store.clone(), "agent-2", None, None);
        late.emit(WorkflowLifecycleEvent {
            event: "node_finished",
            data: serde_json::json!({
                "component_id": "begin",
                "component_name": "Begin",
                "elapsed_time": 0.2,
                "created_at": 1_700_086_404.0
            }),
        });
        assert!(late_store.get("agent-2", "msg-2").is_none());
        late.finish("msg-2").unwrap();
        assert_eq!(late_store.get("agent-2", "msg-2").unwrap().len(), 1);
    }
}
