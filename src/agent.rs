//! RAGFlow-compatible Agent Canvas validation and core workflow execution.
//!
//! This module deliberately implements a small, explicit component set. A
//! canvas containing a component that is not implemented is rejected before
//! the first node runs, so RayRAG never presents a prompt-only fallback as a
//! successful visual workflow execution.

// 同花顺问财（iwencai）工具：RAGFlow `agent/tools/wencai.py`（pywencai）的 Rust 实现。
// 由于 lib.rs 不可改动，模块以 #[path] 挂在 agent 模块下，文件仍位于 src/wencai.rs。
#[path = "wencai.rs"]
pub(crate) mod wencai;

use crate::akshare::{AkShareClient, AkShareRequest};
use crate::arxiv::{ArxivClient, ArxivPaper, ArxivProvider, ArxivSearchRequest, ArxivSortBy};
use crate::baidu::{BaiduClient, BaiduProvider, BaiduSearchRequest};
use crate::baidu_scholar::{BaiduScholarClient, BaiduScholarProvider, BaiduScholarRequest};
use crate::baike::{BaikeArticle, BaikeClient, BaikeProvider, BaikeSearchRequest};
use crate::bing::{BingClient, BingProvider, BingSearchRequest};
use crate::bocha::{BochaClient, BochaProvider, BochaSearchRequest};
use crate::code_exec::{CodeRequest, SandboxClient, process_result};
use crate::crawler::{CrawlerClient, CrawlerRequest};
use crate::duckduckgo::{
    DuckDuckGoChannel, DuckDuckGoClient, DuckDuckGoProvider, DuckDuckGoSearchRequest,
};
use crate::eastmoney::{EastMoneyClient, EastMoneyNewsRequest, EastMoneyProvider};
use crate::email::{EmailClient, EmailConfig, EmailRequest};
use crate::exesql::{ExeSqlConfig, execute_sql};
use crate::generation_params::GenerationParamsPatch;
use crate::github::{GitHubClient, GitHubProvider, GitHubRepository, GitHubSearchRequest};
use crate::google::{GoogleClient, GoogleProvider, GoogleSearchRequest};
use crate::google_scholar::{
    GoogleScholarClient, GoogleScholarProvider, GoogleScholarPublication,
    GoogleScholarSearchRequest, GoogleScholarSortBy,
};
use crate::jin10::{Jin10Client, Jin10Provider, Jin10Request, Jin10Type};
use crate::llm::{ChatMessage, ChunkReference, LlmClient, TokenUsage, ToolCall, ToolChatMessage};
use crate::pubmed::{PubMedArticle, PubMedClient, PubMedProvider, PubMedSearchRequest};
use crate::qweather::{QWeatherClient, QWeatherProvider, QWeatherRequest, QWeatherType};
use crate::searxng::{SearxngClient, SearxngProvider, SearxngRequest};
use crate::tavily::{TavilyClient, TavilyExtractRequest, TavilyProvider, TavilySearchRequest};
use crate::tencent_finance::{TencentFinanceClient, TencentFinanceProvider, TencentFinanceRequest};
use crate::translate::{BaiduTranslateClient, TranslateRequest};
use crate::tushare::{TuShareClient, TuShareRequest};
use crate::wikipedia::{
    WIKIPEDIA_LANGUAGES, WikipediaArticle, WikipediaClient, WikipediaProvider,
    WikipediaSearchRequest,
};
use crate::yahoo_finance::{YahooFinanceClient, YahooFinanceProvider, YahooFinanceRequest};
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use futures_util::stream::{self, StreamExt};
use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use rand::prelude::IndexedRandom;
use regex::Regex;
use reqwest::redirect::Policy as RedirectPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use sha1::{Digest as _, Sha1};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct CanvasNode {
    id: String,
    component_name: String,
    params: Map<String, Value>,
    downstream: Vec<String>,
    upstream: Vec<String>,
    parent_id: Option<String>,
}

#[derive(Debug, Clone)]
struct SubgraphPlan {
    members: HashSet<String>,
    execution_order: Vec<String>,
    outer_downstream: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AgentWorkflow {
    nodes: BTreeMap<String, CanvasNode>,
    begin_id: String,
    execution_order: Vec<String>,
    loop_plans: BTreeMap<String, SubgraphPlan>,
    parallel_plans: BTreeMap<String, SubgraphPlan>,
    initial_globals: Map<String, Value>,
}

/// The fixed Go loop driver reports an error when the explicit (or safety)
/// iteration cap is reached before the termination predicate becomes true.
/// Keeping a typed Rust error lets callers distinguish that outcome while the
/// shared Canvas state still retains mutations from completed iterations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Canvas Loop '{component_id}' exceeded maximum iterations: {maximum}")]
pub struct LoopMaxIterationsExceeded {
    pub component_id: String,
    pub maximum: usize,
}

pub struct WorkflowRunInput<'a> {
    pub question: &'a str,
    pub user_id: &'a str,
    pub inputs: &'a Map<String, Value>,
    pub history: &'a [ChatMessage],
    pub generation: GenerationParamsPatch,
    pub llm_resolver: Option<&'a dyn WorkflowLlmResolver>,
    pub retriever: Option<&'a dyn WorkflowRetriever>,
    pub fallback_kb_ids: &'a [String],
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowRetrievalRequest {
    pub query: String,
    pub kb_ids: Vec<String>,
    pub similarity_threshold: f32,
    pub keywords_similarity_weight: f32,
    pub top_n: usize,
    pub top_k: usize,
    pub rerank_id: Option<String>,
    pub meta_data_filter: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkflowRetrievalResult {
    pub formalized_content: String,
    pub chunks: Vec<Value>,
    pub doc_aggs: Vec<Value>,
    pub references: Vec<ChunkReference>,
}

#[async_trait::async_trait]
pub trait WorkflowRetriever: Send + Sync {
    async fn retrieve(&self, request: WorkflowRetrievalRequest) -> Result<WorkflowRetrievalResult>;
}

/// Resolve a Canvas node's persisted tenant model selector at execution time.
///
/// The returned client owns its normalized endpoint and decoded credential, so
/// neither secrets nor tenant-scoped store borrows escape the current node.
pub trait WorkflowLlmResolver: Send + Sync {
    fn resolve_chat_model(&self, selector: &str) -> Result<LlmClient>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowNodeTrace {
    pub component_id: String,
    pub component_type: String,
    pub outputs: Map<String, Value>,
}

/// Execution-time lifecycle payload forwarded to the Agent SSE adapter.
///
/// The workflow owns scheduling and therefore emits node events here rather
/// than reconstructing them later from the completed trace.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowLifecycleEvent {
    pub event: &'static str,
    pub data: Value,
}

pub trait WorkflowEventObserver: Send + Sync {
    fn emit(&self, event: WorkflowLifecycleEvent);
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WorkflowRunResult {
    pub answer: String,
    pub usage: Option<TokenUsage>,
    pub references: Vec<ChunkReference>,
    pub path: Vec<String>,
    pub trace: Vec<WorkflowNodeTrace>,
}

/// Serializable execution state captured immediately before a UserFillUp
/// node. The outer scheduler cursor and selected-node set are stored alongside
/// Canvas state; Loop/Parallel leaves additionally carry their composite
/// cursor and child snapshots so resume does not rerun upstream effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWorkflowCheckpoint {
    version: u32,
    waiting_component_id: String,
    next_execution_index: usize,
    selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    composite: Option<CompositeCheckpoint>,
    runtime: CanvasRuntime,
    transient: CanvasCheckpointTransient,
    trace: Vec<WorkflowNodeTrace>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CanvasCheckpointTransient {
    last_message: Option<String>,
    usage: TokenUsage,
    provider_calls: usize,
    references: Vec<ChunkReference>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum CompositeCheckpoint {
    Loop(LoopCheckpoint),
    Parallel(ParallelCheckpoint),
}

impl CompositeCheckpoint {
    fn macro_component_id(&self) -> &str {
        match self {
            Self::Loop(state) => &state.loop_component_id,
            Self::Parallel(state) => &state.parallel_component_id,
        }
    }

    fn waiting_component_id(&self) -> Option<&str> {
        match self {
            Self::Loop(state) => Some(&state.cursor.waiting_component_id),
            Self::Parallel(state) => state
                .pending_items
                .get(&state.active_index)
                .map(|pending| pending.cursor.waiting_component_id.as_str()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SubgraphCursor {
    waiting_component_id: String,
    next_execution_index: usize,
    selected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoopCheckpoint {
    loop_component_id: String,
    iteration: usize,
    cursor: SubgraphCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SavedCanvasRuntime {
    runtime: CanvasRuntime,
    transient: CanvasCheckpointTransient,
}

impl SavedCanvasRuntime {
    fn capture(runtime: &CanvasRuntime) -> Self {
        Self {
            runtime: runtime.clone(),
            transient: CanvasCheckpointTransient::capture(runtime),
        }
    }

    fn restore(self) -> CanvasRuntime {
        let mut runtime = self.runtime;
        self.transient.restore(&mut runtime);
        runtime
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParallelPendingItem {
    canvas: SavedCanvasRuntime,
    cursor: SubgraphCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParallelCheckpoint {
    parallel_component_id: String,
    original_items: Vec<Value>,
    completed_items: BTreeMap<usize, SavedCanvasRuntime>,
    pending_items: BTreeMap<usize, ParallelPendingItem>,
    active_index: usize,
}

impl CanvasCheckpointTransient {
    fn capture(runtime: &CanvasRuntime) -> Self {
        Self {
            last_message: runtime.last_message.clone(),
            usage: runtime.usage,
            provider_calls: runtime.provider_calls,
            references: runtime.references.clone(),
        }
    }

    fn restore(self, runtime: &mut CanvasRuntime) {
        runtime.last_message = self.last_message;
        runtime.usage = self.usage;
        runtime.provider_calls = self.provider_calls;
        runtime.references = self.references;
    }
}

#[derive(Debug)]
struct CompositeWait {
    leaf_component_id: String,
    interrupt_id: String,
    state: CompositeCheckpoint,
}

#[derive(Debug)]
enum CompositeExecutionOutcome {
    Completed,
    Waiting(CompositeWait),
}

#[derive(Debug)]
enum ParallelItemOutcome {
    Completed(CanvasRuntime),
    Waiting {
        cursor: SubgraphCursor,
        runtime: CanvasRuntime,
    },
}

/// User-visible form payload returned when a UserFillUp pauses the workflow.
/// `checkpoint` is deliberately not serialized into the HTTP
/// response; the authenticated server-side checkpoint store owns it.
#[derive(Debug, Clone)]
pub struct WorkflowWaitingForUser {
    pub component_id: String,
    pub interrupt_id: String,
    pub tips: Option<String>,
    pub inputs: Map<String, Value>,
    pub path: Vec<String>,
    pub trace: Vec<WorkflowNodeTrace>,
    pub checkpoint: AgentWorkflowCheckpoint,
}

#[derive(Debug, Clone)]
pub enum WorkflowRunOutcome {
    Completed(WorkflowRunResult),
    WaitingForUser(Box<WorkflowWaitingForUser>),
}

/// Per-run Canvas state.
///
/// Unlike the Go runtime, one workflow future owns this value, so reads and
/// writes need no mutex. The serialized fields deliberately mirror RAGFlow's
/// checkpoint wire shape; transient provider/output helpers are skipped.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CanvasRuntime {
    #[serde(default)]
    outputs: BTreeMap<String, Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    sys: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    env: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    path: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    history: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    retrieval: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    globals: Map<String, Value>,
    #[serde(default)]
    cancel_flag: bool,
    #[serde(default)]
    run_id: String,
    #[serde(default)]
    task_id: String,
    #[serde(skip)]
    last_message: Option<String>,
    #[serde(skip)]
    usage: TokenUsage,
    #[serde(skip)]
    provider_calls: usize,
    #[serde(skip)]
    references: Vec<ChunkReference>,
}

/// Validate the persisted DSL shape while retaining compatibility with old
/// prompt-only records (`{}` and `{"components": []}`).
pub fn validate_agent_dsl(dsl: &Value) -> Result<()> {
    AgentWorkflow::from_value(dsl).map(|_| ())
}

/// Structural error categories returned by the pure Agent DSL extractors.
///
/// RAGFlow uses three sentinel errors and `errors.Is`; a closed Rust enum
/// preserves that distinction while carrying enough context for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentDslError {
    #[error("dsl: component not found: {0}")]
    ComponentNotFound(String),
    #[error("dsl: component has no input_form: {0}")]
    MissingInputForm(String),
    #[error("dsl: malformed: {0}")]
    Malformed(String),
}

/// Borrow a component's static `obj.input_form` without cloning its schema.
pub fn extract_component_input_form<'a>(
    dsl: &'a Value,
    component_id: &str,
) -> std::result::Result<&'a Map<String, Value>, AgentDslError> {
    let component = navigate_to_component(dsl, component_id)?;
    let object = component
        .get("obj")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            AgentDslError::Malformed(format!("component {component_id:?} has no obj"))
        })?;
    let Some(form) = object.get("input_form").filter(|form| !form.is_null()) else {
        return Err(AgentDslError::MissingInputForm(component_id.to_owned()));
    };
    form.as_object().ok_or_else(|| {
        AgentDslError::Malformed(format!(
            "component {component_id:?} input_form is not a dict"
        ))
    })
}

/// Borrow a component's optional `obj.params` map without cloning it.
pub fn extract_component_params<'a>(
    dsl: &'a Value,
    component_id: &str,
) -> std::result::Result<Option<&'a Map<String, Value>>, AgentDslError> {
    let component = navigate_to_component(dsl, component_id)?;
    let object = component
        .get("obj")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            AgentDslError::Malformed(format!("component {component_id:?} has no obj"))
        })?;
    let Some(params) = object.get("params").filter(|params| !params.is_null()) else {
        return Ok(None);
    };
    params.as_object().map(Some).ok_or_else(|| {
        AgentDslError::Malformed(format!("component {component_id:?} params is not a dict"))
    })
}

/// Borrow the runtime factory name stored at `obj.component_name`.
pub fn extract_component_name<'a>(
    dsl: &'a Value,
    component_id: &str,
) -> std::result::Result<&'a str, AgentDslError> {
    let component = navigate_to_component(dsl, component_id)?;
    let object = component
        .get("obj")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            AgentDslError::Malformed(format!("component {component_id:?} has no obj"))
        })?;
    object
        .get("component_name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            AgentDslError::Malformed(format!("component {component_id:?} has no component_name"))
        })
}

fn navigate_to_component<'a>(
    dsl: &'a Value,
    component_id: &str,
) -> std::result::Result<&'a Map<String, Value>, AgentDslError> {
    let root = dsl
        .as_object()
        .ok_or_else(|| AgentDslError::Malformed("nil or non-object dsl".into()))?;
    let components = root
        .get("components")
        .and_then(Value::as_object)
        .ok_or_else(|| AgentDslError::Malformed("missing components map".into()))?;
    let component = components
        .get(component_id)
        .ok_or_else(|| AgentDslError::ComponentNotFound(component_id.to_owned()))?;
    component.as_object().ok_or_else(|| {
        AgentDslError::Malformed(format!("component {component_id:?} is not a dict"))
    })
}

/// Return the ID of the first component whose factory name is exactly `Begin`.
///
/// The source Go map has unspecified iteration order. RayRAG preserves the
/// persisted JSON order, making malformed multi-Begin canvases deterministic.
pub fn find_begin_component_id(dsl: &Value) -> std::result::Result<&str, AgentDslError> {
    let root = dsl
        .as_object()
        .ok_or_else(|| AgentDslError::Malformed("nil or non-object dsl".into()))?;
    let components = root
        .get("components")
        .and_then(Value::as_object)
        .ok_or_else(|| AgentDslError::Malformed("missing components map".into()))?;
    components
        .iter()
        .find_map(|(id, component)| {
            (component
                .as_object()
                .and_then(|component| component.get("obj"))
                .and_then(Value::as_object)
                .and_then(|object| object.get("component_name"))
                .and_then(Value::as_str)
                == Some("Begin"))
            .then_some(id.as_str())
        })
        .ok_or_else(|| AgentDslError::ComponentNotFound("Begin component".into()))
}

/// Borrow the Begin component prologue, returning `""` for absent/wrong types.
pub fn extract_prologue(dsl: &Value) -> std::result::Result<&str, AgentDslError> {
    let begin_id = find_begin_component_id(dsl)?;
    let component = navigate_to_component(dsl, begin_id)?;
    Ok(component
        .get("obj")
        .and_then(Value::as_object)
        .and_then(|object| object.get("prologue"))
        .and_then(Value::as_str)
        .unwrap_or_default())
}

/// Borrow the Begin component mode, returning `""` for absent/wrong types.
pub fn extract_mode(dsl: &Value) -> std::result::Result<&str, AgentDslError> {
    let begin_id = find_begin_component_id(dsl)?;
    let component = navigate_to_component(dsl, begin_id)?;
    Ok(component
        .get("obj")
        .and_then(Value::as_object)
        .and_then(|object| object.get("mode"))
        .and_then(Value::as_str)
        .unwrap_or_default())
}

/// Return a component input form, preferring the persisted static schema and
/// synthesizing the five dynamic forms exposed by the fixed Go component set.
pub fn agent_component_input_form<'a>(
    dsl: &'a Value,
    component_id: &str,
) -> std::result::Result<Cow<'a, Map<String, Value>>, AgentDslError> {
    match extract_component_input_form(dsl, component_id) {
        Ok(form) => return Ok(Cow::Borrowed(form)),
        Err(AgentDslError::MissingInputForm(_)) => {}
        Err(error) => return Err(error),
    }

    let component_name = extract_component_name(dsl, component_id)?;
    let params = extract_component_params(dsl, component_id)?
        .cloned()
        .unwrap_or_default();
    let form = match component_name {
        "Agent" | "Generate" | "LLM" => dynamic_agent_input_form(&params),
        "Browser" => serde_json::from_value(serde_json::json!({
            "prompts": {"type": "text", "name": "Prompts"},
            "upload_sources": {"type": "line", "name": "Upload sources"}
        }))
        .expect("static Browser input form is an object"),
        "BGPT" => serde_json::from_value(serde_json::json!({
            "query": {"name": "Query", "type": "line"}
        }))
        .expect("static BGPT input form is an object"),
        "ExeSQL" => serde_json::from_value(serde_json::json!({
            "sql": {"name": "SQL", "type": "line"}
        }))
        .expect("static ExeSQL input form is an object"),
        "YahooFinance" => serde_json::from_value(serde_json::json!({
            "stock_code": {"type": "line", "name": "Stock code/Company name"}
        }))
        .expect("static YahooFinance input form is an object"),
        "DocGenerator" | "DocsGenerator" => serde_json::from_value(serde_json::json!({
            "content": {"name": "Content", "type": "text"}
        }))
        .expect("static DocGenerator input form is an object"),
        _ => return Err(AgentDslError::MissingInputForm(component_id.to_owned())),
    };
    Ok(Cow::Owned(form))
}

#[derive(Debug, thiserror::Error)]
pub enum AgentComponentDebugError {
    #[error(transparent)]
    Dsl(#[from] AgentDslError),
    #[error("component factory: unsupported component {0}")]
    UnsupportedComponent(String),
    #[error("invoke: {0}")]
    Invoke(#[source] anyhow::Error),
}

pub struct AgentComponentDebugContext<'a> {
    pub authenticated_user_id: &'a str,
    pub llm: Option<&'a LlmClient>,
    pub llm_resolver: Option<&'a dyn WorkflowLlmResolver>,
    pub retriever: Option<&'a dyn WorkflowRetriever>,
    pub fallback_kb_ids: &'a [String],
}

/// Invoke one persisted component against a fresh, isolated Canvas state.
///
/// Request values override node params for non-Begin components. `sys.*`
/// values seed the state, except `sys.tenant_id`, which is always fixed to the
/// authenticated caller to prevent credential-scope escalation.
pub async fn debug_agent_component(
    dsl: &Value,
    component_id: &str,
    inputs: &Map<String, Value>,
    context: AgentComponentDebugContext<'_>,
) -> std::result::Result<Map<String, Value>, AgentComponentDebugError> {
    let component_name = extract_component_name(dsl, component_id)?.to_owned();
    if !crate::runtime::is_supported_agent_component(&component_name) {
        return Err(AgentComponentDebugError::UnsupportedComponent(
            component_name,
        ));
    }
    let component = navigate_to_component(dsl, component_id)?;
    let mut params = extract_component_params(dsl, component_id)?
        .cloned()
        .unwrap_or_default();
    if !component_name.eq_ignore_ascii_case("begin") {
        for (name, value) in inputs {
            if !name.starts_with("sys.") {
                params.insert(name.clone(), value.clone());
            }
        }
    }
    let node = CanvasNode {
        id: component_id.to_owned(),
        component_name,
        params,
        downstream: debug_topology(component.get("downstream")),
        upstream: debug_topology(component.get("upstream")),
        parent_id: component
            .get("parent_id")
            .and_then(Value::as_str)
            .map(str::to_owned),
    };

    let mut runtime = CanvasRuntime {
        run_id: format!("debug-{component_id}"),
        task_id: "debug-task".into(),
        ..Default::default()
    };
    runtime.sys.insert(
        "tenant_id".into(),
        Value::String(context.authenticated_user_id.to_owned()),
    );
    for (name, value) in inputs {
        if let Some(sys_name) = name.strip_prefix("sys.")
            && sys_name != "tenant_id"
        {
            runtime.sys.insert(sys_name.to_owned(), value.clone());
        }
    }
    let question = inputs
        .get("query")
        .or_else(|| inputs.get("sys.query"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    runtime
        .sys
        .entry("query")
        .or_insert_with(|| Value::String(question.clone()));
    let history = Vec::new();
    let run_input = WorkflowRunInput {
        question: &question,
        user_id: context.authenticated_user_id,
        inputs,
        history: &history,
        generation: GenerationParamsPatch::default(),
        llm_resolver: context.llm_resolver,
        retriever: context.retriever,
        fallback_kb_ids: context.fallback_kb_ids,
    };
    execute_node_with_timeout(&mut runtime, &node, context.llm, &run_input)
        .await
        .map_err(AgentComponentDebugError::Invoke)?;
    Ok(runtime.outputs.remove(component_id).unwrap_or_default())
}

fn debug_topology(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

fn dynamic_agent_input_form(params: &Map<String, Value>) -> Map<String, Value> {
    let mut system_prompt = params
        .get("system_prompt")
        .or_else(|| params.get("sys_prompt"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let mut user_prompt = String::new();
    match params.get("prompts") {
        Some(Value::String(prompt)) => user_prompt.clone_from(prompt),
        Some(Value::Array(prompts)) => {
            let mut systems = Vec::new();
            let mut users = Vec::new();
            for prompt in prompts.iter().filter_map(Value::as_object) {
                let Some(content) = prompt.get("content").and_then(Value::as_str) else {
                    continue;
                };
                match prompt
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "system" => systems.push(content),
                    "user" | "" => users.push(content),
                    _ => {}
                }
            }
            if !systems.is_empty() {
                let extra = systems.join("\n");
                if system_prompt.trim().is_empty() {
                    system_prompt = extra;
                } else if !extra.trim().is_empty() {
                    system_prompt.push('\n');
                    system_prompt.push_str(&extra);
                }
            }
            if !users.is_empty() {
                user_prompt = users.join("\n");
            }
        }
        _ => {}
    }
    if user_prompt.is_empty()
        && let Some(prompt) = params.get("user_prompt").and_then(Value::as_str)
    {
        user_prompt = prompt.to_owned();
    }

    let mut form = Map::new();
    let mut seen = HashSet::new();
    for prompt in [&system_prompt, &user_prompt] {
        for captures in agent_input_ref_regex().captures_iter(prompt) {
            let key = captures
                .get(1)
                .expect("agent input regex has one capture")
                .as_str()
                .trim();
            if key.is_empty() || !seen.insert(key.to_owned()) {
                continue;
            }
            form.insert(
                key.to_owned(),
                serde_json::json!({
                    "type": "line",
                    "name": key,
                    "optional": false
                }),
            );
        }
    }
    form
}

fn agent_input_ref_regex() -> &'static Regex {
    static AGENT_INPUT_REF: OnceLock<Regex> = OnceLock::new();
    AGENT_INPUT_REF.get_or_init(|| {
        Regex::new(
            r"\{+\s*([a-zA-Z:0-9_]+@[A-Za-z0-9_.-]+|sys\.[A-Za-z0-9_.]+|env\.[A-Za-z0-9_.]+|item|index)\s*\}+",
        )
        .expect("static Agent input reference regex is valid")
    })
}

/// Return a defensive, front-end-facing Agent DSL snapshot.
///
/// Existing React-Flow handles and leaked runtime-only Parallel names are
/// repaired. If a populated components map has no graph, a deterministic
/// default layout is derived from it.
pub fn normalize_agent_dsl_for_canvas(dsl: &Value) -> Value {
    normalize_agent_dsl(dsl, false)
}

/// Return a defensive runtime-facing Agent DSL snapshot.
///
/// In addition to canvas repairs, legacy LoopItem/IterationItem children are
/// folded into their parent and Iteration references become Parallel aliases.
pub fn normalize_agent_dsl_for_run(dsl: &Value) -> Value {
    normalize_agent_dsl(dsl, true)
}

fn normalize_agent_dsl(dsl: &Value, fold_legacy: bool) -> Value {
    let mut normalized = normalize_chunker_dsl(dsl);
    let Some(root) = normalized.as_object_mut() else {
        return normalized;
    };

    enforce_canvas_handle_ids(root);
    if !canvas_graph_has_nodes(root)
        && let Some(components) = root
            .get("components")
            .and_then(Value::as_object)
            .filter(|components| !components.is_empty())
    {
        let (nodes, edges, normalized_components) = build_canvas_graph_from_components(components);
        if !nodes.is_empty() {
            root.insert(
                "graph".into(),
                serde_json::json!({"nodes": nodes, "edges": edges}),
            );
            root.insert("components".into(), Value::Object(normalized_components));
        }
    }

    repair_parallel_canvas_leaks(root);
    if fold_legacy {
        fold_legacy_loop_variants(root);
        rewrite_legacy_iteration_aliases(&mut normalized);
    }
    normalized
}

const LEGACY_CHUNKER_COMPONENT_RENAMES: [(&str, &str); 3] = [
    ("Splitter", "TokenChunker"),
    ("HierarchicalMerger", "TitleChunker"),
    ("PDFGenerator", "DocGenerator"),
];

/// Rewrite fixed-version legacy chunker identifiers into the current DSL.
///
/// This mirrors RAGFlow's pure `agent/dsl_migration.py` boundary: business
/// parameters are retained while component ids, graph topology and embedded
/// variable references are renamed on an owned snapshot.
pub fn normalize_chunker_dsl(dsl: &Value) -> Value {
    let mut normalized = dsl.clone();
    let Some(root) = normalized.as_object_mut() else {
        return normalized;
    };
    let Some(components) = root.get("components").and_then(Value::as_object).cloned() else {
        return normalized;
    };

    let component_id_map: BTreeMap<String, String> = components
        .keys()
        .map(|component_id| (component_id.clone(), rename_legacy_chunker_id(component_id)))
        .collect();

    let mut rewritten_components = Map::new();
    for (old_component_id, mut component) in components {
        rewrite_legacy_chunker_value(&mut component, &component_id_map);
        if let Some(component) = component.as_object_mut() {
            if let Some(object) = component.get_mut("obj").and_then(Value::as_object_mut) {
                let component_name = object.get("component_name").cloned().unwrap_or(Value::Null);
                object.insert(
                    "component_name".into(),
                    component_name
                        .as_str()
                        .and_then(rename_legacy_chunker_component)
                        .map(|name| Value::String(name.to_owned()))
                        .unwrap_or(component_name),
                );
            }
            for field in ["downstream", "upstream"] {
                if let Some(values) = component.get_mut(field).and_then(Value::as_array_mut) {
                    rewrite_legacy_chunker_id_list(values, &component_id_map);
                }
            }
            if let Some(parent_id) = component.get_mut("parent_id") {
                rewrite_exact_legacy_chunker_id(parent_id, &component_id_map);
            }
        }
        let new_component_id = component_id_map
            .get(&old_component_id)
            .cloned()
            .unwrap_or(old_component_id);
        rewritten_components.insert(new_component_id, component);
    }
    root.insert("components".into(), Value::Object(rewritten_components));

    if let Some(path) = root.get_mut("path").and_then(Value::as_array_mut) {
        rewrite_legacy_chunker_id_list(path, &component_id_map);
    }

    if let Some(graph) = root.get_mut("graph").and_then(Value::as_object_mut) {
        if let Some(nodes) = graph.get_mut("nodes").and_then(Value::as_array_mut) {
            for node in nodes.iter_mut().filter_map(Value::as_object_mut) {
                if let Some(node_id) = node.get_mut("id") {
                    rewrite_exact_legacy_chunker_id(node_id, &component_id_map);
                }
                if let Some(parent_id) = node.get_mut("parentId") {
                    rewrite_exact_legacy_chunker_id(parent_id, &component_id_map);
                }
                if node.get("type").and_then(Value::as_str) == Some("splitterNode") {
                    node.insert("type".into(), Value::String("chunkerNode".into()));
                }
                let Some(data) = node.get_mut("data").and_then(Value::as_object_mut) else {
                    continue;
                };
                for field in ["label", "name"] {
                    let Some(name) = data.get(field).and_then(Value::as_str) else {
                        continue;
                    };
                    if let Some(name) = rename_legacy_chunker_component(name) {
                        data.insert(field.into(), Value::String(name.to_owned()));
                    }
                }
                if let Some(form) = data.get_mut("form") {
                    rewrite_legacy_chunker_value(form, &component_id_map);
                }
            }
        }

        if let Some(edges) = graph.get_mut("edges").and_then(Value::as_array_mut) {
            let mut replacements: Vec<_> = component_id_map
                .iter()
                .filter(|(old, new)| old != new)
                .collect();
            replacements.sort_by_key(|(component_id, _)| std::cmp::Reverse(component_id.len()));
            for edge in edges.iter_mut().filter_map(Value::as_object_mut) {
                for field in ["source", "target"] {
                    if let Some(component_id) = edge.get_mut(field) {
                        rewrite_exact_legacy_chunker_id(component_id, &component_id_map);
                    }
                }
                let Some(edge_id) = edge.get("id").and_then(Value::as_str).map(str::to_owned)
                else {
                    continue;
                };
                let mut rewritten = edge_id;
                for (old_component_id, new_component_id) in &replacements {
                    rewritten = rewritten.replace(old_component_id.as_str(), new_component_id);
                }
                edge.insert("id".into(), Value::String(rewritten));
            }
        }
    }

    for field in ["history", "messages", "reference"] {
        if let Some(value) = root.get_mut(field) {
            rewrite_legacy_chunker_value(value, &component_id_map);
        }
    }
    normalized
}

fn rename_legacy_chunker_component(component_name: &str) -> Option<&'static str> {
    LEGACY_CHUNKER_COMPONENT_RENAMES
        .iter()
        .find_map(|(old, new)| (*old == component_name).then_some(*new))
}

fn rename_legacy_chunker_id(component_id: &str) -> String {
    for (old, new) in LEGACY_CHUNKER_COMPONENT_RENAMES {
        if let Some(suffix) = component_id
            .strip_prefix(old)
            .and_then(|suffix| suffix.strip_prefix(':'))
        {
            return format!("{new}:{suffix}");
        }
    }
    component_id.to_owned()
}

fn rewrite_legacy_chunker_id_list(
    values: &mut [Value],
    component_id_map: &BTreeMap<String, String>,
) {
    for value in values {
        rewrite_exact_legacy_chunker_id(value, component_id_map);
    }
}

fn rewrite_exact_legacy_chunker_id(value: &mut Value, component_id_map: &BTreeMap<String, String>) {
    let Some(component_id) = value.as_str() else {
        return;
    };
    if let Some(rewritten) = component_id_map.get(component_id) {
        *value = Value::String(rewritten.clone());
    }
}

fn rewrite_legacy_chunker_value(value: &mut Value, component_id_map: &BTreeMap<String, String>) {
    match value {
        Value::String(text) => {
            if let Some(rewritten) = component_id_map.get(text) {
                text.clone_from(rewritten);
                return;
            }
            static VARIABLE_REFERENCE: OnceLock<Regex> = OnceLock::new();
            let pattern = VARIABLE_REFERENCE.get_or_init(|| {
                Regex::new(r"(\{+\s*)([A-Za-z0-9:_-]+)(@[A-Za-z0-9_.-]+)(\s*\}+)")
                    .expect("legacy chunker variable reference pattern is valid")
            });
            *text = pattern
                .replace_all(text, |captures: &regex::Captures<'_>| {
                    let component_id = captures.get(2).map_or("", |value| value.as_str());
                    format!(
                        "{}{}{}{}",
                        captures.get(1).map_or("", |value| value.as_str()),
                        component_id_map
                            .get(component_id)
                            .map_or(component_id, String::as_str),
                        captures.get(3).map_or("", |value| value.as_str()),
                        captures.get(4).map_or("", |value| value.as_str())
                    )
                })
                .into_owned();
        }
        Value::Array(values) => {
            for value in values {
                rewrite_legacy_chunker_value(value, component_id_map);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_legacy_chunker_value(value, component_id_map);
            }
        }
        _ => {}
    }
}

fn enforce_canvas_handle_ids(root: &mut Map<String, Value>) {
    let Some(edges) = root
        .get_mut("graph")
        .and_then(Value::as_object_mut)
        .and_then(|graph| graph.get_mut("edges"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for edge in edges.iter_mut().filter_map(Value::as_object_mut) {
        if edge
            .get("sourceHandle")
            .and_then(Value::as_str)
            .is_some_and(|handle| matches!(handle, "start" | "end"))
        {
            edge.insert("sourceHandle".into(), Value::String("start".into()));
        }
        if edge
            .get("targetHandle")
            .and_then(Value::as_str)
            .is_some_and(|handle| matches!(handle, "start" | "end"))
        {
            edge.insert("targetHandle".into(), Value::String("end".into()));
        }
    }
}

fn canvas_graph_has_nodes(root: &Map<String, Value>) -> bool {
    root.get("graph")
        .and_then(Value::as_object)
        .and_then(|graph| graph.get("nodes"))
        .and_then(Value::as_array)
        .is_some_and(|nodes| !nodes.is_empty())
}

fn build_canvas_graph_from_components(
    components: &Map<String, Value>,
) -> (Vec<Value>, Vec<Value>, Map<String, Value>) {
    let mut keys: Vec<_> = components.keys().collect();
    keys.sort_unstable();
    let mut nodes = Vec::with_capacity(components.len());
    let mut edges = Vec::new();
    let mut normalized = Map::new();

    for key in keys {
        let Some(component) = components.get(key).and_then(Value::as_object) else {
            continue;
        };
        let index = nodes.len();
        let (mut name, params, downstream) = canvas_component_parts(component);
        if name.is_empty() {
            name.clone_from(key);
        }
        let parent_id = component
            .get("parent_id")
            .and_then(Value::as_str)
            .filter(|parent_id| !parent_id.is_empty());
        let mut graph_node = serde_json::json!({
            "id": key,
            "type": canvas_component_node_type(&name),
            "position": {"x": 50.0 + index as f64 * 350.0, "y": 200.0},
            "data": {"label": name, "name": name, "form": params},
            "sourcePosition": "right",
            "targetPosition": "left"
        });
        if let Some(parent_id) = parent_id {
            graph_node
                .as_object_mut()
                .expect("generated Canvas node is an object")
                .insert("parentId".into(), Value::String(parent_id.to_owned()));
        }
        nodes.push(graph_node);
        for destination in &downstream {
            edges.push(serde_json::json!({
                "id": format!("xy-edge__{key}-{destination}"),
                "source": key,
                "target": destination,
                "sourceHandle": "start",
                "targetHandle": "end"
            }));
        }
        let mut normalized_component = serde_json::json!({
            "id": key,
            "name": name,
            "downstream": downstream,
            "upstream": canvas_string_array(component.get("upstream")),
            "params": params
        });
        if let Some(parent_id) = parent_id {
            normalized_component
                .as_object_mut()
                .expect("generated Canvas component is an object")
                .insert("parent_id".into(), Value::String(parent_id.to_owned()));
        }
        normalized.insert(key.clone(), normalized_component);
    }
    (nodes, edges, normalized)
}

fn canvas_component_parts(
    component: &Map<String, Value>,
) -> (String, Map<String, Value>, Vec<String>) {
    let object = component.get("obj").and_then(Value::as_object);
    let name = object
        .and_then(|object| object.get("component_name"))
        .and_then(Value::as_str)
        .or_else(|| component.get("name").and_then(Value::as_str))
        .unwrap_or_default()
        .to_owned();
    let params = object
        .and_then(|object| object.get("params"))
        .and_then(Value::as_object)
        .or_else(|| component.get("params").and_then(Value::as_object))
        .cloned()
        .unwrap_or_default();
    let mut downstream = object
        .map(|object| canvas_string_array(object.get("downstream")))
        .unwrap_or_default();
    downstream.extend(canvas_string_array(component.get("downstream")));
    (name, params, downstream)
}

fn canvas_string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn repair_parallel_canvas_leaks(root: &mut Map<String, Value>) {
    if let Some(components) = root.get_mut("components").and_then(Value::as_object_mut) {
        for component in components.values_mut().filter_map(Value::as_object_mut) {
            if let Some(object) = component.get_mut("obj").and_then(Value::as_object_mut)
                && object.get("component_name").and_then(Value::as_str) == Some("Parallel")
            {
                object.insert("component_name".into(), Value::String("Iteration".into()));
            }
            if component.get("name").and_then(Value::as_str) == Some("Parallel") {
                component.insert("name".into(), Value::String("Iteration".into()));
            }
        }
    }
    let Some(nodes) = root
        .get_mut("graph")
        .and_then(Value::as_object_mut)
        .and_then(|graph| graph.get_mut("nodes"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for node in nodes.iter_mut().filter_map(Value::as_object_mut) {
        if node.get("type").and_then(Value::as_str) == Some("parallelNode") {
            node.insert("type".into(), Value::String("iterationNode".into()));
        }
        let Some(data) = node.get_mut("data").and_then(Value::as_object_mut) else {
            continue;
        };
        for field in ["label", "name"] {
            if data.get(field).and_then(Value::as_str) == Some("Parallel") {
                data.insert(field.into(), Value::String("Iteration".into()));
            }
        }
    }
}

fn fold_legacy_loop_variants(root: &mut Map<String, Value>) {
    let parent_by_child = canvas_parent_map(root);
    let Some(components) = root.get_mut("components").and_then(Value::as_object_mut) else {
        return;
    };
    let component_ids: Vec<_> = components.keys().cloned().collect();
    for child_id in &component_ids {
        let Some(child) = components.get(child_id).and_then(Value::as_object) else {
            continue;
        };
        if !matches!(canvas_component_name(child), "LoopItem" | "IterationItem") {
            continue;
        }
        let Some(parent_id) = parent_by_child.get(child_id) else {
            continue;
        };
        let child_downstream = canvas_child_downstream(child);
        let Some(parent) = components.get_mut(parent_id).and_then(Value::as_object_mut) else {
            components.shift_remove(child_id);
            continue;
        };
        let parent_downstream = canvas_string_array(parent.get("downstream"));
        let mut downstream = if child_downstream.is_empty() {
            parent_downstream
        } else {
            let mut seen = HashSet::new();
            parent_downstream
                .into_iter()
                .chain(child_downstream)
                .filter(|destination| !destination.is_empty() && seen.insert(destination.clone()))
                .collect()
        };
        downstream.retain(|destination| destination != child_id);
        parent.insert(
            "downstream".into(),
            Value::Array(downstream.into_iter().map(Value::String).collect()),
        );
        for component in components.values_mut().filter_map(Value::as_object_mut) {
            let upstream = canvas_string_array(component.get("upstream"));
            if upstream.iter().any(|upstream| upstream == child_id) {
                let mut seen = HashSet::new();
                component.insert(
                    "upstream".into(),
                    Value::Array(
                        upstream
                            .into_iter()
                            .map(|upstream| {
                                if upstream == *child_id {
                                    parent_id.clone()
                                } else {
                                    upstream
                                }
                            })
                            .filter(|upstream| seen.insert(upstream.clone()))
                            .map(Value::String)
                            .collect(),
                    ),
                );
            }
            let downstream = canvas_string_array(component.get("downstream"));
            if downstream.iter().any(|downstream| downstream == child_id) {
                let mut seen = HashSet::new();
                component.insert(
                    "downstream".into(),
                    Value::Array(
                        downstream
                            .into_iter()
                            .map(|downstream| {
                                if downstream == *child_id {
                                    parent_id.clone()
                                } else {
                                    downstream
                                }
                            })
                            .filter(|downstream| seen.insert(downstream.clone()))
                            .map(Value::String)
                            .collect(),
                    ),
                );
            }
        }
        components.shift_remove(child_id);
    }

    let mut renamed = Vec::new();
    for (component_id, component) in components.iter_mut() {
        let Some(component) = component.as_object_mut() else {
            continue;
        };
        if canvas_component_name(component) != "Iteration" {
            continue;
        }
        if let Some(object) = component.get_mut("obj").and_then(Value::as_object_mut) {
            object.insert("component_name".into(), Value::String("Parallel".into()));
        }
        component.insert("name".into(), Value::String("Parallel".into()));
        renamed.push(component_id.clone());
    }
    let renamed: HashSet<_> = renamed.into_iter().collect();
    let Some(nodes) = root
        .get_mut("graph")
        .and_then(Value::as_object_mut)
        .and_then(|graph| graph.get_mut("nodes"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for node in nodes.iter_mut().filter_map(Value::as_object_mut) {
        if !node
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| renamed.contains(id))
        {
            continue;
        }
        node.insert("type".into(), Value::String("parallelNode".into()));
        if let Some(data) = node.get_mut("data").and_then(Value::as_object_mut) {
            data.insert("label".into(), Value::String("Parallel".into()));
            data.insert("name".into(), Value::String("Parallel".into()));
        }
    }
}

fn canvas_parent_map(root: &Map<String, Value>) -> BTreeMap<String, String> {
    root.get("graph")
        .and_then(Value::as_object)
        .and_then(|graph| graph.get("nodes"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|node| {
            Some((
                node.get("id")?.as_str()?.to_owned(),
                node.get("parentId")?.as_str()?.to_owned(),
            ))
        })
        .filter(|(id, parent)| !id.is_empty() && !parent.is_empty())
        .collect()
}

fn canvas_component_name(component: &Map<String, Value>) -> &str {
    component
        .get("obj")
        .and_then(Value::as_object)
        .and_then(|object| object.get("component_name"))
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .or_else(|| component.get("name").and_then(Value::as_str))
        .unwrap_or_default()
}

fn canvas_child_downstream(component: &Map<String, Value>) -> Vec<String> {
    if component.contains_key("downstream") {
        return canvas_string_array(component.get("downstream"));
    }
    component
        .get("obj")
        .and_then(Value::as_object)
        .map(|object| canvas_string_array(object.get("downstream")))
        .unwrap_or_default()
}

fn rewrite_legacy_iteration_aliases(value: &mut Value) {
    match value {
        Value::String(text) => {
            static LEGACY_ALIAS: OnceLock<Regex> = OnceLock::new();
            let pattern = LEGACY_ALIAS.get_or_init(|| {
                Regex::new(r"(?i)IterationItem:[A-Za-z0-9_:-]+@(item|index|result)\b")
                    .expect("legacy iteration alias pattern is valid")
            });
            *text = pattern
                .replace_all(text, |captures: &regex::Captures<'_>| {
                    captures
                        .get(1)
                        .map(|alias| {
                            if alias.as_str().eq_ignore_ascii_case("result") {
                                "item".to_owned()
                            } else {
                                alias.as_str().to_ascii_lowercase()
                            }
                        })
                        .unwrap_or_default()
                })
                .into_owned();
        }
        Value::Array(values) => {
            for value in values {
                rewrite_legacy_iteration_aliases(value);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_legacy_iteration_aliases(value);
            }
        }
        _ => {}
    }
}

fn canvas_component_node_type(name: &str) -> &'static str {
    match name {
        "Begin" => "beginNode",
        "Retrieval" => "ragNode",
        "Categorize" => "categorizeNode",
        "Message" | "Answer" => "messageNode",
        "RewriteQuestion" => "rewriteNode",
        "DocGenerator" | "DocsGenerator" => "ragNode",
        "ExeSQL" | "Tool" | "Code" => "toolNode",
        "Switch" => "switchNode",
        "File" => "fileNode",
        "Parser" => "parserNode",
        "Tokenizer" => "tokenizerNode",
        "TokenChunker" | "TitleChunker" => "chunkerNode",
        "Extractor" => "contextNode",
        "Loop" => "loopNode",
        "LoopStart" => "loopStartNode",
        "ExitLoop" => "exitLoopNode",
        "Iteration" => "iterationNode",
        "IterationStart" => "iterationStartNode",
        "Parallel" => "parallelNode",
        "DataOperations" => "dataOperationsNode",
        "ListOperations" => "listOperationsNode",
        "VariableAssigner" => "variableAssignerNode",
        "VariableAggregator" => "variableAggregatorNode",
        "Keyword" => "keywordNode",
        "Note" => "noteNode",
        "Placeholder" => "placeholderNode",
        _ => "agentNode",
    }
}

/// Return an owned Agent DSL snapshot with per-run Canvas state reset.
///
/// Structural graph fields are preserved. Runtime accumulators become fresh
/// arrays, `sys.*` globals are zeroed by their JSON type, and `env.*` globals
/// are restored from the matching `variables[name]` declaration.
pub fn reset_agent_dsl(dsl: &Value) -> Value {
    let Some(source) = dsl.as_object() else {
        return Value::Object(Map::new());
    };
    let mut reset = source.clone();
    for field in ["history", "retrieval", "memory", "path"] {
        reset.insert(field.into(), Value::Array(Vec::new()));
    }

    let variables = reset
        .get("variables")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let Some(globals) = reset.get_mut("globals").and_then(Value::as_object_mut) else {
        return Value::Object(reset);
    };
    for (name, value) in globals {
        if name.strip_prefix("sys.").is_some() {
            *value = zero_canvas_value(value);
            continue;
        }
        let Some(variable_name) = name.strip_prefix("env.") else {
            continue;
        };
        *value = variables
            .get(variable_name)
            .and_then(Value::as_object)
            .map_or_else(
                || Value::String(String::new()),
                |variable| {
                    variable
                        .get("value")
                        .filter(|default| !default.is_null())
                        .cloned()
                        .unwrap_or_else(|| zero_canvas_variable(variable))
                },
            );
    }
    Value::Object(reset)
}

fn zero_canvas_value(value: &Value) -> Value {
    match value {
        Value::Null => Value::Null,
        Value::Bool(_) => Value::Bool(false),
        Value::Number(number) if number.is_f64() => {
            Value::Number(Number::from_f64(0.0).expect("zero is finite"))
        }
        Value::Number(_) => Value::Number(Number::from(0)),
        Value::String(_) => Value::String(String::new()),
        Value::Array(_) => Value::Array(Vec::new()),
        Value::Object(_) => Value::Object(Map::new()),
    }
}

fn zero_canvas_variable(variable: &Map<String, Value>) -> Value {
    match variable.get("type").and_then(Value::as_str) {
        Some("number") => Value::Number(Number::from(0)),
        Some("boolean") => Value::Bool(false),
        Some("object") => Value::Object(Map::new()),
        Some(kind) if kind.starts_with("array") => Value::Array(Vec::new()),
        _ => Value::String(String::new()),
    }
}

impl AgentWorkflow {
    /// Compile a visual canvas. `Ok(None)` means this is a legacy prompt-only
    /// agent and the caller should use its `prompt_template`.
    pub fn from_value(dsl: &Value) -> Result<Option<Self>> {
        let normalized = normalize_agent_dsl_for_run(dsl);
        let Some(root) = normalized.as_object() else {
            bail!("Agent DSL must be a JSON object");
        };
        let Some(raw_components) = root.get("components") else {
            // Historical canvas snapshots may contain opaque metadata without
            // a graph. They retain the prompt-only execution path until a
            // visual editor explicitly writes `components`.
            return Ok(None);
        };
        if raw_components.as_array().is_some_and(Vec::is_empty) {
            return Ok(None);
        }
        let components = raw_components
            .as_object()
            .ok_or_else(|| anyhow!("Agent DSL components must be an object"))?;
        if components.is_empty() {
            return Ok(None);
        }

        let graph_parents = canvas_parent_map(root);
        let mut nodes = BTreeMap::new();
        for (id, raw_node) in components {
            validate_identifier(id, "component id")?;
            let node = raw_node
                .as_object()
                .ok_or_else(|| anyhow!("Canvas component '{id}' must be an object"))?;
            let object = node.get("obj").and_then(Value::as_object);
            let component_name = object
                .and_then(|object| object.get("component_name"))
                .and_then(Value::as_str)
                .or_else(|| node.get("name").and_then(Value::as_str))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("Canvas component '{id}' has no component_name"))?
                .to_owned();
            let raw_params = object
                .and_then(|object| object.get("params"))
                .or_else(|| node.get("params"));
            let params = match raw_params {
                Some(value) => value
                    .as_object()
                    .cloned()
                    .ok_or_else(|| anyhow!("Canvas component '{id}' params must be an object"))?,
                None => Map::new(),
            };
            let mut downstream = object
                .and_then(|object| object.get("downstream"))
                .map(|value| string_array(Some(value), id, "obj.downstream"))
                .transpose()?
                .unwrap_or_default();
            if downstream.is_empty() {
                downstream = string_array(node.get("downstream"), id, "downstream")?;
            }
            let upstream = string_array(node.get("upstream"), id, "upstream")?;
            let parent_id = node
                .get("parent_id")
                .map(|value| {
                    value.as_str().map(str::to_owned).ok_or_else(|| {
                        anyhow!("Canvas component '{id}' parent_id must be a string")
                    })
                })
                .transpose()?
                .or_else(|| graph_parents.get(id).cloned());
            nodes.insert(
                id.clone(),
                CanvasNode {
                    id: id.clone(),
                    component_name,
                    params,
                    downstream,
                    upstream,
                    parent_id,
                },
            );
        }

        for node in nodes.values() {
            for target in node
                .upstream
                .iter()
                .chain(node.downstream.iter())
                .chain(node.parent_id.iter())
            {
                if !nodes.contains_key(target) {
                    bail!(
                        "Canvas component '{}' references missing component '{}'",
                        node.id,
                        target
                    );
                }
            }
            if node.component_name.eq_ignore_ascii_case("switch") {
                validate_switch_destinations(node, &nodes)?;
            }
            if node.component_name.eq_ignore_ascii_case("categorize") {
                validate_categorize_destinations(node, &nodes)?;
            }
            if node.component_name.eq_ignore_ascii_case("retrieval") {
                validate_retrieval_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("tavilysearch") {
                validate_tavily_search_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("tavilyextract") {
                validate_tavily_extract_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("duckduckgo") {
                validate_duckduckgo_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("wikipedia") {
                validate_wikipedia_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("baike") {
                validate_baike_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("google") {
                validate_google_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("googlescholar") {
                validate_google_scholar_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("github") {
                validate_github_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("yahoofinance") {
                validate_yahoo_finance_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("arxiv") {
                validate_arxiv_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("pubmed") {
                validate_pubmed_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("begin") {
                validate_begin_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("message") {
                validate_message_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("invoke") {
                validate_invoke_params(node)?;
            }
            if matches!(
                node.component_name.to_ascii_lowercase().as_str(),
                "agent" | "generate" | "llm"
            ) {
                validate_llm_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("stringtransform") {
                validate_string_transform_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("listoperations") {
                validate_list_operations_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("dataoperations") {
                validate_data_operations_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("docgenerator")
                || node.component_name.eq_ignore_ascii_case("docsgenerator")
            {
                validate_doc_generator_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("excelprocessor") {
                validate_excel_processor_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("loop") {
                validate_loop_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("parallel") {
                validate_parallel_params(node)?;
            }
            if node.component_name.eq_ignore_ascii_case("userfillup") {
                validate_user_fill_up_params(node)?;
            }
        }

        let begin_ids: Vec<_> = nodes
            .values()
            .filter(|node| {
                node.component_name.eq_ignore_ascii_case("begin") && node.parent_id.is_none()
            })
            .map(|node| node.id.clone())
            .collect();
        if begin_ids.len() != 1 {
            bail!(
                "Agent canvas must contain exactly one top-level Begin component; found {}",
                begin_ids.len()
            );
        }
        let loop_plans = build_loop_plans(&nodes)?;
        let parallel_plans = build_parallel_plans(&nodes, &loop_plans)?;
        let execution_order = if nodes.values().all(canvas_component_is_supported) {
            canvas_topological_order(&nodes, &loop_plans, &parallel_plans)?
        } else {
            Vec::new()
        };

        let initial_globals = root
            .get("globals")
            .map(|value| {
                value
                    .as_object()
                    .cloned()
                    .ok_or_else(|| anyhow!("Agent DSL globals must be an object"))
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Some(Self {
            nodes,
            begin_id: begin_ids[0].clone(),
            execution_order,
            loop_plans,
            parallel_plans,
            initial_globals,
        }))
    }

    pub fn unsupported_components(&self) -> Vec<String> {
        let mut unsupported: Vec<_> = self
            .nodes
            .values()
            .filter(|node| !canvas_component_is_supported(node))
            .map(|node| format!("{} ({})", node.id, node.component_name))
            .collect();
        unsupported.sort();
        unsupported
    }

    pub async fn run(
        &self,
        llm: Option<&LlmClient>,
        input: WorkflowRunInput<'_>,
    ) -> Result<WorkflowRunResult> {
        match self.run_interactive(llm, input).await? {
            WorkflowRunOutcome::Completed(result) => Ok(result),
            WorkflowRunOutcome::WaitingForUser(waiting) => bail!(
                "Canvas paused at UserFillUp '{}'; use the interactive completion path to resume it",
                waiting.component_id
            ),
        }
    }

    /// Run until normal completion or the first outer/composite UserFillUp.
    pub async fn run_interactive(
        &self,
        llm: Option<&LlmClient>,
        input: WorkflowRunInput<'_>,
    ) -> Result<WorkflowRunOutcome> {
        self.run_interactive_with_observer(llm, input, None).await
    }

    pub async fn run_interactive_observed(
        &self,
        llm: Option<&LlmClient>,
        input: WorkflowRunInput<'_>,
        observer: &dyn WorkflowEventObserver,
    ) -> Result<WorkflowRunOutcome> {
        self.run_interactive_with_observer(llm, input, Some(observer))
            .await
    }

    async fn run_interactive_with_observer(
        &self,
        llm: Option<&LlmClient>,
        input: WorkflowRunInput<'_>,
        observer: Option<&dyn WorkflowEventObserver>,
    ) -> Result<WorkflowRunOutcome> {
        let metric = crate::metrics::CanvasRunMetricGuard::start();
        let result = self.run_interactive_inner(llm, input, None, observer).await;
        metric.finish(if result.is_ok() {
            crate::metrics::CanvasRunOutcome::Success
        } else {
            crate::metrics::CanvasRunOutcome::Error
        });
        result
    }

    /// Resume a server-side checkpoint with exactly one user-supplied value.
    /// The caller must claim and authenticate the checkpoint before invoking
    /// this method; persistence and consume-once coordination live outside the
    /// pure workflow executor.
    pub async fn resume_interactive(
        &self,
        llm: Option<&LlmClient>,
        input: WorkflowRunInput<'_>,
        checkpoint: AgentWorkflowCheckpoint,
        resume_data: Value,
    ) -> Result<WorkflowRunOutcome> {
        self.resume_interactive_with_observer(llm, input, checkpoint, resume_data, None)
            .await
    }

    pub async fn resume_interactive_observed(
        &self,
        llm: Option<&LlmClient>,
        input: WorkflowRunInput<'_>,
        checkpoint: AgentWorkflowCheckpoint,
        resume_data: Value,
        observer: &dyn WorkflowEventObserver,
    ) -> Result<WorkflowRunOutcome> {
        self.resume_interactive_with_observer(llm, input, checkpoint, resume_data, Some(observer))
            .await
    }

    async fn resume_interactive_with_observer(
        &self,
        llm: Option<&LlmClient>,
        input: WorkflowRunInput<'_>,
        checkpoint: AgentWorkflowCheckpoint,
        resume_data: Value,
        observer: Option<&dyn WorkflowEventObserver>,
    ) -> Result<WorkflowRunOutcome> {
        let metric = crate::metrics::CanvasRunMetricGuard::start();
        let result = self
            .run_interactive_inner(llm, input, Some((checkpoint, resume_data)), observer)
            .await;
        metric.finish(if result.is_ok() {
            crate::metrics::CanvasRunOutcome::Success
        } else {
            crate::metrics::CanvasRunOutcome::Error
        });
        result
    }

    async fn run_interactive_inner(
        &self,
        llm: Option<&LlmClient>,
        input: WorkflowRunInput<'_>,
        resume: Option<(AgentWorkflowCheckpoint, Value)>,
        observer: Option<&dyn WorkflowEventObserver>,
    ) -> Result<WorkflowRunOutcome> {
        let unsupported = self.unsupported_components();
        if !unsupported.is_empty() {
            bail!(
                "Canvas contains unsupported components: {}",
                unsupported.join(", ")
            );
        }

        let (mut runtime, mut selected, mut trace, start_index, mut composite_resume) =
            if let Some((checkpoint, resume_data)) = resume {
                if checkpoint.version != 1 {
                    bail!(
                        "Unsupported Agent checkpoint version {}",
                        checkpoint.version
                    );
                }
                if let Some(composite) = checkpoint.composite {
                    let macro_id = composite.macro_component_id();
                    if composite.waiting_component_id()
                        != Some(checkpoint.waiting_component_id.as_str())
                    {
                        bail!(
                            "Agent composite checkpoint leaf does not match waiting component '{}'",
                            checkpoint.waiting_component_id
                        );
                    }
                    let node_id = self
                        .execution_order
                        .get(checkpoint.next_execution_index)
                        .filter(|node_id| node_id.as_str() == macro_id)
                        .ok_or_else(|| {
                            anyhow!(
                                "Agent composite checkpoint cursor does not match macro component '{}'",
                                macro_id
                            )
                        })?;
                    let selected: HashSet<String> = checkpoint.selected.into_iter().collect();
                    if !selected.contains(node_id) {
                        bail!(
                            "Agent composite checkpoint macro component '{}' is not selected",
                            node_id
                        );
                    }
                    let waiting = self
                        .nodes
                        .get(&checkpoint.waiting_component_id)
                        .ok_or_else(|| {
                            anyhow!(
                                "Agent composite checkpoint references missing UserFillUp '{}'",
                                checkpoint.waiting_component_id
                            )
                        })?;
                    if !waiting.component_name.eq_ignore_ascii_case("userfillup") {
                        bail!(
                            "Agent composite checkpoint component '{}' is not UserFillUp",
                            checkpoint.waiting_component_id
                        );
                    }
                    let mut runtime = checkpoint.runtime;
                    checkpoint.transient.restore(&mut runtime);
                    runtime.history = input.history.to_vec();
                    runtime
                        .sys
                        .insert("query".into(), Value::String(String::new()));
                    (
                        runtime,
                        selected,
                        checkpoint.trace,
                        checkpoint.next_execution_index,
                        Some((composite, resume_data)),
                    )
                } else {
                    let node_id = self
                        .execution_order
                        .get(checkpoint.next_execution_index)
                        .filter(|node_id| **node_id == checkpoint.waiting_component_id)
                        .ok_or_else(|| {
                            anyhow!(
                                "Agent checkpoint cursor does not match waiting component '{}'",
                                checkpoint.waiting_component_id
                            )
                        })?
                        .clone();
                    let node = self.nodes.get(&node_id).ok_or_else(|| {
                        anyhow!(
                            "Agent checkpoint references missing UserFillUp '{}'",
                            checkpoint.waiting_component_id
                        )
                    })?;
                    if !node.component_name.eq_ignore_ascii_case("userfillup") {
                        bail!(
                            "Agent checkpoint component '{}' is not UserFillUp",
                            checkpoint.waiting_component_id
                        );
                    }
                    let mut selected: HashSet<String> = checkpoint.selected.into_iter().collect();
                    if !selected.contains(&node_id) {
                        bail!(
                            "Agent checkpoint waiting component '{}' is not selected",
                            checkpoint.waiting_component_id
                        );
                    }
                    let mut runtime = checkpoint.runtime;
                    checkpoint.transient.restore(&mut runtime);
                    runtime.history = input.history.to_vec();
                    // A resume payload belongs only to the paused node; it must not
                    // be rediscovered as a fresh initial sys.query by a later
                    // UserFillUp in the same graph.
                    runtime
                        .sys
                        .insert("query".into(), Value::String(String::new()));
                    let node_inputs = workflow_node_event_inputs(&runtime, node, &input);
                    let node_started_at = emit_workflow_node_started(observer, node, &node_inputs);
                    if let Err(error) = execute_user_fill_up_resume(&mut runtime, node, resume_data)
                    {
                        emit_workflow_node_finished(
                            observer,
                            node,
                            &node_inputs,
                            &runtime,
                            node_started_at,
                            Some(&error),
                        );
                        return Err(error);
                    }
                    emit_workflow_node_finished(
                        observer,
                        node,
                        &node_inputs,
                        &runtime,
                        node_started_at,
                        None,
                    );
                    runtime.path.push(node_id.clone());
                    let mut trace = checkpoint.trace;
                    trace.push(WorkflowNodeTrace {
                        component_id: node_id.clone(),
                        component_type: node.component_name.clone(),
                        outputs: runtime.outputs.get(&node_id).cloned().unwrap_or_default(),
                    });
                    for target in node.downstream.iter().cloned() {
                        selected.insert(target);
                    }
                    (
                        runtime,
                        selected,
                        trace,
                        checkpoint.next_execution_index.saturating_add(1),
                        None,
                    )
                }
            } else {
                let mut runtime = CanvasRuntime {
                    globals: self.initial_globals.clone(),
                    history: input.history.to_vec(),
                    ..Default::default()
                };
                runtime
                    .sys
                    .insert("query".into(), Value::String(input.question.to_owned()));
                runtime
                    .sys
                    .insert("user_id".into(), Value::String(input.user_id.to_owned()));
                runtime.sys.insert(
                    "conversation_turns".into(),
                    Value::Number(Number::from(
                        input
                            .history
                            .iter()
                            .filter(|message| message.role == "user")
                            .count()
                            .saturating_add(1) as u64,
                    )),
                );
                (
                    runtime,
                    HashSet::from([self.begin_id.clone()]),
                    Vec::new(),
                    0,
                    None,
                )
            };

        for (execution_index, node_id) in self.execution_order.iter().enumerate().skip(start_index)
        {
            if !selected.contains(node_id) {
                continue;
            }
            if runtime.cancel_flag {
                bail!("Canvas run cancelled");
            }
            let node = self
                .nodes
                .get(node_id)
                .with_context(|| format!("Canvas execution referenced missing node '{node_id}'"))?;
            let node_inputs = workflow_node_event_inputs(&runtime, node, &input);
            let node_started_at = emit_workflow_node_started(observer, node, &node_inputs);
            let next = if node.component_name.eq_ignore_ascii_case("userfillup") {
                let consumed = match execute_user_fill_up_initial(&mut runtime, node, &input) {
                    Ok(consumed) => consumed,
                    Err(error) => {
                        emit_workflow_node_finished(
                            observer,
                            node,
                            &node_inputs,
                            &runtime,
                            node_started_at,
                            Some(&error),
                        );
                        return Err(error);
                    }
                };
                if !consumed {
                    let selected = selected.iter().cloned().collect::<BTreeSet<_>>();
                    let checkpoint = AgentWorkflowCheckpoint {
                        version: 1,
                        waiting_component_id: node.id.clone(),
                        next_execution_index: execution_index,
                        selected: selected.into_iter().collect(),
                        composite: None,
                        transient: CanvasCheckpointTransient::capture(&runtime),
                        runtime: runtime.clone(),
                        trace: trace.clone(),
                    };
                    let waiting = workflow_waiting_for_user(&runtime, node, trace, checkpoint);
                    return Ok(WorkflowRunOutcome::WaitingForUser(Box::new(waiting)));
                }
                None
            } else if let Some(plan) = self.loop_plans.get(node_id) {
                let resume = match composite_resume.take() {
                    Some((CompositeCheckpoint::Loop(state), data)) => Some((state, data)),
                    Some((other, _)) => {
                        bail!(
                            "Agent composite checkpoint kind does not match Loop '{}': '{}'",
                            node.id,
                            other.macro_component_id()
                        )
                    }
                    None => None,
                };
                let loop_outcome = match execute_loop_with_timeout(
                    &mut runtime,
                    node,
                    plan,
                    &self.nodes,
                    llm,
                    &input,
                    resume,
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        emit_workflow_node_finished(
                            observer,
                            node,
                            &node_inputs,
                            &runtime,
                            node_started_at,
                            Some(&error),
                        );
                        return Err(error);
                    }
                };
                if let CompositeExecutionOutcome::Waiting(waiting) = loop_outcome {
                    let selected = selected.iter().cloned().collect::<BTreeSet<_>>();
                    let leaf = self.nodes.get(&waiting.leaf_component_id).ok_or_else(|| {
                        anyhow!(
                            "Canvas Loop '{}' paused at missing UserFillUp '{}'",
                            node.id,
                            waiting.leaf_component_id
                        )
                    })?;
                    let checkpoint = AgentWorkflowCheckpoint {
                        version: 1,
                        waiting_component_id: waiting.leaf_component_id,
                        next_execution_index: execution_index,
                        selected: selected.into_iter().collect(),
                        composite: Some(waiting.state),
                        transient: CanvasCheckpointTransient::capture(&runtime),
                        runtime: runtime.clone(),
                        trace: trace.clone(),
                    };
                    let mut waiting_for_user =
                        workflow_waiting_for_user(&runtime, leaf, trace, checkpoint);
                    waiting_for_user.interrupt_id = waiting.interrupt_id;
                    return Ok(WorkflowRunOutcome::WaitingForUser(Box::new(
                        waiting_for_user,
                    )));
                }
                None
            } else if let Some(plan) = self.parallel_plans.get(node_id) {
                let resume = match composite_resume.take() {
                    Some((CompositeCheckpoint::Parallel(state), data)) => Some((state, data)),
                    Some((other, _)) => {
                        bail!(
                            "Agent composite checkpoint kind does not match Parallel '{}': '{}'",
                            node.id,
                            other.macro_component_id()
                        )
                    }
                    None => None,
                };
                let parallel_outcome = match execute_parallel_with_timeout(
                    &mut runtime,
                    node,
                    plan,
                    &self.nodes,
                    llm,
                    &input,
                    resume,
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        emit_workflow_node_finished(
                            observer,
                            node,
                            &node_inputs,
                            &runtime,
                            node_started_at,
                            Some(&error),
                        );
                        return Err(error);
                    }
                };
                if let CompositeExecutionOutcome::Waiting(waiting) = parallel_outcome {
                    let selected = selected.iter().cloned().collect::<BTreeSet<_>>();
                    let leaf = self.nodes.get(&waiting.leaf_component_id).ok_or_else(|| {
                        anyhow!(
                            "Canvas Parallel '{}' paused at missing UserFillUp '{}'",
                            node.id,
                            waiting.leaf_component_id
                        )
                    })?;
                    let checkpoint = AgentWorkflowCheckpoint {
                        version: 1,
                        waiting_component_id: waiting.leaf_component_id,
                        next_execution_index: execution_index,
                        selected: selected.into_iter().collect(),
                        composite: Some(waiting.state),
                        transient: CanvasCheckpointTransient::capture(&runtime),
                        runtime: runtime.clone(),
                        trace: trace.clone(),
                    };
                    let mut waiting_for_user =
                        workflow_waiting_for_user(&runtime, leaf, trace, checkpoint);
                    waiting_for_user.interrupt_id = waiting.interrupt_id;
                    return Ok(WorkflowRunOutcome::WaitingForUser(Box::new(
                        waiting_for_user,
                    )));
                }
                None
            } else {
                match execute_node_with_timeout(&mut runtime, node, llm, &input).await {
                    Ok(next) => next,
                    Err(error) => {
                        emit_workflow_node_finished(
                            observer,
                            node,
                            &node_inputs,
                            &runtime,
                            node_started_at,
                            Some(&error),
                        );
                        return Err(error);
                    }
                }
            };
            emit_workflow_node_finished(
                observer,
                node,
                &node_inputs,
                &runtime,
                node_started_at,
                None,
            );
            let outputs = runtime.outputs.get(node_id).cloned().unwrap_or_default();
            runtime.path.push(node_id.clone());
            trace.push(WorkflowNodeTrace {
                component_id: node_id.clone(),
                component_type: node.component_name.clone(),
                outputs,
            });
            let default_next = || {
                self.loop_plans
                    .get(node_id)
                    .or_else(|| self.parallel_plans.get(node_id))
                    .map(|plan| plan.outer_downstream.clone())
                    .unwrap_or_else(|| {
                        node.downstream
                            .iter()
                            .filter(|target| {
                                !self
                                    .loop_plans
                                    .values()
                                    .chain(self.parallel_plans.values())
                                    .any(|plan| plan.members.contains(target.as_str()))
                            })
                            .cloned()
                            .collect()
                    })
            };
            for target in next.unwrap_or_else(default_next) {
                selected.insert(target);
            }
        }

        let answer = runtime
            .last_message
            .or_else(|| {
                runtime.path.iter().rev().find_map(|id| {
                    runtime
                        .outputs
                        .get(id)
                        .and_then(|outputs| outputs.get("content"))
                        .map(stringify)
                })
            })
            .filter(|answer| !answer.is_empty())
            .ok_or_else(|| anyhow!("Canvas completed without a Message or content output"))?;
        let usage = (runtime.provider_calls > 0).then_some(runtime.usage);
        Ok(WorkflowRunOutcome::Completed(WorkflowRunResult {
            answer,
            usage,
            references: runtime.references,
            path: runtime.path,
            trace,
        }))
    }
}

fn workflow_node_event_inputs(
    runtime: &CanvasRuntime,
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
) -> Map<String, Value> {
    let mut inputs = Map::new();
    if node.upstream.is_empty() {
        inputs.insert("query".into(), Value::String(input.question.to_owned()));
    }
    for upstream in &node.upstream {
        let Some(outputs) = runtime.outputs.get(upstream) else {
            continue;
        };
        for (key, value) in outputs {
            inputs.entry(key.clone()).or_insert_with(|| value.clone());
        }
    }
    if inputs.is_empty()
        && let Some(query) = runtime.sys.get("query")
    {
        inputs.insert("query".into(), query.clone());
    }
    inputs
}

fn emit_workflow_node_started(
    observer: Option<&dyn WorkflowEventObserver>,
    node: &CanvasNode,
    inputs: &Map<String, Value>,
) -> Option<Instant> {
    let observer = observer?;
    let started_at = Instant::now();
    observer.emit(WorkflowLifecycleEvent {
        event: "node_started",
        data: serde_json::json!({
            "inputs": inputs,
            "created_at": workflow_event_time_f64(),
            "component_id": node.id,
            "component_name": node.component_name,
            "component_type": node.component_name,
            "thoughts": ""
        }),
    });
    Some(started_at)
}

fn emit_workflow_node_finished(
    observer: Option<&dyn WorkflowEventObserver>,
    node: &CanvasNode,
    inputs: &Map<String, Value>,
    runtime: &CanvasRuntime,
    started_at: Option<Instant>,
    error: Option<&anyhow::Error>,
) {
    let Some(observer) = observer else {
        return;
    };
    let outputs = runtime.outputs.get(&node.id).cloned();
    observer.emit(WorkflowLifecycleEvent {
        event: "node_finished",
        data: serde_json::json!({
            "inputs": inputs,
            "outputs": outputs,
            "component_id": node.id,
            "component_name": node.component_name,
            "component_type": node.component_name,
            "error": error.map(ToString::to_string),
            "elapsed_time": started_at.map(|started_at| started_at.elapsed().as_secs_f64()).unwrap_or_default(),
            "created_at": workflow_event_time_f64()
        }),
    });
}

fn workflow_event_time_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

fn canvas_component_is_supported(node: &CanvasNode) -> bool {
    node.component_name.eq_ignore_ascii_case("exitloop")
        || crate::runtime::is_supported_agent_component(&node.component_name)
}

fn build_loop_plans(
    nodes: &BTreeMap<String, CanvasNode>,
) -> Result<BTreeMap<String, SubgraphPlan>> {
    let mut plans = BTreeMap::new();
    let mut claimed = HashSet::new();
    for loop_node in nodes
        .values()
        .filter(|node| node.component_name.eq_ignore_ascii_case("loop") && node.parent_id.is_none())
    {
        let mut members: HashSet<String> = nodes
            .values()
            .filter(|node| node.parent_id.as_deref() == Some(loop_node.id.as_str()))
            .map(|node| node.id.clone())
            .collect();
        if members.is_empty() {
            members = collect_macro_descendants(nodes, &loop_node.id, "Loop")?;
        }
        for member in &members {
            if !claimed.insert(member.clone()) {
                bail!(
                    "Canvas Loop '{}' body member '{}' is already owned by another runtime macro",
                    loop_node.id,
                    member
                );
            }
            if nodes.get(member).is_some_and(|node| {
                node.component_name.eq_ignore_ascii_case("loop")
                    || node.component_name.eq_ignore_ascii_case("parallel")
            }) {
                bail!(
                    "Canvas Loop '{}' contains nested runtime macro '{}'; nested Loop/Parallel execution is not implemented",
                    loop_node.id,
                    member
                );
            }
        }
        let execution_order =
            canvas_subset_topological_order(nodes, &members, &loop_node.id, "Loop")?;
        let outer_downstream = loop_node
            .downstream
            .iter()
            .filter(|target| !members.contains(*target))
            .cloned()
            .collect();
        plans.insert(
            loop_node.id.clone(),
            SubgraphPlan {
                members,
                execution_order,
                outer_downstream,
            },
        );
    }
    Ok(plans)
}

fn build_parallel_plans(
    nodes: &BTreeMap<String, CanvasNode>,
    loop_plans: &BTreeMap<String, SubgraphPlan>,
) -> Result<BTreeMap<String, SubgraphPlan>> {
    let mut plans = BTreeMap::new();
    let mut claimed: HashSet<String> = loop_plans
        .values()
        .flat_map(|plan| plan.members.iter().cloned())
        .collect();
    for parallel_node in nodes.values().filter(|node| {
        node.component_name.eq_ignore_ascii_case("parallel") && node.parent_id.is_none()
    }) {
        let mut members: HashSet<String> = nodes
            .values()
            .filter(|node| node.parent_id.as_deref() == Some(parallel_node.id.as_str()))
            .map(|node| node.id.clone())
            .collect();
        if members.is_empty() {
            members = collect_macro_descendants(nodes, &parallel_node.id, "Parallel")?;
        }
        if members.is_empty() {
            bail!(
                "Canvas Parallel '{}' must contain at least one body component",
                parallel_node.id
            );
        }
        for member in &members {
            if !claimed.insert(member.clone()) {
                bail!(
                    "Canvas Parallel '{}' body member '{}' is already owned by another runtime macro",
                    parallel_node.id,
                    member
                );
            }
            if nodes.get(member).is_some_and(|node| {
                node.component_name.eq_ignore_ascii_case("loop")
                    || node.component_name.eq_ignore_ascii_case("parallel")
            }) {
                bail!(
                    "Canvas Parallel '{}' contains nested runtime macro '{}'; nested Loop/Parallel execution is not implemented",
                    parallel_node.id,
                    member
                );
            }
        }
        let execution_order =
            canvas_subset_topological_order(nodes, &members, &parallel_node.id, "Parallel")?;
        let outer_downstream = parallel_node
            .downstream
            .iter()
            .filter(|target| !members.contains(*target))
            .cloned()
            .collect();
        plans.insert(
            parallel_node.id.clone(),
            SubgraphPlan {
                members,
                execution_order,
                outer_downstream,
            },
        );
    }
    Ok(plans)
}

fn collect_macro_descendants(
    nodes: &BTreeMap<String, CanvasNode>,
    macro_id: &str,
    macro_name: &str,
) -> Result<HashSet<String>> {
    let macro_node = nodes
        .get(macro_id)
        .ok_or_else(|| anyhow!("Canvas {macro_name} '{macro_id}' does not exist"))?;
    let mut members = HashSet::new();
    let mut queue = VecDeque::new();
    for target in &macro_node.downstream {
        if target != macro_id && members.insert(target.clone()) {
            queue.push_back(target.clone());
        }
    }
    while let Some(node_id) = queue.pop_front() {
        let node = nodes.get(&node_id).ok_or_else(|| {
            anyhow!("Canvas {macro_name} '{macro_id}' references missing body member '{node_id}'")
        })?;
        for target in &node.downstream {
            if target != macro_id && target != &node_id && members.insert(target.clone()) {
                queue.push_back(target.clone());
            }
        }
    }
    Ok(members)
}

fn canvas_subset_topological_order(
    nodes: &BTreeMap<String, CanvasNode>,
    members: &HashSet<String>,
    macro_id: &str,
    macro_name: &str,
) -> Result<Vec<String>> {
    let mut indegree: BTreeMap<String, usize> = members.iter().cloned().map(|id| (id, 0)).collect();
    for member in members {
        let node = nodes.get(member).ok_or_else(|| {
            anyhow!("Canvas {macro_name} '{macro_id}' body member '{member}' is missing")
        })?;
        for target in &node.downstream {
            if let Some(degree) = indegree.get_mut(target) {
                *degree = degree.saturating_add(1);
            }
        }
    }
    let mut ready: VecDeque<String> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let mut order = Vec::with_capacity(members.len());
    while let Some(node_id) = ready.pop_front() {
        order.push(node_id.clone());
        let node = nodes
            .get(&node_id)
            .expect("runtime-macro topological queue contains only known Canvas nodes");
        for target in &node.downstream {
            let Some(degree) = indegree.get_mut(target) else {
                continue;
            };
            *degree = degree.saturating_sub(1);
            if *degree == 0 {
                ready.push_back(target.clone());
            }
        }
    }
    if order.len() != members.len() {
        let unresolved = indegree
            .into_iter()
            .filter_map(|(id, degree)| (degree > 0).then_some(id))
            .collect::<Vec<_>>();
        bail!(
            "Canvas {} '{}' body contains a cycle; unresolved components: {}",
            macro_name,
            macro_id,
            unresolved.join(", ")
        );
    }
    Ok(order)
}

fn canvas_topological_order(
    nodes: &BTreeMap<String, CanvasNode>,
    loop_plans: &BTreeMap<String, SubgraphPlan>,
    parallel_plans: &BTreeMap<String, SubgraphPlan>,
) -> Result<Vec<String>> {
    let members: HashSet<_> = loop_plans
        .values()
        .chain(parallel_plans.values())
        .flat_map(|plan| plan.members.iter().cloned())
        .collect();
    let mut indegree: BTreeMap<String, usize> = nodes
        .keys()
        .filter(|id| !members.contains(*id))
        .cloned()
        .map(|id| (id, 0))
        .collect();
    for node in nodes.values().filter(|node| !members.contains(&node.id)) {
        let downstream = loop_plans
            .get(&node.id)
            .or_else(|| parallel_plans.get(&node.id))
            .map(|plan| plan.outer_downstream.as_slice())
            .unwrap_or(&node.downstream);
        for target in downstream {
            if members.contains(target) {
                continue;
            }
            let degree = indegree.get_mut(target).ok_or_else(|| {
                anyhow!(
                    "Canvas component '{}' references missing component '{}'",
                    node.id,
                    target
                )
            })?;
            *degree = degree.saturating_add(1);
        }
    }

    let mut ready: VecDeque<String> = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| id.clone())
        .collect();
    let outer_count = indegree.len();
    let mut order = Vec::with_capacity(outer_count);
    while let Some(node_id) = ready.pop_front() {
        order.push(node_id.clone());
        let node = nodes
            .get(&node_id)
            .expect("topological queue contains only known Canvas nodes");
        let downstream = loop_plans
            .get(&node_id)
            .or_else(|| parallel_plans.get(&node_id))
            .map(|plan| plan.outer_downstream.as_slice())
            .unwrap_or(&node.downstream);
        for target in downstream {
            if members.contains(target) {
                continue;
            }
            let degree = indegree
                .get_mut(target)
                .expect("Canvas destinations were validated before topological sorting");
            *degree = degree.saturating_sub(1);
            if *degree == 0 {
                ready.push_back(target.clone());
            }
        }
    }

    if order.len() != outer_count {
        let unresolved = indegree
            .into_iter()
            .filter_map(|(id, degree)| (degree > 0).then_some(id))
            .collect::<Vec<_>>();
        bail!(
            "Canvas graph contains a cycle; unresolved components: {}",
            unresolved.join(", ")
        );
    }
    Ok(order)
}

async fn execute_parallel_with_timeout(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    plan: &SubgraphPlan,
    nodes: &BTreeMap<String, CanvasNode>,
    llm: Option<&LlmClient>,
    input: &WorkflowRunInput<'_>,
    resume: Option<(ParallelCheckpoint, Value)>,
) -> Result<CompositeExecutionOutcome> {
    let timeout = component_timeout(&node.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_parallel(runtime, node, plan, nodes, llm, input, resume),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            Err(error).with_context(|| {
                format!(
                    "Canvas component '{}' ({}) timed out after {} seconds",
                    node.id,
                    node.component_name,
                    timeout.as_secs()
                )
            })
        }
        result => result,
    }
}

async fn execute_parallel(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    plan: &SubgraphPlan,
    nodes: &BTreeMap<String, CanvasNode>,
    llm: Option<&LlmClient>,
    input: &WorkflowRunInput<'_>,
    resume: Option<(ParallelCheckpoint, Value)>,
) -> Result<CompositeExecutionOutcome> {
    if runtime.cancel_flag {
        bail!("Canvas run cancelled");
    }
    let context = ParallelItemContext {
        parallel_node: node,
        plan,
        nodes,
        llm,
        input,
    };
    if let Some((mut state, resume_data)) = resume {
        validate_parallel_checkpoint(&state, node, plan, nodes)?;
        let active_index = state.active_index;
        let pending = state.pending_items.remove(&active_index).ok_or_else(|| {
            anyhow!(
                "Canvas Parallel '{}' checkpoint active index {} is not pending",
                node.id,
                active_index
            )
        })?;
        match resume_parallel_item(pending, active_index, context, resume_data).await? {
            ParallelItemOutcome::Completed(local) => {
                state
                    .completed_items
                    .insert(active_index, SavedCanvasRuntime::capture(&local));
            }
            ParallelItemOutcome::Waiting { cursor, runtime } => {
                state.pending_items.insert(
                    active_index,
                    ParallelPendingItem {
                        canvas: SavedCanvasRuntime::capture(&runtime),
                        cursor,
                    },
                );
            }
        }
        if let Some(next) = state.pending_items.keys().next().copied() {
            state.active_index = next;
            return parallel_waiting_outcome(state);
        }
        finish_parallel(
            runtime,
            node,
            state.original_items.len(),
            state.completed_items,
        )?;
        return Ok(CompositeExecutionOutcome::Completed);
    }

    let items_ref = node
        .params
        .get("items_ref")
        .and_then(Value::as_str)
        .expect("Parallel items_ref was validated during compilation");
    let raw_items = get_variable(runtime, items_ref).with_context(|| {
        format!(
            "Canvas Parallel '{}' failed to resolve items_ref '{}'",
            node.id, items_ref
        )
    })?;
    let items = match raw_items {
        Value::Null => Vec::new(),
        Value::Array(items) => items,
        value => bail!(
            "Canvas Parallel '{}' items_ref '{}' expected array, got {}",
            node.id,
            items_ref,
            json_type_name(&value)
        ),
    };
    let item_count = items.len();
    let max_concurrency = parallel_max_concurrency(&node.params);
    let mut completed_items = BTreeMap::new();
    let mut pending_items = BTreeMap::new();
    let mut first_error = None;

    if max_concurrency <= 1 {
        // Match workflowx: 0 and 1 are strictly sequential and do not create
        // worker tasks.
        for (index, item) in items.iter().cloned().enumerate() {
            match execute_parallel_item(runtime, item, index, context).await {
                Ok(ParallelItemOutcome::Completed(local)) => {
                    completed_items.insert(index, SavedCanvasRuntime::capture(&local));
                }
                Ok(ParallelItemOutcome::Waiting { cursor, runtime }) => {
                    pending_items.insert(
                        index,
                        ParallelPendingItem {
                            canvas: SavedCanvasRuntime::capture(&runtime),
                            cursor,
                        },
                    );
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
    } else {
        // workflowx runs index 0 on the caller before it fans the remaining
        // indices out through the bounded worker pool.
        let mut indexed = items.iter().cloned().enumerate();
        if let Some((index, item)) = indexed.next() {
            match execute_parallel_item(runtime, item, index, context).await {
                Ok(ParallelItemOutcome::Completed(local)) => {
                    completed_items.insert(index, SavedCanvasRuntime::capture(&local));
                }
                Ok(ParallelItemOutcome::Waiting { cursor, runtime }) => {
                    pending_items.insert(
                        index,
                        ParallelPendingItem {
                            canvas: SavedCanvasRuntime::capture(&runtime),
                            cursor,
                        },
                    );
                }
                Err(error) => first_error = Some(error),
            }
        }
        let concurrency = max_concurrency.min(item_count.saturating_sub(1).max(1));
        let parent: &CanvasRuntime = runtime;
        let futures = indexed.map(|(index, item)| async move {
            (
                index,
                execute_parallel_item(parent, item, index, context).await,
            )
        });
        let mut fanout = stream::iter(futures).buffer_unordered(concurrency);
        while let Some((index, result)) = fanout.next().await {
            match result {
                Ok(ParallelItemOutcome::Completed(local)) => {
                    completed_items.insert(index, SavedCanvasRuntime::capture(&local));
                }
                Ok(ParallelItemOutcome::Waiting { cursor, runtime }) => {
                    pending_items.insert(
                        index,
                        ParallelPendingItem {
                            canvas: SavedCanvasRuntime::capture(&runtime),
                            cursor,
                        },
                    );
                }
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    if let Some(active_index) = pending_items.keys().next().copied() {
        return parallel_waiting_outcome(ParallelCheckpoint {
            parallel_component_id: node.id.clone(),
            original_items: items,
            completed_items,
            pending_items,
            active_index,
        });
    }
    finish_parallel(runtime, node, item_count, completed_items)?;
    Ok(CompositeExecutionOutcome::Completed)
}

fn finish_parallel(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    item_count: usize,
    mut completed_items: BTreeMap<usize, SavedCanvasRuntime>,
) -> Result<()> {
    let mut snapshots = Vec::with_capacity(item_count);
    for index in 0..item_count {
        let local = completed_items
            .remove(&index)
            .ok_or_else(|| {
                anyhow!(
                    "Canvas Parallel '{}' checkpoint is missing completed item {}",
                    node.id,
                    index
                )
            })?
            .restore();
        merge_parallel_runtime_effects(runtime, &local);
        snapshots.push(parallel_item_snapshot(&local, index));
    }
    let mut outputs = Map::from_iter([("_result".into(), Value::Array(snapshots.clone()))]);
    if let Some(configured) = node.params.get("outputs").and_then(Value::as_object) {
        for (name, raw_spec) in configured {
            let Some(reference) = raw_spec
                .as_object()
                .and_then(|spec| spec.get("ref"))
                .and_then(Value::as_str)
                .filter(|reference| !reference.is_empty())
            else {
                continue;
            };
            let values = snapshots
                .iter()
                .map(|snapshot| resolve_parallel_item_ref(snapshot, reference))
                .collect();
            outputs.insert(name.clone(), Value::Array(values));
        }
    }
    runtime.outputs.insert(node.id.clone(), outputs);
    Ok(())
}

fn parallel_waiting_outcome(state: ParallelCheckpoint) -> Result<CompositeExecutionOutcome> {
    let pending = state
        .pending_items
        .get(&state.active_index)
        .ok_or_else(|| {
            anyhow!(
                "Canvas Parallel '{}' checkpoint active index {} is not pending",
                state.parallel_component_id,
                state.active_index
            )
        })?;
    let leaf_component_id = pending.cursor.waiting_component_id.clone();
    let interrupt_id = format!(
        "parallel:{}:{}:{}",
        state.parallel_component_id, state.active_index, leaf_component_id
    );
    Ok(CompositeExecutionOutcome::Waiting(CompositeWait {
        leaf_component_id,
        interrupt_id,
        state: CompositeCheckpoint::Parallel(state),
    }))
}

fn validate_parallel_checkpoint(
    state: &ParallelCheckpoint,
    node: &CanvasNode,
    plan: &SubgraphPlan,
    nodes: &BTreeMap<String, CanvasNode>,
) -> Result<()> {
    if state.parallel_component_id != node.id {
        bail!(
            "Canvas Parallel checkpoint targets '{}' but scheduler is at '{}'",
            state.parallel_component_id,
            node.id
        );
    }
    let total = state.original_items.len();
    let mut covered = vec![false; total];
    for index in state.completed_items.keys() {
        if *index >= total || covered[*index] {
            bail!(
                "Canvas Parallel '{}' checkpoint has invalid completed index {}",
                node.id,
                index
            );
        }
        covered[*index] = true;
    }
    for (index, pending) in &state.pending_items {
        if *index >= total || covered[*index] {
            bail!(
                "Canvas Parallel '{}' checkpoint has invalid pending index {}",
                node.id,
                index
            );
        }
        covered[*index] = true;
        validate_subgraph_cursor(&pending.cursor, node, plan, nodes)?;
    }
    if let Some(index) = covered.iter().position(|covered| !covered) {
        bail!(
            "Canvas Parallel '{}' checkpoint is missing item index {}",
            node.id,
            index
        );
    }
    if !state.pending_items.contains_key(&state.active_index) {
        bail!(
            "Canvas Parallel '{}' checkpoint active index {} is not pending",
            node.id,
            state.active_index
        );
    }
    Ok(())
}

fn validate_subgraph_cursor(
    cursor: &SubgraphCursor,
    macro_node: &CanvasNode,
    plan: &SubgraphPlan,
    nodes: &BTreeMap<String, CanvasNode>,
) -> Result<()> {
    let waiting_id = plan
        .execution_order
        .get(cursor.next_execution_index)
        .filter(|id| **id == cursor.waiting_component_id)
        .ok_or_else(|| {
            anyhow!(
                "Canvas {} '{}' checkpoint cursor does not match waiting component '{}'",
                macro_node.component_name,
                macro_node.id,
                cursor.waiting_component_id
            )
        })?;
    if !cursor.selected.iter().any(|id| id == waiting_id) {
        bail!(
            "Canvas {} '{}' checkpoint waiting component '{}' is not selected",
            macro_node.component_name,
            macro_node.id,
            waiting_id
        );
    }
    let waiting = nodes.get(waiting_id).ok_or_else(|| {
        anyhow!(
            "Canvas {} '{}' checkpoint references missing body member '{}'",
            macro_node.component_name,
            macro_node.id,
            waiting_id
        )
    })?;
    if !waiting.component_name.eq_ignore_ascii_case("userfillup") {
        bail!(
            "Canvas {} '{}' checkpoint component '{}' is not UserFillUp",
            macro_node.component_name,
            macro_node.id,
            waiting_id
        );
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct ParallelItemContext<'a, 'input> {
    parallel_node: &'a CanvasNode,
    plan: &'a SubgraphPlan,
    nodes: &'a BTreeMap<String, CanvasNode>,
    llm: Option<&'a LlmClient>,
    input: &'a WorkflowRunInput<'input>,
}

async fn execute_parallel_item(
    parent: &CanvasRuntime,
    item: Value,
    index: usize,
    context: ParallelItemContext<'_, '_>,
) -> Result<ParallelItemOutcome> {
    let mut local = parent.clone();
    local.path.clear();
    local.last_message = None;
    local.usage = TokenUsage::default();
    local.provider_calls = 0;
    local.references.clear();
    local.globals.insert("item".into(), item.clone());
    local.globals.insert("__item__".into(), item);
    let index_value = Value::Number(Number::from(index as u64));
    local.globals.insert("index".into(), index_value.clone());
    local.globals.insert("__index__".into(), index_value);

    let selected: HashSet<String> = context
        .plan
        .members
        .iter()
        .filter(|member| {
            context.nodes.get(member.as_str()).is_some_and(|body| {
                !body
                    .upstream
                    .iter()
                    .any(|upstream| context.plan.members.contains(upstream))
            })
        })
        .cloned()
        .collect();
    continue_parallel_item(local, index, context, 0, selected, None).await
}

async fn resume_parallel_item(
    pending: ParallelPendingItem,
    index: usize,
    context: ParallelItemContext<'_, '_>,
    resume_data: Value,
) -> Result<ParallelItemOutcome> {
    validate_subgraph_cursor(
        &pending.cursor,
        context.parallel_node,
        context.plan,
        context.nodes,
    )?;
    let mut local = pending.canvas.restore();
    local.history = context.input.history.to_vec();
    local
        .sys
        .insert("query".into(), Value::String(String::new()));
    let selected = pending.cursor.selected.iter().cloned().collect();
    continue_parallel_item(
        local,
        index,
        context,
        pending.cursor.next_execution_index,
        selected,
        Some(resume_data),
    )
    .await
}

async fn continue_parallel_item(
    mut local: CanvasRuntime,
    index: usize,
    context: ParallelItemContext<'_, '_>,
    start_index: usize,
    mut selected: HashSet<String>,
    mut resume_data: Option<Value>,
) -> Result<ParallelItemOutcome> {
    for (execution_index, body_id) in context
        .plan
        .execution_order
        .iter()
        .enumerate()
        .skip(start_index)
    {
        if !selected.contains(body_id) {
            continue;
        }
        if local.cancel_flag {
            bail!("Canvas run cancelled");
        }
        let body = context.nodes.get(body_id).ok_or_else(|| {
            anyhow!(
                "Canvas Parallel '{}' referenced missing body member '{}'",
                context.parallel_node.id,
                body_id
            )
        })?;
        let next = if body.component_name.eq_ignore_ascii_case("userfillup") {
            if let Some(data) = resume_data.take() {
                execute_user_fill_up_resume(&mut local, body, data)?;
            } else if !execute_user_fill_up_initial(&mut local, body, context.input)? {
                let selected = selected.iter().cloned().collect::<BTreeSet<_>>();
                return Ok(ParallelItemOutcome::Waiting {
                    cursor: SubgraphCursor {
                        waiting_component_id: body.id.clone(),
                        next_execution_index: execution_index,
                        selected: selected.into_iter().collect(),
                    },
                    runtime: local,
                });
            }
            None
        } else {
            execute_node_with_timeout(&mut local, body, context.llm, context.input)
                .await
                .with_context(|| {
                    format!(
                        "Canvas Parallel '{}' item {} member '{}' ({}) failed",
                        context.parallel_node.id, index, body.id, body.component_name
                    )
                })?
        };
        local.path.push(body_id.clone());
        for target in next.unwrap_or_else(|| body.downstream.clone()) {
            if context.plan.members.contains(&target) {
                selected.insert(target);
            }
        }
    }
    if resume_data.is_some() {
        bail!(
            "Canvas Parallel '{}' item {} resume cursor did not execute its UserFillUp target",
            context.parallel_node.id,
            index
        );
    }
    Ok(ParallelItemOutcome::Completed(local))
}

fn parallel_item_snapshot(local: &CanvasRuntime, index: usize) -> Value {
    let mut snapshot = Map::from_iter([
        (
            "item".into(),
            local
                .globals
                .get("__item__")
                .cloned()
                .unwrap_or(Value::Null),
        ),
        ("index".into(), Value::Number(Number::from(index as u64))),
    ]);
    for (component_id, outputs) in &local.outputs {
        snapshot.insert(component_id.clone(), Value::Object(outputs.clone()));
    }
    Value::Object(snapshot)
}

fn resolve_parallel_item_ref(snapshot: &Value, reference: &str) -> Value {
    let Some(snapshot) = snapshot.as_object() else {
        return Value::Null;
    };
    if matches!(reference, "item" | "index") {
        return snapshot.get(reference).cloned().unwrap_or(Value::Null);
    }
    let Some((component_id, path)) = reference.split_once('@') else {
        return Value::Null;
    };
    let Some(mut current) = snapshot.get(component_id) else {
        return Value::Null;
    };
    if path.is_empty() {
        return current.clone();
    }
    for segment in path.split('.') {
        if segment.is_empty() {
            return Value::Null;
        }
        let Some(next) = current.as_object().and_then(|object| object.get(segment)) else {
            return Value::Null;
        };
        current = next;
    }
    current.clone()
}

fn merge_parallel_runtime_effects(parent: &mut CanvasRuntime, local: &CanvasRuntime) {
    parent.usage.prompt_tokens = parent
        .usage
        .prompt_tokens
        .saturating_add(local.usage.prompt_tokens);
    parent.usage.completion_tokens = parent
        .usage
        .completion_tokens
        .saturating_add(local.usage.completion_tokens);
    parent.usage.total_tokens = parent
        .usage
        .total_tokens
        .saturating_add(local.usage.total_tokens);
    parent.provider_calls = parent.provider_calls.saturating_add(local.provider_calls);
    for reference in &local.references {
        if !parent
            .references
            .iter()
            .any(|current| current.id == reference.id && current.kb_id == reference.kb_id)
        {
            parent.references.push(reference.clone());
        }
    }
}

fn parallel_max_concurrency(params: &Map<String, Value>) -> usize {
    let Some(Value::Number(value)) = params.get("max_concurrency") else {
        return 0;
    };
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .or_else(|| {
            value
                .as_i64()
                .and_then(|value| (value > 0).then(|| usize::try_from(value).ok()).flatten())
        })
        .or_else(|| {
            value.as_f64().and_then(|value| {
                (value.is_finite() && value > 0.0)
                    .then_some(value as usize)
                    .filter(|value| *value > 0)
            })
        })
        .unwrap_or(0)
}

async fn execute_loop_with_timeout(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    plan: &SubgraphPlan,
    nodes: &BTreeMap<String, CanvasNode>,
    llm: Option<&LlmClient>,
    input: &WorkflowRunInput<'_>,
    resume: Option<(LoopCheckpoint, Value)>,
) -> Result<CompositeExecutionOutcome> {
    let timeout = component_timeout(&node.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_loop(runtime, node, plan, nodes, llm, input, resume),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            Err(error).with_context(|| {
                format!(
                    "Canvas component '{}' ({}) timed out after {} seconds",
                    node.id,
                    node.component_name,
                    timeout.as_secs()
                )
            })
        }
        result => result,
    }
}

async fn execute_loop(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    plan: &SubgraphPlan,
    nodes: &BTreeMap<String, CanvasNode>,
    llm: Option<&LlmClient>,
    input: &WorkflowRunInput<'_>,
    resume: Option<(LoopCheckpoint, Value)>,
) -> Result<CompositeExecutionOutcome> {
    let maximum = loop_max_iterations(&node.params);
    let (start_iteration, mut resume_cursor, mut resume_data) = if let Some((state, data)) = resume
    {
        if state.loop_component_id != node.id {
            bail!(
                "Canvas Loop checkpoint targets '{}' but scheduler is at '{}'",
                state.loop_component_id,
                node.id
            );
        }
        if state.iteration == 0 || state.iteration > maximum {
            bail!(
                "Canvas Loop '{}' checkpoint iteration {} exceeds maximum {}",
                node.id,
                state.iteration,
                maximum
            );
        }
        validate_subgraph_cursor(&state.cursor, node, plan, nodes)?;
        (state.iteration, Some(state.cursor), Some(data))
    } else {
        (1, None, None)
    };
    for iteration in start_iteration..=maximum {
        if runtime.cancel_flag {
            bail!("Canvas run cancelled");
        }
        let (start_index, mut selected) = if let Some(cursor) = resume_cursor.take() {
            (
                cursor.next_execution_index,
                cursor.selected.into_iter().collect::<HashSet<_>>(),
            )
        } else {
            seed_loop_variables(runtime, node)?;
            (
                0,
                plan.members
                    .iter()
                    .filter(|member| {
                        nodes.get(member.as_str()).is_some_and(|body| {
                            !body
                                .upstream
                                .iter()
                                .any(|upstream| plan.members.contains(upstream))
                        })
                    })
                    .cloned()
                    .collect(),
            )
        };

        for (execution_index, body_id) in plan.execution_order.iter().enumerate().skip(start_index)
        {
            if !selected.contains(body_id) {
                continue;
            }
            if runtime.cancel_flag {
                bail!("Canvas run cancelled");
            }
            let body = nodes.get(body_id).ok_or_else(|| {
                anyhow!(
                    "Canvas Loop '{}' referenced missing body member '{}'",
                    node.id,
                    body_id
                )
            })?;
            let next = if body.component_name.eq_ignore_ascii_case("userfillup") {
                if let Some(data) = resume_data.take() {
                    execute_user_fill_up_resume(runtime, body, data)?;
                } else if !execute_user_fill_up_initial(runtime, body, input)? {
                    let selected = selected.iter().cloned().collect::<BTreeSet<_>>();
                    let leaf_component_id = body.id.clone();
                    return Ok(CompositeExecutionOutcome::Waiting(CompositeWait {
                        interrupt_id: format!(
                            "loop:{}:{}:{}",
                            node.id, iteration, leaf_component_id
                        ),
                        leaf_component_id: leaf_component_id.clone(),
                        state: CompositeCheckpoint::Loop(LoopCheckpoint {
                            loop_component_id: node.id.clone(),
                            iteration,
                            cursor: SubgraphCursor {
                                waiting_component_id: leaf_component_id,
                                next_execution_index: execution_index,
                                selected: selected.into_iter().collect(),
                            },
                        }),
                    }));
                }
                None
            } else {
                execute_node_with_timeout(runtime, body, llm, input)
                    .await
                    .with_context(|| {
                        format!(
                            "Canvas Loop '{}' iteration {} member '{}' ({}) failed",
                            node.id, iteration, body.id, body.component_name
                        )
                    })?
            };
            for target in next.unwrap_or_else(|| body.downstream.clone()) {
                if plan.members.contains(&target) {
                    selected.insert(target);
                }
            }
        }
        if resume_data.is_some() {
            bail!(
                "Canvas Loop '{}' resume cursor did not execute its UserFillUp target",
                node.id
            );
        }

        if evaluate_loop_termination(runtime, node)? {
            return Ok(CompositeExecutionOutcome::Completed);
        }
        if iteration >= maximum {
            return Err(LoopMaxIterationsExceeded {
                component_id: node.id.clone(),
                maximum,
            }
            .into());
        }
    }
    unreachable!("Loop maximum is always positive")
}

fn loop_max_iterations(params: &Map<String, Value>) -> usize {
    let configured = match params.get("maximum_loop_count") {
        Some(Value::Number(value)) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .or_else(|| {
                value
                    .as_i64()
                    .and_then(|value| (value > 0).then(|| usize::try_from(value).ok()).flatten())
            })
            .or_else(|| {
                value.as_f64().and_then(|value| {
                    (value.is_finite() && value > 0.0)
                        .then_some(value as usize)
                        .filter(|value| *value > 0)
                })
            })
            .unwrap_or(0),
        _ => 0,
    };
    if configured == 0 {
        DEFAULT_LOOP_MAX_ITERATIONS
    } else {
        configured
    }
}

fn seed_loop_variables(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let Some(variables) = node.params.get("loop_variables").and_then(Value::as_array) else {
        return Ok(());
    };
    // The fixed Go helper first materializes a map keyed by variable name, so
    // duplicate declarations use the final record rather than whichever item
    // happens to be visited first by the init lambda.
    let mut unique = BTreeMap::new();
    for (index, raw) in variables.iter().enumerate() {
        let variable = raw
            .as_object()
            .expect("Loop variables were validated during compilation");
        let name = variable
            .get("variable")
            .and_then(Value::as_str)
            .expect("Loop variable names were validated during compilation");
        unique.insert(name, (index, variable));
    }
    for (name, (index, variable)) in unique {
        let existing = runtime
            .outputs
            .get(&node.id)
            .and_then(|outputs| outputs.get(name));
        if existing.is_some_and(|value| !value.is_null()) {
            continue;
        }
        let input_mode = variable
            .get("input_mode")
            .and_then(Value::as_str)
            .expect("Loop input modes were validated during compilation");
        let value = match input_mode {
            "constant" => variable.get("value").cloned().unwrap_or(Value::Null),
            "variable" => {
                let reference = variable
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        anyhow!(
                            "Loop '{}' loop_variable[{index}] variable value must be a reference string",
                            node.id
                        )
                    })?;
                get_loop_variable_or_null(runtime, reference).with_context(|| {
                    format!(
                        "Loop '{}' loop_variable[{index}] '{}' has invalid reference '{}'",
                        node.id, name, reference
                    )
                })?
            }
            _ => zero_loop_value(variable.get("type")),
        };
        runtime
            .outputs
            .entry(node.id.clone())
            .or_default()
            .insert(name.to_owned(), value);
    }
    Ok(())
}

fn zero_loop_value(kind: Option<&Value>) -> Value {
    let kind = kind.and_then(Value::as_str).unwrap_or_default();
    match kind {
        "number" => Value::Number(Number::from(0)),
        "boolean" => Value::Bool(false),
        kind if kind.starts_with("object") => Value::Object(Map::new()),
        kind if kind.starts_with("array") => Value::Array(Vec::new()),
        _ => Value::String(String::new()),
    }
}

fn evaluate_loop_termination(runtime: &CanvasRuntime, node: &CanvasNode) -> Result<bool> {
    let Some(conditions) = node
        .params
        .get("loop_termination_condition")
        .and_then(Value::as_array)
    else {
        return Ok(false);
    };
    if conditions.is_empty() {
        return Ok(false);
    }
    let logical_operator = node
        .params
        .get("logical_operator")
        .and_then(Value::as_str)
        .filter(|operator| !operator.is_empty())
        .unwrap_or("and");
    let mut combined = logical_operator == "and";
    for (index, raw) in conditions.iter().enumerate() {
        let condition = raw
            .as_object()
            .expect("Loop conditions were validated during compilation");
        let variable = condition
            .get("variable")
            .and_then(Value::as_str)
            .expect("Loop condition variables were validated during compilation");
        let reference = if variable.contains('.') || variable.contains('@') {
            variable.to_owned()
        } else {
            format!("{}@{variable}", node.id)
        };
        let left = get_loop_variable_or_null(runtime, &reference).with_context(|| {
            format!(
                "Loop '{}' condition[{index}] has invalid lhs reference '{}'",
                node.id, reference
            )
        })?;
        let input_mode = condition
            .get("input_mode")
            .and_then(Value::as_str)
            .filter(|mode| !mode.is_empty())
            .unwrap_or("constant");
        let right = match input_mode {
            "constant" => condition.get("value").cloned().unwrap_or(Value::Null),
            "variable" => {
                let reference = condition
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        anyhow!(
                            "Loop '{}' condition[{index}] variable value must be a reference string",
                            node.id
                        )
                    })?;
                get_loop_variable_or_null(runtime, reference).with_context(|| {
                    format!(
                        "Loop '{}' condition[{index}] has invalid rhs reference '{}'",
                        node.id, reference
                    )
                })?
            }
            mode => {
                bail!(
                    "Loop '{}' condition[{index}] has invalid input mode '{mode}'",
                    node.id
                )
            }
        };
        let operator = condition
            .get("operator")
            .and_then(Value::as_str)
            .expect("Loop condition operators were validated during compilation");
        let result = evaluate_loop_condition_value(&left, operator, &right).with_context(|| {
            format!(
                "Loop '{}' condition[{index}] failed for '{}'",
                node.id, reference
            )
        })?;
        combined = if logical_operator == "or" {
            combined || result
        } else {
            combined && result
        };
    }
    Ok(combined)
}

fn get_loop_variable_or_null(runtime: &CanvasRuntime, reference: &str) -> Result<Value> {
    if trim_selector(reference).is_empty() {
        bail!("Canvas loop variable reference cannot be empty");
    }
    Ok(get_variable(runtime, reference).unwrap_or(Value::Null))
}

fn evaluate_loop_condition_value(left: &Value, operator: &str, right: &Value) -> Result<bool> {
    Ok(match left {
        Value::Null => operator == "empty",
        Value::String(left) => match operator {
            "contains" => left.contains(right.as_str().unwrap_or_default()),
            "not contains" => !left.contains(right.as_str().unwrap_or_default()),
            "start with" => left.starts_with(right.as_str().unwrap_or_default()),
            "end with" => left.ends_with(right.as_str().unwrap_or_default()),
            "is" => right.as_str().is_some_and(|right| left == right),
            "is not" => !right.as_str().is_some_and(|right| left == right),
            "empty" => left.is_empty(),
            "not empty" => !left.is_empty(),
            _ => bail!("invalid operator '{operator}' for string variable"),
        },
        Value::Bool(left) => match operator {
            "is" => *left == right.as_bool().unwrap_or(false),
            "is not" => *left != right.as_bool().unwrap_or(false),
            "empty" => !*left && right.is_null(),
            "not empty" => *left || !right.is_null(),
            _ => bail!("invalid operator '{operator}' for boolean variable"),
        },
        Value::Number(left) => {
            if matches!(operator, "empty" | "not empty") {
                (operator == "empty") == right.is_null()
            } else {
                let left = left
                    .as_f64()
                    .ok_or_else(|| anyhow!("number variable is outside f64 range"))?;
                let right = right
                    .as_f64()
                    .ok_or_else(|| anyhow!("operator '{operator}' requires a numeric value"))?;
                match operator {
                    "=" => left == right,
                    "≠" => left != right,
                    ">" => left > right,
                    "<" => left < right,
                    "≥" => left >= right,
                    "≤" => left <= right,
                    _ => bail!("invalid operator '{operator}' for number variable"),
                }
            }
        }
        Value::Object(left) => match operator {
            "empty" => left.is_empty(),
            "not empty" => !left.is_empty(),
            _ => bail!("invalid operator '{operator}' for object variable"),
        },
        Value::Array(left) => match operator {
            "contains" => left.contains(right),
            "not contains" => !left.contains(right),
            "is" => right.as_array().is_some_and(|right| left == right),
            "is not" => !right.as_array().is_some_and(|right| left == right),
            "empty" => left.is_empty(),
            "not empty" => !left.is_empty(),
            _ => bail!("invalid operator '{operator}' for array variable"),
        },
    })
}

async fn execute_node_with_timeout(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    llm: Option<&LlmClient>,
    input: &WorkflowRunInput<'_>,
) -> Result<Option<Vec<String>>> {
    let timeout = component_timeout(&node.component_name);
    match crate::runtime::with_timeout(timeout, execute_node(runtime, node, llm, input)).await {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            Err(error).with_context(|| {
                format!(
                    "Canvas component '{}' ({}) timed out after {} seconds",
                    node.id,
                    node.component_name,
                    timeout.as_secs()
                )
            })
        }
        result => result,
    }
}

fn component_timeout(component_class: &str) -> Duration {
    resolve_component_timeout_with(component_class, |name| std::env::var(name).ok())
}

fn resolve_component_timeout_with(
    component_class: &str,
    read_env: impl Fn(&str) -> Option<String>,
) -> Duration {
    let upper = component_class.trim().to_ascii_uppercase();
    if !upper.is_empty()
        && let Some(timeout) =
            parse_positive_timeout(read_env(&format!("COMPONENT_EXEC_TIMEOUT_{upper}")))
    {
        return timeout;
    }
    if let Some(timeout) = match upper.as_str() {
        "LLM" | "MESSAGE" | "AGENT" => Some(Duration::from_secs(600)),
        "RETRIEVAL" | "WIKIPEDIA" | "YAHOOFINANCE" => Some(Duration::from_secs(60)),
        "TAVILYSEARCH" | "TAVILY" | "DUCKDUCKGO" | "GOOGLE" | "ARXIV" | "GOOGLESCHOLAR"
        | "GITHUB" | "SEARXNG" | "PUBMED" => Some(Duration::from_secs(12)),
        "EXESQL" | "INVOKE" => Some(Duration::from_secs(3)),
        _ => None,
    } {
        return timeout;
    }
    parse_positive_timeout(read_env("COMPONENT_EXEC_TIMEOUT"))
        .unwrap_or_else(|| Duration::from_secs(600))
}

fn parse_positive_timeout(value: Option<String>) -> Option<Duration> {
    const MAX_SECONDS: i64 = i64::MAX / 1_000_000_000;
    let seconds = value?.trim().parse::<i64>().ok()?;
    (seconds > 0 && seconds <= MAX_SECONDS).then(|| Duration::from_secs(seconds as u64))
}

fn validate_message_params(node: &CanvasNode) -> Result<()> {
    message_content_choices(node)?;
    if node
        .params
        .get("stream")
        .is_some_and(|value| !value.is_boolean())
    {
        bail!("Message '{}' stream must be a boolean", node.id);
    }
    Ok(())
}

fn validate_llm_params(node: &CanvasNode) -> Result<()> {
    validate_node_llm_selector(node)?;
    llm_prompt_specs(node)?;
    if let Some(value) = node.params.get("message_history_window_size")
        && value.as_u64().is_none()
    {
        bail!(
            "Canvas LLM component '{}' message_history_window_size must be a non-negative integer",
            node.id
        );
    }
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "Canvas LLM component '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    if let Some(value) = node.params.get("delay_after_error")
        && value
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .is_none()
    {
        bail!(
            "Canvas LLM component '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    if node.component_name.eq_ignore_ascii_case("agent") {
        if let Some(value) = node.params.get("max_rounds")
            && value.as_u64().is_none()
        {
            bail!(
                "Canvas Agent '{}' max_rounds must be a non-negative integer",
                node.id
            );
        }
        load_canvas_agent_tools(node)?;
    }
    llm_component_generation_patch(node)?;
    Ok(())
}

#[derive(Debug, Clone)]
struct CanvasAgentTool {
    child_id: String,
    component_name: String,
    function_name: String,
    params: Map<String, Value>,
    definition: Value,
}

fn load_canvas_agent_tools(node: &CanvasNode) -> Result<Vec<CanvasAgentTool>> {
    if let Some(mcp) = node.params.get("mcp").filter(|value| !value.is_null()) {
        let mcp = mcp
            .as_array()
            .ok_or_else(|| anyhow!("Canvas Agent '{}' mcp must be an array", node.id))?;
        if !mcp.is_empty() {
            bail!(
                "Canvas Agent '{}' MCP tools are not implemented in the Rust runtime",
                node.id
            );
        }
    }
    let Some(raw_tools) = node.params.get("tools").filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let raw_tools = raw_tools
        .as_array()
        .ok_or_else(|| anyhow!("Canvas Agent '{}' tools must be an array", node.id))?;
    raw_tools
        .iter()
        .enumerate()
        .map(|(index, raw)| {
            let raw = raw.as_object().ok_or_else(|| {
                anyhow!(
                    "Canvas Agent '{}' tool at index {index} must be an object",
                    node.id
                )
            })?;
            let component_name = raw
                .get("component_name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    anyhow!(
                        "Canvas Agent '{}' tool at index {index} requires component_name",
                        node.id
                    )
                })?
                .to_owned();
            let display_name = raw
                .get("name")
                .map(|name| {
                    name.as_str().ok_or_else(|| {
                        anyhow!(
                            "Canvas Agent '{}' tool at index {index} name must be a string",
                            node.id
                        )
                    })
                })
                .transpose()?
                .unwrap_or_default();
            let params = raw
                .get("params")
                .map(|params| {
                    params.as_object().cloned().ok_or_else(|| {
                        anyhow!(
                            "Canvas Agent '{}' tool at index {index} params must be an object",
                            node.id
                        )
                    })
                })
                .transpose()?
                .unwrap_or_default();
            let child = CanvasNode {
                id: format!("{}-->{}", node.id, display_name.replace(' ', "_")),
                component_name: component_name.clone(),
                params: params.clone(),
                downstream: Vec::new(),
                upstream: Vec::new(),
                parent_id: Some(node.id.clone()),
            };
            let (default_name, default_description, properties, required) =
                if component_name.eq_ignore_ascii_case("retrieval") {
                    validate_retrieval_params(&child)?;
                    (
                        "search_my_dateset",
                        "This tool can be utilized for relevant content searching in the datasets.",
                        serde_json::json!({
                            "query": {
                                "type": "string",
                                "description": "The keywords to search the dataset. The keywords should be the most important words/terms(includes synonyms) from the original request."
                            }
                        }),
                        serde_json::json!(["query"]),
                    )
                } else if component_name.eq_ignore_ascii_case("tavilysearch") {
                    validate_tavily_search_params(&child)?;
                    (
                        "tavily_search",
                        "Tavily is a search engine optimized for LLMs, aimed at efficient, quick and persistent search results.\nWhen searching:\n   - Start with specific query which should focus on just a single aspect.\n   - Number of keywords in query should be less than 5.\n   - Broaden search terms if needed\n   - Cross-reference information from multiple sources",
                        serde_json::json!({
                            "query": {
                                "type": "string",
                                "description": "The search keywords to execute with Tavily. The keywords should be the most important words/terms(includes synonyms) from the original request."
                            },
                            "topic": {
                                "type": "string",
                                "description": "default:general. The category of the search.news is useful for retrieving real-time updates, particularly about politics, sports, and major current events covered by mainstream media sources. general is for broader, more general-purpose searches that may include a wide range of sources.",
                                "enum": ["general", "news"]
                            },
                            "include_domains": {
                                "type": "array",
                                "description": "default:[]. A list of domains only from which the search results can be included.",
                                "items": {"type": "string", "description": "Domain name that must be included, e.g. www.yahoo.com"}
                            },
                            "exclude_domains": {
                                "type": "array",
                                "description": "default:[]. A list of domains from which the search results can not be included",
                                "items": {"type": "string", "description": "Domain name that must be excluded, e.g. www.yahoo.com"}
                            }
                        }),
                        serde_json::json!(["query"]),
                    )
                } else if component_name.eq_ignore_ascii_case("tavilyextract") {
                    validate_tavily_extract_params(&child)?;
                    (
                        "tavily_extract",
                        "Extract web page content from one or more specified URLs using Tavily Extract.",
                        serde_json::json!({
                            "urls": {
                                "type": "array",
                                "description": "The URLs to extract content from.",
                                "items": {"type": "string", "description": "The URL to extract content from, e.g. www.yahoo.com"}
                            },
                            "extract_depth": {
                                "type": "string",
                                "description": "The depth of the extraction process. advanced extraction retrieves more data, including tables and embedded content, with higher success but may increase latency.basic extraction costs 1 credit per 5 successful URL extractions, while advanced extraction costs 2 credits per 5 successful URL extractions.",
                                "enum": ["basic", "advanced"]
                            },
                            "format": {
                                "type": "string",
                                "description": "The format of the extracted web page content. markdown returns content in markdown format. text returns plain text and may increase latency.",
                                "enum": ["markdown", "text"]
                            }
                        }),
                        serde_json::json!(["urls"]),
                    )
                } else if component_name.eq_ignore_ascii_case("duckduckgo") {
                    validate_duckduckgo_params(&child)?;
                    (
                        "duckduckgo_search",
                        "DuckDuckGo is a search engine focused on privacy. It offers search capabilities for web pages, images, and provides translation services. DuckDuckGo also features a private AI chat interface, providing users with an AI assistant that prioritizes data protection.",
                        serde_json::json!({
                            "query": {
                                "type": "string",
                                "description": "The search keywords to execute with DuckDuckGo. The keywords should be the most important words/terms(includes synonyms) from the original request."
                            },
                            "channel": {
                                "type": "string",
                                "description": "default:general. The category of the search. `news` is useful for retrieving real-time updates, particularly about politics, sports, and major current events covered by mainstream media sources. `general` is for broader, more general-purpose searches that may include a wide range of sources.",
                                "enum": ["general", "news"]
                            }
                        }),
                        serde_json::json!(["query"]),
                    )
                } else if component_name.eq_ignore_ascii_case("wikipedia") {
                    validate_wikipedia_params(&child)?;
                    (
                        "wikipedia_search",
                        "A wide range of how-to and information pages are made available in wikipedia. Since 2001, it has grown rapidly to become the world's largest reference website. From Wikipedia, the free encyclopedia.",
                        serde_json::json!({
                            "query": {
                                "type": "string",
                                "description": "The search keyword to execute with wikipedia. The keyword MUST be a specific subject that can match the title."
                            }
                        }),
                        serde_json::json!(["query"]),
                    )
                } else if component_name.eq_ignore_ascii_case("google") {
                    validate_google_params(&child)?;
                    (
                        "google_search",
                        "Search the world's information, including webpages, images, videos and more. Google has many special features to help you find exactly what you're looking ...",
                        serde_json::json!({
                            "q": {
                                "type": "string",
                                "description": "The search keywords to execute with Google. The keywords should be the most important words/terms(includes synonyms) from the original request."
                            },
                            "start": {
                                "type": "integer",
                                "description": "Parameter defines the result offset. It skips the given number of results. It's used for pagination. (e.g., 0 (default) is the first page of results, 10 is the 2nd page of results, 20 is the 3rd page of results, etc.). Google Local Results only accepts multiples of 20(e.g. 20 for the second page results, 40 for the third page results, etc.) as the `start` value."
                            },
                            "num": {
                                "type": "integer",
                                "description": "Parameter defines the maximum number of results to return. (e.g., 10 (default) returns 10 results, 40 returns 40 results, and 100 returns 100 results). The use of num may introduce latency, and/or prevent the inclusion of specialized result types. It is better to omit this parameter unless it is strictly necessary to increase the number of results per page. Results are not guaranteed to have the number of results specified in num."
                            }
                        }),
                        serde_json::json!(["q"]),
                    )
                } else if component_name.eq_ignore_ascii_case("googlescholar") {
                    validate_google_scholar_params(&child)?;
                    (
                        "google_scholar_search",
                        "Google Scholar provides a simple way to broadly search for scholarly literature. From one place, you can search across many disciplines and sources: articles, theses, books, abstracts and court opinions, from academic publishers, professional societies, online repositories, universities and other web sites. Google Scholar helps you find relevant work across the world of scholarly research.",
                        serde_json::json!({
                            "query": {
                                "type": "string",
                                "description": "The search keyword to execute with Google Scholar. The keywords should be the most important words/terms(includes synonyms) from the original request."
                            }
                        }),
                        serde_json::json!(["query"]),
                    )
                } else if component_name.eq_ignore_ascii_case("github") {
                    validate_github_params(&child)?;
                    (
                        "github_search",
                        "GitHub repository search is a feature that enables users to find specific repositories on the GitHub platform. This search functionality allows users to locate projects, codebases, and other content hosted on GitHub based on various criteria.",
                        serde_json::json!({
                            "query": {
                                "type": "string",
                                "description": "The search keywords to execute with GitHub. The keywords should be the most important words/terms(includes synonyms) from the original request."
                            }
                        }),
                        serde_json::json!(["query"]),
                    )
                } else if component_name.eq_ignore_ascii_case("yahoofinance") {
                    validate_yahoo_finance_params(&child)?;
                    (
                        "yahoo_finance",
                        "The Yahoo Finance is a service that provides access to real-time and historical stock market data. It enables users to fetch various types of stock information, such as price quotes, historical prices, company profiles, and financial news. The API offers structured data, allowing developers to integrate market data into their applications and analysis tools.",
                        serde_json::json!({
                            "stock_code": {
                                "type": "string",
                                "description": "The stock code or company name."
                            }
                        }),
                        serde_json::json!(["stock_code"]),
                    )
                } else if component_name.eq_ignore_ascii_case("arxiv") {
                    validate_arxiv_params(&child)?;
                    (
                        "arxiv_search",
                        "arXiv is a free distribution service and an open-access archive for nearly 2.4 million scholarly articles in the fields of physics, mathematics, computer science, quantitative biology, quantitative finance, statistics, electrical engineering and systems science, and economics. Materials on this site are not peer-reviewed by arXiv.",
                        serde_json::json!({
                            "query": {
                                "type": "string",
                                "description": "The search keywords to execute with arXiv. The keywords should be the most important words/terms(includes synonyms) from the original request."
                            }
                        }),
                        serde_json::json!(["query"]),
                    )
                } else if component_name.eq_ignore_ascii_case("pubmed") {
                    validate_pubmed_params(&child)?;
                    (
                        "pubmed_search",
                        "PubMed is an openly accessible, free database which includes primarily the MEDLINE database of references and abstracts on life sciences and biomedical topics.\nIn addition to MEDLINE, PubMed provides access to:\n - older references from the print version of Index Medicus, back to 1951 and earlier\n - references to some journals before they were indexed in Index Medicus and MEDLINE, for instance Science, BMJ, and Annals of Surgery\n - very recent entries to records for an article before it is indexed with Medical Subject Headings (MeSH) and added to MEDLINE\n - a collection of books available full-text and other subsets of NLM records[4]\n - PMC citations\n - NCBI Bookshelf",
                        serde_json::json!({
                            "query": {
                                "type": "string",
                                "description": "The search keywords to execute with PubMed. The keywords should be the most important words/terms(includes synonyms) from the original request."
                            }
                        }),
                        serde_json::json!(["query"]),
                    )
                } else {
                    bail!(
                        "Canvas Agent '{}' tool '{}' is not implemented; this Rust slice supports Retrieval, TavilySearch, TavilyExtract, DuckDuckGo, Wikipedia, GoogleScholar, GitHub, YahooFinance, ArXiv and PubMed",
                        node.id,
                        component_name
                    );
                };
            let function_name = params
                .get("function_name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .unwrap_or(default_name);
            let indexed_name = format!("{function_name}_{index}");
            let description = params
                .get("description")
                .and_then(Value::as_str)
                .filter(|description| !description.is_empty())
                .unwrap_or(default_description)
                .to_owned();
            Ok(CanvasAgentTool {
                child_id: child.id,
                component_name,
                function_name: indexed_name.clone(),
                params,
                definition: serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": indexed_name,
                        "description": description,
                        "parameters": {
                            "type": "object",
                            "properties": properties,
                            "required": required
                        }
                    }
                }),
            })
        })
        .collect()
}

fn validate_node_llm_selector(node: &CanvasNode) -> Result<()> {
    for field in ["llm_id", "model_id"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("Canvas component '{}' {field} must be a string", node.id);
        }
    }
    Ok(())
}

fn llm_prompt_specs(node: &CanvasNode) -> Result<Vec<(String, String)>> {
    let raw = node
        .params
        .get("prompts")
        .or_else(|| node.params.get("user_prompt"))
        .or_else(|| node.params.get("prompt"));
    match raw {
        None => Ok(vec![("user".into(), "{sys.query}".into())]),
        Some(Value::String(content)) => Ok(vec![("user".into(), content.clone())]),
        Some(Value::Array(prompts)) if prompts.is_empty() => {
            bail!("Canvas LLM component '{}' prompts cannot be empty", node.id)
        }
        Some(Value::Array(prompts)) => prompts
            .iter()
            .map(|prompt| {
                let prompt = prompt.as_object().ok_or_else(|| {
                    anyhow!(
                        "Canvas LLM component '{}' prompts must contain objects",
                        node.id
                    )
                })?;
                let role = prompt.get("role").and_then(Value::as_str).unwrap_or("user");
                let content = prompt
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        anyhow!(
                            "Canvas LLM component '{}' prompt content must be a string",
                            node.id
                        )
                    })?;
                Ok((role.to_owned(), content.to_owned()))
            })
            .collect(),
        Some(_) => bail!(
            "Canvas LLM component '{}' prompts must be a string or an array",
            node.id
        ),
    }
}

fn message_content_choices(node: &CanvasNode) -> Result<Vec<&str>> {
    if let Some(text) = node.params.get("text") {
        let text = text
            .as_str()
            .ok_or_else(|| anyhow!("Message '{}' text must be a string", node.id))?;
        if !text.is_empty() {
            return Ok(vec![text]);
        }
    }

    let Some(content) = node.params.get("content") else {
        bail!("Message '{}' content cannot be empty", node.id);
    };
    match content {
        Value::String(content) if !content.is_empty() => Ok(vec![content]),
        Value::Array(choices) if !choices.is_empty() => choices
            .iter()
            .map(|choice| {
                choice
                    .as_str()
                    .ok_or_else(|| anyhow!("Message '{}' content must contain strings", node.id))
            })
            .collect(),
        Value::String(_) | Value::Array(_) => {
            bail!("Message '{}' content cannot be empty", node.id)
        }
        _ => bail!(
            "Message '{}' content must be a string or an array of strings",
            node.id
        ),
    }
}

fn execute_message(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let choices = message_content_choices(node)?;
    let template = choices
        .choose(&mut rand::rng())
        .expect("Message choices were validated as non-empty");
    let (content, downloads) = render_message_template(runtime, node, template);
    runtime.last_message = Some(content.clone());
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("content".into(), Value::String(content)),
            ("downloads".into(), Value::Array(downloads)),
        ]),
    );
    Ok(())
}

/// Render the fixed Message selector/Jinja surface while retaining native
/// download descriptors separately from the user-visible content.
fn render_message_template(
    runtime: &CanvasRuntime,
    node: &CanvasNode,
    template: &str,
) -> (String, Vec<Value>) {
    let mut values = BTreeMap::new();
    let mut replacements = Vec::new();
    let mut downloads = Vec::new();
    for capture in selector_regex().captures_iter(template) {
        let selector = capture
            .get(1)
            .expect("selector regex has a capture")
            .as_str();
        if values.contains_key(selector) {
            continue;
        }
        let value = get_variable(runtime, selector).unwrap_or(Value::Null);
        let rendered = stringify_message_value(&value, &mut downloads);
        values.insert(selector.to_owned(), rendered.clone());
        replacements.push((normalize_template_identifier(selector), rendered));
    }

    if !contains_message_jinja_syntax(template) {
        let mut rendered = String::with_capacity(template.len());
        let mut last = 0;
        for capture in selector_regex().captures_iter(template) {
            let full = capture.get(0).expect("selector regex has a full match");
            rendered.push_str(&template[last..full.start()]);
            let selector = capture
                .get(1)
                .expect("selector regex has a capture")
                .as_str();
            rendered.push_str(values.get(selector).map(String::as_str).unwrap_or_default());
            last = full.end();
        }
        rendered.push_str(&template[last..]);
        return (rendered, downloads);
    }

    let mut prepared = String::with_capacity(template.len());
    let mut last = 0;
    for capture in selector_regex().captures_iter(template) {
        let full = capture.get(0).expect("selector regex has a full match");
        prepared.push_str(&template[last..full.start()]);
        let selector = capture
            .get(1)
            .expect("selector regex has a capture")
            .as_str();
        let name = normalize_template_identifier(selector);
        if full.as_str().trim_start().starts_with("{{") {
            prepared.push_str("{{ ");
            prepared.push_str(&name);
            prepared.push_str(" }}");
        } else {
            prepared.push_str(&name);
        }
        last = full.end();
    }
    prepared.push_str(&template[last..]);

    let mut context = Map::new();
    add_string_transform_runtime_context(
        runtime,
        &mut context,
        node.upstream.first().map(String::as_str),
    );
    for (name, value) in &replacements {
        context.insert(name.clone(), Value::String(value.clone()));
    }
    let mut rendered =
        render_sandboxed_jinja(&prepared, context).unwrap_or_else(|_| prepared.clone());
    // Python performs a final plain substitution after sandbox rendering. It
    // also makes malformed Jinja fail soft while still resolving selectors.
    for (name, value) in replacements {
        rendered = rendered.replace(&name, &value);
    }
    (rendered, downloads)
}

fn contains_message_jinja_syntax(template: &str) -> bool {
    template.contains("{{") || template.contains("}}") || template.contains("{%")
}

fn stringify_message_value(value: &Value, downloads: &mut Vec<Value>) -> String {
    if let Some(download_value) = message_download_value(value) {
        let include_in_content = match &download_value {
            Value::Object(_) => download_info_includes_content(&download_value),
            Value::Array(items) => items.iter().any(download_info_includes_content),
            _ => false,
        };
        match &download_value {
            Value::Object(_) => downloads.push(normalize_download_info(&download_value)),
            Value::Array(items) => downloads.extend(items.iter().map(normalize_download_info)),
            _ => {}
        }
        return if include_in_content {
            stringify(&normalize_download_info(&download_value))
        } else {
            String::new()
        };
    }
    stringify(value)
}

fn message_download_value(value: &Value) -> Option<Value> {
    let value = match value {
        Value::String(encoded) => serde_json::from_str(encoded).ok()?,
        value => value.clone(),
    };
    match &value {
        Value::Object(object) if is_download_info(object) => Some(value),
        Value::Array(items)
            if !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.as_object().is_some_and(is_download_info)) =>
        {
            Some(value)
        }
        _ => None,
    }
}

fn is_download_info(value: &Map<String, Value>) -> bool {
    ["doc_id", "filename", "mime_type"]
        .iter()
        .all(|key| value.contains_key(*key))
}

fn download_info_includes_content(value: &Value) -> bool {
    value
        .get("include_download_info_in_content")
        .is_some_and(truthy)
}

fn normalize_download_info(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(normalize_download_info).collect()),
        Value::Object(object) => {
            let mut normalized = object.clone();
            normalized.shift_remove("include_download_info_in_content");
            Value::Object(normalized)
        }
        value => value.clone(),
    }
}

#[derive(Debug)]
struct CanvasAgentToolRun {
    content: String,
    calls: Vec<ToolCall>,
}

async fn execute_canvas_agent_tool_loop(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    llm: &LlmClient,
    input: &WorkflowRunInput<'_>,
    messages: &[ChatMessage],
    patch: GenerationParamsPatch,
    tools: &[CanvasAgentTool],
) -> Result<CanvasAgentToolRun> {
    let definitions: Vec<_> = tools.iter().map(|tool| tool.definition.clone()).collect();
    let mut history: Vec<_> = messages.iter().map(ToolChatMessage::from_chat).collect();
    let mut observed = Vec::new();
    let max_rounds = node
        .params
        .get("max_rounds")
        .and_then(Value::as_u64)
        .unwrap_or(5);

    for _round in 0..=max_rounds {
        let completion = llm
            .tool_chat_completion_with_generation(&history, &definitions, patch)
            .await?;
        record_provider_usage(runtime, completion.usage);
        if completion.tool_calls.is_empty() {
            return Ok(CanvasAgentToolRun {
                content: completion.content,
                calls: observed,
            });
        }

        history.push(ToolChatMessage::assistant(completion.tool_calls.clone()));
        for call in completion.tool_calls {
            let result = match execute_canvas_agent_tool(runtime, node, tools, &call, input).await {
                Ok(result) => result,
                Err(error) => error.to_string(),
            };
            history.push(ToolChatMessage::tool(call.id.clone(), result));
            observed.push(call);
        }
    }

    history.push(ToolChatMessage::user(format!(
        "Exceed max rounds: {max_rounds}"
    )));
    let completion = llm
        .tool_chat_completion_with_generation(&history, &[], patch)
        .await?;
    record_provider_usage(runtime, completion.usage);
    Ok(CanvasAgentToolRun {
        content: completion.content,
        calls: observed,
    })
}

async fn execute_canvas_agent_tool(
    runtime: &mut CanvasRuntime,
    agent: &CanvasNode,
    tools: &[CanvasAgentTool],
    call: &ToolCall,
    input: &WorkflowRunInput<'_>,
) -> Result<String> {
    let tavily = TavilyClient::default();
    let wikipedia = WikipediaClient::default();
    let duckduckgo = DuckDuckGoClient::default();
    let google = GoogleClient::default();
    let google_scholar = GoogleScholarClient::default();
    let github = GitHubClient::default();
    let arxiv = ArxivClient::default();
    let pubmed = PubMedClient::default();
    execute_canvas_agent_tool_with_providers(
        runtime,
        agent,
        tools,
        call,
        input,
        CanvasAgentToolProviders {
            tavily: &tavily,
            wikipedia: &wikipedia,
            duckduckgo: &duckduckgo,
            google: &google,
            google_scholar: &google_scholar,
            github: &github,
            arxiv: &arxiv,
            pubmed: &pubmed,
        },
    )
    .await
}

#[cfg(test)]
async fn execute_canvas_agent_tool_with_tavily(
    runtime: &mut CanvasRuntime,
    agent: &CanvasNode,
    tools: &[CanvasAgentTool],
    call: &ToolCall,
    input: &WorkflowRunInput<'_>,
    tavily: &dyn TavilyProvider,
) -> Result<String> {
    let wikipedia = WikipediaClient::default();
    let duckduckgo = DuckDuckGoClient::default();
    let google = GoogleClient::default();
    let google_scholar = GoogleScholarClient::default();
    let github = GitHubClient::default();
    let arxiv = ArxivClient::default();
    let pubmed = PubMedClient::default();
    execute_canvas_agent_tool_with_providers(
        runtime,
        agent,
        tools,
        call,
        input,
        CanvasAgentToolProviders {
            tavily,
            wikipedia: &wikipedia,
            duckduckgo: &duckduckgo,
            google: &google,
            google_scholar: &google_scholar,
            github: &github,
            arxiv: &arxiv,
            pubmed: &pubmed,
        },
    )
    .await
}

#[derive(Clone, Copy)]
struct CanvasAgentToolProviders<'a> {
    tavily: &'a dyn TavilyProvider,
    wikipedia: &'a dyn WikipediaProvider,
    duckduckgo: &'a dyn DuckDuckGoProvider,
    google: &'a dyn GoogleProvider,
    google_scholar: &'a dyn GoogleScholarProvider,
    github: &'a dyn GitHubProvider,
    arxiv: &'a dyn ArxivProvider,
    pubmed: &'a dyn PubMedProvider,
}

async fn execute_canvas_agent_tool_with_providers(
    runtime: &mut CanvasRuntime,
    agent: &CanvasNode,
    tools: &[CanvasAgentTool],
    call: &ToolCall,
    input: &WorkflowRunInput<'_>,
    providers: CanvasAgentToolProviders<'_>,
) -> Result<String> {
    let tool = tools
        .iter()
        .find(|tool| tool.function_name == call.function.name)
        .ok_or_else(|| anyhow!("LLM tool {} does not exist", call.function.name))?;
    let arguments: Value = serde_json::from_str(&call.function.arguments).with_context(|| {
        format!(
            "Tool arguments for {} must be a JSON object",
            call.function.name
        )
    })?;
    let arguments = arguments.as_object().ok_or_else(|| {
        anyhow!(
            "Tool arguments for {} must be a JSON object",
            call.function.name
        )
    })?;
    if arguments
        .get("query")
        .is_some_and(|query| !query.is_string())
    {
        bail!(
            "Tool argument query for {} must be a string",
            call.function.name
        );
    }
    let mut params = tool.params.clone();
    params.extend(arguments.clone());
    let child = CanvasNode {
        id: tool.child_id.clone(),
        component_name: tool.component_name.clone(),
        params,
        downstream: Vec::new(),
        upstream: Vec::new(),
        parent_id: Some(agent.id.clone()),
    };
    match child.component_name.to_ascii_lowercase().as_str() {
        "retrieval" => execute_retrieval(runtime, &child, input).await?,
        "tavilysearch" => {
            validate_tavily_search_params(&child)?;
            execute_agent_tavily_with_timeout(runtime, &child, providers.tavily).await?;
        }
        "tavilyextract" => {
            validate_tavily_extract_params(&child)?;
            execute_agent_tavily_with_timeout(runtime, &child, providers.tavily).await?;
        }
        "duckduckgo" => {
            validate_duckduckgo_params(&child)?;
            execute_agent_duckduckgo_with_timeout(runtime, &child, providers.duckduckgo).await?;
        }
        "wikipedia" => {
            validate_wikipedia_params(&child)?;
            execute_agent_wikipedia_with_timeout(runtime, &child, providers.wikipedia).await?;
        }
        "baike" => {
            validate_baike_params(&child)?;
            execute_baike_with_provider(runtime, &child, &BaikeClient::default()).await?;
        }
        "google" => {
            validate_google_params(&child)?;
            execute_agent_google_with_timeout(runtime, &child, providers.google).await?;
        }
        "googlescholar" => {
            validate_google_scholar_params(&child)?;
            execute_agent_google_scholar_with_timeout(runtime, &child, providers.google_scholar)
                .await?;
        }
        "github" => {
            validate_github_params(&child)?;
            execute_agent_github_with_timeout(runtime, &child, providers.github).await?;
        }
        "yahoofinance" => {
            validate_yahoo_finance_params(&child)?;
            execute_agent_yahoo_finance_with_timeout(
                runtime,
                &child,
                &YahooFinanceClient::default(),
            )
            .await?;
        }
        "arxiv" => {
            validate_arxiv_params(&child)?;
            execute_agent_arxiv_with_timeout(runtime, &child, providers.arxiv).await?;
        }
        "pubmed" => {
            validate_pubmed_params(&child)?;
            execute_agent_pubmed_with_timeout(runtime, &child, providers.pubmed).await?;
        }
        "bing" => {
            validate_domestic_search_params(&child, "Bing")?;
            execute_agent_bing_with_timeout(runtime, &child, &BingClient::default()).await?;
        }
        "baidu" => {
            validate_domestic_search_params(&child, "Baidu")?;
            execute_agent_baidu_with_timeout(runtime, &child, &BaiduClient::default()).await?;
        }
        "bocha" => {
            validate_domestic_search_params(&child, "Bocha")?;
            execute_agent_bocha_with_timeout(runtime, &child, &BochaClient::default()).await?;
        }
        "tencentfinance" => {
            validate_domestic_search_params(&child, "TencentFinance")?;
            execute_agent_tencent_finance_with_timeout(
                runtime,
                &child,
                &TencentFinanceClient::default(),
            )
            .await?;
        }
        "baiduscholar" | "baiduscholarsearch" => {
            validate_domestic_search_params(&child, "BaiduScholar")?;
            execute_agent_baidu_scholar_with_timeout(
                runtime,
                &child,
                &BaiduScholarClient::default(),
            )
            .await?;
        }
        "eastmoney" => {
            validate_domestic_search_params(&child, "EastMoney")?;
            execute_agent_eastmoney_with_timeout(runtime, &child, &EastMoneyClient::default())
                .await?;
        }
        "jin10" => {
            validate_domestic_search_params(&child, "Jin10")?;
            execute_agent_jin10_with_timeout(runtime, &child, &Jin10Client::default()).await?;
        }
        "qweather" => {
            validate_domestic_search_params(&child, "QWeather")?;
            execute_agent_qweather_with_timeout(runtime, &child, &QWeatherClient::default())
                .await?;
        }
        "searxng" => {
            validate_domestic_search_params(&child, "SearXNG")?;
            execute_agent_searxng_with_timeout(runtime, &child, &SearxngClient::default()).await?;
        }
        _ => unreachable!("Agent tool component was validated while loading"),
    }
    let outputs = runtime.outputs.get(&child.id).ok_or_else(|| {
        anyhow!(
            "Canvas Agent '{}' tool '{}' produced no outputs",
            agent.id,
            call.function.name
        )
    })?;
    // 错误优先：Agent 工具返回必须带 provider 前缀（Python 工具对齐），
    // 即使 formalized_content 也被写入错误文本（供画布 answer 节点使用）。
    if let Some(error) = outputs.get("_ERROR").and_then(Value::as_str) {
        let provider = match child.component_name.to_ascii_lowercase().as_str() {
            "duckduckgo" => "DuckDuckGo",
            "wikipedia" => "Wikipedia",
            "baike" => "百度百科 Baike",
            "google" => "Google",
            "googlescholar" => "GoogleScholar",
            "github" => "GitHub",
            "yahoofinance" => "YahooFinance",
            "arxiv" => "ArXiv",
            "pubmed" => "PubMed",
            "bing" => "Bing",
            "baidu" => "Baidu",
            "bocha" => "Bocha",
            "tencentfinance" => "TencentFinance",
            "baiduscholar" | "baiduscholarsearch" => "BaiduScholar",
            "eastmoney" => "EastMoney",
            "jin10" => "Jin10",
            "qweather" => "QWeather",
            "searxng" => "SearXNG",
            _ => "Tavily",
        };
        return Ok(format!("{provider} error: {error}"));
    }
    if let Some(content) = outputs
        .get("formalized_content")
        .and_then(Value::as_str)
        .filter(|content| !content.is_empty())
    {
        return Ok(content.to_owned());
    }
    if let Some(report) = outputs.get("report").and_then(Value::as_str) {
        return Ok(report.to_owned());
    }
    if let Some(json) = outputs.get("json") {
        return Ok(serde_json::to_string(json)?);
    }
    Ok(serde_json::to_string(outputs)?)
}

async fn execute_agent_duckduckgo_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    duckduckgo: &dyn DuckDuckGoProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_duckduckgo_search_with_provider(runtime, child, duckduckgo),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_duckduckgo_error(
                runtime,
                child,
                format!("DuckDuckGo timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_wikipedia_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    wikipedia: &dyn WikipediaProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_wikipedia_search_with_provider(runtime, child, wikipedia),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_wikipedia_error(
                runtime,
                child,
                format!("Wikipedia timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_arxiv_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    arxiv: &dyn ArxivProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_arxiv_search_with_provider(runtime, child, arxiv),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_arxiv_error(
                runtime,
                child,
                format!("ArXiv timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_google_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    google: &dyn GoogleProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_google_search_with_provider(runtime, child, google),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_google_error(
                runtime,
                child,
                format!("Google timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_google_scholar_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    google_scholar: &dyn GoogleScholarProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_google_scholar_search_with_provider(runtime, child, google_scholar),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_google_scholar_error(
                runtime,
                child,
                format!(
                    "GoogleScholar timed out after {} seconds",
                    timeout.as_secs()
                ),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_github_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    github: &dyn GitHubProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_github_search_with_provider(runtime, child, github),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_github_error(
                runtime,
                child,
                format!("GitHub timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_yahoo_finance_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    yahoo_finance: &dyn YahooFinanceProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_yahoo_finance_with_provider(runtime, child, yahoo_finance),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_yahoo_finance_error(
                runtime,
                child,
                format!("YahooFinance timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_pubmed_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    pubmed: &dyn PubMedProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_pubmed_search_with_provider(runtime, child, pubmed),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_pubmed_error(
                runtime,
                child,
                format!("PubMed timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_tavily_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    tavily: &dyn TavilyProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    let operation = async {
        if child.component_name.eq_ignore_ascii_case("tavilysearch") {
            execute_tavily_search_with_provider(runtime, child, tavily).await
        } else {
            execute_tavily_extract_with_provider(runtime, child, tavily).await
        }
    };
    match crate::runtime::with_timeout(timeout, operation).await {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_tavily_error(
                runtime,
                child,
                format!(
                    "{} timed out after {} seconds",
                    child.component_name,
                    timeout.as_secs()
                ),
            );
            Ok(())
        }
        result => result,
    }
}

fn canvas_agent_tool_calls_output(calls: &[ToolCall]) -> Value {
    Value::Array(
        calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "id": call.id,
                    "type": call.kind,
                    "name": call.function.name,
                    "arguments": call.function.arguments,
                })
            })
            .collect(),
    )
}

async fn execute_node(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    llm: Option<&LlmClient>,
    input: &WorkflowRunInput<'_>,
) -> Result<Option<Vec<String>>> {
    let kind = node.component_name.to_ascii_lowercase();
    let selected_llm = if matches!(kind.as_str(), "agent" | "generate" | "llm" | "categorize") {
        match (node_llm_selector(node), input.llm_resolver) {
            (Some(selector), Some(resolver)) => {
                Some(resolver.resolve_chat_model(selector).with_context(|| {
                    format!(
                        "Canvas component '{}' could not resolve chat model '{}'",
                        node.id, selector
                    )
                })?)
            }
            _ => None,
        }
    } else {
        None
    };
    let llm = selected_llm.as_ref().or(llm);
    match kind.as_str() {
        "begin" => {
            execute_begin(runtime, node, input)?;
            Ok(None)
        }
        "message" => {
            execute_message(runtime, node)?;
            Ok(None)
        }
        "agent" | "generate" | "llm" => {
            let llm = llm.ok_or_else(|| {
                anyhow!(
                    "Canvas LLM component '{}' requires a configured chat model",
                    node.id
                )
            })?;
            let mut messages = llm_messages(runtime, node, input.history)?;
            let patch =
                merge_generation_patch(input.generation, llm_component_generation_patch(node)?);
            let output_schema = llm_output_schema(node);
            if let Some(schema) = output_schema.as_ref()
                && let Some(system) = messages.first_mut()
            {
                system.content.push_str(&structured_output_prompt(schema));
            }
            let agent_tools = if kind == "agent" {
                load_canvas_agent_tools(node)?
            } else {
                Vec::new()
            };
            let attempts = node
                .params
                .get("max_retries")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .saturating_add(1);
            let mut last_error = String::new();
            for attempt in 0..attempts {
                let completion = if agent_tools.is_empty() {
                    llm.chat_completion_with_generation(&messages, patch)
                        .await
                        .map(|completion| {
                            record_provider_usage(runtime, completion.usage);
                            CanvasAgentToolRun {
                                content: completion.content,
                                calls: Vec::new(),
                            }
                        })
                } else {
                    execute_canvas_agent_tool_loop(
                        runtime,
                        node,
                        llm,
                        input,
                        &messages,
                        patch,
                        &agent_tools,
                    )
                    .await
                };
                match completion {
                    Ok(completion) => {
                        if completion.content.contains("**ERROR**") {
                            last_error.clone_from(&completion.content);
                        } else if output_schema.is_some() {
                            if let Some(structured) = parse_structured_content(&completion.content)
                            {
                                let mut outputs =
                                    Map::from_iter([("structured".into(), structured)]);
                                if !completion.calls.is_empty() {
                                    outputs.insert(
                                        "tool_calls".into(),
                                        canvas_agent_tool_calls_output(&completion.calls),
                                    );
                                }
                                runtime.outputs.insert(node.id.clone(), outputs);
                                return Ok(None);
                            }
                            last_error = "The answer can't not be parsed as JSON".into();
                        } else {
                            let mut outputs = Map::from_iter([(
                                "content".into(),
                                Value::String(completion.content),
                            )]);
                            if !completion.calls.is_empty() {
                                outputs.insert(
                                    "tool_calls".into(),
                                    canvas_agent_tool_calls_output(&completion.calls),
                                );
                            }
                            runtime.outputs.insert(node.id.clone(), outputs);
                            return Ok(None);
                        }
                    }
                    Err(error) => {
                        last_error = format!("Canvas LLM component '{}' failed: {error}", node.id);
                    }
                }
                if attempt + 1 == attempts {
                    break;
                }
            }
            let outputs = if let Some(fallback) = node
                .params
                .get("exception_default_value")
                .and_then(Value::as_str)
                .filter(|fallback| !fallback.is_empty())
            {
                Map::from_iter([("content".into(), Value::String(fallback.to_owned()))])
            } else {
                Map::from_iter([("_ERROR".into(), Value::String(last_error))])
            };
            runtime.outputs.insert(node.id.clone(), outputs);
            Ok(None)
        }
        "categorize" => execute_categorize(runtime, node, llm, input)
            .await
            .map(Some),
        "translate" => {
            let translated =
                execute_translate_with_provider(node, input, &BaiduTranslateClient::default())
                    .await?;
            runtime.outputs.insert(node.id.clone(), translated);
            Ok(None)
        }
        "crawler" => {
            validate_crawler_params(node)?;
            let crawled =
                execute_crawler_with_provider(node, input, &CrawlerClient::default()).await?;
            runtime.outputs.insert(node.id.clone(), crawled);
            Ok(None)
        }
        "akshare" => {
            validate_akshare_params(node)?;
            let news =
                execute_akshare_with_provider(node, input, &AkShareClient::default()).await?;
            runtime.outputs.insert(node.id.clone(), news);
            Ok(None)
        }
        "tushare" => {
            validate_tushare_params(node)?;
            let news =
                execute_tushare_with_provider(node, input, &TuShareClient::default()).await?;
            runtime.outputs.insert(node.id.clone(), news);
            Ok(None)
        }
        "wencai" | "iwencai" => {
            validate_wencai_params(node)?;
            let result =
                execute_wencai_with_provider(node, input, &wencai::WenCaiClient::default()).await?;
            runtime.outputs.insert(node.id.clone(), result);
            Ok(None)
        }
        "email" => {
            validate_email_params(node)?;
            let result = execute_email_with_provider(node, input, &EmailClient)?;
            runtime.outputs.insert(node.id.clone(), result);
            Ok(None)
        }
        "exesql" => {
            validate_exesql_params(node)?;
            let result = execute_exesql_with_provider(node, input).await?;
            runtime.outputs.insert(node.id.clone(), result);
            Ok(None)
        }
        "code_exec" => {
            validate_code_exec_params(node)?;
            let result = execute_code_exec_with_provider(node, input).await?;
            runtime.outputs.insert(node.id.clone(), result);
            Ok(None)
        }
        "retrieval" => {
            execute_retrieval(runtime, node, input).await?;
            Ok(None)
        }
        "tavilysearch" => {
            execute_tavily_search_with_provider(runtime, node, &TavilyClient::default()).await?;
            Ok(None)
        }
        "tavilyextract" => {
            execute_tavily_extract_with_provider(runtime, node, &TavilyClient::default()).await?;
            Ok(None)
        }
        "duckduckgo" => {
            execute_duckduckgo_search_with_provider(runtime, node, &DuckDuckGoClient::default())
                .await?;
            Ok(None)
        }
        "wikipedia" => {
            execute_wikipedia_search_with_provider(runtime, node, &WikipediaClient::default())
                .await?;
            Ok(None)
        }
        "google" => {
            execute_google_search_with_provider(runtime, node, &GoogleClient::default()).await?;
            Ok(None)
        }
        "googlescholar" => {
            execute_google_scholar_search_with_provider(
                runtime,
                node,
                &GoogleScholarClient::default(),
            )
            .await?;
            Ok(None)
        }
        "github" => {
            execute_github_search_with_provider(runtime, node, &GitHubClient::default()).await?;
            Ok(None)
        }
        "yahoofinance" => {
            execute_yahoo_finance_with_provider(runtime, node, &YahooFinanceClient::default())
                .await?;
            Ok(None)
        }
        "arxiv" => {
            execute_arxiv_search_with_provider(runtime, node, &ArxivClient::default()).await?;
            Ok(None)
        }
        "pubmed" => {
            execute_pubmed_search_with_provider(runtime, node, &PubMedClient::default()).await?;
            Ok(None)
        }
        "bing" => {
            execute_bing_search_with_provider(runtime, node, &BingClient::default()).await?;
            Ok(None)
        }
        "baidu" => {
            execute_baidu_search_with_provider(runtime, node, &BaiduClient::default()).await?;
            Ok(None)
        }
        "bocha" => {
            execute_bocha_search_with_provider(runtime, node, &BochaClient::default()).await?;
            Ok(None)
        }
        "tencentfinance" => {
            execute_tencent_finance_with_provider(runtime, node, &TencentFinanceClient::default())
                .await?;
            Ok(None)
        }
        "baiduscholar" | "baiduscholarsearch" => {
            execute_baidu_scholar_with_provider(runtime, node, &BaiduScholarClient::default())
                .await?;
            Ok(None)
        }
        "eastmoney" => {
            execute_eastmoney_with_provider(runtime, node, &EastMoneyClient::default()).await?;
            Ok(None)
        }
        "jin10" => {
            execute_jin10_with_provider(runtime, node, &Jin10Client::default()).await?;
            Ok(None)
        }
        "qweather" => {
            execute_qweather_with_provider(runtime, node, &QWeatherClient::default()).await?;
            Ok(None)
        }
        "searxng" => {
            execute_searxng_with_provider(runtime, node, &SearxngClient::default()).await?;
            Ok(None)
        }
        "baike" => {
            execute_baike_with_provider(runtime, node, &BaikeClient::default()).await?;
            Ok(None)
        }
        "invoke" => {
            execute_invoke(runtime, node, input).await?;
            Ok(None)
        }
        "switch" => execute_switch(runtime, node).map(Some),
        "variableaggregator" => {
            execute_variable_aggregator(runtime, node)?;
            Ok(None)
        }
        "variableassigner" => {
            execute_variable_assigner(runtime, node)?;
            Ok(None)
        }
        "stringtransform" => {
            execute_string_transform(runtime, node)?;
            Ok(None)
        }
        "listoperations" => {
            execute_list_operations(runtime, node)?;
            Ok(None)
        }
        "dataoperations" => {
            execute_data_operations(runtime, node)?;
            Ok(None)
        }
        "docgenerator" | "docsgenerator" => {
            execute_doc_generator(runtime, node)?;
            Ok(None)
        }
        "excelprocessor" => {
            execute_excel_processor(runtime, node)?;
            Ok(None)
        }
        "userfillup" => {
            bail!(
                "Canvas UserFillUp '{}' requires the interactive workflow scheduler",
                node.id
            )
        }
        "loop" | "parallel" | "exitloop" => {
            // Loop and Parallel are expanded by AgentWorkflow into nested
            // runtime sub-graphs. Component-level debug invocation and the
            // legacy ExitLoop sentinel retain the fixed no-op marker contract.
            runtime.outputs.entry(node.id.clone()).or_default();
            Ok(None)
        }
        _ => bail!(
            "Canvas component '{}' ({}) is not implemented",
            node.id,
            node.component_name
        ),
    }
}

const MAX_DOC_GENERATOR_BYTES: usize = 16 << 20;

fn validate_doc_generator_params(node: &CanvasNode) -> Result<()> {
    let content = node
        .params
        .get("content")
        .ok_or_else(|| anyhow!("DocGenerator '{}' content cannot be empty", node.id))?;
    match content {
        Value::String(content) if !content.is_empty() => {}
        Value::String(_) => bail!("DocGenerator '{}' content cannot be empty", node.id),
        _ => bail!("DocGenerator '{}' content must be a string", node.id),
    }
    for field in [
        "output_format",
        "filename",
        "header_text",
        "footer_text",
        "watermark_text",
        "header",
        "footer",
        "watermark",
    ] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("DocGenerator '{}' {field} must be a string", node.id);
        }
    }
    let format = node
        .params
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("pdf")
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        format.as_str(),
        "pdf" | "docx" | "txt" | "markdown" | "md" | "html"
    ) {
        bail!(
            "DocGenerator '{}' output_format must be one of pdf, docx, txt, markdown, html",
            node.id
        );
    }
    for field in [
        "add_page_numbers",
        "add_timestamp",
        "include_download_info_in_content",
    ] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_boolean())
        {
            bail!("DocGenerator '{}' {field} must be a boolean", node.id);
        }
    }
    let font_size = node
        .params
        .get("font_size")
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                anyhow!(
                    "DocGenerator '{}' font_size must be a positive integer",
                    node.id
                )
            })
        })
        .transpose()?
        .unwrap_or(12);
    if !(12..=u64::from(u16::MAX)).contains(&font_size) {
        bail!(
            "DocGenerator '{}' font_size must be between 12 and {}",
            node.id,
            u16::MAX
        );
    }
    Ok(())
}

fn execute_doc_generator(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let raw_content = node
        .params
        .get("content")
        .and_then(Value::as_str)
        .expect("DocGenerator content is validated");
    let content = strip_thinking_blocks(&resolve_template_for_display(runtime, raw_content));
    let format = node
        .params
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("pdf")
        .trim()
        .to_ascii_lowercase();
    let canonical_format = if format == "md" { "markdown" } else { &format };
    let extension = if canonical_format == "markdown" {
        "md"
    } else {
        canonical_format
    };
    let generated_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("UTC OffsetDateTime always has an RFC3339 representation");
    let header = doc_generator_string_param(node, "header_text", "header");
    let footer = doc_generator_string_param(node, "footer_text", "footer");
    let watermark = doc_generator_string_param(node, "watermark_text", "watermark");
    let options = crate::document_writer::DocumentWriterOptions {
        header_text: header,
        footer_text: footer,
        watermark_text: watermark,
        add_page_numbers: node
            .params
            .get("add_page_numbers")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        add_timestamp: node
            .params
            .get("add_timestamp")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        font_size: node
            .params
            .get("font_size")
            .and_then(Value::as_u64)
            .unwrap_or(12) as u16,
        font_family: "Noto Sans CJK SC",
        generated_at: &generated_at,
    };
    let (bytes, mime_type) = match canonical_format {
        "pdf" => (
            crate::document_writer::write_pdf(&content, &options)?,
            "application/pdf",
        ),
        "docx" => (
            crate::document_writer::write_docx(&content, &options)?,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        "txt" => (
            crate::document_writer::write_txt(&content, &options),
            "text/plain",
        ),
        "markdown" => (
            crate::document_writer::write_markdown(&content, &options),
            "text/markdown",
        ),
        "html" => (
            crate::document_writer::write_html(&content, &options),
            "text/html",
        ),
        _ => unreachable!("DocGenerator output format is validated"),
    };
    if bytes.is_empty() {
        bail!("DocGenerator '{}' generated an empty document", node.id);
    }
    if bytes.len() > MAX_DOC_GENERATOR_BYTES {
        bail!(
            "DocGenerator '{}' generated file exceeds the 16 MiB safety limit",
            node.id
        );
    }
    let filename = build_doc_generator_filename(
        node.params
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        extension,
    );
    let doc_id = uuid::Uuid::new_v4().to_string();
    let encoded = BASE64_STANDARD.encode(&bytes);
    let preview_url = format!(
        "/api/v1/agents/attachments/{doc_id}/preview?ext={canonical_format}&mime_type={}",
        mime_type.replace('/', "%2F").replace('+', "%2B")
    );
    let include_download_info_in_content = node
        .params
        .get("include_download_info_in_content")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let download_info = serde_json::json!({
        "doc_id": doc_id,
        "filename": filename,
        "mime_type": mime_type,
        "size": bytes.len(),
        "base64": encoded,
        "preview_url": preview_url,
        "include_download_info_in_content": include_download_info_in_content
    });
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("doc_id".into(), Value::String(doc_id.clone())),
            ("filename".into(), Value::String(filename.clone())),
            ("mime_type".into(), Value::String(mime_type.into())),
            ("size".into(), Value::from(bytes.len())),
            ("bytes".into(), Value::String(encoded.clone())),
            (
                "download".into(),
                Value::String(serde_json::to_string(&download_info)?),
            ),
            (
                "attachment".into(),
                serde_json::json!({
                    "doc_id": doc_id,
                    "format": canonical_format,
                    "file_name": filename,
                    "filename": filename,
                    "mime_type": mime_type,
                    "data": format!("data:{mime_type};base64,{encoded}")
                }),
            ),
            ("preview_url".into(), Value::String(preview_url)),
            ("created".into(), Value::String(generated_at)),
        ]),
    );
    Ok(())
}

fn doc_generator_string_param<'a>(node: &'a CanvasNode, primary: &str, alias: &str) -> &'a str {
    node.params
        .get(primary)
        .and_then(Value::as_str)
        .or_else(|| node.params.get(alias).and_then(Value::as_str))
        .unwrap_or_default()
}

fn strip_thinking_blocks(content: &str) -> String {
    static COMPLETE: OnceLock<Regex> = OnceLock::new();
    static DANGLING: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static NEWLINES: OnceLock<Regex> = OnceLock::new();
    let content = COMPLETE
        .get_or_init(|| Regex::new(r"(?s)<think>.*?</think>").expect("valid think regex"))
        .replace_all(content, "");
    let content = DANGLING
        .get_or_init(|| Regex::new(r"(?s)<think>.*$").expect("valid dangling think regex"))
        .replace_all(&content, "");
    let content = TAG
        .get_or_init(|| Regex::new(r"</?think>").expect("valid think tag regex"))
        .replace_all(&content, "");
    NEWLINES
        .get_or_init(|| Regex::new(r"\n{3,}").expect("valid newline regex"))
        .replace_all(content.trim(), "\n\n")
        .into_owned()
}

fn build_doc_generator_filename(raw: &str, extension: &str) -> String {
    let mut name = raw.trim().to_owned();
    if name.is_empty() {
        let now = time::OffsetDateTime::now_utc();
        let uuid = uuid::Uuid::new_v4().simple().to_string();
        return format!(
            "document_{:04}{:02}{:02}_{:02}{:02}{:02}_{}.{}",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
            &uuid[..8],
            extension
        );
    }
    name = name
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '\\' | '/' | '?' | '#' | '%' | '*' | ':' | '|' | '<' | '>' | '"'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    name = name.trim_matches([' ', '.']).to_owned();
    if name.is_empty() {
        return format!("file.{extension}");
    }
    if let Some(dot) = name.rfind('.') {
        name.truncate(dot);
    }
    name = name.chars().take(180).collect::<String>();
    name = name.trim_end().to_owned();
    if name.is_empty() {
        name = "file".into();
    }
    format!("{name}.{extension}")
}

const DEFAULT_INVOKE_TIMEOUT: Duration = Duration::from_secs(60);
const DEFAULT_INVOKE_RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_INVOKE_RESPONSE_BODY: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InvokeDataType {
    Json,
    FormData,
}

#[derive(Debug, Clone)]
struct InvokeVariable {
    key: String,
    reference: String,
    value: Option<Value>,
}

#[derive(Debug, Clone)]
struct InvokeConfig {
    method: String,
    url: String,
    timeout: Duration,
    headers: String,
    proxy: String,
    clean_html: bool,
    datatype: InvokeDataType,
    variables: Vec<InvokeVariable>,
    max_retries: usize,
    delay_after_error: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InvokeEndpointPin {
    hostname: String,
    ip: IpAddr,
}

#[derive(Debug, Clone)]
struct InvokeHttpRequest {
    method: String,
    url: reqwest::Url,
    headers: BTreeMap<String, String>,
    arguments: Map<String, Value>,
    datatype: InvokeDataType,
    timeout: Duration,
    target: InvokeEndpointPin,
    proxy: Option<(reqwest::Url, InvokeEndpointPin)>,
}

#[async_trait::async_trait]
trait InvokeNetwork: Send + Sync {
    async fn resolve(&self, hostname: &str, port: u16) -> Result<Vec<IpAddr>>;
    async fn send(&self, request: InvokeHttpRequest) -> Result<Vec<u8>>;
}

struct ReqwestInvokeNetwork;

#[async_trait::async_trait]
impl InvokeNetwork for ReqwestInvokeNetwork {
    async fn resolve(&self, hostname: &str, port: u16) -> Result<Vec<IpAddr>> {
        let addresses = tokio::net::lookup_host((hostname, port))
            .await
            .with_context(|| format!("could not resolve hostname '{hostname}'"))?;
        let mut resolved = Vec::new();
        for address in addresses {
            if !resolved.contains(&address.ip()) {
                resolved.push(address.ip());
            }
        }
        Ok(resolved)
    }

    async fn send(&self, request: InvokeHttpRequest) -> Result<Vec<u8>> {
        let target_port = request
            .url
            .port_or_known_default()
            .ok_or_else(|| anyhow!("Invoke URL has no usable port"))?;
        let mut client = reqwest::Client::builder()
            .no_proxy()
            .redirect(RedirectPolicy::none())
            .timeout(request.timeout)
            .resolve(
                &request.target.hostname,
                SocketAddr::new(request.target.ip, target_port),
            );
        if let Some((proxy_url, proxy_pin)) = &request.proxy {
            let proxy_port = proxy_url
                .port_or_known_default()
                .ok_or_else(|| anyhow!("Invoke proxy URL has no usable port"))?;
            client = client
                .resolve(
                    &proxy_pin.hostname,
                    SocketAddr::new(proxy_pin.ip, proxy_port),
                )
                .proxy(reqwest::Proxy::all(proxy_url.as_str())?);
        }
        let client = client.build()?;
        let mut builder = match request.method.as_str() {
            "get" => client.get(request.url),
            "post" => client.post(request.url),
            "put" => client.put(request.url),
            _ => unreachable!("Invoke method was validated before transport"),
        };
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        builder = if request.method == "get" {
            builder.query(&invoke_form_pairs(&request.arguments))
        } else if request.datatype == InvokeDataType::Json {
            builder.json(&Value::Object(request.arguments))
        } else {
            builder.form(&invoke_form_pairs(&request.arguments))
        };

        let response = builder.send().await?;
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let remaining = MAX_INVOKE_RESPONSE_BODY.saturating_sub(body.len());
            if remaining == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        }
        Ok(body)
    }
}

static REQWEST_INVOKE_NETWORK: ReqwestInvokeNetwork = ReqwestInvokeNetwork;

fn validate_invoke_params(node: &CanvasNode) -> Result<()> {
    invoke_config(node).map(|_| ())
}

fn invoke_config(node: &CanvasNode) -> Result<InvokeConfig> {
    let method = node
        .params
        .get("method")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("Invoke '{}' method must be a string", node.id))
        })
        .transpose()?
        .unwrap_or("get")
        .to_ascii_lowercase();
    if !matches!(method.as_str(), "get" | "post" | "put") {
        bail!("Invoke '{}' method must be GET, POST, or PUT", node.id);
    }

    let url = node
        .params
        .get("url")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("Invoke '{}' url must be a string", node.id))
        })
        .transpose()?
        .unwrap_or_default()
        .to_owned();
    if url.is_empty() {
        bail!("Invoke '{}' url cannot be empty", node.id);
    }

    let timeout = invoke_positive_seconds(node, "timeout", DEFAULT_INVOKE_TIMEOUT)?;
    let headers = match node.params.get("headers") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(_) => bail!("Invoke '{}' headers must be a JSON string", node.id),
    };
    let proxy = match node.params.get("proxy") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(_) => bail!("Invoke '{}' proxy must be a string", node.id),
    };
    let clean_html = node
        .params
        .get("clean_html")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| anyhow!("Invoke '{}' clean_html must be a boolean", node.id))
        })
        .transpose()?
        .unwrap_or(false);
    let datatype = node
        .params
        .get("datatype")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("Invoke '{}' datatype must be a string", node.id))
        })
        .transpose()?
        .unwrap_or("json")
        .to_ascii_lowercase();
    let datatype = match datatype.as_str() {
        "json" => InvokeDataType::Json,
        "formdata" => InvokeDataType::FormData,
        _ => bail!("Invoke '{}' datatype must be 'json' or 'formdata'", node.id),
    };
    let variables = invoke_variables(node)?;
    let max_retries = node
        .params
        .get("max_retries")
        .map(|value| {
            value
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    anyhow!(
                        "Invoke '{}' max_retries must be a non-negative integer",
                        node.id
                    )
                })
        })
        .transpose()?
        .unwrap_or(0);
    let delay_after_error = node
        .params
        .get("delay_after_error")
        .map(|value| {
            let seconds = value.as_f64().ok_or_else(|| {
                anyhow!(
                    "Invoke '{}' delay_after_error must be a non-negative number",
                    node.id
                )
            })?;
            if !seconds.is_finite() || seconds < 0.0 {
                bail!(
                    "Invoke '{}' delay_after_error must be a non-negative number",
                    node.id
                );
            }
            Duration::try_from_secs_f64(seconds).map_err(|_| {
                anyhow!(
                    "Invoke '{}' delay_after_error is outside the supported range",
                    node.id
                )
            })
        })
        .transpose()?
        .unwrap_or(DEFAULT_INVOKE_RETRY_DELAY);

    Ok(InvokeConfig {
        method,
        url,
        timeout,
        headers,
        proxy,
        clean_html,
        datatype,
        variables,
        max_retries,
        delay_after_error,
    })
}

fn invoke_positive_seconds(node: &CanvasNode, field: &str, default: Duration) -> Result<Duration> {
    let Some(value) = node.params.get(field) else {
        return Ok(default);
    };
    let seconds = value
        .as_u64()
        .ok_or_else(|| anyhow!("Invoke '{}' {field} must be a positive integer", node.id))?;
    if seconds == 0 {
        bail!("Invoke '{}' {field} must be a positive integer", node.id);
    }
    Ok(Duration::from_secs(seconds))
}

fn invoke_variables(node: &CanvasNode) -> Result<Vec<InvokeVariable>> {
    let Some(raw_variables) = node.params.get("variables") else {
        return Ok(Vec::new());
    };
    let raw_variables = raw_variables
        .as_array()
        .ok_or_else(|| anyhow!("Invoke '{}' variables must be an array", node.id))?;
    raw_variables
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let variable = value.as_object().ok_or_else(|| {
                anyhow!("Invoke '{}' variables[{index}] must be an object", node.id)
            })?;
            let key = variable.get("key").and_then(Value::as_str).ok_or_else(|| {
                anyhow!(
                    "Invoke '{}' variables[{index}].key must be a string",
                    node.id
                )
            })?;
            let reference = match variable.get("ref") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(value)) => value.trim().to_owned(),
                Some(_) => {
                    bail!(
                        "Invoke '{}' variables[{index}].ref must be a string",
                        node.id
                    )
                }
            };
            let value = variable
                .get("value")
                .filter(|value| !value.is_null())
                .cloned();
            Ok(InvokeVariable {
                key: key.to_owned(),
                reference,
                value,
            })
        })
        .collect()
}

async fn execute_invoke(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
) -> Result<()> {
    execute_invoke_with_network(runtime, node, input, &REQWEST_INVOKE_NETWORK).await
}

async fn execute_invoke_with_network(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
    network: &dyn InvokeNetwork,
) -> Result<()> {
    if runtime.cancel_flag {
        set_invoke_error(runtime, node, "Task has been canceled");
        return Ok(());
    }
    let config = match invoke_config(node) {
        Ok(config) => config,
        Err(error) => {
            set_invoke_error(runtime, node, error.to_string());
            return Ok(());
        }
    };
    let arguments = match build_invoke_arguments(runtime, input, &config) {
        Ok(arguments) => arguments,
        Err(error) => {
            set_invoke_error(runtime, node, error.to_string());
            return Ok(());
        }
    };
    let headers = match build_invoke_headers(runtime, input, &config.headers) {
        Ok(headers) => headers,
        Err(error) => {
            set_invoke_error(runtime, node, error.to_string());
            return Ok(());
        }
    };
    let proxy_url = match normalize_invoke_proxy(&config.proxy) {
        Ok(proxy_url) => proxy_url,
        Err(_) => {
            set_invoke_error(runtime, node, "URL not valid");
            return Ok(());
        }
    };
    let proxy = if let Some(proxy_url) = proxy_url {
        match validate_and_pin_invoke_url(network, &proxy_url).await {
            Ok(pin) => Some((proxy_url, pin)),
            Err(_) => {
                set_invoke_error(runtime, node, "URL not valid");
                return Ok(());
            }
        }
    } else {
        None
    };

    let attempts = config.max_retries.saturating_add(1);
    let mut last_error = None;
    for _ in 0..attempts {
        if runtime.cancel_flag {
            set_invoke_error(runtime, node, "Task has been canceled");
            return Ok(());
        }
        let url = match build_invoke_url(runtime, input, &config.url) {
            Ok(url) => url,
            Err(_) => {
                set_invoke_error(runtime, node, "URL not valid");
                return Ok(());
            }
        };
        let target = match validate_and_pin_invoke_url(network, &url).await {
            Ok(pin) => pin,
            Err(_) => {
                set_invoke_error(runtime, node, "URL not valid");
                return Ok(());
            }
        };
        // A forwarding proxy resolves a hostname target itself, outside the
        // validated/pinned client dial. The fixed Go component closes that DNS
        // rebinding window by requiring a literal public target in proxy mode.
        if proxy.is_some()
            && url
                .host_str()
                .is_none_or(|hostname| parse_invoke_ip_literal(hostname).is_none())
        {
            set_invoke_error(runtime, node, "URL not valid");
            return Ok(());
        }
        let request = InvokeHttpRequest {
            method: config.method.clone(),
            url,
            headers: headers.clone(),
            arguments: arguments.clone(),
            datatype: config.datatype,
            timeout: config.timeout,
            target,
            proxy: proxy.clone(),
        };
        match network.send(request).await {
            Ok(mut body) => {
                body.truncate(MAX_INVOKE_RESPONSE_BODY);
                let mut result = String::from_utf8_lossy(&body).into_owned();
                if config.clean_html {
                    result = crate::parser::html::HtmlParser::strip_tags(&result);
                }
                runtime.outputs.insert(
                    node.id.clone(),
                    Map::from_iter([("result".into(), Value::String(result))]),
                );
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !config.delay_after_error.is_zero() {
            tokio::time::sleep(config.delay_after_error).await;
        }
    }
    if let Some(error) = last_error {
        set_invoke_error(runtime, node, error);
    }
    Ok(())
}

fn set_invoke_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: impl Into<String>) {
    let error_string = error.into();
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error_string.clone())),
            (
                "formalized_content".into(),
                Value::String(format!("[Tool error] {error_string}")),
            ),
        ]),
    );
}

fn build_invoke_arguments(
    runtime: &CanvasRuntime,
    input: &WorkflowRunInput<'_>,
    config: &InvokeConfig,
) -> Result<Map<String, Value>> {
    let mut arguments = Map::new();
    for variable in &config.variables {
        let mut value = resolve_invoke_argument(runtime, input, variable);
        if config.datatype == InvokeDataType::Json
            && let Value::String(encoded) = &value
            && let Ok(decoded) = serde_json::from_str::<Value>(encoded)
        {
            value = decoded;
        }
        arguments.insert(variable.key.clone(), value);
    }
    Ok(arguments)
}

fn resolve_invoke_argument(
    runtime: &CanvasRuntime,
    input: &WorkflowRunInput<'_>,
    variable: &InvokeVariable,
) -> Value {
    if !variable.reference.is_empty() {
        let override_value = input.inputs.get(&variable.reference).cloned();
        let canvas_value = get_variable(runtime, &variable.reference)
            .ok()
            .filter(|value| !value.is_null());
        if override_value.is_some() || canvas_value.is_some() {
            return normalize_invoke_variable(override_value.or(canvas_value));
        }
    }
    if let Some(value) = &variable.value {
        return match value {
            Value::String(template) => {
                Value::String(render_invoke_template(runtime, input, template))
            }
            value => value.clone(),
        };
    }
    if !variable.reference.is_empty() {
        return resolve_invoke_variable(runtime, input, &variable.reference);
    }
    Value::String(String::new())
}

fn resolve_invoke_variable(
    runtime: &CanvasRuntime,
    input: &WorkflowRunInput<'_>,
    name: &str,
) -> Value {
    normalize_invoke_variable(
        input
            .inputs
            .get(name)
            .cloned()
            .or_else(|| get_variable(runtime, name).ok()),
    )
}

fn normalize_invoke_variable(value: Option<Value>) -> Value {
    match value {
        None | Some(Value::Null) => Value::String(String::new()),
        Some(value) => value,
    }
}

fn render_invoke_template(
    runtime: &CanvasRuntime,
    input: &WorkflowRunInput<'_>,
    template: &str,
) -> String {
    render_invoke_pattern(runtime, input, template, selector_regex())
}

fn render_invoke_header(
    runtime: &CanvasRuntime,
    input: &WorkflowRunInput<'_>,
    template: &str,
) -> String {
    render_invoke_pattern(runtime, input, template, invoke_header_regex())
}

fn render_invoke_pattern(
    runtime: &CanvasRuntime,
    input: &WorkflowRunInput<'_>,
    template: &str,
    pattern: &Regex,
) -> String {
    pattern
        .replace_all(template, |captures: &regex::Captures<'_>| {
            invoke_python_string(&resolve_invoke_variable(runtime, input, &captures[1]))
        })
        .into_owned()
}

fn invoke_header_regex() -> &'static Regex {
    static HEADER: OnceLock<Regex> = OnceLock::new();
    HEADER.get_or_init(|| {
        Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_.@-]*)\}").expect("static Invoke header regex is valid")
    })
}

fn invoke_python_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(value) => value.to_string(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn build_invoke_url(
    runtime: &CanvasRuntime,
    input: &WorkflowRunInput<'_>,
    configured: &str,
) -> Result<reqwest::Url> {
    let mut rendered = render_invoke_template(runtime, input, configured.trim());
    if !rendered.starts_with("http://") && !rendered.starts_with("https://") {
        rendered.insert_str(0, "http://");
    }
    reqwest::Url::parse(&rendered).context("Invoke URL is invalid")
}

fn build_invoke_headers(
    runtime: &CanvasRuntime,
    input: &WorkflowRunInput<'_>,
    configured: &str,
) -> Result<BTreeMap<String, String>> {
    if configured.is_empty() {
        return Ok(BTreeMap::new());
    }
    let parsed: Value =
        serde_json::from_str(configured).context("Invoke headers are not valid JSON")?;
    let object = parsed
        .as_object()
        .ok_or_else(|| anyhow!("Invoke headers must be a JSON object."))?;
    let mut headers = BTreeMap::new();
    for (name, value) in object {
        let value = value
            .as_str()
            .ok_or_else(|| anyhow!("Invoke header '{name}' must be a string"))?;
        let value = render_invoke_header(runtime, input, value);
        reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("Invoke header name '{name}' is invalid"))?;
        reqwest::header::HeaderValue::from_str(&value)
            .with_context(|| format!("Invoke header '{name}' has an invalid value"))?;
        headers.insert(name.clone(), value);
    }
    Ok(headers)
}

fn normalize_invoke_proxy(configured: &str) -> Result<Option<reqwest::Url>> {
    let proxy = configured.trim();
    let without_scheme = proxy
        .strip_prefix("http://")
        .or_else(|| proxy.strip_prefix("https://"))
        .unwrap_or(proxy)
        .trim_matches('/');
    if without_scheme.is_empty() {
        return Ok(None);
    }
    let normalized = if proxy.starts_with("http://") || proxy.starts_with("https://") {
        proxy.to_owned()
    } else {
        format!("http://{proxy}")
    };
    Ok(Some(
        reqwest::Url::parse(&normalized).context("Invoke proxy URL is invalid")?,
    ))
}

async fn validate_and_pin_invoke_url(
    network: &dyn InvokeNetwork,
    url: &reqwest::Url,
) -> Result<InvokeEndpointPin> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("Invoke URL scheme is not allowed");
    }
    let raw_hostname = url
        .host_str()
        .filter(|hostname| !hostname.is_empty())
        .ok_or_else(|| anyhow!("Invoke URL is missing a host"))?;
    let hostname = raw_hostname
        .strip_prefix('[')
        .and_then(|hostname| hostname.strip_suffix(']'))
        .unwrap_or(raw_hostname);
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("Invoke URL has no usable port"))?;
    let addresses = if let Some(ip) = parse_invoke_ip_literal(hostname) {
        vec![ip]
    } else {
        network.resolve(hostname, port).await?
    };
    if addresses.is_empty() || addresses.iter().any(|ip| !invoke_ip_is_public(*ip)) {
        bail!("Invoke URL resolves to a non-public address");
    }
    Ok(InvokeEndpointPin {
        hostname: hostname.to_owned(),
        ip: addresses[0],
    })
}

fn parse_invoke_ip_literal(hostname: &str) -> Option<IpAddr> {
    hostname
        .strip_prefix('[')
        .and_then(|hostname| hostname.strip_suffix(']'))
        .unwrap_or(hostname)
        .parse()
        .ok()
}

fn invoke_ip_is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => invoke_ipv4_is_public(ip),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(invoke_ipv4_is_public)
            .unwrap_or_else(|| invoke_ipv6_is_public(ip)),
    }
}

fn invoke_ipv4_is_public(ip: Ipv4Addr) -> bool {
    ![
        (Ipv4Addr::new(0, 0, 0, 0), 8),
        (Ipv4Addr::new(10, 0, 0, 0), 8),
        (Ipv4Addr::new(100, 64, 0, 0), 10),
        (Ipv4Addr::new(127, 0, 0, 0), 8),
        (Ipv4Addr::new(169, 254, 0, 0), 16),
        (Ipv4Addr::new(172, 16, 0, 0), 12),
        (Ipv4Addr::new(192, 0, 0, 0), 24),
        (Ipv4Addr::new(192, 0, 2, 0), 24),
        (Ipv4Addr::new(192, 88, 99, 0), 24),
        (Ipv4Addr::new(192, 168, 0, 0), 16),
        (Ipv4Addr::new(198, 18, 0, 0), 15),
        (Ipv4Addr::new(198, 51, 100, 0), 24),
        (Ipv4Addr::new(203, 0, 113, 0), 24),
        (Ipv4Addr::new(224, 0, 0, 0), 4),
        (Ipv4Addr::new(240, 0, 0, 0), 4),
    ]
    .iter()
    .any(|(network, prefix)| ipv4_has_prefix(ip, *network, *prefix))
}

fn ipv4_has_prefix(ip: Ipv4Addr, network: Ipv4Addr, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (u32::BITS - prefix)
    };
    u32::from(ip) & mask == u32::from(network) & mask
}

fn invoke_ipv6_is_public(ip: Ipv6Addr) -> bool {
    ![
        (Ipv6Addr::UNSPECIFIED, 128),
        (Ipv6Addr::LOCALHOST, 128),
        (Ipv6Addr::new(0x0064, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
        (Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0, 0), 64),
        (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
        (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
        (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
        (Ipv6Addr::new(0x5f00, 0, 0, 0, 0, 0, 0, 0), 16),
        (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
        (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
        (Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10),
        (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
    ]
    .iter()
    .any(|(network, prefix)| ipv6_has_prefix(ip, *network, *prefix))
}

fn ipv6_has_prefix(ip: Ipv6Addr, network: Ipv6Addr, prefix: u32) -> bool {
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (u128::BITS - prefix)
    };
    u128::from(ip) & mask == u128::from(network) & mask
}

fn invoke_form_pairs(arguments: &Map<String, Value>) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for (key, value) in arguments {
        invoke_form_value(key, value, &mut pairs);
    }
    pairs
}

fn invoke_form_value(key: &str, value: &Value, pairs: &mut Vec<(String, String)>) {
    match value {
        Value::Null => {}
        Value::Array(values) => {
            for value in values {
                invoke_form_value(key, value, pairs);
            }
        }
        value => pairs.push((key.to_owned(), invoke_python_string(value))),
    }
}

fn node_llm_selector(node: &CanvasNode) -> Option<&str> {
    node.params
        .get("llm_id")
        .or_else(|| node.params.get("model_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|selector| !selector.is_empty())
}

const DEFAULT_LOOP_MAX_ITERATIONS: usize = 1024;
const DEFAULT_USER_FILL_UP_TIPS: &str = "Please fill up the form";

fn validate_user_fill_up_params(node: &CanvasNode) -> Result<()> {
    if node
        .params
        .get("inputs")
        .is_some_and(|value| !value.is_object())
    {
        bail!("UserFillUp '{}' inputs must be an object", node.id);
    }
    if node
        .params
        .get("enable_tips")
        .is_some_and(|value| !value.is_boolean())
    {
        bail!("UserFillUp '{}' enable_tips must be a boolean", node.id);
    }
    for field in ["tips", "layout_recognize"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("UserFillUp '{}' {field} must be a string", node.id);
        }
    }
    Ok(())
}

fn workflow_waiting_for_user(
    runtime: &CanvasRuntime,
    node: &CanvasNode,
    trace: Vec<WorkflowNodeTrace>,
    checkpoint: AgentWorkflowCheckpoint,
) -> WorkflowWaitingForUser {
    let enable_tips = node
        .params
        .get("enable_tips")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let tips = enable_tips.then(|| {
        resolve_template_for_display(
            runtime,
            node.params
                .get("tips")
                .and_then(Value::as_str)
                .unwrap_or(DEFAULT_USER_FILL_UP_TIPS),
        )
    });
    WorkflowWaitingForUser {
        component_id: node.id.clone(),
        interrupt_id: node.id.clone(),
        tips,
        inputs: user_fill_up_fields(node),
        path: runtime.path.clone(),
        trace,
        checkpoint,
    }
}

fn user_fill_up_fields(node: &CanvasNode) -> Map<String, Value> {
    node.params
        .get("inputs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// Execute the legacy initial-input fast path. `false` means the node must
/// interrupt; `true` means outputs were written and normal scheduling may
/// continue.
fn execute_user_fill_up_initial(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
) -> Result<bool> {
    let fields = user_fill_up_fields(node);
    if fields.is_empty()
        || runtime
            .sys
            .get("__initial_user_input_consumed__")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        return Ok(false);
    }

    let explicit: Map<String, Value> = fields
        .keys()
        .filter_map(|name| {
            input
                .inputs
                .get(name)
                .cloned()
                .map(|value| (name.clone(), value))
        })
        .collect();
    let (data, consumed_initial_query) = if !explicit.is_empty() {
        (Value::Object(explicit), false)
    } else {
        match runtime.sys.get("query").cloned().unwrap_or(Value::Null) {
            Value::Object(values) => {
                let matched = values
                    .into_iter()
                    .filter(|(name, _)| fields.contains_key(name))
                    .collect::<Map<_, _>>();
                if matched.is_empty() {
                    return Ok(false);
                }
                (Value::Object(matched), true)
            }
            Value::String(value) if !value.is_empty() && fields.len() == 1 => {
                (Value::String(value), true)
            }
            _ => return Ok(false),
        }
    };

    execute_user_fill_up_resume(runtime, node, data)?;
    if consumed_initial_query {
        runtime
            .sys
            .insert("__initial_user_input_consumed__".into(), Value::Bool(true));
    }
    Ok(true)
}

fn execute_user_fill_up_resume(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    data: Value,
) -> Result<()> {
    let fields = user_fill_up_fields(node);
    let mut outputs = Map::from_iter([
        ("user_input".into(), data.clone()),
        (node.id.clone(), data.clone()),
    ]);
    match &data {
        Value::Object(values) => {
            for (name, schema) in &fields {
                if let Some(value) = values.get(name) {
                    outputs.insert(
                        name.clone(),
                        resolve_user_fill_up_value(node, name, schema, value.clone())?,
                    );
                }
            }
        }
        value if fields.len() == 1 => {
            let (name, schema) = fields
                .iter()
                .next()
                .expect("one UserFillUp field was checked");
            outputs.insert(
                name.clone(),
                resolve_user_fill_up_value(node, name, schema, value.clone())?,
            );
        }
        _ => {}
    }
    runtime.outputs.insert(node.id.clone(), outputs);
    Ok(())
}

fn resolve_user_fill_up_value(
    node: &CanvasNode,
    field_name: &str,
    schema: &Value,
    raw: Value,
) -> Result<Value> {
    let schema = schema.as_object();
    let raw_descriptor = raw.as_object().filter(|value| {
        value.contains_key("value") || value.contains_key("type") || value.contains_key("optional")
    });
    let input_type = raw_descriptor
        .and_then(|descriptor| descriptor.get("type"))
        .or_else(|| schema.and_then(|schema| schema.get("type")))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let optional = raw_descriptor
        .and_then(|descriptor| descriptor.get("optional"))
        .or_else(|| schema.and_then(|schema| schema.get("optional")))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let descriptor_value = raw_descriptor
        .and_then(|descriptor| descriptor.get("value"))
        .cloned();
    let value = descriptor_value.unwrap_or(raw);

    if input_type.to_ascii_lowercase().contains("file") {
        if optional && value.is_null() {
            return Ok(Value::Null);
        }
        bail!(
            "UserFillUp '{}' file input '{}' requires the file parsing service",
            node.id,
            field_name
        );
    }
    if input_type == "object"
        && let Value::String(text) = &value
        && !text.trim().is_empty()
        && let Ok(decoded) = serde_json::from_str(text)
    {
        return Ok(decoded);
    }
    Ok(value)
}

fn validate_parallel_params(node: &CanvasNode) -> Result<()> {
    if !node
        .params
        .get("items_ref")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        bail!("Parallel '{}' items_ref is required", node.id);
    }
    Ok(())
}

fn validate_loop_params(node: &CanvasNode) -> Result<()> {
    if let Some(raw_variables) = node.params.get("loop_variables") {
        let variables = raw_variables
            .as_array()
            .ok_or_else(|| anyhow!("Loop '{}' loop_variables must be an array", node.id))?;
        for (index, raw) in variables.iter().enumerate() {
            let variable = raw.as_object().ok_or_else(|| {
                anyhow!(
                    "Loop '{}' loop_variable[{index}] must be an object",
                    node.id
                )
            })?;
            let name = variable
                .get("variable")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    anyhow!(
                        "Loop '{}' loop_variable[{index}] is incomplete (missing 'variable')",
                        node.id
                    )
                })?;
            let _ = name;
            if !variable
                .get("input_mode")
                .is_some_and(|value| value.is_string())
            {
                bail!(
                    "Loop '{}' loop_variable[{index}] is incomplete (missing 'input_mode')",
                    node.id
                );
            }
            if !variable.contains_key("value") {
                bail!(
                    "Loop '{}' loop_variable[{index}] is incomplete (missing 'value')",
                    node.id
                );
            }
            if variable.get("type").is_none_or(Value::is_null) {
                bail!(
                    "Loop '{}' loop_variable[{index}] is incomplete (missing 'type')",
                    node.id
                );
            }
        }
    }

    if let Some(raw_conditions) = node.params.get("loop_termination_condition") {
        let conditions = raw_conditions.as_array().ok_or_else(|| {
            anyhow!(
                "Loop '{}' loop_termination_condition must be an array",
                node.id
            )
        })?;
        for (index, raw) in conditions.iter().enumerate() {
            let condition = raw.as_object().ok_or_else(|| {
                anyhow!(
                    "Loop '{}' loop_termination_condition[{index}] must be an object",
                    node.id
                )
            })?;
            if !condition
                .get("variable")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
            {
                bail!(
                    "Loop '{}' loop_termination_condition[{index}] is incomplete (missing 'variable')",
                    node.id
                );
            }
            if !condition
                .get("operator")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
            {
                bail!(
                    "Loop '{}' loop_termination_condition[{index}] is incomplete (missing 'operator')",
                    node.id
                );
            }
            if condition
                .get("input_mode")
                .is_some_and(|value| !value.is_string())
            {
                bail!(
                    "Loop '{}' loop_termination_condition[{index}] input_mode must be a string",
                    node.id
                );
            }
        }
    }

    let logical_operator = node
        .params
        .get("logical_operator")
        .and_then(Value::as_str)
        .filter(|operator| !operator.is_empty())
        .unwrap_or("and");
    if !matches!(logical_operator, "and" | "or") {
        bail!("Loop '{}' logical_operator must be 'and' or 'or'", node.id);
    }
    Ok(())
}

fn validate_begin_params(node: &CanvasNode) -> Result<()> {
    let mode = node
        .params
        .get("mode")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("Begin '{}' mode must be a string", node.id))
        })
        .transpose()?
        .unwrap_or("conversational");
    if !matches!(mode, "conversational" | "task" | "Webhook") {
        bail!(
            "Begin '{}' mode must be 'conversational', 'task' or 'Webhook'",
            node.id
        );
    }
    if node
        .params
        .get("inputs")
        .is_some_and(|value| !value.is_object())
    {
        bail!("Begin '{}' inputs must be an object", node.id);
    }
    Ok(())
}

fn execute_begin(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
) -> Result<()> {
    let fields = node.params.get("inputs").and_then(Value::as_object);
    let mut merged = input.inputs.clone();
    let mut consumed_initial_query = false;
    if merged.is_empty()
        && let Some(fields) = fields.filter(|fields| !fields.is_empty())
    {
        let query = get_variable(runtime, "sys.query").unwrap_or_default();
        if let Value::Object(values) = &query {
            for key in fields.keys() {
                if let Some(value) = values.get(key) {
                    merged.insert(
                        key.clone(),
                        if value.is_object() {
                            value.clone()
                        } else {
                            serde_json::json!({ "value": value })
                        },
                    );
                }
            }
            consumed_initial_query = !merged.is_empty();
        } else if fields.len() == 1 && truthy(&query) {
            let key = fields.keys().next().expect("one Begin input field exists");
            merged.insert(key.clone(), serde_json::json!({ "value": query }));
            consumed_initial_query = true;
        }
    }

    let mut outputs = Map::new();
    for (key, value) in merged {
        outputs.insert(key.clone(), resolve_begin_input(node, &key, value)?);
    }
    runtime.outputs.insert(node.id.clone(), outputs);
    if consumed_initial_query {
        runtime
            .sys
            .insert("__initial_user_input_consumed__".into(), Value::Bool(true));
    }
    Ok(())
}

fn resolve_begin_input(node: &CanvasNode, key: &str, value: Value) -> Result<Value> {
    let Value::Object(mut descriptor) = value else {
        return Ok(value);
    };
    let input_type = descriptor
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if input_type.to_ascii_lowercase().contains("file") {
        if descriptor
            .get("optional")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && descriptor.get("value").is_none_or(Value::is_null)
        {
            return Ok(Value::Null);
        }
        bail!(
            "Begin '{}' file input '{}' requires the file parsing service",
            node.id,
            key
        );
    }
    let raw = descriptor.remove("value").unwrap_or(Value::Null);
    if input_type == "object"
        && let Value::String(text) = &raw
        && !text.trim().is_empty()
        && let Ok(decoded) = serde_json::from_str(text)
    {
        return Ok(decoded);
    }
    Ok(raw)
}

fn record_provider_usage(runtime: &mut CanvasRuntime, usage: Option<TokenUsage>) {
    if let Some(usage) = usage {
        runtime.usage.prompt_tokens = runtime
            .usage
            .prompt_tokens
            .saturating_add(usage.prompt_tokens);
        runtime.usage.completion_tokens = runtime
            .usage
            .completion_tokens
            .saturating_add(usage.completion_tokens);
        runtime.usage.total_tokens = runtime.usage.total_tokens.saturating_add(
            usage
                .total_tokens
                .max(usage.prompt_tokens.saturating_add(usage.completion_tokens)),
        );
    }
    runtime.provider_calls += 1;
}

async fn execute_code_exec_with_provider(
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
) -> Result<Map<String, Value>> {
    let language = node
        .params
        .get("lang")
        .and_then(Value::as_str)
        .unwrap_or("python3")
        .to_string();
    let code = node
        .params
        .get("script")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            input
                .inputs
                .get("script")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
    if code.trim().is_empty() {
        let message = "**ERROR**: CodeExec script must not be empty";
        return Ok(Map::from_iter([(
            "_ERROR".into(),
            Value::String(message.into()),
        )]));
    }
    let mut arguments = Map::new();
    if let Some(raw) = node.params.get("arguments")
        && let Value::Object(map) = raw {
            arguments = map.clone();
        }
    let provider = std::env::var("RAYRAG_SANDBOX_PROVIDER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            crate::api::runtime_config::system_settings_get("sandbox.provider_type")
                .and_then(|value| value.as_str().map(str::to_owned))
        })
        .unwrap_or_else(|| "self_managed".into());
    if provider.eq_ignore_ascii_case("local") {
        // 本地沙箱（对齐 RAGFlow LocalProvider）：无外部服务依赖；
        // 本地执行不可用（缺解释器/语言不支持）时回退到远程 executor_manager（local→remote 回退）
        let sandbox = crate::sandbox::LocalSandbox::default();
        match sandbox.execute(&code, &language, &arguments).await {
            Ok(execution) => {
                let mut metadata = Map::new();
                if let Some(structured) = &execution.structured {
                    metadata.insert("result_present".into(), Value::Bool(true));
                    metadata.insert(
                        "result_value".into(),
                        structured.get("value").cloned().unwrap_or(Value::Null),
                    );
                }
                let result = crate::code_exec::SandboxResult {
                    stdout: execution.stdout,
                    stderr: Some(execution.stderr),
                    exit_code: 0,
                    status: None,
                    detail: None,
                    time_used_ms: None,
                    memory_used_kb: None,
                    artifacts: vec![],
                    metadata,
                };
                Ok(process_result(&result))
            }
            Err(local_error) => match run_remote_sandbox(&code, &language, &arguments).await {
                Ok(result) => Ok(process_result(&result)),
                Err(remote_error) => {
                    let message =
                        format!("**ERROR**: {local_error}; remote fallback: {remote_error}");
                    Ok(Map::from_iter([("_ERROR".into(), Value::String(message))]))
                }
            },
        }
    } else if provider.eq_ignore_ascii_case("self_managed") {
        match run_remote_sandbox(&code, &language, &arguments).await {
            Ok(result) => Ok(process_result(&result)),
            Err(error) => {
                let message = format!("**ERROR**: {error}");
                Ok(Map::from_iter([("_ERROR".into(), Value::String(message))]))
            }
        }
    } else {
        Ok(Map::from_iter([(
            "_ERROR".into(),
            Value::String(format!(
                "**ERROR**: Unsupported sandbox provider: {provider}"
            )),
        )]))
    }
}

/// 通过 executor_manager 远程沙箱执行代码（对齐 RAGFlow SelfManagedProvider）。
async fn run_remote_sandbox(
    code: &str,
    language: &str,
    arguments: &Map<String, Value>,
) -> Result<crate::code_exec::SandboxResult> {
    let client = SandboxClient::default();
    let request = CodeRequest {
        language: language.to_string(),
        code: code.to_string(),
        arguments: arguments.clone(),
    };
    client.run(&request).await
}

async fn execute_exesql_with_provider(
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
) -> Result<Map<String, Value>> {
    let mut sql = String::new();
    if let Some(content) = input.inputs.get("sql") {
        sql = match content {
            Value::String(value) => value.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join(";"),
            other => other.to_string(),
        };
    }
    if sql.trim().is_empty() {
        sql = input.question.to_string();
    }
    let config = ExeSqlConfig::from_params_and_env(&node.params);
    match execute_sql(&config, &sql).await {
        Ok(rows) => Ok(Map::from_iter([("result".into(), Value::String(rows))])),
        Err(error) => {
            let message = format!("**ERROR**: {error}");
            Ok(Map::from_iter([("_ERROR".into(), Value::String(message))]))
        }
    }
}

fn execute_email_with_provider(
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
    client: &EmailClient,
) -> Result<Map<String, Value>> {
    let to_email = input
        .inputs
        .get("to_email")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let config = EmailConfig::from_params_and_env(&node.params);
    let request = EmailRequest {
        to_email,
        cc_email: node
            .params
            .get("cc_email")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        content: node
            .params
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        subject: node
            .params
            .get("subject")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    };
    match client.send(&config, &request) {
        Ok((success, message)) => {
            let mut outputs = Map::from_iter([("success".into(), Value::Bool(success))]);
            if !success {
                outputs.insert("_ERROR".into(), Value::String(message));
            }
            Ok(outputs)
        }
        Err(error) => {
            let message = format!("**ERROR**: {error}");
            Ok(Map::from_iter([
                ("success".into(), Value::Bool(false)),
                ("_ERROR".into(), Value::String(message)),
            ]))
        }
    }
}

async fn execute_tushare_with_provider(
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
    client: &TuShareClient,
) -> Result<Map<String, Value>> {
    let mut content = String::new();
    if let Some(input_content) = input.inputs.get("content") {
        content = match input_content {
            Value::String(value) => value.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join(","),
            other => other.to_string(),
        };
    }
    if content.trim().is_empty() {
        content = input.question.to_string();
    }
    let token = node
        .params
        .get("token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| std::env::var("TUSHARE_TOKEN").unwrap_or_default());
    let src = node
        .params
        .get("src")
        .and_then(Value::as_str)
        .unwrap_or("eastmoney")
        .to_string();
    let start_date = node
        .params
        .get("start_date")
        .and_then(Value::as_str)
        .unwrap_or("2024-01-01 09:00:00")
        .to_string();
    let end_date = node
        .params
        .get("end_date")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let end_date = if end_date.is_empty() {
        time::OffsetDateTime::now_utc()
            .format(
                &time::format_description::parse("[year]-[month]-[day] [hour]:[minute]:[second]")
                    .expect("static time format"),
            )
            .unwrap_or_default()
    } else {
        end_date
    };
    let keyword = node
        .params
        .get("keyword")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let request = TuShareRequest {
        token,
        src,
        start_date,
        end_date,
        keyword,
    };
    match client.news(&request).await {
        Ok(markdown) => Ok(Map::from_iter([(
            "content".into(),
            Value::String(markdown),
        )])),
        Err(error) => {
            let message = format!("**ERROR**: {error}");
            Ok(Map::from_iter([("_ERROR".into(), Value::String(message))]))
        }
    }
}

/// 执行同花顺问财（iwencai）组件：query 必填（节点参数 > 上游输入 > sys.query），
/// top_n 默认 10，query_type 默认 stock；输出 key 为 formalized_content（对齐
/// RAGFlow ToolBase 的标准工具输出），错误前缀 **ERROR**。
async fn execute_wencai_with_provider(
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
    client: &wencai::WenCaiClient,
) -> Result<Map<String, Value>> {
    let mut query = node
        .params
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if query.trim().is_empty()
        && let Some(input_query) = input.inputs.get("query") {
            query = match input_query {
                Value::String(value) => value.clone(),
                Value::Array(items) => items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
                    .join(","),
                other => other.to_string(),
            };
        }
    if query.trim().is_empty() {
        query = input.question.to_string();
    }
    let top_n = node
        .params
        .get("top_n")
        .and_then(Value::as_u64)
        .unwrap_or(10) as usize;
    let query_type = node
        .params
        .get("query_type")
        .and_then(Value::as_str)
        .unwrap_or("stock")
        .to_string();
    let cookie = node
        .params
        .get("cookie")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let request = wencai::WenCaiRequest {
        query,
        top_n,
        query_type,
        cookie,
    };
    match client.search(&request).await {
        Ok(markdown) => Ok(Map::from_iter([(
            "formalized_content".into(),
            Value::String(markdown),
        )])),
        Err(error) => {
            let message = format!("**ERROR**: {error}");
            Ok(Map::from_iter([("_ERROR".into(), Value::String(message))]))
        }
    }
}

async fn execute_akshare_with_provider(
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
    client: &AkShareClient,
) -> Result<Map<String, Value>> {
    let mut symbol = String::new();
    if let Some(content) = input.inputs.get("content") {
        symbol = match content {
            Value::String(value) => value.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join(","),
            other => other.to_string(),
        };
    }
    if symbol.trim().is_empty() {
        symbol = input.question.to_string();
    }
    let top_n = node
        .params
        .get("top_n")
        .and_then(Value::as_u64)
        .unwrap_or(10) as usize;
    let request = AkShareRequest { symbol, top_n };
    match client.stock_news_em(&request).await {
        Ok(news) => {
            let content = AkShareClient::render_markdown(&news);
            Ok(Map::from_iter([("content".into(), Value::String(content))]))
        }
        Err(error) => {
            let message = format!("**ERROR**: {error}");
            Ok(Map::from_iter([("_ERROR".into(), Value::String(message))]))
        }
    }
}

async fn execute_crawler_with_provider(
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
    client: &CrawlerClient,
) -> Result<Map<String, Value>> {
    let mut url = String::new();
    if let Some(content) = input.inputs.get("content") {
        url = match content {
            Value::String(value) => value.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join(" - "),
            other => other.to_string(),
        };
    }
    if url.trim().is_empty() {
        url = input.question.to_string();
    }
    if url.trim().is_empty()
        && let Some(query) = node.params.get("query").and_then(Value::as_str) {
            url = query.to_string();
        }
    let extract_type = node
        .params
        .get("extract_type")
        .and_then(Value::as_str)
        .unwrap_or("markdown")
        .to_string();
    let proxy = node
        .params
        .get("proxy")
        .and_then(Value::as_str)
        .map(str::to_string);
    let request = CrawlerRequest {
        url,
        extract_type,
        proxy,
    };
    match client.fetch(&request).await {
        Ok(extracted) => Ok(Map::from_iter([(
            "content".into(),
            Value::String(extracted),
        )])),
        Err(error) => {
            let message = format!("An unexpected error occurred: {error}");
            Ok(Map::from_iter([("_ERROR".into(), Value::String(message))]))
        }
    }
}

async fn execute_translate_with_provider(
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
    client: &BaiduTranslateClient,
) -> Result<Map<String, Value>> {
    let mut text = String::new();
    if let Some(content) = input.inputs.get("content") {
        text = match content {
            Value::String(s) => s.clone(),
            Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join("\n"),
            other => other.to_string(),
        };
    }
    if text.trim().is_empty() {
        text = input.question.to_string();
    }
    if text.trim().is_empty()
        && let Some(query) = node.params.get("query").and_then(Value::as_str) {
            text = query.to_string();
        }
    let source_lang = node
        .params
        .get("source_lang")
        .and_then(Value::as_str)
        .unwrap_or("auto")
        .to_string();
    let target_lang = node
        .params
        .get("target_lang")
        .and_then(Value::as_str)
        .unwrap_or("EN")
        .to_string();
    let request = TranslateRequest {
        text,
        source_lang,
        target_lang,
    };
    match client.translate(&request).await {
        Ok(translated) => Ok(Map::from_iter([(
            "content".into(),
            Value::String(translated),
        )])),
        Err(error) => {
            let message = format!("Translate error: {error}");
            Ok(Map::from_iter([("_ERROR".into(), Value::String(message))]))
        }
    }
}

async fn execute_categorize(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    llm: Option<&LlmClient>,
    input: &WorkflowRunInput<'_>,
) -> Result<Vec<String>> {
    let llm = llm.ok_or_else(|| {
        anyhow!(
            "Categorize component '{}' requires a configured chat model",
            node.id
        )
    })?;
    let categories = node
        .params
        .get("category_description")
        .and_then(Value::as_object)
        .filter(|categories| !categories.is_empty())
        .ok_or_else(|| anyhow!("Categorize '{}' categories cannot be empty", node.id))?;
    let query_selector = node
        .params
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("sys.query");
    let query = if is_exact_selector(query_selector)
        || query_selector.contains('@')
        || query_selector.starts_with("sys.")
        || query_selector.starts_with("env.")
    {
        stringify(&get_variable(runtime, query_selector).unwrap_or(Value::Null))
    } else {
        resolve_template(runtime, query_selector)?
    };
    let history_window = node
        .params
        .get("message_history_window_size")
        .and_then(Value::as_u64)
        .unwrap_or(1) as usize;

    let mut category_names = Vec::with_capacity(categories.len());
    let mut descriptions = Vec::new();
    let mut examples = Vec::new();
    for (name, raw) in categories {
        let description = raw.as_object().ok_or_else(|| {
            anyhow!(
                "Categorize '{}' category '{}' must be an object",
                node.id,
                name
            )
        })?;
        category_names.push(name.clone());
        if let Some(value) = description
            .get("description")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            descriptions.push(format!("Category: {name}\nDescription: {value}"));
        }
        if let Some(values) = description.get("examples").and_then(Value::as_array) {
            for value in values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                examples.push(format!(
                    "USER: \"{}\" → {name}",
                    value.replace('\n', "    ")
                ));
            }
        }
    }
    let mut system_prompt = format!(
        "You are an advanced classification system. Classify the user request into exactly one of these categories:\n - {}\n\n{}\n\nReturn only the category name without explanations. Use Other only when no specific category fits.",
        category_names.join("\n - "),
        descriptions.join("\n------\n")
    );
    if !examples.is_empty() {
        system_prompt.push_str("\n\nExamples:\n");
        system_prompt.push_str(&examples.join("\n"));
    }

    let history_count = history_window.saturating_mul(2);
    let recent = if history_count == 0 {
        &[][..]
    } else if input.history.len() > history_count {
        &input.history[input.history.len() - history_count..]
    } else {
        input.history
    };
    let mut real_data: Vec<String> = recent
        .iter()
        .map(|message| {
            format!(
                "{}: \"{}\"",
                message.role.to_ascii_uppercase(),
                message.content.replace('\n', "")
            )
        })
        .collect();
    real_data.push(format!("USER: \"{}\"", query.replace('\n', "")));
    let messages = vec![
        ChatMessage::new("system", system_prompt),
        ChatMessage::new(
            "user",
            format!("---- Real Data ----\n{} →", real_data.join(" | ")),
        ),
    ];
    let patch = merge_generation_patch(
        input.generation,
        GenerationParamsPatch::from_request(&Value::Object(node.params.clone()))?,
    );
    let completion = llm
        .chat_completion_with_generation(&messages, patch)
        .await
        .with_context(|| format!("Categorize component '{}' failed", node.id))?;
    record_provider_usage(runtime, completion.usage);

    let answer = completion.content.to_ascii_lowercase();
    let fallback = category_names
        .last()
        .expect("non-empty categories were checked");
    let mut selected = fallback;
    let mut selected_count = 0;
    for name in &category_names {
        let normalized = name.to_ascii_lowercase();
        let count = answer.matches(normalized.as_str()).count();
        if count > selected_count {
            selected = name;
            selected_count = count;
        }
    }
    let destination = categories
        .get(selected)
        .and_then(Value::as_object)
        .expect("category objects were validated");
    let next = destination_array(destination.get("to"), &node.id, "category.to")?;
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("category_name".into(), Value::String(selected.clone())),
            (
                "_next".into(),
                Value::Array(next.iter().cloned().map(Value::String).collect()),
            ),
        ]),
    );
    Ok(next)
}

async fn execute_tavily_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn TavilyProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let api_key = match tavily_api_key(node) {
        Ok(api_key) => api_key,
        Err(error) => {
            set_tavily_error(runtime, node, error.to_string());
            return Ok(());
        }
    };
    let request = TavilySearchRequest {
        query,
        search_depth: validate_tavily_enum(node, "search_depth", "basic", &["basic", "advanced"])?,
        topic: validate_tavily_enum(node, "topic", "general", &["general", "news"])?,
        max_results: tavily_positive_usize(node, "max_results", 6)?,
        days: tavily_positive_usize(node, "days", 14)?,
        include_answer: node
            .params
            .get("include_answer")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        // The fixed Python implementation unconditionally disables these two
        // response-heavy fields immediately before every request.
        include_raw_content: false,
        include_images: false,
        include_image_descriptions: node
            .params
            .get("include_image_descriptions")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        include_domains: resolve_tavily_string_list(runtime, node, "include_domains")?,
        exclude_domains: resolve_tavily_string_list(runtime, node, "exclude_domains")?,
    };
    let attempts = tavily_attempts(node);
    let delay = tavily_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..attempts {
        if runtime.cancel_flag {
            set_tavily_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&api_key, &request).await {
            Ok(results) => match tavily_search_outputs(&results) {
                Ok((outputs, references, chunks, doc_aggs)) => {
                    runtime
                        .retrieval
                        .insert("chunks".into(), Value::Array(chunks));
                    runtime
                        .retrieval
                        .insert("doc_aggs".into(), Value::Array(doc_aggs));
                    append_web_references(runtime, references);
                    runtime.outputs.insert(node.id.clone(), outputs);
                    return Ok(());
                }
                Err(error) => last_error = Some(error.to_string()),
            },
            Err(error) => last_error = Some(error.to_string()),
        }
        // Search sleeps after every failed attempt, including the terminal
        // attempt, matching agent/tools/tavily.py.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_tavily_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "Tavily Search failed without an error".into()),
    );
    Ok(())
}

async fn execute_duckduckgo_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn DuckDuckGoProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = DuckDuckGoSearchRequest {
        query,
        channel: duckduckgo_channel(node)?,
        top_n: duckduckgo_top_n(node)?,
    };
    let delay = duckduckgo_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..duckduckgo_attempts(node) {
        if runtime.cancel_flag {
            set_duckduckgo_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(results) => match duckduckgo_search_outputs(&results) {
                Ok((outputs, references, chunks, doc_aggs)) => {
                    runtime
                        .retrieval
                        .insert("chunks".into(), Value::Array(chunks));
                    runtime
                        .retrieval
                        .insert("doc_aggs".into(), Value::Array(doc_aggs));
                    append_web_references(runtime, references);
                    runtime.outputs.insert(node.id.clone(), outputs);
                    return Ok(());
                }
                Err(error) => last_error = Some(error.to_string()),
            },
            Err(error) => last_error = Some(error.to_string()),
        }
        // agent/tools/duckduckgo.py sleeps after every failed outer attempt,
        // including the terminal attempt.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_duckduckgo_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "DuckDuckGo failed without an error".into()),
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Mainland-China search/finance tools (Bing, Baidu, Bocha, Tencent Finance,
// Baidu Scholar). These mirror the RAGFlow tool result shape so a canvas can
// swap Google/SerpApi for a domestic provider without changing consumers.
// ---------------------------------------------------------------------------

/// Map `[{title, link, snippet}]` rows into the RAGFlow tool output shape.
fn domestic_search_outputs(results: &[Value], provider_label: &str) -> Result<TavilySearchOutput> {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for result in results {
        let result = result
            .as_object()
            .ok_or_else(|| anyhow!("{provider_label} result must be an object"))?;
        let content = result
            .get("snippet")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = tavily_data_image_regex().replace_all(content, "");
        let content: String = content.chars().take(10_000).collect();
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let url = result
            .get("link")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if content.is_empty() && title.is_empty() {
            continue;
        }
        let id = hash_str2int(&content, 100_000_000).to_string();
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": title,
            "similarity": 1,
            "url": url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": title,
            "doc_id": id,
            "count": 1,
            "url": url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(1.0),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            title.replace('\n', " "),
            url.replace('\n', " "),
            content
        ));
    }
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        ("json".into(), Value::Array(results.to_vec())),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    Ok((outputs, references, chunks, doc_aggs))
}

fn set_domestic_search_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    // 错误必须写入 formalized_content，让下游 answer 节点感知工具失败，
    // 而不是拿到空内容后由 LLM 自由发挥。
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error)),
        ]),
    );
}

async fn execute_bing_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn BingProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = BingSearchRequest {
        query,
        channel: bing_channel(node)?,
        top_n: bing_top_n(node)?,
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(results) => {
                let rows = crate::bing::bing_results_to_tool_rows(&results);
                match domestic_search_outputs(&rows, "Bing") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "Bing failed without an error".into()),
    );
    Ok(())
}

async fn execute_baidu_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn BaiduProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = BaiduSearchRequest {
        query,
        top_n: bing_top_n(node)?,
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(results) => {
                let rows = crate::baidu::baidu_results_to_tool_rows(&results);
                match domestic_search_outputs(&rows, "Baidu") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "Baidu failed without an error".into()),
    );
    Ok(())
}

async fn execute_bocha_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn BochaProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let api_key = std::env::var("BOCHA_API_KEY").unwrap_or_default();
    if api_key.is_empty() {
        set_domestic_search_error(runtime, node, "BOCHA_API_KEY is not configured".into());
        return Ok(());
    }
    let request = BochaSearchRequest {
        query,
        channel: bing_channel(node)?,
        top_n: bing_top_n(node)?,
        freshness: node
            .params
            .get("freshness")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&api_key, &request).await {
            Ok(results) => {
                let rows = crate::bocha::bocha_results_to_tool_rows(&results);
                match domestic_search_outputs(&rows, "Bocha") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "Bocha failed without an error".into()),
    );
    Ok(())
}

async fn execute_eastmoney_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn EastMoneyProvider,
) -> Result<()> {
    let symbol = node
        .params
        .get("symbol")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let symbol = match symbol {
        Some(symbol) => symbol,
        None => resolve_tavily_string(runtime, node, "query", "sys.query")?,
    };
    if symbol.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = EastMoneyNewsRequest {
        symbol,
        top_n: bing_top_n(node)?,
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(results) => {
                let rows = crate::eastmoney::eastmoney_results_to_tool_rows(&results);
                match domestic_search_outputs(&rows, "EastMoney") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "EastMoney failed without an error".into()),
    );
    Ok(())
}

async fn execute_jin10_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn Jin10Provider,
) -> Result<()> {
    let secret_key = node
        .params
        .get("secret_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("JIN10_SECRET_KEY").ok())
        .unwrap_or_default();
    if secret_key.is_empty() {
        set_domestic_search_error(
            runtime,
            node,
            "JIN10_SECRET_KEY is not configured (set the env var or the node secret_key)".into(),
        );
        return Ok(());
    }
    let r#type = match node.params.get("type").and_then(Value::as_str) {
        None | Some("flash") => Jin10Type::Flash,
        Some("calendar") => Jin10Type::Calendar,
        Some("symbols") => Jin10Type::Symbols,
        Some("news") => Jin10Type::News,
        Some(_) => {
            set_domestic_search_error(
                runtime,
                node,
                "Jin10 type must be flash, calendar, symbols or news".into(),
            );
            return Ok(());
        }
    };
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    let contain = if query.is_empty() {
        String::new()
    } else {
        query
    };
    let request = Jin10Request {
        r#type,
        flash_type: node
            .params
            .get("flash_type")
            .and_then(Value::as_u64)
            .unwrap_or(1)
            .clamp(1, 5) as u8,
        calendar_type: node
            .params
            .get("calendar_type")
            .and_then(Value::as_str)
            .unwrap_or("cj")
            .to_string(),
        calendar_datatype: node
            .params
            .get("calendar_datatype")
            .and_then(Value::as_str)
            .unwrap_or("data")
            .to_string(),
        symbols_type: node
            .params
            .get("symbols_type")
            .and_then(Value::as_str)
            .unwrap_or("GOODS")
            .to_string(),
        symbols_datatype: node
            .params
            .get("symbols_datatype")
            .and_then(Value::as_str)
            .unwrap_or("symbols")
            .to_string(),
        contain,
        filter: node
            .params
            .get("filter")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        top_n: bing_top_n(node)?,
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&secret_key, &request).await {
            Ok(results) => {
                let rows = crate::jin10::jin10_results_to_tool_rows(&results);
                match domestic_search_outputs(&rows, "Jin10") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "Jin10 failed without an error".into()),
    );
    Ok(())
}

async fn execute_qweather_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn QWeatherProvider,
) -> Result<()> {
    let api_key = node
        .params
        .get("web_apikey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("QWEATHER_API_KEY").ok())
        .unwrap_or_default();
    if api_key.is_empty() {
        set_domestic_search_error(
            runtime,
            node,
            "QWEATHER_API_KEY is not configured (set the env var or the node web_apikey)".into(),
        );
        return Ok(());
    }
    let location = match node.params.get("location").and_then(Value::as_str) {
        Some(location) if !location.trim().is_empty() => location.trim().to_string(),
        _ => resolve_tavily_string(runtime, node, "query", "sys.query")?,
    };
    if location.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let r#type = match node.params.get("type").and_then(Value::as_str) {
        None | Some("weather") => QWeatherType::Weather,
        Some("indices") => QWeatherType::Indices,
        Some("airquality") | Some("air") => QWeatherType::AirQuality,
        Some(_) => {
            set_domestic_search_error(
                runtime,
                node,
                "QWeather type must be weather, indices or airquality".into(),
            );
            return Ok(());
        }
    };
    let request = QWeatherRequest {
        location,
        r#type,
        time_period: node
            .params
            .get("time_period")
            .and_then(Value::as_str)
            .unwrap_or("now")
            .to_string(),
        lang: node
            .params
            .get("lang")
            .and_then(Value::as_str)
            .unwrap_or("zh")
            .to_string(),
        paid: node
            .params
            .get("paid")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        top_n: bing_top_n(node)?,
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&api_key, &request).await {
            Ok(results) => {
                let rows = crate::qweather::qweather_results_to_tool_rows(&results);
                match domestic_search_outputs(&rows, "QWeather") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "QWeather failed without an error".into()),
    );
    Ok(())
}

async fn execute_searxng_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn SearxngProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let searxng_url = node
        .params
        .get("searxng_url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("SEARXNG_URL")
                .ok()
                .filter(|value| !value.is_empty())
        });
    let searxng_url = match searxng_url {
        Some(url) => url,
        None => {
            set_domestic_search_error(
                runtime,
                node,
                "SEARXNG_URL is not configured (set SEARXNG_URL or node searxng_url)".into(),
            );
            return Ok(());
        }
    };
    let request = SearxngRequest {
        query,
        searxng_url,
        top_n: bing_top_n(node)?,
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(results) => {
                let rows = crate::searxng::searxng_results_to_tool_rows(&results);
                match domestic_search_outputs(&rows, "SearXNG") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "SearXNG failed without an error".into()),
    );
    Ok(())
}

async fn execute_baike_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn BaikeProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = BaikeSearchRequest {
        query,
        top_n: bing_top_n(node)?,
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(articles) => {
                let rows: Vec<Value> = articles
                    .iter()
                    .map(|article: &BaikeArticle| {
                        serde_json::json!({
                            "title": article.title,
                            "link": article.url,
                            "snippet": article.snippet
                        })
                    })
                    .collect();
                match domestic_search_outputs(&rows, "BaiduBaike") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "BaiduBaike failed without an error".into()),
    );
    Ok(())
}

async fn execute_tencent_finance_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn TencentFinanceProvider,
) -> Result<()> {
    let symbol = node
        .params
        .get("symbol")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("TencentFinance '{}' symbol is required", node.id))?;
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider
            .quote(&TencentFinanceRequest {
                symbol: symbol.into(),
            })
            .await
        {
            Ok(quote) => {
                let row = crate::tencent_finance::tencent_quote_to_tool_row(&quote);
                let rows = vec![row];
                match domestic_search_outputs(&rows, "TencentFinance") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "TencentFinance failed without an error".into()),
    );
    Ok(())
}

async fn execute_baidu_scholar_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn BaiduScholarProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = BaiduScholarRequest {
        query,
        top_n: bing_top_n(node)?,
    };
    let delay = bing_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..bing_attempts(node) {
        if runtime.cancel_flag {
            set_domestic_search_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(results) => {
                let rows = crate::baidu_scholar::baidu_scholar_results_to_tool_rows(&results);
                match domestic_search_outputs(&rows, "BaiduScholar") {
                    Ok((outputs, references, chunks, doc_aggs)) => {
                        runtime
                            .retrieval
                            .insert("chunks".into(), Value::Array(chunks));
                        runtime
                            .retrieval
                            .insert("doc_aggs".into(), Value::Array(doc_aggs));
                        append_web_references(runtime, references);
                        runtime.outputs.insert(node.id.clone(), outputs);
                        return Ok(());
                    }
                    Err(error) => last_error = Some(error.to_string()),
                }
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_domestic_search_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "BaiduScholar failed without an error".into()),
    );
    Ok(())
}

async fn execute_agent_bing_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn BingProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_bing_search_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!("Bing timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_baidu_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn BaiduProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_baidu_search_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!("Baidu timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_bocha_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn BochaProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_bocha_search_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!("Bocha timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_tencent_finance_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn TencentFinanceProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_tencent_finance_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!(
                    "TencentFinance timed out after {} seconds",
                    timeout.as_secs()
                ),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_baidu_scholar_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn BaiduScholarProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_baidu_scholar_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!("BaiduScholar timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_eastmoney_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn EastMoneyProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_eastmoney_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!("EastMoney timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_jin10_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn Jin10Provider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_jin10_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!("Jin10 timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_qweather_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn QWeatherProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_qweather_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!("QWeather timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_agent_searxng_with_timeout(
    runtime: &mut CanvasRuntime,
    child: &CanvasNode,
    provider: &dyn SearxngProvider,
) -> Result<()> {
    let timeout = component_timeout(&child.component_name);
    match crate::runtime::with_timeout(
        timeout,
        execute_searxng_with_provider(runtime, child, provider),
    )
    .await
    {
        Err(error)
            if error
                .downcast_ref::<tokio::time::error::Elapsed>()
                .is_some() =>
        {
            set_domestic_search_error(
                runtime,
                child,
                format!("SearXNG timed out after {} seconds", timeout.as_secs()),
            );
            Ok(())
        }
        result => result,
    }
}

async fn execute_wikipedia_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn WikipediaProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = WikipediaSearchRequest {
        query,
        language: wikipedia_language(node)?,
        top_n: wikipedia_top_n(node)?,
    };
    let delay = wikipedia_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..wikipedia_attempts(node) {
        if runtime.cancel_flag {
            set_wikipedia_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(articles) => {
                let (outputs, references, chunks, doc_aggs) = wikipedia_search_outputs(&articles);
                runtime
                    .retrieval
                    .insert("chunks".into(), Value::Array(chunks));
                runtime
                    .retrieval
                    .insert("doc_aggs".into(), Value::Array(doc_aggs));
                append_web_references(runtime, references);
                runtime.outputs.insert(node.id.clone(), outputs);
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        // agent/tools/wikipedia.py sleeps after every failed outer attempt,
        // including the terminal attempt.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_wikipedia_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "Wikipedia failed without an error".into()),
    );
    Ok(())
}

async fn execute_google_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn GoogleProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "q", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = GoogleSearchRequest {
        query,
        country: google_country(node)?,
        language: google_language(node)?,
    };
    let api_key = google_api_key(node)?;
    let delay = google_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..google_attempts(node) {
        if runtime.cancel_flag {
            set_google_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&api_key, &request).await {
            Ok(results) => match google_search_outputs(&results) {
                Ok((outputs, references, chunks, doc_aggs)) => {
                    runtime
                        .retrieval
                        .insert("chunks".into(), Value::Array(chunks));
                    runtime
                        .retrieval
                        .insert("doc_aggs".into(), Value::Array(doc_aggs));
                    append_web_references(runtime, references);
                    runtime.outputs.insert(node.id.clone(), outputs);
                    return Ok(());
                }
                Err(error) => last_error = Some(error.to_string()),
            },
            Err(error) => last_error = Some(error.to_string()),
        }
        // agent/tools/google.py sleeps after every failed attempt, including
        // the terminal attempt.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_google_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "Google failed without an error".into()),
    );
    Ok(())
}

async fn execute_google_scholar_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn GoogleScholarProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([
                ("formalized_content".into(), Value::String(String::new())),
                ("json".into(), Value::Array(Vec::new())),
            ]),
        );
        return Ok(());
    }
    let request = GoogleScholarSearchRequest {
        query,
        top_n: google_scholar_top_n(node)?,
        sort_by: google_scholar_sort_by(node)?,
        year_low: google_scholar_year(node, "year_low")?,
        year_high: google_scholar_year(node, "year_high")?,
        patents: google_scholar_patents(node)?,
    };
    let delay = google_scholar_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..google_scholar_attempts(node) {
        if runtime.cancel_flag {
            set_google_scholar_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(publications) => {
                let (outputs, references, chunks, doc_aggs) =
                    google_scholar_search_outputs(&publications);
                runtime
                    .retrieval
                    .insert("chunks".into(), Value::Array(chunks));
                runtime
                    .retrieval
                    .insert("doc_aggs".into(), Value::Array(doc_aggs));
                append_web_references(runtime, references);
                runtime.outputs.insert(node.id.clone(), outputs);
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        // agent/tools/googlescholar.py sleeps after every failed outer attempt,
        // including the terminal attempt.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_google_scholar_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "GoogleScholar failed without an error".into()),
    );
    Ok(())
}

async fn execute_github_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn GitHubProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        // github.py clears only formalized_content on its empty-query path.
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = GitHubSearchRequest {
        query,
        top_n: github_top_n(node)?,
    };
    let delay = github_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..github_attempts(node) {
        if runtime.cancel_flag {
            set_github_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(repositories) => {
                let (outputs, references, chunks, doc_aggs) = github_search_outputs(&repositories);
                runtime
                    .retrieval
                    .insert("chunks".into(), Value::Array(chunks));
                runtime
                    .retrieval
                    .insert("doc_aggs".into(), Value::Array(doc_aggs));
                append_web_references(runtime, references);
                runtime.outputs.insert(node.id.clone(), outputs);
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        // agent/tools/github.py sleeps after every failed outer attempt,
        // including the terminal attempt.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_github_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "GitHub failed without an error".into()),
    );
    Ok(())
}

async fn execute_yahoo_finance_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn YahooFinanceProvider,
) -> Result<()> {
    let stock_code = resolve_tavily_string(runtime, node, "stock_code", "sys.query")?;
    if stock_code.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("report".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = YahooFinanceRequest {
        stock_code,
        info: yahoo_finance_flag(node, "info", true),
        history: yahoo_finance_flag(node, "history", false),
        count: yahoo_finance_flag(node, "count", false),
        financials: yahoo_finance_flag(node, "financials", false),
        income_stmt: yahoo_finance_flag(node, "income_stmt", false),
        balance_sheet: yahoo_finance_flag(node, "balance_sheet", false),
        cash_flow_statement: yahoo_finance_flag(node, "cash_flow_statement", false),
        news: yahoo_finance_flag(node, "news", true),
    };
    let delay = yahoo_finance_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..yahoo_finance_attempts(node) {
        if runtime.cancel_flag {
            set_yahoo_finance_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.report(&request).await {
            Ok(report) => {
                runtime.outputs.insert(
                    node.id.clone(),
                    Map::from_iter([("report".into(), Value::String(report))]),
                );
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        // yahoofinance.py sleeps after every failed outer attempt, including
        // the terminal attempt.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_yahoo_finance_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "YahooFinance failed without an error".into()),
    );
    Ok(())
}

async fn execute_arxiv_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn ArxivProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = ArxivSearchRequest {
        query,
        top_n: arxiv_top_n(node)?,
        sort_by: arxiv_sort_by(node)?,
    };
    let delay = arxiv_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..arxiv_attempts(node) {
        if runtime.cancel_flag {
            set_arxiv_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(papers) => {
                let (outputs, references, chunks, doc_aggs) = arxiv_search_outputs(&papers);
                runtime
                    .retrieval
                    .insert("chunks".into(), Value::Array(chunks));
                runtime
                    .retrieval
                    .insert("doc_aggs".into(), Value::Array(doc_aggs));
                append_web_references(runtime, references);
                runtime.outputs.insert(node.id.clone(), outputs);
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        // agent/tools/arxiv.py sleeps after every failed outer attempt,
        // including the terminal attempt.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_arxiv_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "ArXiv failed without an error".into()),
    );
    Ok(())
}

async fn execute_pubmed_search_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn PubMedProvider,
) -> Result<()> {
    let query = resolve_tavily_string(runtime, node, "query", "sys.query")?;
    if query.is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            Map::from_iter([("formalized_content".into(), Value::String(String::new()))]),
        );
        return Ok(());
    }
    let request = PubMedSearchRequest {
        query,
        top_n: pubmed_top_n(node)?,
        email: pubmed_email(node)?,
    };
    let delay = pubmed_retry_delay(node)?;
    let mut last_error = None;
    for _ in 0..pubmed_attempts(node) {
        if runtime.cancel_flag {
            set_pubmed_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.search(&request).await {
            Ok(articles) => {
                let (outputs, references, chunks, doc_aggs) = pubmed_search_outputs(&articles);
                runtime
                    .retrieval
                    .insert("chunks".into(), Value::Array(chunks));
                runtime
                    .retrieval
                    .insert("doc_aggs".into(), Value::Array(doc_aggs));
                append_web_references(runtime, references);
                runtime.outputs.insert(node.id.clone(), outputs);
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        // agent/tools/pubmed.py sleeps after every failed outer attempt,
        // including the terminal attempt.
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    set_pubmed_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "PubMed failed without an error".into()),
    );
    Ok(())
}

async fn execute_tavily_extract_with_provider(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    provider: &dyn TavilyProvider,
) -> Result<()> {
    let api_key = match tavily_api_key(node) {
        Ok(api_key) => api_key,
        Err(error) => {
            set_tavily_error(runtime, node, error.to_string());
            return Ok(());
        }
    };
    let request = TavilyExtractRequest {
        urls: resolve_tavily_urls(runtime, node)?,
        extract_depth: validate_tavily_enum(
            node,
            "extract_depth",
            "basic",
            &["basic", "advanced"],
        )?,
        format: validate_tavily_enum(node, "format", "markdown", &["markdown", "text"])?,
        // Fixed Python ignores the configured include_images value.
        include_images: false,
    };
    let mut last_error = None;
    for _ in 0..tavily_attempts(node) {
        if runtime.cancel_flag {
            set_tavily_error(runtime, node, "Task has been canceled".into());
            return Ok(());
        }
        match provider.extract(&api_key, &request).await {
            Ok(results) => {
                runtime.outputs.insert(
                    node.id.clone(),
                    Map::from_iter([("json".into(), Value::Array(results))]),
                );
                return Ok(());
            }
            Err(error) => last_error = Some(error.to_string()),
        }
        // Extract intentionally has no delay_after_error sleep upstream.
    }
    set_tavily_error(
        runtime,
        node,
        last_error.unwrap_or_else(|| "Tavily Extract failed without an error".into()),
    );
    Ok(())
}

fn resolve_tavily_string(
    runtime: &CanvasRuntime,
    node: &CanvasNode,
    field: &str,
    default: &str,
) -> Result<String> {
    let value = node
        .params
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or(default);
    if is_exact_selector(value)
        || value.starts_with("sys.")
        || value.starts_with("env.")
        || value.contains('@')
    {
        Ok(stringify(&get_variable(runtime, value)?))
    } else {
        resolve_template(runtime, value)
    }
}

fn resolve_tavily_string_list(
    runtime: &CanvasRuntime,
    node: &CanvasNode,
    field: &str,
) -> Result<Vec<String>> {
    let Some(value) = node.params.get(field) else {
        return Ok(Vec::new());
    };
    let resolved = resolve_parameter(runtime, value)?;
    tavily_string_list(&resolved, &node.id, field)
}

fn resolve_tavily_urls(runtime: &CanvasRuntime, node: &CanvasNode) -> Result<Vec<String>> {
    let Some(urls) = node.params.get("urls") else {
        return Ok(Vec::new());
    };
    match urls {
        Value::String(_) => Ok(resolve_tavily_string(runtime, node, "urls", "")?
            .split(',')
            .map(str::to_owned)
            .collect()),
        Value::Array(_) => {
            let resolved = resolve_parameter(runtime, urls)?;
            tavily_string_list(&resolved, &node.id, "urls")
        }
        _ => bail!(
            "TavilyExtract '{}' urls must be a string or string array",
            node.id
        ),
    }
}

fn tavily_api_key(node: &CanvasNode) -> Result<String> {
    let api_key = node
        .params
        .get("api_key")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|api_key| !api_key.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            std::env::var("TAVILY_API_KEY")
                .ok()
                .map(|api_key| api_key.trim().to_owned())
                .filter(|api_key| !api_key.is_empty())
        })
        .ok_or_else(|| anyhow!("Tavily api_key is required"))?;
    Ok(api_key)
}

fn tavily_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn tavily_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "Tavily component '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

type TavilySearchOutput = (
    Map<String, Value>,
    Vec<ChunkReference>,
    Vec<Value>,
    Vec<Value>,
);

fn tavily_search_outputs(results: &[Value]) -> Result<TavilySearchOutput> {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for result in results {
        let result = result
            .as_object()
            .ok_or_else(|| anyhow!("Tavily Search result must be an object"))?;
        let content = result
            .get("raw_content")
            .and_then(Value::as_str)
            .filter(|content| !content.is_empty())
            .or_else(|| result.get("content").and_then(Value::as_str))
            .unwrap_or_default();
        let content = tavily_data_image_regex().replace_all(content, "");
        let content: String = content.chars().take(10_000).collect();
        if content.is_empty() {
            continue;
        }
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Tavily Search result title must be a string"))?;
        let url = result
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Tavily Search result url must be a string"))?;
        let score = result
            .get("score")
            .and_then(Value::as_f64)
            .ok_or_else(|| anyhow!("Tavily Search result score must be a number"))?;
        if !score.is_finite() {
            bail!("Tavily Search result score must be finite");
        }
        let id = hash_str2int(&content, 100_000_000).to_string();
        let similarity = score as f32;
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": title,
            "similarity": score,
            "url": url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": title,
            "doc_id": id,
            "count": 1,
            "url": url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(similarity),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            title.replace('\n', " "),
            url.replace('\n', " "),
            content
        ));
    }
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        ("json".into(), Value::Array(results.to_vec())),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    Ok((outputs, references, chunks, doc_aggs))
}

fn wikipedia_search_outputs(
    articles: &[WikipediaArticle],
) -> (
    Map<String, Value>,
    Vec<ChunkReference>,
    Vec<Value>,
    Vec<Value>,
) {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for article in articles {
        let content = tavily_data_image_regex().replace_all(&article.summary, "");
        let content: String = content.chars().take(10_000).collect();
        if content.is_empty() {
            continue;
        }
        let id = hash_str2int(&content, 100_000_000).to_string();
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": article.title,
            "similarity": 1,
            "url": article.url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": article.title,
            "doc_id": id,
            "count": 1,
            "url": article.url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(1.0),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            article.title.replace('\n', " "),
            article.url.replace('\n', " "),
            content
        ));
    }
    let json: Vec<Value> = articles
        .iter()
        .map(|article| {
            serde_json::json!({
                "title": article.title,
                "snippet": article.snippet,
                "url": article.url
            })
        })
        .collect();
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        ("json".into(), serde_json::json!({"results": json})),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    (outputs, references, chunks, doc_aggs)
}

fn google_search_outputs(results: &[Value]) -> Result<TavilySearchOutput> {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for result in results {
        let result = result
            .as_object()
            .ok_or_else(|| anyhow!("Google organic result must be an object"))?;
        // Python evaluates r["snippet"] before dict.get chooses its default,
        // so snippet remains required even when the richer description exists.
        let snippet = result
            .get("snippet")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Google organic result snippet must be a string"))?;
        let description = result
            .get("about_this_result")
            .and_then(Value::as_object)
            .and_then(|about| about.get("source"))
            .and_then(Value::as_object)
            .and_then(|source| source.get("description"));
        let content = match description {
            None => snippet,
            Some(Value::String(content)) if content.is_empty() => continue,
            Some(Value::String(content)) => content,
            Some(Value::Null | Value::Bool(false)) => continue,
            Some(Value::Number(number)) if number.as_f64().is_some_and(|number| number == 0.0) => {
                continue;
            }
            Some(Value::Array(values)) if values.is_empty() => continue,
            Some(Value::Object(values)) if values.is_empty() => continue,
            Some(_) => bail!("Google organic result description must be a string"),
        };
        let content = tavily_data_image_regex().replace_all(content, "");
        let content: String = content.chars().take(10_000).collect();
        if content.is_empty() {
            continue;
        }
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Google organic result title must be a string"))?;
        let url = result
            .get("link")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Google organic result link must be a string"))?;
        let id = hash_str2int(&content, 100_000_000).to_string();
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": title,
            "similarity": 1,
            "url": url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": title,
            "doc_id": id,
            "count": 1,
            "url": url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(1.0),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            title.replace('\n', " "),
            url.replace('\n', " "),
            content
        ));
    }
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        ("json".into(), Value::Array(results.to_vec())),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    Ok((outputs, references, chunks, doc_aggs))
}

fn google_scholar_search_outputs(
    publications: &[GoogleScholarPublication],
) -> (
    Map<String, Value>,
    Vec<ChunkReference>,
    Vec<Value>,
    Vec<Value>,
) {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for publication in publications {
        let formatted = publication.formatted_content();
        let content = tavily_data_image_regex().replace_all(&formatted, "");
        let content: String = content.chars().take(10_000).collect();
        if content.is_empty() {
            continue;
        }
        let id = hash_str2int(&content, 100_000_000).to_string();
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": publication.title,
            "similarity": 1,
            "url": publication.pub_url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": publication.title,
            "doc_id": id,
            "count": 1,
            "url": publication.pub_url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(1.0),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            publication.title.replace('\n', " "),
            publication.pub_url.replace('\n', " "),
            content
        ));
    }
    let json = publications
        .iter()
        .map(GoogleScholarPublication::to_python_json)
        .collect();
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        ("json".into(), Value::Array(json)),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    (outputs, references, chunks, doc_aggs)
}

fn github_search_outputs(
    repositories: &[GitHubRepository],
) -> (
    Map<String, Value>,
    Vec<ChunkReference>,
    Vec<Value>,
    Vec<Value>,
) {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for repository in repositories {
        let formatted = repository.formatted_content();
        let content = tavily_data_image_regex().replace_all(&formatted, "");
        let content: String = content.chars().take(10_000).collect();
        if content.is_empty() {
            continue;
        }
        let id = hash_str2int(&content, 100_000_000).to_string();
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": repository.name,
            "similarity": 1,
            "url": repository.html_url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": repository.name,
            "doc_id": id,
            "count": 1,
            "url": repository.html_url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(1.0),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            repository.name.replace('\n', " "),
            repository.html_url.replace('\n', " "),
            content
        ));
    }
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        (
            "json".into(),
            Value::Array(
                repositories
                    .iter()
                    .map(|repository| repository.raw.clone())
                    .collect(),
            ),
        ),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    (outputs, references, chunks, doc_aggs)
}

fn arxiv_search_outputs(
    papers: &[ArxivPaper],
) -> (
    Map<String, Value>,
    Vec<ChunkReference>,
    Vec<Value>,
    Vec<Value>,
) {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for paper in papers {
        let content = tavily_data_image_regex().replace_all(&paper.summary, "");
        let content: String = content.chars().take(10_000).collect();
        if content.is_empty() {
            continue;
        }
        let url = paper.pdf_url.as_deref().unwrap_or_default();
        let id = hash_str2int(&content, 100_000_000).to_string();
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": paper.title,
            "similarity": 1,
            "url": url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": paper.title,
            "doc_id": id,
            "count": 1,
            "url": url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(1.0),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            paper.title.replace('\n', " "),
            url.replace('\n', " "),
            content
        ));
    }
    let go_results: Vec<_> = papers.iter().map(ArxivPaper::to_go_result).collect();
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        ("json".into(), serde_json::json!({"results": go_results})),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    (outputs, references, chunks, doc_aggs)
}

fn pubmed_search_outputs(
    articles: &[PubMedArticle],
) -> (
    Map<String, Value>,
    Vec<ChunkReference>,
    Vec<Value>,
    Vec<Value>,
) {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for article in articles {
        let formatted = article.formatted_content();
        let content = tavily_data_image_regex().replace_all(&formatted, "");
        let content: String = content.chars().take(10_000).collect();
        if content.is_empty() {
            continue;
        }
        let url = article.url();
        let id = hash_str2int(&content, 100_000_000).to_string();
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": article.title,
            "similarity": 1,
            "url": url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": article.title,
            "doc_id": id,
            "count": 1,
            "url": url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(1.0),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            article.title.replace('\n', " "),
            url.replace('\n', " "),
            content
        ));
    }
    let go_results: Vec<_> = articles.iter().map(PubMedArticle::to_go_result).collect();
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        ("json".into(), serde_json::json!({"results": go_results})),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    (outputs, references, chunks, doc_aggs)
}

fn duckduckgo_search_outputs(results: &[Value]) -> Result<TavilySearchOutput> {
    let mut chunks = Vec::new();
    let mut doc_aggs = Vec::new();
    let mut references = Vec::new();
    let mut formalized = Vec::new();
    for result in results {
        let result = result
            .as_object()
            .ok_or_else(|| anyhow!("DuckDuckGo result must be an object"))?;
        let content = result
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("DuckDuckGo result body must be a string"))?;
        let content = tavily_data_image_regex().replace_all(content, "");
        let content: String = content.chars().take(10_000).collect();
        if content.is_empty() {
            continue;
        }
        let title = result
            .get("title")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("DuckDuckGo result title must be a string"))?;
        let url = result
            .get("href")
            .and_then(Value::as_str)
            .or_else(|| result.get("url").and_then(Value::as_str))
            .ok_or_else(|| anyhow!("DuckDuckGo result href/url must be a string"))?;
        let id = hash_str2int(&content, 100_000_000).to_string();
        chunks.push(serde_json::json!({
            "chunk_id": id,
            "content": content,
            "doc_id": id,
            "docnm_kwd": title,
            "similarity": 1,
            "url": url
        }));
        doc_aggs.push(serde_json::json!({
            "doc_name": title,
            "doc_id": id,
            "count": 1,
            "url": url
        }));
        references.push(ChunkReference {
            id: id.clone(),
            kb_id: id.clone(),
            content: content.clone(),
            similarity: Some(1.0),
            vector_similarity: None,
            term_similarity: None,
        });
        formalized.push(format!(
            "\nID: {}\n├── Title: {}\n├── URL: {}\n└── Content:\n{}",
            hash_str2int(&id, 500),
            title.replace('\n', " "),
            url.replace('\n', " "),
            content
        ));
    }
    let outputs = Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized.join("\n")),
        ),
        ("json".into(), Value::Array(results.to_vec())),
        ("doc_aggs".into(), Value::Array(doc_aggs.clone())),
    ]);
    Ok((outputs, references, chunks, doc_aggs))
}

fn append_web_references(runtime: &mut CanvasRuntime, references: Vec<ChunkReference>) {
    for reference in references {
        if !runtime
            .references
            .iter()
            .any(|current| current.id == reference.id && current.kb_id == reference.kb_id)
        {
            runtime.references.push(reference);
        }
    }
}

fn set_wikipedia_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn set_google_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn set_google_scholar_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn set_github_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn set_yahoo_finance_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn set_arxiv_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn set_pubmed_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn set_duckduckgo_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn set_tavily_error(runtime: &mut CanvasRuntime, node: &CanvasNode, error: String) {
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("_ERROR".into(), Value::String(error.clone())),
            ("formalized_content".into(), Value::String(error.clone())),
        ]),
    );
}

fn hash_str2int(value: &str, modulus: u64) -> u64 {
    Sha1::digest(value.as_bytes())
        .iter()
        .fold(0_u64, |remainder, byte| {
            (remainder * 256 + u64::from(*byte)) % modulus
        })
}

fn tavily_data_image_regex() -> &'static Regex {
    static DATA_IMAGE: OnceLock<Regex> = OnceLock::new();
    DATA_IMAGE.get_or_init(|| {
        Regex::new(r"!?\[[a-z]+\]\(data:image/png;base64,[ 0-9A-Za-z/_=+\-]+\)")
            .expect("static Tavily data-image regex is valid")
    })
}

async fn execute_retrieval(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    input: &WorkflowRunInput<'_>,
) -> Result<()> {
    let empty_response = node
        .params
        .get("empty_response")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let query_value = node
        .params
        .get("query")
        .cloned()
        .unwrap_or_else(|| Value::String("sys.query".into()));
    let query = match query_value {
        Value::String(selector)
            if is_exact_selector(&selector)
                || selector.contains('@')
                || selector.starts_with("sys.")
                || selector.starts_with("env.") =>
        {
            stringify(&get_variable(runtime, &selector).unwrap_or(Value::Null))
        }
        Value::String(template) => resolve_template(runtime, &template)?,
        value => stringify(&resolve_parameter(runtime, &value)?),
    };
    let query = user_prefix_regex().replace(&query, "").into_owned();
    if query.trim().is_empty() {
        runtime.outputs.insert(
            node.id.clone(),
            retrieval_outputs(empty_response, Vec::new(), Vec::new()),
        );
        return Ok(());
    }
    let retriever = input.retriever.ok_or_else(|| {
        anyhow!(
            "Retrieval component '{}' requires the server retrieval backend",
            node.id
        )
    })?;
    let mut kb_ids = resolve_retrieval_kb_ids(runtime, node)?;
    if kb_ids.is_empty() {
        kb_ids.extend(input.fallback_kb_ids.iter().cloned());
    }
    let result = retriever
        .retrieve(WorkflowRetrievalRequest {
            query,
            kb_ids,
            similarity_threshold: probability_param(node, "similarity_threshold", 0.2)?,
            keywords_similarity_weight: probability_param(node, "keywords_similarity_weight", 0.5)?,
            top_n: positive_usize_param(node, "top_n", 8, 1024)?,
            top_k: positive_usize_param(node, "top_k", 1024, 4096)?,
            rerank_id: node
                .params
                .get("rerank_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            meta_data_filter: node
                .params
                .get("meta_data_filter")
                .map(|value| resolve_parameter(runtime, value))
                .transpose()?,
        })
        .await
        .with_context(|| format!("Retrieval component '{}' failed", node.id))?;
    runtime
        .retrieval
        .insert("chunks".into(), Value::Array(result.chunks.clone()));
    runtime
        .retrieval
        .insert("doc_aggs".into(), Value::Array(result.doc_aggs.clone()));
    for reference in &result.references {
        if !runtime
            .references
            .iter()
            .any(|current| current.id == reference.id && current.kb_id == reference.kb_id)
        {
            runtime.references.push(reference.clone());
        }
    }
    let formalized = if result.formalized_content.is_empty() {
        empty_response
    } else {
        result.formalized_content
    };
    runtime.outputs.insert(
        node.id.clone(),
        retrieval_outputs(formalized, result.chunks, result.doc_aggs),
    );
    Ok(())
}

fn retrieval_outputs(
    formalized_content: String,
    chunks: Vec<Value>,
    doc_aggs: Vec<Value>,
) -> Map<String, Value> {
    Map::from_iter([
        (
            "formalized_content".into(),
            Value::String(formalized_content),
        ),
        ("json".into(), Value::Array(chunks)),
        ("doc_aggs".into(), Value::Array(doc_aggs)),
    ])
}

fn resolve_retrieval_kb_ids(runtime: &CanvasRuntime, node: &CanvasNode) -> Result<Vec<String>> {
    let values = node
        .params
        .get("dataset_ids")
        .filter(|value| value.as_array().is_some_and(|values| !values.is_empty()))
        .or_else(|| node.params.get("kb_ids"));
    let Some(values) = values else {
        return Ok(Vec::new());
    };
    let values = values
        .as_array()
        .ok_or_else(|| anyhow!("Retrieval '{}' dataset ids must be an array", node.id))?;
    let mut result = Vec::new();
    for value in values {
        let value = value
            .as_str()
            .ok_or_else(|| anyhow!("Retrieval '{}' dataset ids must contain strings", node.id))?;
        let resolved = if value.contains('@')
            || value.starts_with("sys.")
            || value.starts_with("env.")
            || is_exact_selector(value)
        {
            get_variable(runtime, value)?
        } else {
            Value::String(value.to_owned())
        };
        match resolved {
            Value::String(value) if !value.trim().is_empty() => result.push(value),
            Value::Array(values) => result.extend(
                values
                    .into_iter()
                    .filter_map(|value| value.as_str().map(str::to_owned))
                    .filter(|value| !value.trim().is_empty()),
            ),
            Value::Null => {}
            _ => bail!(
                "Retrieval '{}' dataset variable '{}' must resolve to a string or string array",
                node.id,
                value
            ),
        }
    }
    result.sort();
    result.dedup();
    Ok(result)
}

fn probability_param(node: &CanvasNode, field: &str, default: f32) -> Result<f32> {
    let value = node
        .params
        .get(field)
        .map(|value| {
            value
                .as_f64()
                .ok_or_else(|| anyhow!("Retrieval '{}' {field} must be a number", node.id))
        })
        .transpose()?
        .unwrap_or(default as f64);
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        bail!("Retrieval '{}' {field} must be between 0 and 1", node.id);
    }
    Ok(value as f32)
}

fn positive_usize_param(
    node: &CanvasNode,
    field: &str,
    default: usize,
    maximum: usize,
) -> Result<usize> {
    let value = node
        .params
        .get(field)
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                anyhow!("Retrieval '{}' {field} must be a positive integer", node.id)
            })
        })
        .transpose()?
        .unwrap_or(default as u64);
    if value == 0 || value > maximum as u64 {
        bail!(
            "Retrieval '{}' {field} must be between 1 and {maximum}",
            node.id
        );
    }
    Ok(value as usize)
}

fn execute_switch(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<Vec<String>> {
    let conditions: &[Value] = match node.params.get("conditions") {
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("Switch '{}' conditions must be an array", node.id))?,
        None => &[],
    };
    let python_dialect = !node.params.contains_key("default");
    for (condition_index, condition) in conditions.iter().enumerate() {
        let condition = condition
            .as_object()
            .ok_or_else(|| anyhow!("Switch '{}' condition must be an object", node.id))?;
        let legacy_condition = !condition.contains_key("clauses");
        let (logical_operator, items) = if legacy_condition {
            (
                condition
                    .get("logical_operator")
                    .and_then(Value::as_str)
                    .unwrap_or("and"),
                condition
                    .get("items")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            )
        } else {
            (
                condition.get("op").and_then(Value::as_str).unwrap_or("and"),
                condition
                    .get("clauses")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            )
        };
        let mut results = Vec::new();
        for item in items {
            let item = match item.as_object() {
                Some(item) => item,
                None if legacy_condition && !python_dialect => continue,
                None => {
                    bail!("Switch '{}' condition item must be an object", node.id);
                }
            };
            if legacy_condition {
                let selector = item
                    .get("cpn_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if selector.is_empty() {
                    continue;
                }
                let operator = item.get("operator").and_then(Value::as_str);
                let operator = if python_dialect {
                    operator.ok_or_else(|| {
                        anyhow!("Switch '{}' condition operator is required", node.id)
                    })?
                } else {
                    operator.unwrap_or_default()
                };
                let right = item
                    .get("value")
                    .cloned()
                    .unwrap_or(Value::String(String::new()));
                if python_dialect {
                    let left = get_variable(runtime, selector)?;
                    let right = coerce_legacy_switch_right(&left, right).with_context(|| {
                        format!(
                            "Switch '{}' cannot convert the comparison value to a number",
                            node.id
                        )
                    })?;
                    results.push(process_legacy_switch_operator(&left, operator, &right)?);
                } else {
                    let left = modern_legacy_switch_left(runtime, selector);
                    let operator = match operator {
                        "" | "=" => "==",
                        "<>" => "!=",
                        operator => operator,
                    };
                    results.push(process_modern_switch_operator(&left, operator, &right)?);
                }
            } else {
                let left_expression = item.get("left").and_then(Value::as_str).unwrap_or_default();
                let left = modern_switch_left(runtime, left_expression);
                let operator = item.get("op").and_then(Value::as_str).unwrap_or("==");
                let right = item.get("right").cloned().unwrap_or(Value::Null);
                results.push(process_modern_switch_operator(&left, operator, &right)?);
            }
        }
        let matched = if logical_operator == "and" {
            !results.is_empty() && results.iter().all(|value| *value)
        } else if logical_operator == "or" || (legacy_condition && python_dialect) {
            results.iter().any(|value| *value)
        } else {
            false
        };
        if matched {
            let mut next = destination_array(condition.get("to"), &node.id, "condition.to")?;
            if next.is_empty() && !python_dialect {
                next.push(format!("matched_{condition_index}"));
            }
            save_switch_outputs(runtime, node, &next, python_dialect);
            return Ok(next);
        }
    }
    let next = match node
        .params
        .get("default")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        Some(default) => vec![default.to_owned()],
        None => {
            let mut targets =
                destination_array(node.params.get("end_cpn_ids"), &node.id, "end_cpn_ids")?;
            if !python_dialect {
                targets.truncate(1);
            }
            targets
        }
    };
    save_switch_outputs(runtime, node, &next, python_dialect);
    Ok(next)
}

fn save_switch_outputs(
    runtime: &mut CanvasRuntime,
    node: &CanvasNode,
    next: &[String],
    include_python_next: bool,
) {
    let targets = Value::Array(next.iter().cloned().map(Value::String).collect());
    let mut outputs = Map::from_iter([("_next".into(), targets.clone())]);
    if include_python_next {
        // Python exposes the graph display names in `next` and the executable
        // component ids in `_next`. RayRAG's compact DSL has no separate graph
        // display-name table, so the stable component ids are also its names.
        outputs.insert("next".into(), targets);
    }
    runtime.outputs.insert(node.id.clone(), outputs);
}

fn execute_variable_aggregator(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let groups = node
        .params
        .get("groups")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("VariableAggregator '{}' groups must be an array", node.id))?;
    if groups.is_empty() {
        bail!("VariableAggregator '{}' groups cannot be empty", node.id);
    }
    let mut outputs = Map::new();
    for group in groups {
        let group = group
            .as_object()
            .ok_or_else(|| anyhow!("VariableAggregator '{}' group must be an object", node.id))?;
        let name = group
            .get("group_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("VariableAggregator '{}' group_name is required", node.id))?;
        let variables = group
            .get("variables")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                anyhow!(
                    "VariableAggregator '{}' variables must be an array",
                    node.id
                )
            })?;
        if variables.is_empty() {
            bail!(
                "VariableAggregator '{}' variables of group '{}' cannot be empty",
                node.id,
                name
            );
        }
        for variable in variables {
            let selector = variable
                .as_str()
                .or_else(|| variable.get("value").and_then(Value::as_str))
                .ok_or_else(|| anyhow!("VariableAggregator '{}' selector is invalid", node.id))?;
            let value = get_variable(runtime, selector)?;
            if truthy(&value) {
                outputs.insert(name.to_owned(), value);
                break;
            }
        }
    }
    runtime.outputs.insert(node.id.clone(), outputs);
    Ok(())
}

fn execute_variable_assigner(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let variables = node
        .params
        .get("variables")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("VariableAssigner '{}' variables must be an array", node.id))?
        .clone();
    for item in variables {
        let item = item
            .as_object()
            .ok_or_else(|| anyhow!("VariableAssigner '{}' item must be an object", node.id))?;
        let selector = item
            .get("variable")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("VariableAssigner '{}' variable is required", node.id))?;
        let operator = item
            .get("operator")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("VariableAssigner '{}' operator is required", node.id))?;
        let parameter = item.get("parameter");
        if !matches!(operator, "clear" | "remove_first" | "remove_last") && parameter.is_none() {
            bail!("VariableAssigner '{}' variable is not complete", node.id);
        }
        let current = get_variable(runtime, selector).unwrap_or(Value::Null);
        let updated = assign_value(
            runtime,
            current,
            operator,
            parameter.unwrap_or(&Value::Null),
        )?;
        set_variable(runtime, selector, updated)?;
    }
    runtime.outputs.entry(node.id.clone()).or_default();
    Ok(())
}

fn execute_string_transform(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let method = string_transform_method(node)?;
    let delimiters = string_transform_delimiters(node)?;
    let result = match method {
        "split" => {
            let selector = node
                .params
                .get("split_ref")
                .map(|value| {
                    value.as_str().ok_or_else(|| {
                        anyhow!("StringTransform '{}' split_ref must be a string", node.id)
                    })
                })
                .transpose()?
                .unwrap_or_default();
            let direct_line = node
                .upstream
                .first()
                .and_then(|component_id| runtime.outputs.get(component_id))
                .and_then(|outputs| outputs.get("line"))
                .and_then(Value::as_str)
                .filter(|line| !line.is_empty())
                .map(|line| Value::String(line.to_owned()));
            let source = if let Some(line) = direct_line {
                line
            } else if selector.is_empty() {
                Value::String(String::new())
            } else {
                get_variable(runtime, selector).unwrap_or(Value::Null)
            };
            // RAGFlow treats every false-y input as an empty string, but
            // rejects truthy non-string values instead of silently JSON
            // encoding them before splitting.
            let source = match source {
                Value::String(source) => source,
                value if !truthy(&value) => String::new(),
                value => bail!(
                    "StringTransform '{}' split input is not a string: {}",
                    node.id,
                    json_type_name(&value)
                ),
            };
            let pattern = delimiters
                .iter()
                .map(|delimiter| regex::escape(delimiter))
                .collect::<Vec<_>>()
                .join("|");
            let splitter = Regex::new(&pattern)?;
            Value::Array(
                splitter
                    .split(&source)
                    .map(|value| Value::String(value.to_owned()))
                    .collect(),
            )
        }
        "merge" => {
            let script = node
                .params
                .get("script")
                .map(|value| {
                    value.as_str().ok_or_else(|| {
                        anyhow!("StringTransform '{}' script must be a string", node.id)
                    })
                })
                .transpose()?
                .unwrap_or_default();
            Value::String(resolve_string_transform_template_with_input(
                runtime,
                script,
                delimiters[0],
                node.upstream.first().map(String::as_str),
            ))
        }
        _ => unreachable!("string_transform_method validates the method"),
    };
    runtime
        .outputs
        .insert(node.id.clone(), Map::from_iter([("result".into(), result)]));
    Ok(())
}

fn validate_string_transform_params(node: &CanvasNode) -> Result<()> {
    string_transform_method(node)?;
    string_transform_delimiters(node)?;
    for field in ["script", "split_ref"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("StringTransform '{}' {field} must be a string", node.id);
        }
    }
    Ok(())
}

fn string_transform_method(node: &CanvasNode) -> Result<&str> {
    let method = node
        .params
        .get("method")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("StringTransform '{}' method must be a string", node.id))
        })
        .transpose()?
        .unwrap_or("split");
    if !matches!(method, "split" | "merge") {
        bail!(
            "StringTransform '{}' method must be 'split' or 'merge'",
            node.id
        );
    }
    Ok(method)
}

fn string_transform_delimiters(node: &CanvasNode) -> Result<Vec<&str>> {
    let Some(value) = node.params.get("delimiters") else {
        return Ok(vec![","]);
    };
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("StringTransform '{}' delimiters must be an array", node.id))?;
    if values.is_empty() {
        bail!("StringTransform '{}' delimiters cannot be empty", node.id);
    }
    values
        .iter()
        .map(|value| {
            value.as_str().ok_or_else(|| {
                anyhow!(
                    "StringTransform '{}' delimiters must contain strings",
                    node.id
                )
            })
        })
        .collect()
}

/// Resolve RAGFlow canvas references for StringTransform merge mode. Lists use
/// the component's first delimiter, unlike general Message interpolation,
/// which serializes native JSON arrays.
#[cfg(test)]
fn resolve_string_transform_template(
    runtime: &CanvasRuntime,
    template: &str,
    delimiter: &str,
) -> String {
    resolve_string_transform_template_with_input(runtime, template, delimiter, None)
}

fn resolve_string_transform_template_with_input(
    runtime: &CanvasRuntime,
    template: &str,
    delimiter: &str,
    input_component: Option<&str>,
) -> String {
    if !contains_jinja_syntax(template) {
        return resolve_string_transform_references(runtime, template, delimiter);
    }

    let (prepared, mut context, replacements) =
        prepare_string_transform_template(runtime, template, delimiter);
    add_string_transform_runtime_context(runtime, &mut context, input_component);

    let mut rendered =
        render_sandboxed_jinja(&prepared, context).unwrap_or_else(|_| prepared.clone());

    // Python Message.get_kwargs rewrites selector names before Jinja rendering
    // and performs a final plain substitution pass. Keep that pass so a
    // single-brace selector next to Jinja statements still resolves, including
    // after a parse/execution failure.
    for (name, value) in replacements {
        rendered = rendered.replace(&name, &value);
    }
    rendered
}

/// Loud Jinja renderer used by StringTransform's fail-soft wrapper.
///
/// Keeping the error-returning layer separate mirrors RAGFlow's standalone
/// resolver while the component caller intentionally retains its script on
/// parse, execution or fuel errors.
fn render_sandboxed_jinja(template: &str, context: Map<String, Value>) -> Result<String> {
    // The fixed RAGFlow Python runtime uses SandboxedEnvironment and silently
    // falls back to its rewritten script when rendering fails. MiniJinja has no
    // filesystem/network access without a loader or custom functions; fuel and
    // recursion limits additionally bound malicious or accidental templates.
    let mut environment = Environment::new();
    environment.set_auto_escape_callback(|_| AutoEscape::None);
    environment.set_undefined_behavior(UndefinedBehavior::Chainable);
    environment.set_fuel(Some(100_000));
    environment.set_recursion_limit(64);
    environment
        .render_str(template, Value::Object(context))
        .map_err(|error| anyhow!("StringTransform Jinja render failed: {error}"))
}

fn contains_jinja_syntax(template: &str) -> bool {
    template.contains("{{")
        || template.contains("{%")
        || template.contains("{#")
        || template.contains('|')
}

fn prepare_string_transform_template(
    runtime: &CanvasRuntime,
    template: &str,
    delimiter: &str,
) -> (String, Map<String, Value>, Vec<(String, String)>) {
    let mut prepared = String::with_capacity(template.len());
    let mut context = Map::new();
    let mut replacements = Vec::new();
    let mut last = 0;
    for capture in string_transform_selector_regex().captures_iter(template) {
        let full = capture.get(0).expect("selector regex has a full match");
        prepared.push_str(&template[last..full.start()]);
        let selector = capture
            .get(1)
            .expect("selector regex has a capture")
            .as_str();
        let name = normalize_template_identifier(selector);
        let value =
            string_transform_value(get_string_transform_variable(runtime, selector), delimiter);
        if full.as_str().trim_start().starts_with("{{") {
            prepared.push_str("{{ ");
            prepared.push_str(&name);
            prepared.push_str(" }}");
        } else {
            prepared.push_str(&name);
        }
        context.insert(name.clone(), Value::String(value.clone()));
        if !replacements.iter().any(|(existing, _)| existing == &name) {
            replacements.push((name, value));
        }
        last = full.end();
    }
    prepared.push_str(&template[last..]);
    (prepared, context, replacements)
}

fn add_string_transform_runtime_context(
    runtime: &CanvasRuntime,
    context: &mut Map<String, Value>,
    input_component: Option<&str>,
) {
    for (component_id, outputs) in &runtime.outputs {
        context
            .entry(component_id.clone())
            .or_insert_with(|| Value::Object(outputs.clone()));
        let normalized = normalize_template_identifier(component_id);
        context
            .entry(normalized)
            .or_insert_with(|| Value::Object(outputs.clone()));
    }

    if !runtime.sys.is_empty() {
        context
            .entry("sys")
            .or_insert_with(|| Value::Object(runtime.sys.clone()));
    }
    if !runtime.env.is_empty() {
        context
            .entry("env")
            .or_insert_with(|| Value::Object(runtime.env.clone()));
    }

    // Legacy DSL snapshots stored `sys.x` / `env.x` as flattened globals.
    // Merge those keys without overriding the explicit namespace maps above.
    for (name, value) in &runtime.globals {
        if let Some((namespace, path)) = name.split_once('.')
            && matches!(namespace, "sys" | "env")
        {
            let root = context
                .entry(namespace.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            insert_template_context_path(root, path, value.clone());
        }
        if is_template_identifier(name) {
            context.entry(name.clone()).or_insert_with(|| value.clone());
        }
    }

    for (alias, storage_name) in [
        ("item", "__item__"),
        ("index", "__index__"),
        ("result", "__result__"),
    ] {
        if let Some(value) = runtime
            .globals
            .get(alias)
            .or_else(|| runtime.globals.get(storage_name))
        {
            context.insert(alias.to_owned(), value.clone());
        }
    }

    // The fixed Go scheduler passes the first upstream node's output map as
    // component inputs. Those values take precedence over state/global names.
    if let Some(outputs) = input_component.and_then(|id| runtime.outputs.get(id)) {
        for (name, value) in outputs {
            if is_template_identifier(name) {
                context.insert(name.clone(), value.clone());
            }
        }
    }
}

fn insert_template_context_path(root: &mut Value, path: &str, value: Value) {
    let mut current = root;
    let mut keys = path.split('.').peekable();
    while let Some(key) = keys.next() {
        if keys.peek().is_none() {
            if let Some(object) = current.as_object_mut() {
                object.insert(key.to_owned(), value);
            }
            return;
        }
        let Some(object) = current.as_object_mut() else {
            return;
        };
        current = object
            .entry(key.to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
}

fn normalize_template_identifier(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if matches!(character, '@' | ':' | '.' | '-') {
                '_'
            } else {
                character
            }
        })
        .collect()
}

fn is_template_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn string_transform_value(value: Value, delimiter: &str) -> String {
    match value {
        Value::Array(values) => values
            .iter()
            .map(stringify)
            .collect::<Vec<_>>()
            .join(delimiter),
        value => stringify(&value),
    }
}

fn get_string_transform_variable(runtime: &CanvasRuntime, selector: &str) -> Value {
    let alias = match selector {
        "item" => Some("__item__"),
        "index" => Some("__index__"),
        "result" => Some("__result__"),
        _ => None,
    };
    if let Some(storage_name) = alias {
        return runtime
            .globals
            .get(selector)
            .or_else(|| runtime.globals.get(storage_name))
            .cloned()
            .unwrap_or(Value::Null);
    }
    get_variable(runtime, selector).unwrap_or(Value::Null)
}

fn resolve_string_transform_references(
    runtime: &CanvasRuntime,
    template: &str,
    delimiter: &str,
) -> String {
    let mut rendered = String::with_capacity(template.len());
    let mut last = 0;
    for capture in string_transform_selector_regex().captures_iter(template) {
        let full = capture.get(0).expect("selector regex has a full match");
        rendered.push_str(&template[last..full.start()]);
        let selector = capture
            .get(1)
            .expect("selector regex has a capture")
            .as_str();
        let value = get_string_transform_variable(runtime, selector);
        rendered.push_str(&string_transform_value(value, delimiter));
        last = full.end();
    }
    rendered.push_str(&template[last..]);
    rendered
}

fn string_transform_selector_regex() -> &'static Regex {
    static SELECTOR: OnceLock<Regex> = OnceLock::new();
    SELECTOR.get_or_init(|| {
        Regex::new(
            r"\{+\s*([A-Za-z0-9:_-]+@[A-Za-z0-9_.-]+|sys\.[A-Za-z0-9_.]+|env\.[A-Za-z0-9_.]+|item|index|result)\s*\}+",
        )
        .expect("static StringTransform selector regex is valid")
    })
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn validate_data_operations_params(node: &CanvasNode) -> Result<()> {
    data_operation_name(node)?;
    data_operation_queries(node)?;
    data_operation_string_list(node, "select_keys")?;
    data_operation_string_list(node, "remove_keys")?;
    for field in ["filter_values", "updates", "rename_keys"] {
        data_operation_object_list(node, field)?;
    }
    Ok(())
}

fn execute_data_operations(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let mut input_objects = Vec::new();
    for query in data_operation_queries(node)? {
        let value = get_variable(runtime, &query)
            .with_context(|| format!("DataOperations '{}' query '{query}'", node.id))?;
        match value {
            Value::Object(value) => input_objects.push(value),
            Value::Array(values) => input_objects.extend(
                values
                    .into_iter()
                    .filter_map(|value| value.as_object().cloned()),
            ),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    let result =
        match data_operation_name(node)? {
            "select_keys" => {
                let selected = data_operation_string_list(node, "select_keys")?;
                Value::Array(
                    input_objects
                        .into_iter()
                        .map(|object| {
                            Value::Object(Map::from_iter(object.into_iter().filter(|(key, _)| {
                                selected.iter().any(|selected| selected == key)
                            })))
                        })
                        .collect(),
                )
            }
            "literal_eval" => Value::Array(
                input_objects
                    .into_iter()
                    .map(Value::Object)
                    .map(data_operation_recursive_eval)
                    .collect(),
            ),
            "combine" => data_operation_combine(input_objects),
            "filter_values" => {
                let rules = data_operation_object_list(node, "filter_values")?;
                Value::Array(
                    input_objects
                        .into_iter()
                        .filter(|object| {
                            rules
                                .iter()
                                .all(|rule| data_operation_rule_matches(runtime, object, rule))
                        })
                        .map(Value::Object)
                        .collect(),
                )
            }
            "append_or_update" => {
                let updates = data_operation_object_list(node, "updates")?;
                Value::Array(
                    input_objects
                        .into_iter()
                        .map(|mut object| {
                            for update in &updates {
                                let key = update
                                    .get("key")
                                    .map(data_operation_norm)
                                    .unwrap_or_default();
                                let key = key.trim();
                                if key.is_empty() {
                                    continue;
                                }
                                let value = update.get("value").cloned().unwrap_or(Value::Null);
                                object.insert(
                                    key.to_owned(),
                                    data_operation_update_value(runtime, value),
                                );
                            }
                            Value::Object(object)
                        })
                        .collect(),
                )
            }
            "remove_keys" => {
                let remove = data_operation_string_list(node, "remove_keys")?;
                Value::Array(
                    input_objects
                        .into_iter()
                        .map(|mut object| {
                            for key in &remove {
                                object.remove(key);
                            }
                            Value::Object(object)
                        })
                        .collect(),
                )
            }
            "rename_keys" => {
                let pairs = data_operation_object_list(node, "rename_keys")?;
                Value::Array(
                    input_objects
                        .into_iter()
                        .map(|mut object| {
                            for pair in &pairs {
                                let old = pair
                                    .get("old_key")
                                    .map(data_operation_norm)
                                    .unwrap_or_default();
                                let new = pair
                                    .get("new_key")
                                    .map(data_operation_norm)
                                    .unwrap_or_default();
                                let (old, new) = (old.trim(), new.trim());
                                if old.is_empty() || new.is_empty() || old == new {
                                    continue;
                                }
                                if let Some(value) = object.remove(old) {
                                    object.insert(new.to_owned(), value);
                                }
                            }
                            Value::Object(object)
                        })
                        .collect(),
                )
            }
            _ => unreachable!("data_operation_name validates the operation"),
        };
    runtime
        .outputs
        .insert(node.id.clone(), Map::from_iter([("result".into(), result)]));
    Ok(())
}

const MAX_EXCEL_PROCESSOR_BYTES: usize = 16 << 20;
const MAX_EXCEL_PROCESSOR_ROWS: usize = 100_000;

#[derive(Debug)]
struct ExcelSource {
    filename: String,
    bytes: Vec<u8>,
}

fn validate_excel_processor_params(node: &CanvasNode) -> Result<()> {
    excel_processor_operation(node)?;
    for field in [
        "sheet_selection",
        "merge_strategy",
        "join_on",
        "transform_instructions",
        "output_format",
        "output_filename",
        "sheet_name",
    ] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("ExcelProcessor '{}' {field} must be a string", node.id);
        }
    }
    if let Some(format) = node.params.get("output_format").and_then(Value::as_str)
        && !matches!(format.to_ascii_lowercase().as_str(), "xlsx" | "csv")
    {
        bail!(
            "ExcelProcessor '{}' output_format must be xlsx or csv",
            node.id
        );
    }
    if let Some(strategy) = node.params.get("merge_strategy").and_then(Value::as_str)
        && !matches!(strategy.to_ascii_lowercase().as_str(), "concat" | "join")
    {
        bail!(
            "ExcelProcessor '{}' merge_strategy must be concat or join",
            node.id
        );
    }
    if node
        .params
        .get("input_files")
        .is_some_and(|value| !value.is_array())
    {
        bail!("ExcelProcessor '{}' input_files must be an array", node.id);
    }
    if node
        .params
        .get("file_refs")
        .is_some_and(|value| !value.is_array())
    {
        bail!("ExcelProcessor '{}' file_refs must be an array", node.id);
    }
    if node
        .params
        .get("output_data")
        .is_some_and(|value| !value.is_array() && !value.is_object())
    {
        bail!(
            "ExcelProcessor '{}' output_data must be an array or object",
            node.id
        );
    }
    Ok(())
}

fn excel_processor_operation(node: &CanvasNode) -> Result<&str> {
    let operation = node
        .params
        .get("operation")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("ExcelProcessor '{}' operation must be a string", node.id))
        })
        .transpose()?
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("read");
    if !matches!(
        operation.to_ascii_lowercase().as_str(),
        "read" | "write" | "merge" | "transform" | "output"
    ) {
        bail!(
            "ExcelProcessor '{}' operation must be one of read, write, merge, transform, output",
            node.id
        );
    }
    Ok(operation)
}

fn execute_excel_processor(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let outputs = match excel_processor_operation(node)?
        .to_ascii_lowercase()
        .as_str()
    {
        "read" => excel_processor_read(runtime, node)?,
        "merge" => excel_processor_merge(runtime, node)?,
        "transform" => excel_processor_transform(runtime, node)?,
        "write" | "output" => excel_processor_output(runtime, node)?,
        _ => unreachable!("excel_processor_operation validates the operation"),
    };
    runtime.outputs.insert(node.id.clone(), outputs);
    Ok(())
}

fn excel_processor_read(runtime: &CanvasRuntime, node: &CanvasNode) -> Result<Map<String, Value>> {
    let sources = excel_processor_sources(runtime, node)?;
    if sources.is_empty() {
        if node.params.contains_key("input_files") {
            return Ok(Map::from_iter([
                ("data".into(), Value::Object(Map::new())),
                ("rows".into(), Value::Array(Vec::new())),
                ("sheet_names".into(), Value::Array(Vec::new())),
                ("size".into(), Value::from(0)),
                (
                    "summary".into(),
                    Value::String("No Excel files found".into()),
                ),
                ("markdown".into(), Value::String("No data".into())),
            ]));
        }
        bail!(
            "ExcelProcessor '{}' file_ref has no spreadsheet bytes; provide bytes, file_ref, file_refs, or input_files",
            node.id
        );
    }

    let mut data = Map::new();
    let mut summaries = Vec::new();
    let mut markdown = Vec::new();
    let mut first_rows = None;
    let mut sheet_names = Vec::new();
    for source in sources {
        let sheets = read_excel_source(&source)?;
        for sheet in &sheets {
            if !sheet_names.iter().any(|name| name == &sheet.name) {
                sheet_names.push(sheet.name.clone());
            }
        }
        let selected = select_excel_sheets(node, &sheets);
        let multiple = selected.len() > 1;
        for sheet in selected {
            first_rows.get_or_insert_with(|| sheet.rows.clone());
            let key = if multiple {
                format!("{}_{}", source.filename, sheet.name)
            } else {
                source.filename.clone()
            };
            let records = excel_rows_to_records(&sheet.rows);
            let columns = sheet.rows.first().map(Vec::len).unwrap_or(0);
            summaries.push(format!(
                "**{key}**: {} rows, {columns} columns ({})",
                records.len(),
                sheet
                    .rows
                    .first()
                    .map(|headers| headers
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            ));
            markdown.push(format!("### {key}\n\n{}", excel_markdown(&sheet.rows, 10)));
            data.insert(key, Value::Array(records));
        }
    }
    let first_rows = first_rows.unwrap_or_default();
    Ok(Map::from_iter([
        ("data".into(), Value::Object(data)),
        ("rows".into(), excel_string_rows_value(&first_rows)),
        (
            "sheet_names".into(),
            Value::Array(sheet_names.into_iter().map(Value::String).collect()),
        ),
        ("size".into(), Value::from(first_rows.len())),
        ("summary".into(), Value::String(summaries.join("\n"))),
        ("markdown".into(), Value::String(markdown.join("\n\n"))),
    ]))
}

fn excel_processor_merge(runtime: &CanvasRuntime, node: &CanvasNode) -> Result<Map<String, Value>> {
    let sources = excel_processor_sources(runtime, node)?;
    if sources.is_empty() {
        if node.params.contains_key("input_files") {
            return Ok(Map::from_iter([
                ("data".into(), serde_json::json!({})),
                ("rows".into(), Value::Array(Vec::new())),
                ("sheet_names".into(), Value::Array(Vec::new())),
                ("size".into(), Value::from(0)),
                ("summary".into(), Value::String("No data to merge".into())),
                ("markdown".into(), Value::String("No data".into())),
            ]));
        }
        bail!("ExcelProcessor '{}' file_refs must not be empty", node.id);
    }

    let mut raw_rows = Vec::new();
    let mut record_sets = Vec::new();
    let mut sheet_names = Vec::new();
    for source in &sources {
        let sheets = read_excel_source(source)?;
        for sheet in &sheets {
            if !sheet_names.iter().any(|name| name == &sheet.name) {
                sheet_names.push(sheet.name.clone());
            }
        }
        for sheet in select_excel_sheets(node, &sheets) {
            raw_rows.extend(sheet.rows.clone());
            record_sets.push(excel_rows_to_records(&sheet.rows));
        }
    }
    let strategy = node
        .params
        .get("merge_strategy")
        .and_then(Value::as_str)
        .unwrap_or("concat")
        .to_ascii_lowercase();
    let records = if strategy == "join" {
        let join_on = node
            .params
            .get("join_on")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        if join_on.is_empty() {
            record_sets.into_iter().flatten().collect()
        } else {
            excel_outer_join(record_sets, join_on)
        }
    } else {
        record_sets.into_iter().flatten().collect()
    };
    let markdown_rows = excel_records_to_rows(&records);
    Ok(Map::from_iter([
        ("data".into(), serde_json::json!({"merged": records})),
        ("rows".into(), excel_string_rows_value(&raw_rows)),
        (
            "sheet_names".into(),
            Value::Array(sheet_names.into_iter().map(Value::String).collect()),
        ),
        ("size".into(), Value::from(raw_rows.len())),
        (
            "summary".into(),
            Value::String(format!(
                "Merged {} sources into {} rows, {} columns",
                sources.len(),
                markdown_rows.len().saturating_sub(1),
                markdown_rows.first().map(Vec::len).unwrap_or(0)
            )),
        ),
        (
            "markdown".into(),
            Value::String(excel_markdown(&markdown_rows, 20)),
        ),
    ]))
}

fn excel_processor_transform(
    runtime: &CanvasRuntime,
    node: &CanvasNode,
) -> Result<Map<String, Value>> {
    let Some(value) = node.params.get("transform_data") else {
        return Ok(Map::from_iter([(
            "summary".into(),
            Value::String("No transform data reference provided".into()),
        )]));
    };
    if value.as_str().is_some_and(|value| value.trim().is_empty()) {
        return Ok(Map::from_iter([(
            "summary".into(),
            Value::String("No transform data reference provided".into()),
        )]));
    }
    let value = resolve_excel_parameter(runtime, value)?;
    if value.is_null() {
        return Ok(Map::from_iter([(
            "summary".into(),
            Value::String("Transform data is empty".into()),
        )]));
    }
    if !value.is_array() && !value.is_object() {
        return Ok(Map::from_iter([
            ("data".into(), serde_json::json!({"raw": stringify(&value)})),
            ("markdown".into(), Value::String(stringify(&value))),
            (
                "summary".into(),
                Value::String("Transformed data ready for processing".into()),
            ),
        ]));
    }
    let sheets = excel_data_to_sheets(&value, "Sheet1")?;
    let data = match &value {
        Value::Object(object) if object.values().all(Value::is_array) => value.clone(),
        Value::Object(_) => Value::Array(vec![value.clone()]),
        Value::Array(_) => value.clone(),
        _ => unreachable!("scalar transform values returned above"),
    };
    let markdown = sheets
        .iter()
        .map(|(name, rows)| format!("### {name}\n\n{}", excel_value_markdown(rows, usize::MAX)))
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(Map::from_iter([
        ("data".into(), data),
        ("markdown".into(), Value::String(markdown)),
        (
            "summary".into(),
            Value::String("Transformed data ready for processing".into()),
        ),
    ]))
}

fn excel_processor_output(
    runtime: &CanvasRuntime,
    node: &CanvasNode,
) -> Result<Map<String, Value>> {
    let operation = excel_processor_operation(node)?.to_ascii_lowercase();
    let value = if operation == "write" {
        node.params.get("output_data")
    } else {
        node.params.get("transform_data")
    };
    let Some(value) = value else {
        return Ok(Map::from_iter([(
            "summary".into(),
            Value::String("No data reference for output".into()),
        )]));
    };
    if value.as_str().is_some_and(|value| value.trim().is_empty()) {
        return Ok(Map::from_iter([(
            "summary".into(),
            Value::String("No data reference for output".into()),
        )]));
    }
    let value = resolve_excel_parameter(runtime, value)?;
    if value.is_null() {
        return Ok(Map::from_iter([(
            "summary".into(),
            Value::String("No data to output".into()),
        )]));
    }
    let default_sheet = node
        .params
        .get("sheet_name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
        .unwrap_or("Sheet1");
    let sheets = excel_data_to_sheets(&value, default_sheet)?;
    let format = node
        .params
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("xlsx")
        .to_ascii_lowercase();
    let filename = node
        .params
        .get("output_filename")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("output");
    let (bytes, mime_type, filename) = if format == "csv" {
        let rows = sheets
            .first()
            .map(|(_, rows)| rows.as_slice())
            .unwrap_or(&[]);
        (
            excel_write_csv(rows)?,
            "text/csv",
            format!("{filename}.csv"),
        )
    } else {
        (
            crate::parser::excel::write_xlsx_sheets(&sheets)?,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            format!("{filename}.xlsx"),
        )
    };
    if bytes.len() > MAX_EXCEL_PROCESSOR_BYTES {
        bail!(
            "ExcelProcessor '{}' generated file exceeds the 16 MiB safety limit",
            node.id
        );
    }
    let encoded = BASE64_STANDARD.encode(&bytes);
    let rows = sheets
        .first()
        .map(|(_, rows)| rows.clone())
        .unwrap_or_default();
    let row_count: usize = sheets
        .iter()
        .map(|(_, rows)| rows.len().saturating_sub(1))
        .sum();
    let sheet_names = sheets
        .iter()
        .map(|(name, _)| Value::String(name.clone()))
        .collect::<Vec<_>>();
    Ok(Map::from_iter([
        ("data".into(), excel_sheets_data(&sheets)),
        (
            "rows".into(),
            Value::Array(rows.into_iter().map(Value::Array).collect()),
        ),
        ("sheet_names".into(), Value::Array(sheet_names)),
        ("size".into(), Value::from(bytes.len())),
        ("bytes".into(), Value::String(encoded.clone())),
        (
            "attachment".into(),
            serde_json::json!({
                "format": format,
                "file_name": filename,
                "filename": filename,
                "mime_type": mime_type,
                "data": format!("data:{mime_type};base64,{encoded}")
            }),
        ),
        (
            "summary".into(),
            Value::String(format!(
                "Generated {filename} with {} sheet(s), {row_count} total rows",
                sheets.len()
            )),
        ),
    ]))
}

fn excel_processor_sources(runtime: &CanvasRuntime, node: &CanvasNode) -> Result<Vec<ExcelSource>> {
    let mut sources = Vec::new();
    if let Some(files) = node.params.get("input_files").and_then(Value::as_array) {
        for (index, file) in files.iter().enumerate() {
            let (name, value) = match file {
                Value::Object(object) if object.contains_key("input") => (
                    object
                        .get("name")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    object.get("input").expect("input key was checked"),
                ),
                _ => (None, file),
            };
            let resolved = resolve_excel_parameter(runtime, value)
                .with_context(|| format!("ExcelProcessor '{}' input_files[{index}]", node.id))?;
            collect_excel_sources(&resolved, name.as_deref(), &mut sources)?;
        }
        return Ok(sources);
    }
    if let Some(files) = node.params.get("file_refs") {
        for file in files
            .as_array()
            .expect("file_refs is validated as an array")
        {
            let resolved = resolve_excel_parameter(runtime, file)?;
            collect_excel_sources(&resolved, None, &mut sources)?;
        }
        return Ok(sources);
    }
    for field in ["bytes", "file_ref"] {
        if let Some(value) = node.params.get(field) {
            let resolved = resolve_excel_parameter(runtime, value)?;
            collect_excel_sources(&resolved, None, &mut sources)?;
            if !sources.is_empty() {
                break;
            }
        }
    }
    Ok(sources)
}

fn resolve_excel_parameter(runtime: &CanvasRuntime, value: &Value) -> Result<Value> {
    let Value::String(text) = value else {
        return Ok(value.clone());
    };
    let trimmed = text.trim();
    if is_exact_selector(trimmed)
        || trimmed.contains('@')
        || trimmed.starts_with("sys.")
        || trimmed.starts_with("env.")
        || runtime.globals.contains_key(trimmed)
    {
        get_variable(runtime, trimmed)
    } else {
        Ok(value.clone())
    }
}

fn collect_excel_sources(
    value: &Value,
    name_hint: Option<&str>,
    output: &mut Vec<ExcelSource>,
) -> Result<()> {
    match value {
        Value::Null => Ok(()),
        Value::Array(values) => {
            for value in values {
                collect_excel_sources(value, name_hint, output)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            let filename = name_hint
                .or_else(|| object.get("name").and_then(Value::as_str))
                .or_else(|| object.get("filename").and_then(Value::as_str))
                .or_else(|| object.get("file_name").and_then(Value::as_str));
            let value = ["bytes", "data", "content", "base64"]
                .iter()
                .find_map(|field| object.get(*field).filter(|value| value.is_string()));
            let Some(value) = value else {
                bail!(
                    "ExcelProcessor file descriptor requires inline bytes/data/content/base64; persisted id-only files require the external file service"
                );
            };
            collect_excel_sources(value, filename, output)
        }
        Value::String(value) => {
            let (mime, encoded) = if let Some(payload) = value.strip_prefix("data:") {
                let (metadata, encoded) = payload
                    .split_once(',')
                    .ok_or_else(|| anyhow!("ExcelProcessor data URI has no comma separator"))?;
                if !metadata.to_ascii_lowercase().ends_with(";base64") {
                    bail!("ExcelProcessor data URI must use base64 encoding");
                }
                (Some(metadata.trim_end_matches(";base64")), encoded)
            } else {
                (None, value.as_str())
            };
            let bytes = BASE64_STANDARD
                .decode(encoded.trim())
                .map_err(|error| anyhow!("ExcelProcessor file_ref is not valid base64: {error}"))?;
            if bytes.is_empty() {
                bail!("ExcelProcessor file_ref decodes to an empty file");
            }
            if bytes.len() > MAX_EXCEL_PROCESSOR_BYTES {
                bail!("ExcelProcessor input exceeds the 16 MiB safety limit");
            }
            let default_name = if mime == Some("text/csv") {
                "uploaded.csv"
            } else {
                "uploaded.xlsx"
            };
            output.push(ExcelSource {
                filename: name_hint.unwrap_or(default_name).to_owned(),
                bytes,
            });
            Ok(())
        }
        _ => bail!("ExcelProcessor file bytes must be a base64 string or file descriptor"),
    }
}

fn read_excel_source(source: &ExcelSource) -> Result<Vec<crate::parser::excel::SpreadsheetSheet>> {
    if source.filename.to_ascii_lowercase().ends_with(".csv") {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(source.bytes.as_slice());
        let rows = reader
            .records()
            .take(MAX_EXCEL_PROCESSOR_ROWS)
            .map(|record| {
                record
                    .map(|record| record.iter().map(str::to_owned).collect::<Vec<_>>())
                    .map_err(Into::into)
            })
            .collect::<Result<Vec<_>>>()?;
        return Ok(vec![crate::parser::excel::SpreadsheetSheet {
            name: "Sheet1".into(),
            rows,
            truncated: false,
            merged_ranges: Vec::new(),
        }]);
    }
    crate::parser::excel::read_xlsx_sheets(&source.bytes, MAX_EXCEL_PROCESSOR_ROWS)
        .with_context(|| format!("ExcelProcessor failed to parse '{}'", source.filename))
}

fn select_excel_sheets<'a>(
    node: &CanvasNode,
    sheets: &'a [crate::parser::excel::SpreadsheetSheet],
) -> Vec<&'a crate::parser::excel::SpreadsheetSheet> {
    if let Some(sheet_name) = node
        .params
        .get("sheet_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return sheets
            .iter()
            .filter(|sheet| sheet.name == sheet_name)
            .collect();
    }
    if !node.params.contains_key("input_files") && !node.params.contains_key("sheet_selection") {
        return sheets.first().into_iter().collect();
    }
    match node
        .params
        .get("sheet_selection")
        .and_then(Value::as_str)
        .unwrap_or("all")
    {
        "first" => sheets.first().into_iter().collect(),
        "all" | "" => sheets.iter().collect(),
        selection => {
            let requested = selection
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .collect::<HashSet<_>>();
            sheets
                .iter()
                .filter(|sheet| requested.contains(sheet.name.as_str()))
                .collect()
        }
    }
}

fn excel_rows_to_records(rows: &[Vec<String>]) -> Vec<Value> {
    let Some(headers) = rows.first() else {
        return Vec::new();
    };
    let headers = unique_excel_headers(headers);
    rows.iter()
        .skip(1)
        .map(|row| {
            Value::Object(Map::from_iter(headers.iter().enumerate().map(
                |(index, header)| {
                    (
                        header.clone(),
                        Value::String(row.get(index).cloned().unwrap_or_default()),
                    )
                },
            )))
        })
        .collect()
}

fn unique_excel_headers(headers: &[String]) -> Vec<String> {
    let mut used = HashSet::new();
    headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            let base = if header.is_empty() {
                format!("column_{}", index + 1)
            } else {
                header.clone()
            };
            let mut candidate = base.clone();
            let mut suffix = 1;
            while !used.insert(candidate.clone()) {
                candidate = format!("{base}.{suffix}");
                suffix += 1;
            }
            candidate
        })
        .collect()
}

fn excel_records_to_rows(records: &[Value]) -> Vec<Vec<String>> {
    let mut headers = Vec::new();
    for object in records.iter().filter_map(Value::as_object) {
        for key in object.keys() {
            if !headers.iter().any(|header| header == key) {
                headers.push(key.clone());
            }
        }
    }
    if headers.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![headers.clone()];
    rows.extend(records.iter().filter_map(Value::as_object).map(|object| {
        headers
            .iter()
            .map(|header| object.get(header).map(stringify).unwrap_or_default())
            .collect()
    }));
    rows
}

fn excel_outer_join(record_sets: Vec<Vec<Value>>, join_on: &str) -> Vec<Value> {
    let mut sets = record_sets.into_iter();
    let mut joined = sets.next().unwrap_or_default();
    for right in sets {
        let mut used = HashSet::new();
        let mut next = Vec::new();
        for left in &joined {
            let Some(left_object) = left.as_object() else {
                continue;
            };
            let left_key = left_object.get(join_on).map(stringify).unwrap_or_default();
            let mut matched = false;
            for (index, right) in right.iter().enumerate() {
                let Some(right_object) = right.as_object() else {
                    continue;
                };
                if right_object.get(join_on).map(stringify).unwrap_or_default() != left_key {
                    continue;
                }
                matched = true;
                used.insert(index);
                next.push(Value::Object(merge_excel_join_rows(
                    left_object,
                    right_object,
                    join_on,
                )));
            }
            if !matched {
                next.push(left.clone());
            }
        }
        next.extend(
            right
                .into_iter()
                .enumerate()
                .filter(|(index, _)| !used.contains(index))
                .map(|(_, value)| value),
        );
        joined = next;
    }
    joined
}

fn merge_excel_join_rows(
    left: &Map<String, Value>,
    right: &Map<String, Value>,
    join_on: &str,
) -> Map<String, Value> {
    let mut merged = left.clone();
    for (key, value) in right {
        if key == join_on {
            merged.entry(key.clone()).or_insert_with(|| value.clone());
        } else if merged.contains_key(key) {
            merged.insert(format!("{key}_right"), value.clone());
        } else {
            merged.insert(key.clone(), value.clone());
        }
    }
    merged
}

fn excel_data_to_sheets(
    value: &Value,
    default_sheet: &str,
) -> Result<Vec<(String, Vec<Vec<Value>>)>> {
    match value {
        Value::Object(object) if !object.is_empty() && object.values().all(Value::is_array) => {
            object
                .iter()
                .map(|(name, rows)| Ok((name.clone(), excel_value_to_rows(rows)?)))
                .collect()
        }
        Value::Object(_) | Value::Array(_) => Ok(vec![(
            default_sheet.to_owned(),
            excel_value_to_rows(value)?,
        )]),
        _ => bail!("ExcelProcessor output data must be an object or array"),
    }
}

fn excel_value_to_rows(value: &Value) -> Result<Vec<Vec<Value>>> {
    match value {
        Value::Object(object) => {
            let headers = object.keys().cloned().collect::<Vec<_>>();
            Ok(vec![
                headers.iter().cloned().map(Value::String).collect(),
                headers
                    .iter()
                    .map(|header| object.get(header).cloned().unwrap_or(Value::Null))
                    .collect(),
            ])
        }
        Value::Array(values) if values.is_empty() => Ok(Vec::new()),
        Value::Array(values) if values.iter().all(Value::is_object) => {
            let mut headers = Vec::new();
            for object in values.iter().filter_map(Value::as_object) {
                for key in object.keys() {
                    if !headers.iter().any(|header| header == key) {
                        headers.push(key.clone());
                    }
                }
            }
            let mut rows = vec![headers.iter().cloned().map(Value::String).collect()];
            rows.extend(values.iter().filter_map(Value::as_object).map(|object| {
                headers
                    .iter()
                    .map(|header| object.get(header).cloned().unwrap_or(Value::Null))
                    .collect()
            }));
            Ok(rows)
        }
        Value::Array(values) if values.iter().all(Value::is_array) => {
            Ok(values.iter().filter_map(Value::as_array).cloned().collect())
        }
        Value::Array(values) => Ok(values.iter().cloned().map(|value| vec![value]).collect()),
        _ => bail!("ExcelProcessor rows must be an object or array"),
    }
}

fn excel_sheets_data(sheets: &[(String, Vec<Vec<Value>>)]) -> Value {
    Value::Object(Map::from_iter(sheets.iter().map(|(name, rows)| {
        let string_rows = rows
            .iter()
            .map(|row| row.iter().map(stringify).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        (
            name.clone(),
            Value::Array(excel_rows_to_records(&string_rows)),
        )
    })))
}

fn excel_write_csv(rows: &[Vec<Value>]) -> Result<Vec<u8>> {
    let mut writer = csv::WriterBuilder::new().from_writer(Vec::new());
    for row in rows {
        writer.write_record(row.iter().map(stringify))?;
    }
    writer.flush()?;
    Ok(writer.into_inner().map_err(|error| error.into_error())?)
}

fn excel_string_rows_value(rows: &[Vec<String>]) -> Value {
    Value::Array(
        rows.iter()
            .map(|row| Value::Array(row.iter().cloned().map(Value::String).collect()))
            .collect(),
    )
}

fn excel_markdown(rows: &[Vec<String>], maximum: usize) -> String {
    let values = rows
        .iter()
        .map(|row| row.iter().cloned().map(Value::String).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    excel_value_markdown(&values, maximum)
}

fn excel_value_markdown(rows: &[Vec<Value>], maximum: usize) -> String {
    let Some(headers) = rows.first() else {
        return "No data".into();
    };
    let escape = |value: &Value| {
        stringify(value)
            .replace('|', "\\|")
            .replace(['\r', '\n'], " ")
    };
    let mut output = format!(
        "| {} |\n|{}|",
        headers.iter().map(&escape).collect::<Vec<_>>().join(" | "),
        headers.iter().map(|_| "---").collect::<Vec<_>>().join("|")
    );
    for row in rows.iter().skip(1).take(maximum) {
        output.push_str("\n| ");
        output.push_str(&row.iter().map(&escape).collect::<Vec<_>>().join(" | "));
        output.push_str(" |");
    }
    output
}

fn data_operation_name(node: &CanvasNode) -> Result<&str> {
    let operation = node
        .params
        .get("operations")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("DataOperations '{}' operations must be a string", node.id))
        })
        .transpose()?
        .unwrap_or("literal_eval");
    let operation = if operation.is_empty() {
        "literal_eval"
    } else {
        operation
    };
    if !matches!(
        operation,
        "select_keys"
            | "literal_eval"
            | "combine"
            | "filter_values"
            | "append_or_update"
            | "remove_keys"
            | "rename_keys"
    ) {
        bail!(
            "DataOperations '{}' operations must be one of select_keys, literal_eval, combine, filter_values, append_or_update, remove_keys, rename_keys",
            node.id
        );
    }
    Ok(operation)
}

fn data_operation_queries(node: &CanvasNode) -> Result<Vec<String>> {
    let Some(value) = node.params.get("query") else {
        return Ok(Vec::new());
    };
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(value) => Ok(data_operation_csv(value)),
        Value::Array(values) => values
            .iter()
            .map(|value| match value {
                Value::String(value) => Ok(value.trim().to_owned()),
                Value::Object(value) => value
                    .get("input")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        anyhow!(
                            "DataOperations '{}' query objects must contain a string input",
                            node.id
                        )
                    }),
                _ => bail!(
                    "DataOperations '{}' query must contain strings or input objects",
                    node.id
                ),
            })
            .collect::<Result<Vec<_>>>()
            .map(|queries| {
                queries
                    .into_iter()
                    .filter(|query| !query.is_empty())
                    .collect()
            }),
        _ => bail!(
            "DataOperations '{}' query must be a string or array",
            node.id
        ),
    }
}

fn data_operation_string_list(node: &CanvasNode, field: &str) -> Result<Vec<String>> {
    let Some(value) = node.params.get(field) else {
        return Ok(Vec::new());
    };
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(value) => Ok(data_operation_csv(value)),
        Value::Array(values) => values
            .iter()
            .map(|value| match value {
                Value::String(value) => Ok(value.trim().to_owned()),
                Value::Object(value) => value
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        anyhow!(
                            "DataOperations '{}' {field} objects must contain a string name",
                            node.id
                        )
                    }),
                _ => bail!(
                    "DataOperations '{}' {field} must contain strings or name objects",
                    node.id
                ),
            })
            .collect::<Result<Vec<_>>>()
            .map(|values| {
                values
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect()
            }),
        _ => bail!(
            "DataOperations '{}' {field} must be a string or array",
            node.id
        ),
    }
}

fn data_operation_object_list(node: &CanvasNode, field: &str) -> Result<Vec<Map<String, Value>>> {
    let Some(value) = node.params.get(field) else {
        return Ok(Vec::new());
    };
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value.as_object().cloned().ok_or_else(|| {
                    anyhow!("DataOperations '{}' {field} must contain objects", node.id)
                })
            })
            .collect(),
        _ => bail!("DataOperations '{}' {field} must be an array", node.id),
    }
}

fn data_operation_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn data_operation_recursive_eval(value: Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, data_operation_recursive_eval(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(data_operation_recursive_eval)
                .collect(),
        ),
        Value::String(value) => {
            let trimmed = value.trim();
            let lowered = trimmed.to_ascii_lowercase();
            let looks_like_literal = trimmed.starts_with(['{', '[', '(', '\'', '"'])
                || matches!(lowered.as_str(), "true" | "false" | "null" | "none")
                || (!trimmed.is_empty()
                    && trimmed
                        .chars()
                        .all(|character| character.is_ascii_digit() || character == '.'));
            if !looks_like_literal {
                return Value::String(value);
            }
            match trimmed {
                "True" => Value::Bool(true),
                "False" => Value::Bool(false),
                "None" => Value::Null,
                _ => serde_json::from_str(trimmed)
                    .ok()
                    .or_else(|| {
                        trimmed
                            .parse::<f64>()
                            .ok()
                            .and_then(Number::from_f64)
                            .map(Value::Number)
                    })
                    .unwrap_or(Value::String(value)),
            }
        }
        value => value,
    }
}

fn data_operation_combine(input_objects: Vec<Map<String, Value>>) -> Value {
    let mut combined = Map::new();
    for object in input_objects {
        for (key, value) in object {
            let Some(existing) = combined.remove(&key) else {
                combined.insert(key, value);
                continue;
            };
            let merged = match (existing, value) {
                (Value::Array(mut existing), Value::Array(value)) => {
                    existing.extend(value);
                    Value::Array(existing)
                }
                (Value::Array(mut existing), value) => {
                    existing.push(value);
                    Value::Array(existing)
                }
                (existing, Value::Array(value)) => {
                    let mut merged = Vec::with_capacity(value.len() + 1);
                    merged.push(existing);
                    merged.extend(value);
                    Value::Array(merged)
                }
                (existing, value) => Value::Array(vec![existing, value]),
            };
            combined.insert(key, merged);
        }
    }
    Value::Object(combined)
}

fn data_operation_rule_matches(
    runtime: &CanvasRuntime,
    object: &Map<String, Value>,
    rule: &Map<String, Value>,
) -> bool {
    let Some(key) = rule.get("key").and_then(Value::as_str) else {
        return false;
    };
    let Some(value) = object.get(key) else {
        return false;
    };
    let operation = rule
        .get("operator")
        .and_then(Value::as_str)
        .unwrap_or("equals")
        .to_ascii_lowercase();
    let target = rule
        .get("value")
        .map(data_operation_norm)
        .unwrap_or_default();
    let resolved = resolve_template(runtime, &target).unwrap_or_else(|_| target.clone());
    let target = if resolved.is_empty() {
        target.as_str()
    } else {
        resolved.as_str()
    };
    let value = data_operation_norm(value);
    match operation.as_str() {
        "=" | "equals" => value == target,
        "≠" | "!=" => value != target,
        "contains" => value.contains(target),
        "start with" => value.starts_with(target),
        "end with" => value.ends_with(target),
        _ => false,
    }
}

fn data_operation_update_value(runtime: &CanvasRuntime, value: Value) -> Value {
    let Value::String(text) = &value else {
        return value;
    };
    if text.contains('{')
        && let Ok(rendered) = resolve_template(runtime, text)
        && !rendered.is_empty()
    {
        return Value::String(rendered);
    }
    get_variable(runtime, text)
        .ok()
        .filter(|value| !value.is_null())
        .unwrap_or(value)
}

fn data_operation_norm(value: &Value) -> String {
    list_operation_norm(value)
}

fn validate_list_operations_params(node: &CanvasNode) -> Result<()> {
    list_operation_query(node)?;
    list_operation_name(node)?;
    list_operation_sort_fields(node)?;
    if node
        .params
        .get("filter")
        .is_some_and(|value| !value.is_object())
    {
        bail!("ListOperations '{}' filter must be an object", node.id);
    }
    Ok(())
}

fn execute_list_operations(runtime: &mut CanvasRuntime, node: &CanvasNode) -> Result<()> {
    let query = list_operation_query(node)?;
    let source = get_variable(runtime, query)
        .with_context(|| format!("ListOperations '{}' query '{query}'", node.id))?;
    let items = source.as_array().cloned().ok_or_else(|| {
        anyhow!(
            "ListOperations '{}' input should be an array, got {}",
            node.id,
            json_type_name(&source)
        )
    })?;
    let operation = list_operation_name(node)?;
    let n = coerce_list_operation_n(node.params.get("n"));
    let strict = coerce_list_operation_strict(node.params.get("strict"));

    let result = match operation {
        "nth" => {
            let index = if n > 0 {
                i128::from(n) - 1
            } else {
                items.len() as i128 + i128::from(n)
            };
            if n != 0 && index >= 0 && index < items.len() as i128 {
                vec![items[index as usize].clone()]
            } else if strict {
                bail!(
                    "ListOperations '{}' nth requires n to be within the valid range in strict mode, got {n}",
                    node.id
                );
            } else {
                Vec::new()
            }
        }
        "head" | "tail" => {
            let in_range = n >= 1 && i128::from(n) <= items.len() as i128;
            if strict && !in_range {
                bail!(
                    "ListOperations '{}' {operation} requires n to be within the valid range in strict mode, got {n}",
                    node.id
                );
            }
            if n < 1 {
                Vec::new()
            } else {
                let count = usize::try_from(n).unwrap_or(usize::MAX).min(items.len());
                if operation == "head" {
                    items[..count].to_vec()
                } else {
                    items[items.len() - count..].to_vec()
                }
            }
        }
        "filter" => {
            let filter = node.params.get("filter").and_then(Value::as_object);
            let operator = filter
                .and_then(|value| value.get("operator"))
                .and_then(Value::as_str)
                .unwrap_or("=");
            let target = filter
                .and_then(|value| value.get("value"))
                .map(list_operation_norm)
                .unwrap_or_default();
            items
                .into_iter()
                .filter(|item| list_operation_filter_matches(item, operator, &target))
                .collect()
        }
        "sort" => {
            let mut result = items;
            let fields = list_operation_sort_fields(node)?;
            let objects = result.first().is_some_and(Value::is_object);
            if objects && result.iter().any(|value| !value.is_object()) {
                bail!(
                    "ListOperations '{}' cannot sort mixed object and non-object items",
                    node.id
                );
            }
            let descending = node
                .params
                .get("sort_method")
                .and_then(Value::as_str)
                .is_some_and(|method| method == "desc");
            result.sort_by(|left, right| {
                let order = if objects && !fields.is_empty() {
                    compare_list_operation_fields(left, right, &fields)
                } else {
                    compare_list_operation_values(left, right)
                };
                if descending { order.reverse() } else { order }
            });
            result
        }
        "drop_duplicates" => {
            let mut seen = HashSet::new();
            items
                .into_iter()
                .filter(|item| seen.insert(list_operation_dedup_key(item)))
                .collect()
        }
        _ => unreachable!("list_operation_name validates the operation"),
    };

    let first = result.first().cloned().unwrap_or(Value::Null);
    let last = result.last().cloned().unwrap_or(Value::Null);
    runtime.outputs.insert(
        node.id.clone(),
        Map::from_iter([
            ("result".into(), Value::Array(result)),
            ("first".into(), first),
            ("last".into(), last),
        ]),
    );
    Ok(())
}

fn list_operation_query(node: &CanvasNode) -> Result<&str> {
    node.params
        .get("query")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .ok_or_else(|| anyhow!("ListOperations '{}' query cannot be empty", node.id))
}

fn list_operation_name(node: &CanvasNode) -> Result<&str> {
    let operation = node
        .params
        .get("operations")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("ListOperations '{}' operations must be a string", node.id))
        })
        .transpose()?
        .unwrap_or("nth")
        .trim();
    let operation = if operation.eq_ignore_ascii_case("topn") {
        "head"
    } else if operation.is_empty() {
        "nth"
    } else {
        operation
    };
    if !matches!(
        operation,
        "nth" | "head" | "tail" | "filter" | "sort" | "drop_duplicates"
    ) {
        bail!(
            "ListOperations '{}' operations must be one of nth, head, tail, filter, sort, drop_duplicates",
            node.id
        );
    }
    Ok(operation)
}

fn coerce_list_operation_n(value: Option<&Value>) -> i64 {
    match value.unwrap_or(&Value::Null) {
        Value::Bool(value) => i64::from(*value),
        Value::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_f64().map(|value| value as i64))
            .unwrap_or(0),
        Value::String(value) => value.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

fn coerce_list_operation_strict(value: Option<&Value>) -> bool {
    match value.unwrap_or(&Value::Bool(false)) {
        Value::Bool(value) => *value,
        Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        _ => false,
    }
}

fn list_operation_sort_fields(node: &CanvasNode) -> Result<Vec<String>> {
    let Some(value) = node.params.get("sort_by") else {
        return Ok(Vec::new());
    };
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(value) => Ok(value
            .split(',')
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .map(str::to_owned)
            .collect()),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        anyhow!("ListOperations '{}' sort_by must contain strings", node.id)
                    })
            })
            .collect::<Result<Vec<_>>>()
            .map(|fields| {
                fields
                    .into_iter()
                    .filter(|field| !field.is_empty())
                    .collect()
            }),
        _ => bail!(
            "ListOperations '{}' sort_by must be a string or string array",
            node.id
        ),
    }
}

fn list_operation_filter_matches(value: &Value, operator: &str, target: &str) -> bool {
    let value = list_operation_norm(value);
    match operator {
        "=" => value == target,
        "≠" => value != target,
        "contains" => value.contains(target),
        "start with" => value.starts_with(target),
        "end with" => value.ends_with(target),
        _ => false,
    }
}

/// Match Python's user-facing scalar spelling where it is observable in the
/// filter contract; compound values retain deterministic JSON spelling.
fn list_operation_norm(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(value) => value.clone(),
        Value::Number(value) => value.to_string(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn compare_list_operation_fields(left: &Value, right: &Value, fields: &[String]) -> Ordering {
    let left = left.as_object().expect("object sort was checked");
    let right = right.as_object().expect("object sort was checked");
    for field in fields {
        let order = compare_list_operation_values(
            left.get(field).unwrap_or(&Value::Null),
            right.get(field).unwrap_or(&Value::Null),
        );
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

fn compare_list_operation_values(left: &Value, right: &Value) -> Ordering {
    if let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) {
        return left.partial_cmp(&right).unwrap_or(Ordering::Equal);
    }
    match (left, right) {
        (Value::Array(left), Value::Array(right)) => compare_list_operation_slices(left, right),
        (Value::Object(left), Value::Object(right)) => {
            let mut left: Vec<_> = left.iter().collect();
            let mut right: Vec<_> = right.iter().collect();
            left.sort_by_key(|(key, _)| *key);
            right.sort_by_key(|(key, _)| *key);
            for ((left_key, left_value), (right_key, right_value)) in left.iter().zip(&right) {
                let order = left_key.cmp(right_key);
                if order != Ordering::Equal {
                    return order;
                }
                let order = compare_list_operation_values(left_value, right_value);
                if order != Ordering::Equal {
                    return order;
                }
            }
            left.len().cmp(&right.len())
        }
        _ => list_operation_norm(left).cmp(&list_operation_norm(right)),
    }
}

fn compare_list_operation_slices(left: &[Value], right: &[Value]) -> Ordering {
    for (left, right) in left.iter().zip(right) {
        let order = compare_list_operation_values(left, right);
        if order != Ordering::Equal {
            return order;
        }
    }
    left.len().cmp(&right.len())
}

fn list_operation_dedup_key(value: &Value) -> String {
    serde_json::to_string(&canonical_list_operation_value(value)).unwrap_or_default()
}

fn canonical_list_operation_value(value: &Value) -> Value {
    match value {
        Value::Array(values) => {
            Value::Array(values.iter().map(canonical_list_operation_value).collect())
        }
        Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort();
            Value::Object(Map::from_iter(keys.into_iter().map(|key| {
                (key.clone(), canonical_list_operation_value(&values[key]))
            })))
        }
        value => value.clone(),
    }
}

fn coerce_legacy_switch_right(left: &Value, right: Value) -> Result<Value> {
    if !left.is_number() && !left.is_boolean() {
        return Ok(right);
    }
    let number =
        switch_number(&right).ok_or_else(|| anyhow!("'{}' is not numeric", stringify(&right)))?;
    Number::from_f64(number)
        .map(Value::Number)
        .ok_or_else(|| anyhow!("comparison value must be finite"))
}

fn modern_switch_left(runtime: &CanvasRuntime, expression: &str) -> Value {
    if !expression.contains('{') || !is_exact_selector(expression) {
        return Value::String(expression.to_owned());
    }
    get_variable(runtime, expression).unwrap_or_else(|_| Value::String(expression.to_owned()))
}

fn modern_legacy_switch_left(runtime: &CanvasRuntime, expression: &str) -> Value {
    if expression.contains('@') || expression.starts_with("sys.") || expression.starts_with("env.")
    {
        return get_variable(runtime, expression)
            .unwrap_or_else(|_| Value::String(format!("{{{{{expression}}}}}")));
    }
    Value::String(expression.to_owned())
}

fn process_legacy_switch_operator(left: &Value, operator: &str, right: &Value) -> Result<bool> {
    let left_text = stringify(left);
    let right_text = stringify(right);
    Ok(match operator {
        "contains" => left_text
            .to_lowercase()
            .contains(&right_text.to_lowercase()),
        "not contains" => !left_text
            .to_lowercase()
            .contains(&right_text.to_lowercase()),
        "start with" => left_text
            .to_lowercase()
            .starts_with(&right_text.to_lowercase()),
        "end with" => left_text
            .to_lowercase()
            .ends_with(&right_text.to_lowercase()),
        "empty" => !truthy(left),
        "not empty" => truthy(left),
        "=" => python_switch_equal(left, right),
        "≠" => !python_switch_equal(left, right),
        ">" => compare_legacy_switch_values(left, right, |order| order.is_gt())?,
        "<" => compare_legacy_switch_values(left, right, |order| order.is_lt())?,
        "≥" => compare_legacy_switch_values(left, right, |order| order.is_ge())?,
        "≤" => compare_legacy_switch_values(left, right, |order| order.is_le())?,
        _ => bail!("Unsupported Switch operator: {operator}"),
    })
}

fn process_modern_switch_operator(left: &Value, operator: &str, right: &Value) -> Result<bool> {
    let string_pair = || {
        (
            switch_display(left).to_lowercase(),
            switch_display(right).to_lowercase(),
        )
    };
    Ok(match operator {
        "==" => modern_switch_equal(left, right),
        "!=" => !modern_switch_equal(left, right),
        "contains" => {
            let (left, right) = string_pair();
            left.contains(&right)
        }
        "not contains" => {
            let (left, right) = string_pair();
            !left.contains(&right)
        }
        "start with" => {
            let (left, right) = string_pair();
            left.starts_with(&right)
        }
        "end with" => {
            let (left, right) = string_pair();
            left.ends_with(&right)
        }
        "empty" => modern_switch_empty(left),
        "not empty" => !modern_switch_empty(left),
        ">" | "<" | ">=" | "<=" => {
            let left = switch_number(left).ok_or_else(|| {
                anyhow!("Switch operator '{operator}' requires a numeric left operand")
            })?;
            let right = switch_number(right).ok_or_else(|| {
                anyhow!("Switch operator '{operator}' requires a numeric right operand")
            })?;
            match operator {
                ">" => left > right,
                "<" => left < right,
                ">=" => left >= right,
                "<=" => left <= right,
                _ => unreachable!("the comparison operator was matched above"),
            }
        }
        _ => bail!("Unsupported Switch operator: {operator}"),
    })
}

fn python_switch_equal(left: &Value, right: &Value) -> bool {
    match (switch_number(left), switch_number(right)) {
        (Some(left), Some(right)) if left.is_finite() && right.is_finite() => left == right,
        _ => left == right,
    }
}

fn modern_switch_equal(left: &Value, right: &Value) -> bool {
    match (switch_number(left), switch_number(right)) {
        (Some(left), Some(right)) if left.is_finite() && right.is_finite() => left == right,
        _ => switch_display(left).eq_ignore_ascii_case(&switch_display(right)),
    }
}

fn compare_legacy_switch_values(
    left: &Value,
    right: &Value,
    predicate: impl Fn(std::cmp::Ordering) -> bool,
) -> Result<bool> {
    if let (Some(left), Some(right)) = (switch_number(left), switch_number(right)) {
        return Ok(left.partial_cmp(&right).is_some_and(predicate));
    }
    if let (Some(left), Some(right)) = (left.as_str(), right.as_str()) {
        return Ok(predicate(left.cmp(right)));
    }
    bail!(
        "Switch operands '{}' and '{}' cannot be ordered",
        switch_display(left),
        switch_display(right)
    )
}

fn switch_number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.parse().ok(),
        Value::Bool(value) => Some(u8::from(*value).into()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn switch_display(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => {
            if *value {
                "true".to_owned()
            } else {
                "false".to_owned()
            }
        }
        Value::String(value) => value.clone(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn modern_switch_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

fn assign_value(
    runtime: &CanvasRuntime,
    current: Value,
    operator: &str,
    parameter: &Value,
) -> Result<Value> {
    match operator {
        "overwrite" => selector_value(runtime, parameter),
        "set" if current.is_null() || current.is_string() => resolve_parameter(runtime, parameter),
        "set" => Ok(parameter.clone()),
        "clear" => Ok(match current {
            Value::Array(_) => Value::Array(Vec::new()),
            Value::Object(_) => Value::Object(Map::new()),
            Value::Bool(_) => Value::Bool(false),
            Value::Number(number) if number.is_f64() => Number::from_f64(0.0)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            Value::Number(_) => Value::Number(Number::from(0)),
            Value::String(_) => Value::String(String::new()),
            _ => Value::Null,
        }),
        "append" => {
            let parameter = selector_value(runtime, parameter)?;
            let mut values = match current {
                Value::Null => Vec::new(),
                Value::Array(values) => values,
                _ => return Ok(Value::String("ERROR:VARIABLE_NOT_LIST".into())),
            };
            if values
                .first()
                .is_some_and(|first| !same_json_type(first, &parameter))
            {
                return Ok(Value::String(
                    "ERROR:PARAMETER_NOT_LIST_ELEMENT_TYPE".into(),
                ));
            }
            values.push(parameter);
            Ok(Value::Array(values))
        }
        "extend" => {
            let parameter = selector_value(runtime, parameter)?;
            let mut values = match current {
                Value::Null => Vec::new(),
                Value::Array(values) => values,
                _ => return Ok(Value::String("ERROR:VARIABLE_NOT_LIST".into())),
            };
            let Value::Array(extension) = parameter else {
                return Ok(Value::String("ERROR:PARAMETER_NOT_LIST".into()));
            };
            if let (Some(first), Some(extension_first)) = (values.first(), extension.first())
                && !same_json_type(first, extension_first)
            {
                return Ok(Value::String(
                    "ERROR:PARAMETER_NOT_LIST_ELEMENT_TYPE".into(),
                ));
            }
            values.extend(extension);
            Ok(Value::Array(values))
        }
        "remove_first" => {
            let Value::Array(mut values) = current else {
                return Ok(Value::String("ERROR:VARIABLE_NOT_LIST".into()));
            };
            if !values.is_empty() {
                values.remove(0);
            }
            Ok(Value::Array(values))
        }
        "remove_last" => {
            let Value::Array(mut values) = current else {
                return Ok(Value::String("ERROR:VARIABLE_NOT_LIST".into()));
            };
            values.pop();
            Ok(Value::Array(values))
        }
        "+=" | "-=" | "*=" | "/=" => {
            let (Some(left), Some(right)) = (json_number(&current), json_number(parameter)) else {
                return Ok(Value::String(
                    "ERROR:VARIABLE_NOT_NUMBER or PARAMETER_NOT_NUMBER".into(),
                ));
            };
            if operator == "/=" && right == 0.0 {
                return Ok(Value::String("ERROR:DIVIDE_BY_ZERO".into()));
            }
            let value = match operator {
                "+=" => left + right,
                "-=" => left - right,
                "*=" => left * right,
                "/=" => left / right,
                _ => unreachable!(),
            };
            Number::from_f64(value)
                .map(Value::Number)
                .ok_or_else(|| anyhow!("VariableAssigner produced a non-finite number"))
        }
        _ => Ok(Value::Null),
    }
}

fn selector_value(runtime: &CanvasRuntime, parameter: &Value) -> Result<Value> {
    let selector = parameter
        .as_str()
        .ok_or_else(|| anyhow!("VariableAssigner selector parameter must be a string"))?;
    get_variable(runtime, selector)
}

fn json_number(value: &Value) -> Option<f64> {
    if value.is_boolean() {
        None
    } else {
        value.as_f64()
    }
}

fn same_json_type(left: &Value, right: &Value) -> bool {
    matches!(
        (left, right),
        (Value::Null, Value::Null)
            | (Value::Bool(_), Value::Bool(_))
            | (Value::Number(_), Value::Number(_))
            | (Value::String(_), Value::String(_))
            | (Value::Array(_), Value::Array(_))
            | (Value::Object(_), Value::Object(_))
    )
}

fn resolve_parameter(runtime: &CanvasRuntime, value: &Value) -> Result<Value> {
    match value {
        Value::String(text) if is_exact_selector(text) => get_variable(runtime, text),
        Value::String(text) => Ok(Value::String(resolve_template(runtime, text)?)),
        Value::Array(values) => values
            .iter()
            .map(|value| resolve_parameter(runtime, value))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), resolve_parameter(runtime, value)?)))
            .collect::<Result<Map<_, _>>>()
            .map(Value::Object),
        value => Ok(value.clone()),
    }
}

fn get_variable(runtime: &CanvasRuntime, expression: &str) -> Result<Value> {
    let expression = trim_selector(expression);
    if let Some(storage_name) = match expression {
        "item" => Some("__item__"),
        "index" => Some("__index__"),
        "result" => Some("__result__"),
        _ => None,
    } {
        return runtime
            .globals
            .get(expression)
            .or_else(|| runtime.globals.get(storage_name))
            .cloned()
            .ok_or_else(|| anyhow!("Canvas variable '{expression}' does not exist"));
    }
    if let Some(path) = expression.strip_prefix("sys.") {
        return get_namespace_variable(&runtime.sys, &runtime.globals, "sys", path);
    }
    if let Some(path) = expression.strip_prefix("env.") {
        return get_namespace_variable(&runtime.env, &runtime.globals, "env", path);
    }
    let Some((component_id, path)) = expression.split_once('@') else {
        return runtime
            .globals
            .get(expression)
            .cloned()
            .ok_or_else(|| anyhow!("Canvas variable '{expression}' does not exist"));
    };
    let (root, nested) = path.split_once('.').unwrap_or((path, ""));
    let value = runtime
        .outputs
        .get(component_id)
        .and_then(|outputs| outputs.get(root))
        .cloned()
        .ok_or_else(|| anyhow!("Canvas variable '{component_id}@{path}' is not available"))?;
    get_nested(value, nested)
}

fn get_namespace_variable(
    namespace: &Map<String, Value>,
    legacy_globals: &Map<String, Value>,
    prefix: &str,
    path: &str,
) -> Result<Value> {
    let legacy_name = format!("{prefix}.{path}");
    if let Some(value) = legacy_globals.get(&legacy_name) {
        return Ok(value.clone());
    }
    let value = get_nested(Value::Object(namespace.clone()), path)?;
    if value.is_null() {
        bail!("Canvas variable '{legacy_name}' does not exist");
    }
    Ok(value)
}

fn set_variable(runtime: &mut CanvasRuntime, expression: &str, value: Value) -> Result<()> {
    let expression = trim_selector(expression);
    if let Some(path) = expression.strip_prefix("sys.") {
        if runtime.globals.contains_key(expression) {
            runtime.globals.insert(expression.to_owned(), value);
        } else {
            set_namespace_variable(&mut runtime.sys, path, value)?;
        }
        return Ok(());
    }
    if let Some(path) = expression.strip_prefix("env.") {
        if runtime.globals.contains_key(expression) {
            runtime.globals.insert(expression.to_owned(), value);
        } else {
            set_namespace_variable(&mut runtime.env, path, value)?;
        }
        return Ok(());
    }
    let Some((component_id, path)) = expression.split_once('@') else {
        runtime.globals.insert(expression.to_owned(), value);
        return Ok(());
    };
    let (root, nested) = path.split_once('.').unwrap_or((path, ""));
    let outputs = runtime.outputs.entry(component_id.to_owned()).or_default();
    if nested.is_empty() {
        outputs.insert(root.to_owned(), value);
        return Ok(());
    }
    let root_value = outputs
        .entry(root.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    set_nested(root_value, nested, value)
}

fn set_namespace_variable(
    namespace: &mut Map<String, Value>,
    path: &str,
    value: Value,
) -> Result<()> {
    let mut root = Value::Object(std::mem::take(namespace));
    set_nested(&mut root, path, value)?;
    *namespace = root
        .as_object_mut()
        .map(std::mem::take)
        .expect("set_nested preserves the namespace object");
    Ok(())
}

fn get_nested(mut value: Value, path: &str) -> Result<Value> {
    if path.is_empty() {
        return Ok(value);
    }
    for key in path.split('.') {
        if let Value::String(text) = &value
            && let Ok(decoded) = serde_json::from_str(text)
        {
            value = decoded;
        }
        value = match value {
            Value::Object(values) => values.get(key).cloned().unwrap_or(Value::Null),
            Value::Array(values) => key
                .parse::<usize>()
                .ok()
                .and_then(|index| values.get(index).cloned())
                .unwrap_or(Value::Null),
            Value::Null => return Ok(Value::Null),
            _ => return Ok(Value::Null),
        };
    }
    Ok(value)
}

fn set_nested(root: &mut Value, path: &str, value: Value) -> Result<()> {
    let keys: Vec<_> = path.split('.').collect();
    let mut current = root;
    for key in &keys[..keys.len().saturating_sub(1)] {
        if !current.is_object() {
            *current = Value::Object(Map::new());
        }
        let object = current
            .as_object_mut()
            .expect("the current value was normalized to an object");
        current = object
            .entry((*key).to_owned())
            .or_insert_with(|| Value::Object(Map::new()));
    }
    if !current.is_object() {
        *current = Value::Object(Map::new());
    }
    let object = current
        .as_object_mut()
        .expect("the current value was normalized to an object");
    object.insert(keys[keys.len() - 1].to_owned(), value);
    Ok(())
}

fn resolve_template(runtime: &CanvasRuntime, template: &str) -> Result<String> {
    let (rendered, error) = resolve_template_with_partial(runtime, template);
    if let Some(error) = error {
        return Err(error);
    }
    Ok(rendered)
}

fn resolve_template_with_partial(
    runtime: &CanvasRuntime,
    template: &str,
) -> (String, Option<anyhow::Error>) {
    let mut rendered = String::with_capacity(template.len());
    let mut first_error = None;
    let mut last = 0;
    for capture in selector_regex().captures_iter(template) {
        let full = capture.get(0).expect("selector regex has a full match");
        rendered.push_str(&template[last..full.start()]);
        let selector = capture
            .get(1)
            .expect("selector regex has a capture")
            .as_str();
        match get_variable(runtime, selector) {
            Ok(Value::Null) => {
                if first_error.is_none() {
                    first_error = Some(anyhow!("Canvas variable '{selector}' is unresolved"));
                }
            }
            Ok(value) => rendered.push_str(&stringify(&value)),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(anyhow!("Canvas failed to resolve '{selector}': {error}"));
                }
            }
        }
        last = full.end();
    }
    rendered.push_str(&template[last..]);
    (rendered, first_error)
}

fn resolve_template_for_display(runtime: &CanvasRuntime, template: &str) -> String {
    resolve_template_with_partial(runtime, template).0
}

#[cfg(test)]
fn extract_template_refs(template: &str) -> Vec<String> {
    let mut references = Vec::new();
    for capture in selector_regex().captures_iter(template) {
        let selector = capture
            .get(1)
            .expect("selector regex has a capture")
            .as_str();
        if !references
            .iter()
            .any(|existing: &String| existing == selector)
        {
            references.push(selector.to_owned());
        }
    }
    references
}

fn selector_regex() -> &'static Regex {
    static SELECTOR: OnceLock<Regex> = OnceLock::new();
    SELECTOR.get_or_init(|| {
        Regex::new(
            r"\{+\s*([A-Za-z0-9:_-]+@[A-Za-z0-9_.-]+|sys\.[A-Za-z0-9_.]+|env\.[A-Za-z0-9_.]+|item|index|result)\s*\}+",
        )
        .expect("static canvas selector regex is valid")
    })
}

fn user_prefix_regex() -> &'static Regex {
    static USER_PREFIX: OnceLock<Regex> = OnceLock::new();
    USER_PREFIX.get_or_init(|| {
        Regex::new(r"(?i)^user[:：\s]*").expect("static user prefix regex is valid")
    })
}

fn is_exact_selector(text: &str) -> bool {
    selector_regex()
        .find(text)
        .is_some_and(|found| found.start() == 0 && found.end() == text.len())
}

fn trim_selector(expression: &str) -> &str {
    expression.trim().trim_matches('{').trim_matches('}').trim()
}

fn stringify(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(value) => value.clone(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn merge_generation_patch(
    request: GenerationParamsPatch,
    component: GenerationParamsPatch,
) -> GenerationParamsPatch {
    GenerationParamsPatch {
        temperature: component.temperature.or(request.temperature),
        top_p: component.top_p.or(request.top_p),
        frequency_penalty: component.frequency_penalty.or(request.frequency_penalty),
        presence_penalty: component.presence_penalty.or(request.presence_penalty),
        max_tokens: component.max_tokens.or(request.max_tokens),
        reasoning: component.reasoning.or(request.reasoning),
    }
}

fn llm_messages(
    runtime: &CanvasRuntime,
    node: &CanvasNode,
    history: &[ChatMessage],
) -> Result<Vec<ChatMessage>> {
    let system_prompt = node
        .params
        .get("sys_prompt")
        .or_else(|| node.params.get("system_prompt"))
        .map(|prompt| {
            prompt.as_str().ok_or_else(|| {
                anyhow!(
                    "Canvas LLM component '{}' system prompt must be a string",
                    node.id
                )
            })
        })
        .transpose()?
        .map(|prompt| resolve_template(runtime, prompt))
        .transpose()?
        .unwrap_or_default();
    let history_window = node
        .params
        .get("message_history_window_size")
        .and_then(Value::as_u64)
        .unwrap_or(13) as usize;
    let history_count = history_window.saturating_mul(2);
    let recent = if history_count == 0 {
        &[][..]
    } else if history.len() > history_count {
        &history[history.len() - history_count..]
    } else {
        history
    };
    let mut messages = recent.to_vec();
    let history_size = messages.len();
    for (role, content) in llm_prompt_specs(node)? {
        let content = resolve_template(runtime, &content)?;
        let formatted = ChatMessage::new(role, content);
        if messages.len() == history_size
            && messages
                .last()
                .is_some_and(|message| message.role == formatted.role)
        {
            let last = messages
                .last_mut()
                .expect("same-role replacement checked a non-empty history");
            *last = formatted;
        } else {
            messages.push(formatted);
        }
    }
    if messages
        .last()
        .is_none_or(|message| message.role != "user" || message.content.trim().is_empty())
    {
        bail!(
            "Canvas LLM component '{}' user message is empty after prompt preparation",
            node.id
        );
    }
    let mut with_system = Vec::with_capacity(messages.len() + 1);
    with_system.push(ChatMessage::new("system", system_prompt));
    with_system.extend(messages);
    Ok(with_system)
}

fn llm_component_generation_patch(node: &CanvasNode) -> Result<GenerationParamsPatch> {
    Ok(GenerationParamsPatch {
        temperature: llm_component_float(node, "temperature", "temperatureEnabled")?,
        top_p: llm_component_float(node, "top_p", "topPEnabled")?,
        frequency_penalty: llm_component_float(
            node,
            "frequency_penalty",
            "frequencyPenaltyEnabled",
        )?,
        presence_penalty: llm_component_float(node, "presence_penalty", "presencePenaltyEnabled")?,
        max_tokens: llm_component_max_tokens(node)?,
        reasoning: node
            .params
            .get("reasoning")
            .and_then(Value::as_bool),
    })
}

fn llm_component_float(node: &CanvasNode, field: &str, enabled: &str) -> Result<Option<f32>> {
    let Some(value) = node.params.get(field) else {
        validate_llm_enabled(node, enabled)?;
        return Ok(None);
    };
    let value = value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            anyhow!(
                "Canvas LLM component '{}' {field} must be a finite number",
                node.id
            )
        })?;
    if !(0.0..=1.0).contains(&value) {
        bail!(
            "Canvas LLM component '{}' {field} must be in range [0, 1]",
            node.id
        );
    }
    let enabled = validate_llm_enabled(node, enabled)?.unwrap_or(true);
    Ok((enabled && value > 0.0).then_some(value as f32))
}

fn llm_component_max_tokens(node: &CanvasNode) -> Result<Option<u32>> {
    let Some(value) = node.params.get("max_tokens") else {
        validate_llm_enabled(node, "maxTokensEnabled")?;
        return Ok(None);
    };
    let value = value.as_u64().ok_or_else(|| {
        anyhow!(
            "Canvas LLM component '{}' max_tokens must be a non-negative integer",
            node.id
        )
    })?;
    if value > 128_000 {
        bail!(
            "Canvas LLM component '{}' max_tokens must be in range [0, 128000]",
            node.id
        );
    }
    let enabled = validate_llm_enabled(node, "maxTokensEnabled")?.unwrap_or(true);
    Ok((enabled && value > 0).then_some(value as u32))
}

fn validate_llm_enabled(node: &CanvasNode, field: &str) -> Result<Option<bool>> {
    node.params
        .get(field)
        .map(|value| {
            value.as_bool().ok_or_else(|| {
                anyhow!(
                    "Canvas LLM component '{}' {field} must be a boolean",
                    node.id
                )
            })
        })
        .transpose()
}

fn llm_output_schema(node: &CanvasNode) -> Option<Value> {
    let schema = node
        .params
        .get("outputs")
        .and_then(Value::as_object)
        .and_then(|outputs| outputs.get("structured"))
        .or_else(|| node.params.get("output_structure"))?;
    let schema = schema.as_object()?;
    let properties = schema.get("properties").and_then(Value::as_object)?;
    (!properties.is_empty()).then(|| Value::Object(schema.clone()))
}

fn structured_output_prompt(schema: &Value) -> String {
    format!(
        concat!(
            "You’re a helpful AI assistant. You could answer questions and output in JSON format.\n",
            "constraints:\n",
            "    - You must output in JSON format.\n",
            "    - Do not output boolean value, use string type instead.\n",
            "    - Do not output integer or float value, use number type instead.\n",
            "eg:\n",
            "    Here is the JSON schema:\n",
            "    {{\"properties\": {{\"age\": {{\"type\": \"number\",\"description\": \"\"}},",
            "\"name\": {{\"type\": \"string\",\"description\": \"\"}}}},",
            "\"required\": [\"age\",\"name\"],",
            "\"type\": \"Object Array String Number Boolean\",\"value\": \"\"}}\n\n",
            "    Here is the user's question:\n",
            "    My name is John Doe and I am 30 years old.\n\n",
            "    output:\n",
            "    {{\"name\": \"John Doe\", \"age\": 30}}\n",
            "Here is the JSON schema:\n    {}"
        ),
        serde_json::to_string_pretty(schema).unwrap_or_else(|_| "{}".into())
    )
}

fn parse_structured_content(content: &str) -> Option<Value> {
    let mut content = content.trim();
    if let Some((_, suffix)) = content.rsplit_once("</think>") {
        content = suffix.trim();
    }
    if let Some((_, suffix)) = content.rsplit_once("```json") {
        content = suffix.trim();
    }
    if let Some(content_without_fence) = content.strip_suffix("```") {
        content = content_without_fence.trim();
    }
    if let Ok(value) = serde_json::from_str(content) {
        return Some(value);
    }
    let content = content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))?
        .trim()
        .strip_suffix("```")?
        .trim();
    serde_json::from_str(content).ok()
}

fn string_array(value: Option<&Value>, component_id: &str, field: &str) -> Result<Vec<String>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("Canvas component '{component_id}' {field} must be an array"))?;
    let mut result = Vec::new();
    for value in values {
        let value = value.as_str().ok_or_else(|| {
            anyhow!("Canvas component '{component_id}' {field} must contain strings")
        })?;
        if value.trim().is_empty() {
            bail!("Canvas component '{component_id}' {field} contains an empty id");
        }
        if result.iter().any(|existing| existing == value) {
            bail!("Canvas component '{component_id}' {field} contains duplicate '{value}'");
        }
        result.push(value.to_owned());
    }
    Ok(result)
}

fn destination_array(
    value: Option<&Value>,
    component_id: &str,
    field: &str,
) -> Result<Vec<String>> {
    match value {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(vec![value.clone()]),
        Some(Value::Array(_)) => string_array(value, component_id, field),
        _ => bail!("Canvas component '{component_id}' {field} must be a string or array"),
    }
}

fn validate_switch_destinations(
    node: &CanvasNode,
    nodes: &BTreeMap<String, CanvasNode>,
) -> Result<()> {
    let conditions: &[Value] = match node.params.get("conditions") {
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("Switch '{}' conditions must be an array", node.id))?,
        None => &[],
    };
    let python_dialect = !node.params.contains_key("default");
    if python_dialect && conditions.is_empty() {
        bail!("Switch '{}' conditions cannot be empty", node.id);
    }
    for (condition_index, condition) in conditions.iter().enumerate() {
        let condition = condition
            .as_object()
            .ok_or_else(|| anyhow!("Switch '{}' condition must be an object", node.id))?;
        let legacy_condition = !condition.contains_key("clauses");
        let clauses = if legacy_condition {
            condition.get("items")
        } else {
            condition.get("clauses")
        };
        if let Some(clauses) = clauses
            && !clauses.is_array()
        {
            bail!(
                "Switch '{}' condition {} must be an array",
                node.id,
                if legacy_condition { "items" } else { "clauses" }
            );
        }
        if let Some(clauses) = clauses.and_then(Value::as_array) {
            for clause in clauses {
                let clause = clause.as_object().ok_or_else(|| {
                    anyhow!("Switch '{}' condition item must be an object", node.id)
                })?;
                if legacy_condition {
                    if clause.get("cpn_id").is_some_and(|value| !value.is_string()) {
                        bail!("Switch '{}' condition cpn_id must be a string", node.id);
                    }
                } else {
                    if clause.get("left").is_some_and(|value| !value.is_string()) {
                        bail!("Switch '{}' condition left must be a string", node.id);
                    }
                    if clause.get("op").is_some_and(|value| !value.is_string()) {
                        bail!("Switch '{}' condition op must be a string", node.id);
                    }
                }
            }
        }
        let mut destinations = destination_array(condition.get("to"), &node.id, "condition.to")?;
        if destinations.is_empty() && python_dialect {
            bail!("Switch '{}' condition.to cannot be empty", node.id);
        }
        if destinations.is_empty() && !python_dialect {
            destinations.push(format!("matched_{condition_index}"));
        }
        for destination in destinations {
            validate_control_destination(node, nodes, &destination, "Switch")?;
        }
    }
    let end_destinations =
        destination_array(node.params.get("end_cpn_ids"), &node.id, "end_cpn_ids")?;
    if python_dialect && end_destinations.is_empty() {
        bail!("Switch '{}' end_cpn_ids cannot be empty", node.id);
    }
    for destination in &end_destinations {
        validate_control_destination(node, nodes, destination, "Switch")?;
    }
    if let Some(default) = node.params.get("default") {
        let default = default
            .as_str()
            .ok_or_else(|| anyhow!("Switch '{}' default must be a string", node.id))?;
        if !default.is_empty() {
            validate_control_destination(node, nodes, default, "Switch")?;
        }
    }
    if !python_dialect
        && node
            .params
            .get("default")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        && end_destinations.is_empty()
    {
        bail!("Switch '{}' requires default or end_cpn_ids", node.id);
    }
    Ok(())
}

fn validate_categorize_destinations(
    node: &CanvasNode,
    nodes: &BTreeMap<String, CanvasNode>,
) -> Result<()> {
    validate_node_llm_selector(node)?;
    if let Some(value) = node.params.get("message_history_window_size")
        && value.as_u64().is_none()
    {
        bail!(
            "Categorize '{}' message_history_window_size must be a non-negative integer",
            node.id
        );
    }
    let categories = node
        .params
        .get("category_description")
        .and_then(Value::as_object)
        .filter(|categories| !categories.is_empty())
        .ok_or_else(|| anyhow!("Categorize '{}' categories cannot be empty", node.id))?;
    for (name, value) in categories {
        if name.trim().is_empty() {
            bail!("Categorize '{}' category name cannot be empty", node.id);
        }
        let category = value.as_object().ok_or_else(|| {
            anyhow!(
                "Categorize '{}' category '{}' must be an object",
                node.id,
                name
            )
        })?;
        let destinations = destination_array(category.get("to"), &node.id, "category.to")?;
        if destinations.is_empty() {
            bail!(
                "Categorize '{}' category '{}' destination cannot be empty",
                node.id,
                name
            );
        }
        for destination in destinations {
            validate_control_destination(node, nodes, &destination, "Categorize")?;
        }
        if let Some(examples) = category.get("examples") {
            let examples = examples.as_array().ok_or_else(|| {
                anyhow!(
                    "Categorize '{}' category '{}' examples must be an array",
                    node.id,
                    name
                )
            })?;
            if examples.iter().any(|example| !example.is_string()) {
                bail!(
                    "Categorize '{}' category '{}' examples must contain strings",
                    node.id,
                    name
                );
            }
        }
        if let Some(description) = category.get("description")
            && !description.is_string()
        {
            bail!(
                "Categorize '{}' category '{}' description must be a string",
                node.id,
                name
            );
        }
    }
    Ok(())
}

fn validate_control_destination(
    node: &CanvasNode,
    nodes: &BTreeMap<String, CanvasNode>,
    destination: &str,
    component_kind: &str,
) -> Result<()> {
    if !nodes.contains_key(destination) {
        bail!(
            "{component_kind} '{}' references missing destination '{}'",
            node.id,
            destination
        );
    }
    if !node
        .downstream
        .iter()
        .any(|declared| declared == destination)
    {
        bail!(
            "{component_kind} '{}' destination '{}' is not a declared downstream",
            node.id,
            destination
        );
    }
    Ok(())
}

fn validate_retrieval_params(node: &CanvasNode) -> Result<()> {
    probability_param(node, "similarity_threshold", 0.2)?;
    probability_param(node, "keywords_similarity_weight", 0.5)?;
    positive_usize_param(node, "top_n", 8, 1024)?;
    positive_usize_param(node, "top_k", 1024, 4096)?;
    for field in ["dataset_ids", "kb_ids"] {
        if let Some(value) = node.params.get(field) {
            string_array(Some(value), &node.id, field)?;
        }
    }

    let retrieval_from = node
        .params
        .get("retrieval_from")
        .and_then(Value::as_str)
        .unwrap_or("dataset");
    if retrieval_from != "dataset" {
        bail!(
            "Retrieval '{}' retrieval_from='{retrieval_from}' is not supported; only dataset retrieval is implemented",
            node.id
        );
    }
    if node
        .params
        .get("memory_ids")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.is_empty())
    {
        bail!(
            "Retrieval '{}' memory retrieval is not implemented",
            node.id
        );
    }
    if node.params.get("use_kg").and_then(Value::as_bool) == Some(true) {
        bail!(
            "Retrieval '{}' knowledge graph mode is not implemented",
            node.id
        );
    }
    if node.params.get("toc_enhance").and_then(Value::as_bool) == Some(true)
        || node.params.get("toc").and_then(Value::as_bool) == Some(true)
    {
        bail!("Retrieval '{}' TOC enhancement is not implemented", node.id);
    }
    if node
        .params
        .get("cross_languages")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.is_empty())
    {
        bail!(
            "Retrieval '{}' cross-language expansion is not implemented",
            node.id
        );
    }
    if let Some(filter) = node
        .params
        .get("meta_data_filter")
        .filter(|value| !value.is_null())
        && filter
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or("manual")
            != "manual"
    {
        bail!(
            "Retrieval '{}' only supports manual metadata filters",
            node.id
        );
    }
    Ok(())
}

fn validate_tavily_search_params(node: &CanvasNode) -> Result<()> {
    validate_tavily_common_params(node)?;
    validate_optional_tavily_string(node, "query")?;
    validate_tavily_enum(node, "search_depth", "basic", &["basic", "advanced"])?;
    validate_tavily_enum(node, "topic", "general", &["general", "news"])?;
    tavily_positive_usize(node, "max_results", 6)?;
    tavily_positive_usize(node, "days", 14)?;
    for field in [
        "include_answer",
        "include_raw_content",
        "include_images",
        "include_image_descriptions",
    ] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_boolean())
        {
            bail!("TavilySearch '{}' {field} must be a boolean", node.id);
        }
    }
    for field in ["include_domains", "exclude_domains"] {
        if let Some(value) = node.params.get(field) {
            tavily_string_list(value, &node.id, field)?;
        }
    }
    Ok(())
}

fn validate_tavily_extract_params(node: &CanvasNode) -> Result<()> {
    validate_tavily_common_params(node)?;
    validate_tavily_enum(node, "extract_depth", "basic", &["basic", "advanced"])?;
    validate_tavily_enum(node, "format", "markdown", &["markdown", "text"])?;
    if node
        .params
        .get("include_images")
        .is_some_and(|value| !value.is_boolean())
    {
        bail!(
            "TavilyExtract '{}' include_images must be a boolean",
            node.id
        );
    }
    if let Some(urls) = node.params.get("urls") {
        match urls {
            Value::String(_) => {}
            Value::Array(_) => {
                tavily_string_list(urls, &node.id, "urls")?;
            }
            _ => bail!(
                "TavilyExtract '{}' urls must be a string or string array",
                node.id
            ),
        }
    }
    Ok(())
}

fn validate_tavily_common_params(node: &CanvasNode) -> Result<()> {
    for field in ["api_key", "description", "function_name"] {
        validate_optional_tavily_string(node, field)?;
    }
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "Tavily component '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    if let Some(value) = node.params.get("delay_after_error")
        && value
            .as_f64()
            .filter(|delay| delay.is_finite() && *delay >= 0.0)
            .is_none()
    {
        bail!(
            "Tavily component '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(())
}

fn validate_optional_tavily_string(node: &CanvasNode, field: &str) -> Result<()> {
    if node
        .params
        .get(field)
        .is_some_and(|value| !value.is_string())
    {
        bail!("Tavily component '{}' {field} must be a string", node.id);
    }
    Ok(())
}

fn validate_tavily_enum(
    node: &CanvasNode,
    field: &str,
    default: &str,
    allowed: &[&str],
) -> Result<String> {
    let value = node
        .params
        .get(field)
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("Tavily component '{}' {field} must be a string", node.id))
        })
        .transpose()?
        .unwrap_or(default);
    if !allowed.contains(&value) {
        bail!(
            "Tavily component '{}' {field} must be one of {}",
            node.id,
            allowed.join(", ")
        );
    }
    Ok(value.to_owned())
}

fn tavily_positive_usize(node: &CanvasNode, field: &str, default: usize) -> Result<usize> {
    let value = node
        .params
        .get(field)
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                anyhow!(
                    "Tavily component '{}' {field} must be a positive integer",
                    node.id
                )
            })
        })
        .transpose()?
        .unwrap_or(default as u64);
    if value == 0 || value > usize::MAX as u64 {
        bail!(
            "Tavily component '{}' {field} must be a positive integer",
            node.id
        );
    }
    Ok(value as usize)
}

fn tavily_string_list(value: &Value, node_id: &str, field: &str) -> Result<Vec<String>> {
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("Tavily component '{node_id}' {field} must be an array"))?;
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .or_else(|| value.get("value").and_then(Value::as_str))
                .map(str::to_owned)
                .ok_or_else(|| {
                    anyhow!("Tavily component '{node_id}' {field} entries must be strings")
                })
        })
        .collect()
}

fn validate_duckduckgo_params(node: &CanvasNode) -> Result<()> {
    for field in ["query", "channel", "topic", "description", "function_name"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("DuckDuckGo '{}' {field} must be a string", node.id);
        }
    }
    duckduckgo_channel(node)?;
    duckduckgo_top_n(node)?;
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "DuckDuckGo '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    duckduckgo_retry_delay(node)?;
    Ok(())
}

fn duckduckgo_channel(node: &CanvasNode) -> Result<DuckDuckGoChannel> {
    if let Some(topic) = node.params.get("topic") {
        return match topic.as_str() {
            Some("general") => Ok(DuckDuckGoChannel::Text),
            Some("news") => Ok(DuckDuckGoChannel::News),
            _ => bail!("DuckDuckGo '{}' topic must be general or news", node.id),
        };
    }
    match node.params.get("channel").and_then(Value::as_str) {
        None | Some("text" | "general") => Ok(DuckDuckGoChannel::Text),
        Some("news") => Ok(DuckDuckGoChannel::News),
        Some(_) => bail!(
            "DuckDuckGo '{}' channel must be text, general or news",
            node.id
        ),
    }
}

fn duckduckgo_top_n(node: &CanvasNode) -> Result<usize> {
    let value = node
        .params
        .get("top_n")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| anyhow!("DuckDuckGo '{}' top_n must be a positive integer", node.id))
        })
        .transpose()?
        .unwrap_or(10);
    if value == 0 || value > usize::MAX as u64 {
        bail!("DuckDuckGo '{}' top_n must be a positive integer", node.id);
    }
    Ok(value as usize)
}

fn duckduckgo_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn duckduckgo_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "DuckDuckGo '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

// -- Domestic search tools (Bing/Baidu/Bocha/TencentFinance/BaiduScholar) --

fn validate_code_exec_params(node: &CanvasNode) -> Result<()> {
    for field in ["lang", "script"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("CodeExec '{}' {field} must be a string", node.id);
        }
    }
    if node
        .params
        .get("arguments")
        .is_some_and(|value| !value.is_object())
    {
        bail!("CodeExec '{}' arguments must be an object", node.id);
    }
    Ok(())
}

fn validate_exesql_params(node: &CanvasNode) -> Result<()> {
    for field in ["db_type", "database", "username", "host", "password", "sql"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("ExeSQL '{}' {field} must be a string", node.id);
        }
    }
    for field in ["port", "max_records"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| value.as_u64().is_none())
        {
            bail!("ExeSQL '{}' {field} must be a positive integer", node.id);
        }
    }
    Ok(())
}

fn validate_email_params(node: &CanvasNode) -> Result<()> {
    for field in [
        "smtp_server",
        "smtp_port",
        "email",
        "smtp_username",
        "password",
        "sender_name",
        "to_email",
        "cc_email",
        "subject",
    ] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("Email '{}' {field} must be a string", node.id);
        }
    }
    Ok(())
}

fn validate_tushare_params(node: &CanvasNode) -> Result<()> {
    for field in ["token", "src", "start_date", "end_date", "keyword"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("TuShare '{}' {field} must be a string", node.id);
        }
    }
    Ok(())
}

fn validate_akshare_params(node: &CanvasNode) -> Result<()> {
    if let Some(value) = node.params.get("top_n")
        && value.as_u64().is_none()
    {
        bail!("AkShare '{}' top_n must be a positive integer", node.id);
    }
    Ok(())
}

fn validate_wencai_params(node: &CanvasNode) -> Result<()> {
    for field in ["query", "query_type", "cookie"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("WenCai '{}' {field} must be a string", node.id);
        }
    }
    if let Some(value) = node.params.get("top_n")
        && value.as_u64().is_none()
    {
        bail!("WenCai '{}' top_n must be a positive integer", node.id);
    }
    Ok(())
}

fn validate_crawler_params(node: &CanvasNode) -> Result<()> {
    if let Some(value) = node.params.get("extract_type")
        && let Some(extract_type) = value.as_str()
        && !matches!(extract_type, "html" | "markdown" | "content")
    {
        bail!(
            "Crawler '{}' extract_type must be one of html/markdown/content",
            node.id
        );
    }
    for field in ["proxy", "query"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("Crawler '{}' {field} must be a string", node.id);
        }
    }
    Ok(())
}

fn validate_baike_params(node: &CanvasNode) -> Result<()> {
    validate_domestic_search_params(node, "Baike")?;
    Ok(())
}

fn validate_domestic_search_params(node: &CanvasNode, provider_label: &str) -> Result<()> {
    for field in ["query", "channel", "topic", "description", "function_name"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("{provider_label} '{}' {field} must be a string", node.id);
        }
    }
    bing_channel(node)?;
    bing_top_n(node)?;
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "{provider_label} '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    bing_retry_delay(node)?;
    Ok(())
}

fn bing_channel(node: &CanvasNode) -> Result<String> {
    if let Some(topic) = node.params.get("topic") {
        return match topic.as_str() {
            Some("general") => Ok("general".into()),
            Some("news") => Ok("news".into()),
            _ => bail!(
                "domestic search '{}' topic must be general or news",
                node.id
            ),
        };
    }
    match node.params.get("channel").and_then(Value::as_str) {
        None | Some("general" | "text" | "web") => Ok("general".into()),
        Some("news") => Ok("news".into()),
        Some(_) => bail!(
            "domestic search '{}' channel must be general, text, web or news",
            node.id
        ),
    }
}

fn bing_top_n(node: &CanvasNode) -> Result<usize> {
    let value = node
        .params
        .get("top_n")
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                anyhow!(
                    "domestic search '{}' top_n must be a positive integer",
                    node.id
                )
            })
        })
        .transpose()?
        .unwrap_or(10);
    if value == 0 || value > usize::MAX as u64 {
        bail!(
            "domestic search '{}' top_n must be a positive integer",
            node.id
        );
    }
    Ok(value as usize)
}

fn bing_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn bing_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "domestic search '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn validate_wikipedia_params(node: &CanvasNode) -> Result<()> {
    for field in ["query", "language", "description", "function_name"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("Wikipedia '{}' {field} must be a string", node.id);
        }
    }
    wikipedia_language(node)?;
    wikipedia_top_n(node)?;
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "Wikipedia '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    wikipedia_retry_delay(node)?;
    Ok(())
}

fn wikipedia_language(node: &CanvasNode) -> Result<String> {
    let language = node
        .params
        .get("language")
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| anyhow!("Wikipedia '{}' language must be a string", node.id))
        })
        .transpose()?
        .unwrap_or("en");
    if !WIKIPEDIA_LANGUAGES.contains(&language) {
        bail!(
            "Wikipedia '{}' language must be one of {}",
            node.id,
            WIKIPEDIA_LANGUAGES.join(", ")
        );
    }
    Ok(language.to_owned())
}

fn wikipedia_top_n(node: &CanvasNode) -> Result<usize> {
    let value = node
        .params
        .get("top_n")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| anyhow!("Wikipedia '{}' top_n must be a positive integer", node.id))
        })
        .transpose()?
        .unwrap_or(10);
    if value == 0 || value > usize::MAX as u64 {
        bail!("Wikipedia '{}' top_n must be a positive integer", node.id);
    }
    Ok(value as usize)
}

fn wikipedia_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn wikipedia_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "Wikipedia '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

const GOOGLE_COUNTRIES: &str = "af al dz as ad ao ai aq ag ar am aw au at az bs bh bd bb by be bz bj bm bt bo ba bw bv br io bn bg bf bi kh cm ca cv ky cf td cl cn cx cc co km cg cd ck cr ci hr cu cy cz dk dj dm do ec eg sv gq er ee et fk fo fj fi fr gf pf tf ga gm ge de gh gi gr gl gd gp gu gt gn gw gy ht hm va hn hk hu is in id ir iq ie il it jm jp jo kz ke ki kp kr kw kg la lv lb ls lr ly li lt lu mo mk mg mw my mv ml mt mh mq mr mu yt mx fm md mc mn ms ma mz mm na nr np nl an nc nz ni ne ng nu nf mp no om pk pw ps pa pg py pe ph pn pl pt pr qa re ro ru rw sh kn lc pm vc ws sm st sa sn rs sc sl sg sk si sb so za gs es lk sd sr sj sz se ch sy tw tj tz th tl tg tk to tt tn tr tm tc tv ug ua ae uk gb us um uy uz vu ve vn vg vi wf eh ye zm zw";
const GOOGLE_LANGUAGES: &str = "af ak sq ws am ar hy az eu be bem bn bh xx-bork bs br bg bt km ca chr ny zh-cn zh-tw co hr cs da nl xx-elmer en eo et ee fo tl fi fr fy gaa gl ka de el kl gn gu xx-hacker ht ha haw iw hi hu is ig id ia ga it ja jw kn kk rw rn xx-klingon kg ko kri ku ckb ky lo la lv ln lt loz lg ach mk mg ms ml mt mv mi mr mfe mo mn sr-me my ne pcm nso no nn oc or om ps fa xx-pirate pl pt pt-br pt-pt pa qu ro rm nyn ru gd sr sh st tn crs sn sd si sk sl so es es-419 su sw sv tg ta tt te th ti to lua tum tr tk tw ug uk ur uz vu vi cy wo xh yi yo zu";

fn validate_google_params(node: &CanvasNode) -> Result<()> {
    for field in [
        "q",
        "api_key",
        "country",
        "language",
        "description",
        "function_name",
    ] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("Google '{}' {field} must be a string", node.id);
        }
    }
    google_api_key(node)?;
    google_country(node)?;
    google_language(node)?;
    for field in ["start", "num"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| value.as_i64().is_none())
        {
            bail!("Google '{}' {field} must be an integer", node.id);
        }
    }
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "Google '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    google_retry_delay(node)?;
    Ok(())
}

fn google_api_key(node: &CanvasNode) -> Result<String> {
    let api_key = node
        .params
        .get("api_key")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if api_key.is_empty() {
        bail!("SerpApi API key does not support empty value.");
    }
    Ok(api_key.to_owned())
}

fn google_country(node: &CanvasNode) -> Result<String> {
    let country = node
        .params
        .get("country")
        .and_then(Value::as_str)
        .unwrap_or("cn");
    if !GOOGLE_COUNTRIES
        .split_ascii_whitespace()
        .any(|allowed| allowed == country)
    {
        bail!("Google '{}' country is not supported: {country}", node.id);
    }
    Ok(country.to_owned())
}

fn google_language(node: &CanvasNode) -> Result<String> {
    let language = node
        .params
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("en");
    if !GOOGLE_LANGUAGES
        .split_ascii_whitespace()
        .any(|allowed| allowed == language)
    {
        bail!("Google '{}' language is not supported: {language}", node.id);
    }
    Ok(language.to_owned())
}

fn google_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn google_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "Google '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn validate_google_scholar_params(node: &CanvasNode) -> Result<()> {
    for field in ["query", "sort_by", "description", "function_name"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("GoogleScholar '{}' {field} must be a string", node.id);
        }
    }
    google_scholar_sort_by(node)?;
    google_scholar_top_n(node)?;
    google_scholar_year(node, "year_low")?;
    google_scholar_year(node, "year_high")?;
    google_scholar_patents(node)?;
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "GoogleScholar '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    google_scholar_retry_delay(node)?;
    Ok(())
}

fn google_scholar_sort_by(node: &CanvasNode) -> Result<GoogleScholarSortBy> {
    match node.params.get("sort_by").and_then(Value::as_str) {
        None | Some("relevance") => Ok(GoogleScholarSortBy::Relevance),
        Some("date") => Ok(GoogleScholarSortBy::Date),
        Some(_) => bail!(
            "GoogleScholar '{}' sort_by must be date or relevance",
            node.id
        ),
    }
}

fn google_scholar_top_n(node: &CanvasNode) -> Result<usize> {
    let value = node
        .params
        .get("top_n")
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                anyhow!(
                    "GoogleScholar '{}' top_n must be a positive integer",
                    node.id
                )
            })
        })
        .transpose()?
        .unwrap_or(12);
    if value == 0 || value > usize::MAX as u64 {
        bail!(
            "GoogleScholar '{}' top_n must be a positive integer",
            node.id
        );
    }
    Ok(value as usize)
}

fn google_scholar_year(node: &CanvasNode, field: &str) -> Result<Option<i32>> {
    let Some(value) = node.params.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let value = value.as_i64().ok_or_else(|| {
        anyhow!(
            "GoogleScholar '{}' {field} must be an integer or null",
            node.id
        )
    })?;
    i32::try_from(value).map(Some).map_err(|_| {
        anyhow!(
            "GoogleScholar '{}' {field} must fit a signed 32-bit integer",
            node.id
        )
    })
}

fn google_scholar_patents(node: &CanvasNode) -> Result<bool> {
    node.params
        .get("patents")
        .map(|value| {
            value
                .as_bool()
                .ok_or_else(|| anyhow!("GoogleScholar '{}' patents must be a boolean", node.id))
        })
        .transpose()
        .map(|value| value.unwrap_or(true))
}

fn google_scholar_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn google_scholar_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "GoogleScholar '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn validate_github_params(node: &CanvasNode) -> Result<()> {
    for field in ["query", "description", "function_name"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("GitHub '{}' {field} must be a string", node.id);
        }
    }
    github_top_n(node)?;
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "GitHub '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    github_retry_delay(node)?;
    Ok(())
}

fn github_top_n(node: &CanvasNode) -> Result<usize> {
    let value = node
        .params
        .get("top_n")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| anyhow!("GitHub '{}' top_n must be a positive integer", node.id))
        })
        .transpose()?
        // GitHubParam defaults to 10. The web client explicitly seeds 5.
        .unwrap_or(10);
    if value == 0 || value > usize::MAX as u64 {
        bail!("GitHub '{}' top_n must be a positive integer", node.id);
    }
    Ok(value as usize)
}

fn github_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn github_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "GitHub '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn validate_yahoo_finance_params(node: &CanvasNode) -> Result<()> {
    for field in ["stock_code", "description", "function_name"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("YahooFinance '{}' {field} must be a string", node.id);
        }
    }
    for field in [
        "info",
        "history",
        "count",
        "financials",
        "income_stmt",
        "balance_sheet",
        "cash_flow_statement",
        "news",
    ] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_boolean())
        {
            bail!("YahooFinance '{}' {field} must be a boolean", node.id);
        }
    }
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "YahooFinance '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    yahoo_finance_retry_delay(node)?;
    Ok(())
}

fn yahoo_finance_flag(node: &CanvasNode, field: &str, default: bool) -> bool {
    node.params
        .get(field)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn yahoo_finance_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn yahoo_finance_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "YahooFinance '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn validate_arxiv_params(node: &CanvasNode) -> Result<()> {
    for field in ["query", "sort_by", "description", "function_name"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("ArXiv '{}' {field} must be a string", node.id);
        }
    }
    arxiv_sort_by(node)?;
    arxiv_top_n(node)?;
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "ArXiv '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    arxiv_retry_delay(node)?;
    Ok(())
}

fn arxiv_sort_by(node: &CanvasNode) -> Result<ArxivSortBy> {
    match node.params.get("sort_by").and_then(Value::as_str) {
        None | Some("submittedDate") => Ok(ArxivSortBy::SubmittedDate),
        Some("lastUpdatedDate") => Ok(ArxivSortBy::LastUpdatedDate),
        Some("relevance") => Ok(ArxivSortBy::Relevance),
        Some(_) => bail!(
            "ArXiv '{}' sort_by must be submittedDate, lastUpdatedDate or relevance",
            node.id
        ),
    }
}

fn arxiv_top_n(node: &CanvasNode) -> Result<usize> {
    let value = node
        .params
        .get("top_n")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| anyhow!("ArXiv '{}' top_n must be a positive integer", node.id))
        })
        .transpose()?
        .unwrap_or(12);
    if value == 0 || value > usize::MAX as u64 {
        bail!("ArXiv '{}' top_n must be a positive integer", node.id);
    }
    Ok(value as usize)
}

fn arxiv_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn arxiv_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "ArXiv '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn validate_pubmed_params(node: &CanvasNode) -> Result<()> {
    for field in ["query", "email", "description", "function_name"] {
        if node
            .params
            .get(field)
            .is_some_and(|value| !value.is_string())
        {
            bail!("PubMed '{}' {field} must be a string", node.id);
        }
    }
    pubmed_top_n(node)?;
    if let Some(value) = node.params.get("max_retries")
        && value.as_u64().is_none()
    {
        bail!(
            "PubMed '{}' max_retries must be a non-negative integer",
            node.id
        );
    }
    pubmed_retry_delay(node)?;
    Ok(())
}

fn pubmed_email(node: &CanvasNode) -> Result<String> {
    node.params
        .get("email")
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("PubMed '{}' email must be a string", node.id))
        })
        .transpose()
        .map(|email| email.unwrap_or_else(|| "A.N.Other@example.com".into()))
}

fn pubmed_top_n(node: &CanvasNode) -> Result<usize> {
    let value = node
        .params
        .get("top_n")
        .map(|value| {
            value
                .as_u64()
                .ok_or_else(|| anyhow!("PubMed '{}' top_n must be a positive integer", node.id))
        })
        .transpose()?
        .unwrap_or(12);
    if value == 0 || value > usize::MAX as u64 {
        bail!("PubMed '{}' top_n must be a positive integer", node.id);
    }
    Ok(value as usize)
}

fn pubmed_attempts(node: &CanvasNode) -> usize {
    node.params
        .get("max_retries")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(1)
        .min(usize::MAX as u64) as usize
}

fn pubmed_retry_delay(node: &CanvasNode) -> Result<Duration> {
    let seconds = node
        .params
        .get("delay_after_error")
        .and_then(Value::as_f64)
        .unwrap_or(2.0);
    if !seconds.is_finite() || seconds < 0.0 {
        bail!(
            "PubMed '{}' delay_after_error must be a non-negative finite number",
            node.id
        );
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn validate_identifier(value: &str, description: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("Canvas {description} cannot be empty");
    }
    if value.contains('@') || value.contains('{') || value.contains('}') {
        bail!("Canvas {description} '{value}' contains a reserved character");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct RecordingLlmResolver {
        api_base: String,
        selectors: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl WorkflowLlmResolver for RecordingLlmResolver {
        fn resolve_chat_model(&self, selector: &str) -> Result<LlmClient> {
            self.selectors.lock().unwrap().push(selector.to_owned());
            let model = selector
                .split_once('@')
                .map_or(selector, |(model, _)| model)
                .to_owned();
            Ok(LlmClient::new(crate::llm::LlmConfig {
                api_base: self.api_base.clone(),
                api_key: "tenant-secret".into(),
                model,
                ..crate::llm::LlmConfig::default()
            }))
        }
    }

    fn extractor_dsl() -> Value {
        json!({
            "components": {
                "begin": {
                    "obj": {
                        "component_name": "Begin",
                        "params": {"mode": "Manual"},
                        "input_form": {"query": {"type": "string"}}
                    }
                },
                "answer": {"obj": {"component_name": "Answer"}}
            }
        })
    }

    #[test]
    fn component_extractors_borrow_happy_path_values() {
        let dsl = extractor_dsl();
        assert_eq!(
            extract_component_input_form(&dsl, "begin").unwrap()["query"]["type"],
            "string"
        );
        assert_eq!(
            extract_component_params(&dsl, "begin").unwrap().unwrap()["mode"],
            "Manual"
        );
        assert_eq!(extract_component_params(&dsl, "answer").unwrap(), None);
        assert_eq!(extract_component_name(&dsl, "begin").unwrap(), "Begin");
        assert_eq!(find_begin_component_id(&dsl).unwrap(), "begin");
    }

    #[test]
    fn component_extractors_preserve_error_categories() {
        let dsl = extractor_dsl();
        assert!(matches!(
            extract_component_input_form(&dsl, "missing"),
            Err(AgentDslError::ComponentNotFound(id)) if id == "missing"
        ));
        assert!(matches!(
            extract_component_input_form(&dsl, "answer"),
            Err(AgentDslError::MissingInputForm(id)) if id == "answer"
        ));
        assert!(matches!(
            extract_component_input_form(&Value::Null, "begin"),
            Err(AgentDslError::Malformed(_))
        ));
        assert!(matches!(
            extract_component_input_form(&json!({"components": {"bare": {}}}), "bare"),
            Err(AgentDslError::Malformed(_))
        ));
        assert!(matches!(
            extract_component_input_form(
                &json!({"components": {"bad": {
                    "obj": {"component_name": "Begin", "input_form": []}
                }}}),
                "bad"
            ),
            Err(AgentDslError::Malformed(_))
        ));
        assert!(matches!(
            extract_component_params(
                &json!({"components": {"bad": {
                    "obj": {"component_name": "Begin", "params": "wrong"}
                }}}),
                "bad"
            ),
            Err(AgentDslError::Malformed(_))
        ));
        assert!(matches!(
            extract_component_name(&json!({"components": {"bad": {"obj": {}}}}), "bad"),
            Err(AgentDslError::Malformed(_))
        ));
    }

    #[test]
    fn component_input_form_prefers_static_and_synthesizes_dynamic_forms() {
        let static_dsl = extractor_dsl();
        assert!(matches!(
            agent_component_input_form(&static_dsl, "begin").unwrap(),
            Cow::Borrowed(_)
        ));

        for (name, expected) in [
            (
                "Browser",
                json!({
                    "prompts": {"type": "text", "name": "Prompts"},
                    "upload_sources": {"type": "line", "name": "Upload sources"}
                }),
            ),
            ("BGPT", json!({"query": {"name": "Query", "type": "line"}})),
            ("ExeSQL", json!({"sql": {"name": "SQL", "type": "line"}})),
            (
                "YahooFinance",
                json!({
                    "stock_code": {
                        "type": "line",
                        "name": "Stock code/Company name"
                    }
                }),
            ),
        ] {
            let dsl = json!({"components": {
                "dynamic": {"obj": {"component_name": name, "params": {}}}
            }});
            let form = agent_component_input_form(&dsl, "dynamic").unwrap();
            assert!(matches!(form, Cow::Owned(_)), "{name}");
            assert_eq!(form.as_ref(), expected.as_object().unwrap(), "{name}");
        }
    }

    #[test]
    fn agent_dynamic_input_form_uses_prompt_refs_in_first_seen_order() {
        let dsl = json!({"components": {
            "agent": {"obj": {"component_name": "Agent", "params": {
                "sys_prompt": "Use {sys.query} and {{env.region}}.",
                "prompts": [
                    {"role": "system", "content": "Again {{sys.query}}"},
                    {"role": "user", "content": "Input {tool:0@result} / {item}"}
                ],
                "user_prompt": "ignored because prompts supplied"
            }}},
            "message": {"obj": {"component_name": "Message", "params": {}}}
        }});
        let form = agent_component_input_form(&dsl, "agent").unwrap();
        assert_eq!(
            form.keys().map(String::as_str).collect::<Vec<_>>(),
            ["sys.query", "env.region", "tool:0@result", "item"]
        );
        assert!(
            form.values()
                .all(|field| field["type"] == "line" && field["optional"] == false)
        );
        for component_name in ["Generate", "LLM"] {
            let dsl = json!({"components": {
                "dynamic": {"obj": {"component_name": component_name, "params": {
                    "sys_prompt": "Region {{env.region}}",
                    "prompts": "Question {sys.query}"
                }}}
            }});
            let form = agent_component_input_form(&dsl, "dynamic").unwrap();
            assert_eq!(
                form.keys().map(String::as_str).collect::<Vec<_>>(),
                ["env.region", "sys.query"],
                "{component_name}"
            );
        }
        assert!(matches!(
            agent_component_input_form(&dsl, "message"),
            Err(AgentDslError::MissingInputForm(id)) if id == "message"
        ));
    }

    #[tokio::test]
    async fn component_debug_is_isolated_passthrough_and_protects_tenant_state() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": []
            },
            "message": {
                "obj": {
                    "component_name": "Message",
                    "params": {"content": ["tenant={{sys.tenant_id}}"]}
                },
                "downstream": []
            },
            "browser": {
                "obj": {"component_name": "Browser", "params": {}},
                "downstream": []
            }
        }});
        let begin_inputs = Map::from_iter([
            ("query".into(), json!("hello")),
            ("user_id".into(), json!("user-1")),
        ]);
        assert_eq!(
            debug_agent_component(
                &dsl,
                "begin",
                &begin_inputs,
                AgentComponentDebugContext {
                    authenticated_user_id: "tenant-owner",
                    llm: None,
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                }
            )
            .await
            .unwrap(),
            begin_inputs
        );

        let attack = Map::from_iter([("sys.tenant_id".into(), json!("attacker"))]);
        assert_eq!(
            debug_agent_component(
                &dsl,
                "message",
                &attack,
                AgentComponentDebugContext {
                    authenticated_user_id: "tenant-owner",
                    llm: None,
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap()["content"],
            "tenant=tenant-owner"
        );
        assert!(matches!(
            debug_agent_component(
                &dsl,
                "browser",
                &Map::new(),
                AgentComponentDebugContext {
                    authenticated_user_id: "owner",
                    llm: None,
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await,
            Err(AgentComponentDebugError::UnsupportedComponent(name)) if name == "Browser"
        ));
    }

    #[test]
    fn message_params_are_validated_when_canvas_is_compiled() {
        for params in [
            json!({}),
            json!({"content": []}),
            json!({"content": [1]}),
            json!({"content": ["ok"], "stream": "yes"}),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["message"], "upstream": []},
                "message": {"obj": {"component_name": "Message", "params": params}, "downstream": [], "upstream": ["begin"]}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_err(), "{dsl}");
        }

        for params in [
            json!({"content": [""]}),
            json!({"content": "legacy"}),
            json!({"text": "go-v2"}),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["message"], "upstream": []},
                "message": {"obj": {"component_name": "Message", "params": params}, "downstream": [], "upstream": ["begin"]}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_ok(), "{dsl}");
        }
    }

    #[test]
    fn message_renders_sandboxed_jinja_and_fails_soft() {
        let mut runtime = CanvasRuntime::default();
        runtime.env.insert("enabled".into(), json!(true));
        runtime.env.insert("name".into(), json!("Ada"));
        runtime.env.insert("payload".into(), json!({"count": 2}));
        let mut node = CanvasNode {
            id: "message".into(),
            component_name: "Message".into(),
            params: Map::from_iter([(
                "content".into(),
                json!(["{% if env.enabled %}Hello {{env.name}} {env.payload}{% endif %}"]),
            )]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        execute_message(&mut runtime, &node).unwrap();
        assert_eq!(
            runtime.outputs["message"]["content"],
            r#"Hello Ada {"count":2}"#
        );
        assert_eq!(runtime.outputs["message"]["downloads"], json!([]));

        node.params
            .insert("content".into(), json!(["{% if %}fallback={env.name}"]));
        execute_message(&mut runtime, &node).unwrap();
        assert_eq!(
            runtime.outputs["message"]["content"],
            "{% if %}fallback=Ada"
        );
    }

    #[test]
    fn message_extracts_and_normalizes_download_descriptors_once_per_selector() {
        let mut runtime = CanvasRuntime::default();
        runtime.env.insert(
            "file".into(),
            json!({
                "doc_id": "doc-1",
                "filename": "report.csv",
                "mime_type": "text/csv",
                "url": "/downloads/doc-1",
                "include_download_info_in_content": false
            }),
        );
        let node = CanvasNode {
            id: "message".into(),
            component_name: "Message".into(),
            params: Map::from_iter([(
                "content".into(),
                json!(["before={env.file}; repeated={env.file}"]),
            )]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        execute_message(&mut runtime, &node).unwrap();
        assert_eq!(runtime.outputs["message"]["content"], "before=; repeated=");
        assert_eq!(
            runtime.outputs["message"]["downloads"],
            json!([{
                "doc_id": "doc-1",
                "filename": "report.csv",
                "mime_type": "text/csv",
                "url": "/downloads/doc-1"
            }])
        );

        runtime.env.insert(
            "file".into(),
            Value::String(
                json!({
                    "doc_id": "doc-2",
                    "filename": "visible.txt",
                    "mime_type": "text/plain",
                    "include_download_info_in_content": true
                })
                .to_string(),
            ),
        );
        execute_message(&mut runtime, &node).unwrap();
        assert_eq!(
            runtime.outputs["message"]["content"],
            concat!(
                "before={\"doc_id\":\"doc-2\",\"filename\":\"visible.txt\",",
                "\"mime_type\":\"text/plain\"}; repeated=",
                "{\"doc_id\":\"doc-2\",\"filename\":\"visible.txt\",",
                "\"mime_type\":\"text/plain\"}"
            )
        );
        assert_eq!(
            runtime.outputs["message"]["downloads"],
            json!([{
                "doc_id": "doc-2",
                "filename": "visible.txt",
                "mime_type": "text/plain"
            }])
        );
    }

    #[test]
    fn begin_metadata_uses_component_name_and_fails_soft_per_field() {
        let dsl = json!({"components": {
            "sally:0": {
                "obj": {
                    "component_name": "Begin",
                    "prologue": "hello",
                    "mode": "Agent"
                }
            },
            "malformed": [],
            "jack:0": {"obj": {"component_name": "LLM"}}
        }});
        assert_eq!(find_begin_component_id(&dsl).unwrap(), "sally:0");
        assert_eq!(extract_prologue(&dsl).unwrap(), "hello");
        assert_eq!(extract_mode(&dsl).unwrap(), "Agent");

        let absent_fields = json!({"components": {"custom": {"obj": {"component_name": "Begin"}}}});
        assert_eq!(extract_prologue(&absent_fields).unwrap(), "");
        assert_eq!(extract_mode(&absent_fields).unwrap(), "");
        assert!(matches!(
            find_begin_component_id(&json!({"components": {
                "only": {"obj": {"component_name": "LLM"}}
            }})),
            Err(AgentDslError::ComponentNotFound(name)) if name == "Begin component"
        ));
        assert!(matches!(
            find_begin_component_id(&Value::Null),
            Err(AgentDslError::Malformed(_))
        ));
    }

    #[test]
    fn canvas_normalizer_derives_a_sorted_graph_and_flat_component_view() {
        let original = json!({"components": {
            "z": {
                "obj": {"component_name": "Message", "params": {"content": ["done"]}},
                "downstream": []
            },
            "a": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["z"],
                "upstream": []
            },
            "bad": "skip"
        }});
        let normalized = normalize_agent_dsl_for_canvas(&original);
        let nodes = normalized["graph"]["nodes"].as_array().unwrap();
        assert_eq!(
            nodes
                .iter()
                .map(|node| node["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
        assert_eq!(nodes[0]["position"], json!({"x": 50.0, "y": 200.0}));
        assert_eq!(nodes[1]["position"], json!({"x": 400.0, "y": 200.0}));
        assert_eq!(nodes[0]["type"], "beginNode");
        assert_eq!(nodes[0]["data"]["form"], json!({}));
        assert_eq!(
            normalized["graph"]["edges"][0],
            json!({
                "id": "xy-edge__a-z",
                "source": "a",
                "target": "z",
                "sourceHandle": "start",
                "targetHandle": "end"
            })
        );
        assert_eq!(normalized["components"]["a"]["name"], "Begin");
        assert!(normalized["components"]["a"].get("obj").is_none());
        assert_eq!(
            original["components"]["a"]["obj"]["component_name"],
            "Begin"
        );
        assert!(original.get("graph").is_none());
    }

    #[test]
    fn canvas_normalizer_repairs_handles_and_parallel_leaks_without_aliasing_input() {
        let original = json!({
            "graph": {
                "nodes": [{
                    "id": "Iteration:abc",
                    "type": "parallelNode",
                    "data": {"label": "Parallel", "name": "Parallel"}
                }],
                "edges": [
                    {"sourceHandle": "end", "targetHandle": "start"},
                    {"sourceHandle": "tool-1", "targetHandle": "tool-1"}
                ]
            },
            "components": {
                "Iteration:abc": {
                    "name": "Parallel",
                    "obj": {"component_name": "Parallel", "params": {}}
                }
            }
        });
        let normalized = normalize_agent_dsl_for_canvas(&original);
        assert_eq!(normalized["graph"]["edges"][0]["sourceHandle"], "start");
        assert_eq!(normalized["graph"]["edges"][0]["targetHandle"], "end");
        assert_eq!(normalized["graph"]["edges"][1]["sourceHandle"], "tool-1");
        assert_eq!(
            normalized["components"]["Iteration:abc"]["obj"]["component_name"],
            "Iteration"
        );
        assert_eq!(
            normalized["components"]["Iteration:abc"]["name"],
            "Iteration"
        );
        assert_eq!(normalized["graph"]["nodes"][0]["type"], "iterationNode");
        assert_eq!(
            normalized["graph"]["nodes"][0]["data"],
            json!({"label": "Iteration", "name": "Iteration"})
        );
        assert_eq!(original["graph"]["edges"][0]["sourceHandle"], "end");
        assert_eq!(
            original["components"]["Iteration:abc"]["obj"]["component_name"],
            "Parallel"
        );
    }

    #[test]
    fn chunker_dsl_migration_rewrites_every_fixed_structural_surface() {
        let original = json!({
            "components": {
                "Splitter:short": {
                    "obj": {
                        "component_name": "Splitter",
                        "params": {
                            "exact": "HierarchicalMerger:longer",
                            "template": "before {{{ Splitter:short@chunks }}} after",
                            "nested": [{"value": "{{PDFGenerator:pdf@file.name}}"}]
                        }
                    },
                    "downstream": ["HierarchicalMerger:longer"],
                    "upstream": ["Begin:0"],
                    "parent_id": "PDFGenerator:pdf"
                },
                "HierarchicalMerger:longer": {
                    "obj": {"component_name": "HierarchicalMerger", "params": {}},
                    "downstream": ["PDFGenerator:pdf"],
                    "upstream": ["Splitter:short"]
                },
                "PDFGenerator:pdf": {
                    "obj": {"component_name": "PDFGenerator", "params": {}},
                    "downstream": [],
                    "upstream": ["HierarchicalMerger:longer"]
                },
                "Custom:1": {
                    "obj": {"component_name": "Custom Splitter", "params": {}},
                    "downstream": []
                },
                "Nameless:1": {"obj": {}, "downstream": []}
            },
            "path": ["Splitter:short", "HierarchicalMerger:longer", "PDFGenerator:pdf"],
            "graph": {
                "nodes": [
                    {
                        "id": "Splitter:short",
                        "type": "splitterNode",
                        "data": {
                            "label": "Splitter",
                            "name": "Splitter",
                            "form": {"input": "{{ Splitter:short@chunks }}"}
                        }
                    },
                    {
                        "id": "HierarchicalMerger:longer",
                        "parentId": "PDFGenerator:pdf",
                        "type": "splitterNode",
                        "data": {
                            "label": "HierarchicalMerger",
                            "name": "custom-name",
                            "form": {}
                        }
                    },
                    {
                        "id": "PDFGenerator:pdf",
                        "type": "ragNode",
                        "data": {"label": "PDFGenerator", "name": "PDFGenerator"}
                    }
                ],
                "edges": [{
                    "id": "xy-edge__Splitter:short-HierarchicalMerger:longer-PDFGenerator:pdf",
                    "source": "Splitter:short",
                    "target": "HierarchicalMerger:longer"
                }]
            },
            "history": [{"content": "{{Splitter:short@chunks}}"}],
            "messages": ["PDFGenerator:pdf"],
            "reference": {"value": "{{ HierarchicalMerger:longer@result }}"},
            "unrelated": "Splitter:short"
        });

        let normalized = normalize_chunker_dsl(&original);
        assert!(normalized["components"].get("Splitter:short").is_none());
        assert!(
            normalized["components"]
                .get("HierarchicalMerger:longer")
                .is_none()
        );
        assert!(normalized["components"].get("PDFGenerator:pdf").is_none());
        assert_eq!(
            normalized["components"]["TokenChunker:short"]["obj"]["component_name"],
            "TokenChunker"
        );
        assert_eq!(
            normalized["components"]["TokenChunker:short"]["obj"]["params"],
            json!({
                "exact": "TitleChunker:longer",
                "template": "before {{{ TokenChunker:short@chunks }}} after",
                "nested": [{"value": "{{DocGenerator:pdf@file.name}}"}]
            })
        );
        assert_eq!(
            normalized["components"]["TokenChunker:short"]["downstream"],
            json!(["TitleChunker:longer"])
        );
        assert_eq!(
            normalized["components"]["TokenChunker:short"]["upstream"],
            json!(["Begin:0"])
        );
        assert_eq!(
            normalized["components"]["TokenChunker:short"]["parent_id"],
            "DocGenerator:pdf"
        );
        assert_eq!(
            normalized["components"]["TitleChunker:longer"]["obj"]["component_name"],
            "TitleChunker"
        );
        assert_eq!(
            normalized["components"]["DocGenerator:pdf"]["obj"]["component_name"],
            "DocGenerator"
        );
        assert_eq!(
            normalized["components"]["Custom:1"]["obj"]["component_name"],
            "Custom Splitter"
        );
        assert!(normalized["components"]["Nameless:1"]["obj"]["component_name"].is_null());
        assert_eq!(
            normalized["path"],
            json!([
                "TokenChunker:short",
                "TitleChunker:longer",
                "DocGenerator:pdf"
            ])
        );
        assert_eq!(normalized["graph"]["nodes"][0]["id"], "TokenChunker:short");
        assert_eq!(normalized["graph"]["nodes"][0]["type"], "chunkerNode");
        assert_eq!(
            normalized["graph"]["nodes"][0]["data"]["label"],
            "TokenChunker"
        );
        assert_eq!(
            normalized["graph"]["nodes"][0]["data"]["name"],
            "TokenChunker"
        );
        assert_eq!(
            normalized["graph"]["nodes"][0]["data"]["form"]["input"],
            "{{ TokenChunker:short@chunks }}"
        );
        assert_eq!(
            normalized["graph"]["nodes"][1]["parentId"],
            "DocGenerator:pdf"
        );
        assert_eq!(
            normalized["graph"]["nodes"][1]["data"]["name"],
            "custom-name"
        );
        assert_eq!(
            normalized["graph"]["edges"][0],
            json!({
                "id": "xy-edge__TokenChunker:short-TitleChunker:longer-DocGenerator:pdf",
                "source": "TokenChunker:short",
                "target": "TitleChunker:longer"
            })
        );
        assert_eq!(
            normalized["history"][0]["content"],
            "{{TokenChunker:short@chunks}}"
        );
        assert_eq!(normalized["messages"][0], "DocGenerator:pdf");
        assert_eq!(
            normalized["reference"]["value"],
            "{{ TitleChunker:longer@result }}"
        );
        assert_eq!(normalized["unrelated"], "Splitter:short");
        assert_eq!(
            original["components"]["Splitter:short"]["obj"]["component_name"],
            "Splitter"
        );
        assert_eq!(normalize_chunker_dsl(&normalized), normalized);

        let canvas = normalize_agent_dsl_for_canvas(&original);
        let runtime = normalize_agent_dsl_for_run(&original);
        assert!(canvas["components"].get("TokenChunker:short").is_some());
        assert!(runtime["components"].get("TokenChunker:short").is_some());
    }

    #[test]
    fn chunker_dsl_migration_requires_a_components_object_like_python() {
        for input in [
            Value::Null,
            json!([]),
            json!({"graph": {"nodes": [{"id": "Splitter:one", "type": "splitterNode"}]}}),
            json!({"components": [], "history": ["{{Splitter:one@chunks}}"]}),
        ] {
            assert_eq!(normalize_chunker_dsl(&input), input);
        }
    }

    #[test]
    fn run_normalizer_folds_legacy_children_and_rewrites_iteration_aliases() {
        let original = json!({
            "graph": {
                "nodes": [
                    {
                        "id": "Iteration:abc",
                        "type": "iterationNode",
                        "data": {"label": "Iteration", "name": "Iteration"}
                    },
                    {
                        "id": "IterationItem:def",
                        "type": "iterationStartNode",
                        "parentId": "Iteration:abc"
                    },
                    {"id": "Body:1", "type": "messageNode"}
                ],
                "edges": []
            },
            "components": {
                "Iteration:abc": {
                    "obj": {
                        "component_name": "Iteration",
                        "params": {"items_ref": "sys.items"}
                    },
                    "downstream": ["IterationItem:def", "Done:1"]
                },
                "IterationItem:def": {
                    "obj": {"component_name": "IterationItem", "params": {}},
                    "downstream": ["Body:1", "Done:1"]
                },
                "Body:1": {
                    "obj": {
                        "component_name": "Message",
                        "params": {
                            "content": ["{IterationItem:def@index}: {iterationitem:def@result}"]
                        }
                    },
                    "upstream": ["IterationItem:def"],
                    "downstream": ["IterationItem:def"]
                }
            }
        });
        let normalized = normalize_agent_dsl_for_run(&original);
        assert!(normalized["components"].get("IterationItem:def").is_none());
        assert_eq!(
            normalized["components"]["Iteration:abc"]["obj"]["component_name"],
            "Parallel"
        );
        assert_eq!(
            normalized["components"]["Iteration:abc"]["downstream"],
            json!(["Done:1", "Body:1"])
        );
        assert_eq!(
            normalized["components"]["Body:1"]["obj"]["params"]["content"][0],
            "{index}: {item}"
        );
        assert_eq!(
            normalized["components"]["Body:1"]["upstream"],
            json!(["Iteration:abc"])
        );
        assert_eq!(
            normalized["components"]["Body:1"]["downstream"],
            json!(["Iteration:abc"])
        );
        assert_eq!(
            normalized["graph"]["nodes"][0]["data"],
            json!({"label": "Parallel", "name": "Parallel"})
        );
        assert_eq!(normalized["graph"]["nodes"][0]["type"], "parallelNode");
        assert!(original["components"].get("IterationItem:def").is_some());
    }

    #[test]
    fn normalizers_are_idempotent_and_tolerate_non_object_values() {
        for input in [Value::Null, json!([]), json!("dsl")] {
            assert_eq!(normalize_agent_dsl_for_canvas(&input), input);
            assert_eq!(normalize_agent_dsl_for_run(&input), input);
        }
        let dsl = json!({
            "graph": {"nodes": [{"id": "a"}], "edges": [null, "bad"]},
            "components": {"a": null}
        });
        let once = normalize_agent_dsl_for_canvas(&dsl);
        assert_eq!(normalize_agent_dsl_for_canvas(&once), once);
    }

    #[test]
    fn canvas_decoder_accepts_import_topology_and_graph_parent_metadata() {
        let dsl = json!({
            "graph": {
                "nodes": [
                    {"id": "begin"},
                    {"id": "message", "parentId": "begin"}
                ],
                "edges": []
            },
            "globals": {"env.region": "test"},
            "components": {
                "begin": {
                    "obj": {
                        "component_name": "Begin",
                        "params": {},
                        "downstream": ["message"]
                    },
                    "name": "WrongFlatName",
                    "params": {"mode": "invalid"},
                    "downstream": []
                },
                "message": {
                    "obj": {
                        "component_name": "Message",
                        "params": {"content": ["decoded"]}
                    },
                    "upstream": ["begin"]
                }
            }
        });

        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        assert_eq!(workflow.begin_id, "begin");
        assert_eq!(workflow.nodes["begin"].component_name, "Begin");
        assert_eq!(workflow.nodes["begin"].downstream, ["message"]);
        assert_eq!(workflow.nodes["message"].upstream, ["begin"]);
        assert_eq!(
            workflow.nodes["message"].parent_id.as_deref(),
            Some("begin")
        );
        assert_eq!(workflow.initial_globals["env.region"], "test");
    }

    #[test]
    fn component_timeout_resolution_matches_fixed_precedence_and_defaults() {
        let resolve = |class: &str, values: &[(&str, &str)]| {
            resolve_component_timeout_with(class, |name| {
                values
                    .iter()
                    .find_map(|(key, value)| (*key == name).then(|| (*value).to_owned()))
            })
        };

        assert_eq!(
            resolve(
                "Retrieval",
                &[
                    ("COMPONENT_EXEC_TIMEOUT_RETRIEVAL", "7"),
                    ("COMPONENT_EXEC_TIMEOUT", "9")
                ]
            ),
            Duration::from_secs(7)
        );
        assert_eq!(
            resolve("Retrieval", &[("COMPONENT_EXEC_TIMEOUT", "9")]),
            Duration::from_secs(60)
        );
        assert_eq!(
            resolve(" retrieval ", &[("COMPONENT_EXEC_TIMEOUT", "9")]),
            Duration::from_secs(60)
        );
        assert_eq!(
            resolve("tavilysearch", &[("COMPONENT_EXEC_TIMEOUT", "99")]),
            Duration::from_secs(12)
        );
        assert_eq!(
            resolve("duckduckgo", &[("COMPONENT_EXEC_TIMEOUT", "99")]),
            Duration::from_secs(12)
        );
        assert_eq!(
            resolve("wikipedia", &[("COMPONENT_EXEC_TIMEOUT", "99")]),
            Duration::from_secs(60)
        );
        assert_eq!(
            resolve("googlescholar", &[("COMPONENT_EXEC_TIMEOUT", "99")]),
            Duration::from_secs(12)
        );
        assert_eq!(
            resolve("github", &[("COMPONENT_EXEC_TIMEOUT", "99")]),
            Duration::from_secs(12)
        );
        assert_eq!(
            resolve("YahooFinance", &[("COMPONENT_EXEC_TIMEOUT", "99")]),
            Duration::from_secs(60)
        );
        assert_eq!(
            resolve("Wikipedia", &[("COMPONENT_EXEC_TIMEOUT_WIKIPEDIA", "8")]),
            Duration::from_secs(8)
        );
        assert_eq!(
            resolve("tavilyextract", &[("COMPONENT_EXEC_TIMEOUT", "99")]),
            Duration::from_secs(99)
        );
        assert_eq!(resolve("tavilyextract", &[]), Duration::from_secs(600));
        assert_eq!(
            resolve("Custom", &[("COMPONENT_EXEC_TIMEOUT", "9")]),
            Duration::from_secs(9)
        );
        assert_eq!(
            resolve(
                "ExeSQL",
                &[
                    ("COMPONENT_EXEC_TIMEOUT_EXESQL", "invalid"),
                    ("COMPONENT_EXEC_TIMEOUT", "99")
                ]
            ),
            Duration::from_secs(3)
        );
        assert_eq!(resolve("Unknown", &[]), Duration::from_secs(600));
        for invalid in ["", "0", "-1", "1.5", "9223372037"] {
            assert_eq!(
                resolve("Unknown", &[("COMPONENT_EXEC_TIMEOUT", invalid)]),
                Duration::from_secs(600),
                "{invalid}"
            );
        }
    }

    #[test]
    fn fixed_ragflow_dsl_fixtures_normalize_without_legacy_runtime_names() {
        let Some(repository) = std::env::var_os("RAGFLOW_FIXTURE_REPO") else {
            return;
        };
        const COMMIT: &str = "cb93883f3f8c975eecb2fed81210effeb3bdb06f";
        for name in [
            "agent_msg.json",
            "all.json",
            "dfx_picture_parser.json",
            "questions_category.json",
            "resume.json",
            "subaget.json",
            "switch.json",
        ] {
            let path = format!("internal/agent/dsl/testdata/{name}");
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&repository)
                .args(["show", &format!("{COMMIT}:{path}")])
                .output()
                .unwrap();
            assert!(output.status.success(), "{name}: git show failed");
            let fixture: Value = serde_json::from_slice(&output.stdout).unwrap();
            let canvas = normalize_agent_dsl_for_canvas(&fixture);
            let runtime = normalize_agent_dsl_for_run(&fixture);
            assert!(canvas.is_object(), "{name}: canvas is not an object");
            assert!(runtime.is_object(), "{name}: runtime is not an object");
            for (component_id, component) in runtime
                .get("components")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let Some(component) = component.as_object() else {
                    continue;
                };
                assert!(
                    !matches!(
                        canvas_component_name(component),
                        "LoopItem" | "IterationItem" | "Iteration"
                    ),
                    "{name}: {component_id} retained legacy runtime name"
                );
            }
        }
    }

    #[test]
    fn reset_agent_dsl_clears_run_state_restores_env_and_preserves_structure() {
        let original = json!({
            "components": [],
            "graph": {"nodes": [{"id": "begin"}]},
            "history": ["message"],
            "retrieval": [{"id": "chunk"}],
            "memory": ["memory"],
            "path": ["begin"],
            "variables": {
                "configured": {"type": "string", "value": "default"},
                "number": {"type": "number"},
                "boolean": {"type": "boolean"},
                "object": {"type": "object"},
                "array": {"type": "array[string]"}
            },
            "globals": {
                "sys.query": "question",
                "sys.turns": 3,
                "sys.score": 0.8,
                "sys.ready": true,
                "sys.items": [1, 2],
                "sys.meta": {"x": 1},
                "env.configured": "stale",
                "env.number": 4,
                "env.boolean": true,
                "env.object": {"x": 1},
                "env.array": ["x"],
                "env.undeclared": "stale",
                "user.keep": {"nested": [1, 2]}
            }
        });
        let reset = reset_agent_dsl(&original);

        for field in ["history", "retrieval", "memory", "path"] {
            assert_eq!(reset[field], json!([]), "{field}");
        }
        assert_eq!(reset["globals"]["sys.query"], "");
        assert_eq!(reset["globals"]["sys.turns"], 0);
        assert_eq!(reset["globals"]["sys.score"], 0.0);
        assert_eq!(reset["globals"]["sys.ready"], false);
        assert_eq!(reset["globals"]["sys.items"], json!([]));
        assert_eq!(reset["globals"]["sys.meta"], json!({}));
        assert_eq!(reset["globals"]["env.configured"], "default");
        assert_eq!(reset["globals"]["env.number"], 0);
        assert_eq!(reset["globals"]["env.boolean"], false);
        assert_eq!(reset["globals"]["env.object"], json!({}));
        assert_eq!(reset["globals"]["env.array"], json!([]));
        assert_eq!(reset["globals"]["env.undeclared"], "");
        assert_eq!(reset["globals"]["user.keep"], json!({"nested": [1, 2]}));
        assert_eq!(reset["graph"], original["graph"]);
        assert_eq!(original["history"], json!(["message"]));
        assert_eq!(original["globals"]["sys.query"], "question");

        let no_globals = reset_agent_dsl(&json!({"components": []}));
        assert!(no_globals.get("globals").is_none());
        assert_eq!(reset_agent_dsl(&Value::Null), json!({}));
    }

    #[test]
    fn canvas_state_json_round_trip_has_stable_checkpoint_shape() {
        let mut state = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("hello"))]),
            env: Map::from_iter([("counter".into(), json!(0))]),
            path: vec!["begin".into(), "message".into()],
            history: vec![ChatMessage::new("user", "hello")],
            retrieval: Map::from_iter([("chunks".into(), json!([{"id": "chunk-1"}]))]),
            globals: Map::from_iter([("__item__".into(), json!("row"))]),
            cancel_flag: true,
            run_id: "run-1".into(),
            task_id: "task-1".into(),
            last_message: Some("transient".into()),
            provider_calls: 3,
            references: vec![ChunkReference {
                id: "chunk-1".into(),
                kb_id: "kb-1".into(),
                content: "body".into(),
                similarity: None,
                vector_similarity: None,
                term_similarity: None,
            }],
            ..CanvasRuntime::default()
        };
        state.outputs.insert(
            "message".into(),
            Map::from_iter([("content".into(), json!("hi world"))]),
        );

        let raw = serde_json::to_value(&state).unwrap();
        assert_eq!(raw["sys"]["query"], "hello");
        assert_eq!(raw["outputs"]["message"]["content"], "hi world");
        assert_eq!(raw["cancel_flag"], true);
        assert_eq!(raw["run_id"], "run-1");
        assert_eq!(raw["task_id"], "task-1");
        assert!(raw.get("last_message").is_none());
        assert!(raw.get("provider_calls").is_none());
        assert!(raw.get("references").is_none());

        let restored: CanvasRuntime = serde_json::from_value(raw).unwrap();
        assert_eq!(restored.sys["query"], "hello");
        assert_eq!(restored.path, ["begin", "message"]);
        assert!(restored.cancel_flag);
        assert_eq!(restored.run_id, "run-1");
        assert_eq!(restored.task_id, "task-1");
        assert_eq!(restored.history.len(), 1);
        assert!(restored.last_message.is_none());
        assert!(restored.references.is_empty());
    }

    #[test]
    fn canvas_state_serde_handles_empty_owned_restore_and_invalid_json() {
        let empty = CanvasRuntime {
            run_id: "run-empty".into(),
            task_id: "task-empty".into(),
            ..CanvasRuntime::default()
        };
        let bytes = serde_json::to_vec(&empty).unwrap();
        assert!(!bytes.is_empty());
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).unwrap(),
            json!({
                "outputs": {},
                "cancel_flag": false,
                "run_id": "run-empty",
                "task_id": "task-empty"
            })
        );

        let mut restored = CanvasRuntime {
            outputs: BTreeMap::from([(
                "stale".into(),
                Map::from_iter([("value".into(), json!("discard"))]),
            )]),
            run_id: "old".into(),
            task_id: "old".into(),
            cancel_flag: true,
            ..CanvasRuntime::default()
        };
        let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
        <CanvasRuntime as Deserialize>::deserialize_in_place(&mut deserializer, &mut restored)
            .unwrap();
        assert!(restored.outputs.is_empty());
        assert_eq!(restored.run_id, "run-empty");
        assert_eq!(restored.task_id, "task-empty");
        assert!(!restored.cancel_flag);
        assert!(serde_json::from_slice::<CanvasRuntime>(br#"{"outputs":"bad"}"#).is_err());
        assert!(serde_json::from_slice::<CanvasRuntime>(b"{").is_err());
    }

    #[test]
    fn canvas_state_resolves_namespaces_json_paths_and_nested_writes() {
        let mut state = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!({"parts": ["a", "b"]}))]),
            env: Map::from_iter([("encoded".into(), json!("{\"answer\":42}"))]),
            globals: Map::from_iter([
                ("env.legacy".into(), json!("kept")),
                ("__item__".into(), json!("row")),
            ]),
            ..CanvasRuntime::default()
        };
        state.outputs.insert(
            "node".into(),
            Map::from_iter([("scalar".into(), json!("old"))]),
        );

        assert_eq!(
            get_variable(&state, "sys.query.parts.1").unwrap(),
            json!("b")
        );
        assert_eq!(
            get_variable(&state, "env.encoded.answer").unwrap(),
            json!(42)
        );
        assert_eq!(get_variable(&state, "env.legacy").unwrap(), "kept");
        assert_eq!(get_variable(&state, "item").unwrap(), "row");

        set_variable(&mut state, "sys.session.id", json!("s-1")).unwrap();
        set_variable(&mut state, "node@scalar.deep", json!(7)).unwrap();
        set_variable(&mut state, "env.legacy", json!("updated")).unwrap();
        assert_eq!(
            get_variable(&state, "sys.session.id").unwrap(),
            json!("s-1")
        );
        assert_eq!(get_variable(&state, "node@scalar.deep").unwrap(), json!(7));
        assert_eq!(state.globals["env.legacy"], "updated");
    }

    #[derive(Default)]
    struct MockRetriever {
        request: std::sync::Mutex<Option<WorkflowRetrievalRequest>>,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl WorkflowRetriever for MockRetriever {
        async fn retrieve(
            &self,
            request: WorkflowRetrievalRequest,
        ) -> Result<WorkflowRetrievalResult> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *self.request.lock().unwrap() = Some(request);
            Ok(WorkflowRetrievalResult {
                formalized_content: "Reference 1 | Guide.md\nRust ownership guide".into(),
                chunks: vec![json!({
                    "chunk_id": "chunk-1",
                    "kb_id": "fallback-kb",
                    "content": "Rust ownership guide",
                    "score": 0.91
                })],
                doc_aggs: vec![json!({"doc_name": "Guide.md", "count": 1})],
                references: vec![ChunkReference {
                    id: "chunk-1".into(),
                    kb_id: "fallback-kb".into(),
                    content: "Rust ownership guide".into(),
                    similarity: Some(0.91),
                    vector_similarity: Some(0.88),
                    term_similarity: Some(0.73),
                }],
            })
        }
    }

    #[derive(Default)]
    struct MockTavily {
        searches: std::sync::Mutex<Vec<(String, TavilySearchRequest)>>,
        extracts: std::sync::Mutex<Vec<(String, TavilyExtractRequest)>>,
        search_failures: std::sync::atomic::AtomicUsize,
        extract_failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl TavilyProvider for MockTavily {
        async fn search(&self, api_key: &str, request: &TavilySearchRequest) -> Result<Vec<Value>> {
            self.searches
                .lock()
                .unwrap()
                .push((api_key.into(), request.clone()));
            if self
                .search_failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary search failure");
            }
            Ok(vec![json!({
                "url": "https://example.test/rust",
                "title": "Rust\nGuide",
                "content": "alpha ![img](data:image/png;base64,AAAA) beta",
                "raw_content": null,
                "score": 0.91
            })])
        }

        async fn extract(
            &self,
            api_key: &str,
            request: &TavilyExtractRequest,
        ) -> Result<Vec<Value>> {
            self.extracts
                .lock()
                .unwrap()
                .push((api_key.into(), request.clone()));
            if self
                .extract_failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary extract failure");
            }
            Ok(vec![json!({
                "url": "https://example.test/a",
                "raw_content": "page body"
            })])
        }
    }

    #[derive(Default)]
    struct MockWikipedia {
        searches: std::sync::Mutex<Vec<WikipediaSearchRequest>>,
        failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl WikipediaProvider for MockWikipedia {
        async fn search(&self, request: &WikipediaSearchRequest) -> Result<Vec<WikipediaArticle>> {
            self.searches.lock().unwrap().push(request.clone());
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary Wikipedia failure");
            }
            Ok(vec![
                WikipediaArticle {
                    title: "Rust\nLanguage".into(),
                    url: "https://en.wikipedia.org/wiki/Rust_(programming_language)".into(),
                    summary: "alpha ![img](data:image/png;base64,AAAA) beta".into(),
                    snippet: "<span>Rust</span> language".into(),
                },
                WikipediaArticle {
                    title: "Empty".into(),
                    url: "https://en.wikipedia.org/wiki/Empty".into(),
                    summary: String::new(),
                    snippet: "empty summary".into(),
                },
            ])
        }
    }

    #[derive(Default)]
    struct MockDuckDuckGo {
        searches: std::sync::Mutex<Vec<DuckDuckGoSearchRequest>>,
        failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl DuckDuckGoProvider for MockDuckDuckGo {
        async fn search(&self, request: &DuckDuckGoSearchRequest) -> Result<Vec<Value>> {
            self.searches.lock().unwrap().push(request.clone());
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary DuckDuckGo failure");
            }
            let url_key = match request.channel {
                DuckDuckGoChannel::Text => "href",
                DuckDuckGoChannel::News => "url",
            };
            let mut result = serde_json::json!({
                "title": "Rust\nSearch",
                "body": "alpha ![img](data:image/png;base64,AAAA) beta"
            });
            result[url_key] = Value::String("https://example.test/rust".into());
            Ok(vec![result])
        }
    }

    #[derive(Default)]
    struct MockGoogle {
        searches: std::sync::Mutex<Vec<(String, GoogleSearchRequest)>>,
        failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl GoogleProvider for MockGoogle {
        async fn search(&self, api_key: &str, request: &GoogleSearchRequest) -> Result<Vec<Value>> {
            self.searches
                .lock()
                .unwrap()
                .push((api_key.into(), request.clone()));
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary Google failure");
            }
            Ok(vec![
                json!({
                    "position": 1,
                    "title": "Rust\nSearch",
                    "link": "https://example.test/rust",
                    "snippet": "fallback snippet",
                    "about_this_result": {
                        "source": {
                            "description": "alpha ![img](data:image/png;base64,AAAA) beta"
                        }
                    }
                }),
                // Python skips a falsey description before reading title/link.
                json!({
                    "snippet": "not selected",
                    "about_this_result": {"source": {"description": null}}
                }),
            ])
        }
    }

    #[derive(Default)]
    struct MockGoogleScholar {
        searches: std::sync::Mutex<Vec<GoogleScholarSearchRequest>>,
        failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl GoogleScholarProvider for MockGoogleScholar {
        async fn search(
            &self,
            request: &GoogleScholarSearchRequest,
        ) -> Result<Vec<GoogleScholarPublication>> {
            self.searches.lock().unwrap().push(request.clone());
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary Google Scholar failure");
            }
            Ok(vec![GoogleScholarPublication {
                title: "Rust\nScholarship".into(),
                authors: vec!["Alice".into(), "Bob".into()],
                abstract_text: Some("alpha ![img](data:image/png;base64,AAAA) beta".into()),
                pub_url: "https://example.test/rust-scholar".into(),
                venue: "RustConf".into(),
                year: "2026".into(),
                gsrank: 1,
            }])
        }
    }

    #[derive(Default)]
    struct MockGitHub {
        searches: std::sync::Mutex<Vec<GitHubSearchRequest>>,
        failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl GitHubProvider for MockGitHub {
        async fn search(&self, request: &GitHubSearchRequest) -> Result<Vec<GitHubRepository>> {
            self.searches.lock().unwrap().push(request.clone());
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary GitHub failure");
            }
            let raw = json!({
                "name": "rayrag",
                "full_name": "example/rayrag",
                "html_url": "https://github.com/example/rayrag",
                "description": "alpha ![img](data:image/png;base64,AAAA) beta",
                "watchers": 42,
                "stargazers_count": 42,
                "extra": "preserved"
            });
            Ok(vec![GitHubRepository {
                name: "rayrag\nrepository".into(),
                full_name: "example/rayrag".into(),
                html_url: "https://github.com/example/rayrag".into(),
                description: raw["description"].clone(),
                watchers: raw["watchers"].clone(),
                stargazers_count: 42,
                raw,
            }])
        }
    }

    #[derive(Default)]
    struct MockYahooFinance {
        reports: std::sync::Mutex<Vec<YahooFinanceRequest>>,
        failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl YahooFinanceProvider for MockYahooFinance {
        async fn report(&self, request: &YahooFinanceRequest) -> Result<String> {
            self.reports.lock().unwrap().push(request.clone());
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary Yahoo Finance failure");
            }
            Ok(format!("# Information:\n{}", request.stock_code))
        }
    }

    #[derive(Default)]
    struct MockArxiv {
        searches: std::sync::Mutex<Vec<ArxivSearchRequest>>,
        failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ArxivProvider for MockArxiv {
        async fn search(&self, request: &ArxivSearchRequest) -> Result<Vec<ArxivPaper>> {
            self.searches.lock().unwrap().push(request.clone());
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary ArXiv failure");
            }
            Ok(vec![
                ArxivPaper {
                    title: "Rust\nRetrieval".into(),
                    authors: vec!["Alice".into(), "Bob".into()],
                    summary: "alpha ![img](data:image/png;base64,AAAA) beta".into(),
                    pdf_url: Some("http://arxiv.org/pdf/2501.12345v1".into()),
                    entry_id: "http://arxiv.org/abs/2501.12345v1".into(),
                },
                ArxivPaper {
                    title: "Empty".into(),
                    authors: vec!["Carol".into()],
                    summary: String::new(),
                    pdf_url: None,
                    entry_id: "http://arxiv.org/abs/2409.99999v2".into(),
                },
            ])
        }
    }

    #[derive(Default)]
    struct MockPubMed {
        searches: std::sync::Mutex<Vec<PubMedSearchRequest>>,
        failures: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl PubMedProvider for MockPubMed {
        async fn search(&self, request: &PubMedSearchRequest) -> Result<Vec<PubMedArticle>> {
            self.searches.lock().unwrap().push(request.clone());
            if self
                .failures
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |remaining| remaining.checked_sub(1),
                )
                .is_ok()
            {
                bail!("temporary PubMed failure");
            }
            Ok(vec![PubMedArticle {
                pmid: "31415926".into(),
                title: "Rust\nBiomedical Retrieval".into(),
                authors: vec!["Alice Smith".into(), "Bob Jones".into()],
                journal: "Journal of Rust Medicine".into(),
                volume: "10".into(),
                issue: "2".into(),
                pages: "101-110".into(),
                doi: Some("10.1000/rust.pubmed".into()),
                abstract_text: "alpha ![img](data:image/png;base64,AAAA) beta".into(),
                publication_date: "2024 Jan".into(),
            }])
        }
    }

    #[derive(Default)]
    struct BlockingRetriever {
        release: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl WorkflowRetriever for BlockingRetriever {
        async fn retrieve(
            &self,
            _request: WorkflowRetrievalRequest,
        ) -> Result<WorkflowRetrievalResult> {
            self.release.notified().await;
            Ok(WorkflowRetrievalResult {
                formalized_content: "live result".into(),
                ..WorkflowRetrievalResult::default()
            })
        }
    }

    struct ChannelWorkflowObserver(tokio::sync::mpsc::UnboundedSender<WorkflowLifecycleEvent>);

    impl WorkflowEventObserver for ChannelWorkflowObserver {
        fn emit(&self, event: WorkflowLifecycleEvent) {
            let _ = self.0.send(event);
        }
    }

    #[derive(Default)]
    struct ParallelConcurrencyRetriever {
        active: std::sync::atomic::AtomicUsize,
        maximum: std::sync::atomic::AtomicUsize,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl WorkflowRetriever for ParallelConcurrencyRetriever {
        async fn retrieve(
            &self,
            request: WorkflowRetrievalRequest,
        ) -> Result<WorkflowRetrievalResult> {
            let active = self
                .active
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .saturating_add(1);
            self.maximum
                .fetch_max(active, std::sync::atomic::Ordering::SeqCst);
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let delay = match request.query.as_str() {
                "a" => 45,
                "b" => 5,
                "c" => 25,
                _ => 1,
            };
            tokio::time::sleep(Duration::from_millis(delay)).await;
            self.active
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            if request.query == "error" {
                bail!("synthetic item failure");
            }
            Ok(WorkflowRetrievalResult {
                formalized_content: request.query,
                ..WorkflowRetrievalResult::default()
            })
        }
    }

    fn branch_canvas() -> Value {
        json!({
            "components": {
                "begin": {
                    "obj": {"component_name": "Begin", "params": {}},
                    "downstream": ["assign:0"], "upstream": []
                },
                "assign:0": {
                    "obj": {"component_name": "VariableAssigner", "params": {"variables": [
                        {"variable": "env.score", "operator": "+=", "parameter": 2}
                    ]}},
                    "downstream": ["switch:0"], "upstream": ["begin"]
                },
                "switch:0": {
                    "obj": {"component_name": "Switch", "params": {
                        "conditions": [{"logical_operator": "and", "items": [
                            {"cpn_id": "env.score", "operator": "≥", "value": 3}
                        ], "to": ["message:yes"]}],
                        "end_cpn_ids": ["message:no"]
                    }},
                    "downstream": ["message:yes", "message:no"], "upstream": ["assign:0"]
                },
                "message:yes": {
                    "obj": {"component_name": "Message", "params": {"content": ["score={env.score}; q={sys.query}"]}},
                    "downstream": [], "upstream": ["switch:0"]
                },
                "message:no": {
                    "obj": {"component_name": "Message", "params": {"content": ["no"]}},
                    "downstream": [], "upstream": ["switch:0"]
                }
            },
            "globals": {"env.score": 1}
        })
    }

    fn counter_loop_canvas(initial: Value, threshold: i64, maximum: i64) -> Value {
        json!({
            "components": {
                "begin": {
                    "obj": {"component_name": "Begin", "params": {}},
                    "downstream": ["loop"], "upstream": []
                },
                "loop": {
                    "obj": {"component_name": "Loop", "params": {
                        "loop_variables": [{
                            "variable": "counter",
                            "input_mode": "constant",
                            "value": initial,
                            "type": "number"
                        }],
                        "loop_termination_condition": [{
                            "variable": "counter",
                            "operator": "≥",
                            "value": threshold,
                            "input_mode": "constant"
                        }],
                        "logical_operator": "and",
                        "maximum_loop_count": maximum
                    }},
                    "downstream": ["bump", "done"], "upstream": ["begin"]
                },
                "bump": {
                    "obj": {"component_name": "VariableAssigner", "params": {
                        "variables": [{
                            "variable": "loop@counter",
                            "operator": "+=",
                            "parameter": 1
                        }]
                    }},
                    "parent_id": "loop",
                    "downstream": [], "upstream": ["loop"]
                },
                "done": {
                    "obj": {"component_name": "Message", "params": {
                        "content": ["counter={loop@counter}"]
                    }},
                    "downstream": [], "upstream": ["loop"]
                }
            }
        })
    }

    fn parallel_string_canvas(max_concurrency: usize) -> Value {
        json!({
            "components": {
                "begin": {
                    "obj": {"component_name": "Begin", "params": {}},
                    "downstream": ["parallel"], "upstream": []
                },
                "parallel": {
                    "obj": {"component_name": "Parallel", "params": {
                        "items_ref": "begin@items",
                        "max_concurrency": max_concurrency,
                        "outputs": {
                            "lines": {"ref": "format@result", "type": "Array<string>"},
                            "items": {"ref": "item", "type": "Array<unknown>"},
                            "indices": {"ref": "index", "type": "Array<integer>"},
                            "missing": {"ref": "format@absent", "type": "Array<unknown>"}
                        }
                    }},
                    "downstream": ["done"], "upstream": ["begin"]
                },
                "assign": {
                    "obj": {"component_name": "VariableAssigner", "params": {
                        "variables": [{
                            "variable": "env.parallel_item",
                            "operator": "set",
                            "parameter": "{item}"
                        }]
                    }},
                    "parent_id": "parallel",
                    "downstream": ["format"], "upstream": ["parallel"]
                },
                "format": {
                    "obj": {"component_name": "StringTransform", "params": {
                        "method": "merge",
                        "script": "{index}:{env.parallel_item}",
                        "delimiters": ["|"]
                    }},
                    "parent_id": "parallel",
                    "downstream": [], "upstream": ["assign"]
                },
                "done": {
                    "obj": {"component_name": "Message", "params": {
                        "content": ["{parallel@lines}"]
                    }},
                    "downstream": [], "upstream": ["parallel"]
                }
            }
        })
    }

    fn parallel_retrieval_canvas(max_concurrency: usize) -> Value {
        json!({
            "components": {
                "begin": {
                    "obj": {"component_name": "Begin", "params": {}},
                    "downstream": ["parallel"], "upstream": []
                },
                "parallel": {
                    "obj": {"component_name": "Parallel", "params": {
                        "items_ref": "begin@items",
                        "max_concurrency": max_concurrency,
                        "outputs": {
                            "results": {"ref": "retrieve@formalized_content"}
                        }
                    }},
                    "downstream": ["done"], "upstream": ["begin"]
                },
                "retrieve": {
                    "obj": {"component_name": "Retrieval", "params": {
                        "query": "{item}",
                        "kb_ids": []
                    }},
                    "parent_id": "parallel",
                    "downstream": [], "upstream": ["parallel"]
                },
                "done": {
                    "obj": {"component_name": "Message", "params": {
                        "content": ["{parallel@results}"]
                    }},
                    "downstream": [], "upstream": ["parallel"]
                }
            }
        })
    }

    fn run_list_operation(params: Value, items: Value) -> Result<Map<String, Value>> {
        let mut runtime = CanvasRuntime::default();
        runtime.globals.insert("sys.items".into(), items);
        let node = CanvasNode {
            id: "list".into(),
            component_name: "ListOperations".into(),
            params: params.as_object().cloned().unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        execute_list_operations(&mut runtime, &node)?;
        Ok(runtime.outputs.remove("list").unwrap())
    }

    fn run_data_operation(
        params: Value,
        globals: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Result<Value> {
        let mut runtime = CanvasRuntime::default();
        runtime.globals.extend(
            globals
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value)),
        );
        let node = CanvasNode {
            id: "data".into(),
            component_name: "DataOperations".into(),
            params: params.as_object().cloned().unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        execute_data_operations(&mut runtime, &node)?;
        Ok(runtime
            .outputs
            .remove("data")
            .unwrap()
            .remove("result")
            .unwrap())
    }

    fn run_excel_operation(
        params: Value,
        globals: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Result<Map<String, Value>> {
        let mut runtime = CanvasRuntime::default();
        runtime.globals.extend(
            globals
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value)),
        );
        let node = CanvasNode {
            id: "excel".into(),
            component_name: "ExcelProcessor".into(),
            params: params.as_object().cloned().unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_excel_processor_params(&node)?;
        execute_excel_processor(&mut runtime, &node)?;
        Ok(runtime.outputs.remove("excel").unwrap())
    }

    fn run_doc_generator(
        component_name: &str,
        params: Value,
        globals: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Result<Map<String, Value>> {
        let mut runtime = CanvasRuntime::default();
        runtime.globals.extend(
            globals
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value)),
        );
        let node = CanvasNode {
            id: "docs".into(),
            component_name: component_name.into(),
            params: params.as_object().cloned().unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_doc_generator_params(&node)?;
        execute_doc_generator(&mut runtime, &node)?;
        Ok(runtime.outputs.remove("docs").unwrap())
    }

    fn run_switch(
        params: Value,
        globals: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Result<(Vec<String>, Map<String, Value>)> {
        let mut runtime = CanvasRuntime::default();
        runtime.globals.extend(
            globals
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value)),
        );
        let node = CanvasNode {
            id: "switch".into(),
            component_name: "Switch".into(),
            params: params.as_object().cloned().unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let next = execute_switch(&mut runtime, &node)?;
        Ok((next, runtime.outputs.remove("switch").unwrap()))
    }

    #[tokio::test]
    async fn core_canvas_executes_assignment_switch_and_message() {
        let workflow = AgentWorkflow::from_value(&branch_canvas())
            .unwrap()
            .unwrap();
        let result = workflow
            .run(
                None,
                WorkflowRunInput {
                    question: "hello",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "score=3.0; q=hello");
        assert_eq!(
            result.path,
            ["begin", "assign:0", "switch:0", "message:yes"]
        );
        assert_eq!(result.usage, None);
    }

    #[tokio::test]
    async fn parallel_runs_isolated_items_and_collects_ordered_declared_outputs() {
        let workflow = AgentWorkflow::from_value(&parallel_string_canvas(0))
            .unwrap()
            .unwrap();
        assert!(workflow.unsupported_components().is_empty());
        assert_eq!(workflow.execution_order, ["begin", "parallel", "done"]);
        assert_eq!(
            workflow.parallel_plans["parallel"].execution_order,
            ["assign", "format"]
        );
        assert_eq!(
            workflow.parallel_plans["parallel"].outer_downstream,
            ["done"]
        );

        let inputs = Map::from_iter([("items".into(), json!(["alpha", "beta", "gamma"]))]);
        let result = workflow
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &inputs,
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, r#"["0:alpha","1:beta","2:gamma"]"#);
        assert_eq!(result.path, ["begin", "parallel", "done"]);
        let outputs = &result
            .trace
            .iter()
            .find(|trace| trace.component_id == "parallel")
            .unwrap()
            .outputs;
        assert_eq!(outputs["lines"], json!(["0:alpha", "1:beta", "2:gamma"]));
        assert_eq!(outputs["items"], json!(["alpha", "beta", "gamma"]));
        assert_eq!(outputs["indices"], json!([0, 1, 2]));
        assert_eq!(outputs["missing"], json!([null, null, null]));
        assert_eq!(outputs["_result"].as_array().unwrap().len(), 3);
        assert_eq!(outputs["_result"][0]["item"], "alpha");
        assert_eq!(outputs["_result"][1]["index"], 1);
        assert_eq!(outputs["_result"][2]["format"]["result"], "2:gamma");
        assert!(
            result
                .trace
                .iter()
                .all(|trace| !matches!(trace.component_id.as_str(), "assign" | "format"))
        );
    }

    #[tokio::test]
    async fn parallel_max_concurrency_is_bounded_and_keeps_input_order() {
        let inputs = Map::from_iter([("items".into(), json!(["a", "b", "c", "d"]))]);

        let concurrent = ParallelConcurrencyRetriever::default();
        let result = AgentWorkflow::from_value(&parallel_retrieval_canvas(2))
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &inputs,
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: Some(&concurrent),
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, r#"["a","b","c","d"]"#);
        assert_eq!(
            concurrent.maximum.load(std::sync::atomic::Ordering::SeqCst),
            2
        );
        assert_eq!(
            concurrent.calls.load(std::sync::atomic::Ordering::SeqCst),
            4
        );

        let sequential = ParallelConcurrencyRetriever::default();
        AgentWorkflow::from_value(&parallel_retrieval_canvas(0))
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &inputs,
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: Some(&sequential),
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(
            sequential.maximum.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn parallel_null_items_produce_an_empty_ordered_batch() {
        let inputs = Map::from_iter([("items".into(), Value::Null)]);
        let result = AgentWorkflow::from_value(&parallel_string_canvas(8))
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &inputs,
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "[]");
        let outputs = &result
            .trace
            .iter()
            .find(|trace| trace.component_id == "parallel")
            .unwrap()
            .outputs;
        for field in ["_result", "lines", "items", "indices", "missing"] {
            assert_eq!(outputs[field], json!([]), "{field}");
        }
    }

    #[tokio::test]
    async fn parallel_rejects_non_array_items_and_missing_items_ref() {
        let missing = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["parallel"], "upstream": []
            },
            "parallel": {
                "obj": {"component_name": "Parallel", "params": {}},
                "downstream": ["body"], "upstream": ["begin"]
            },
            "body": {
                "obj": {"component_name": "Message", "params": {"content": ["body"]}},
                "parent_id": "parallel",
                "downstream": [], "upstream": ["parallel"]
            }
        }});
        assert!(
            AgentWorkflow::from_value(&missing)
                .unwrap_err()
                .to_string()
                .contains("items_ref is required")
        );

        let inputs = Map::from_iter([("items".into(), json!("not-an-array"))]);
        let error = AgentWorkflow::from_value(&parallel_string_canvas(4))
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &inputs,
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("expected array, got string"));
    }

    #[tokio::test]
    async fn parallel_drains_remaining_items_before_returning_an_item_error() {
        let retriever = ParallelConcurrencyRetriever::default();
        let inputs = Map::from_iter([("items".into(), json!(["ok", "error", "after"]))]);
        let error = AgentWorkflow::from_value(&parallel_retrieval_canvas(0))
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &inputs,
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: Some(&retriever),
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("item 1"));
        assert!(format!("{error:#}").contains("synthetic item failure"));
        assert_eq!(retriever.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn loop_runs_grouped_body_do_while_and_collapses_outer_trace() {
        let workflow = AgentWorkflow::from_value(&counter_loop_canvas(json!(0), 3, 50))
            .unwrap()
            .unwrap();
        assert_eq!(workflow.execution_order, ["begin", "loop", "done"]);
        assert_eq!(workflow.loop_plans["loop"].execution_order, ["bump"]);
        assert_eq!(workflow.loop_plans["loop"].outer_downstream, ["done"]);

        let result = workflow
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "counter=3.0");
        assert_eq!(result.path, ["begin", "loop", "done"]);
        assert_eq!(
            result
                .trace
                .iter()
                .find(|trace| trace.component_id == "loop")
                .unwrap()
                .outputs["counter"],
            json!(3.0)
        );
        assert!(
            result
                .trace
                .iter()
                .all(|trace| trace.component_id != "bump")
        );
    }

    #[tokio::test]
    async fn loop_variable_mode_dereferences_live_state_once() {
        let mut dsl = counter_loop_canvas(json!(0), 8, 50);
        dsl["components"]["begin"]["downstream"] = json!(["seed"]);
        dsl["components"]["seed"] = json!({
            "obj": {"component_name": "VariableAssigner", "params": {
                "variables": [{
                    "variable": "seed@initial",
                    "operator": "set",
                    "parameter": 5
                }]
            }},
            "downstream": ["loop"], "upstream": ["begin"]
        });
        dsl["components"]["loop"]["upstream"] = json!(["seed"]);
        dsl["components"]["loop"]["obj"]["params"]["loop_variables"][0]["input_mode"] =
            json!("variable");
        dsl["components"]["loop"]["obj"]["params"]["loop_variables"][0]["value"] =
            json!("seed@initial");

        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "counter=8.0");
        assert_eq!(result.path, ["begin", "seed", "loop", "done"]);
    }

    #[tokio::test]
    async fn loop_duplicate_variable_declarations_use_the_last_spec() {
        let mut dsl = counter_loop_canvas(json!(0), 5, 2);
        dsl["components"]["loop"]["obj"]["params"]["loop_variables"] = json!([
            {
                "variable": "counter",
                "input_mode": "constant",
                "value": 0,
                "type": "number"
            },
            {
                "variable": "counter",
                "input_mode": "constant",
                "value": 4,
                "type": "number"
            }
        ]);
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "counter=5.0");
    }

    #[tokio::test]
    async fn loop_cap_error_preserves_completed_iteration_state() {
        let workflow = AgentWorkflow::from_value(&counter_loop_canvas(json!(0), 100, 5))
            .unwrap()
            .unwrap();
        let mut runtime = CanvasRuntime::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "go",
            user_id: "user-1",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let error = execute_loop(
            &mut runtime,
            &workflow.nodes["loop"],
            &workflow.loop_plans["loop"],
            &workflow.nodes,
            None,
            &run_input,
            None,
        )
        .await
        .unwrap_err();
        let cap = error.downcast_ref::<LoopMaxIterationsExceeded>().unwrap();
        assert_eq!(cap.maximum, 5);
        assert_eq!(runtime.outputs["loop"]["counter"], json!(5.0));
    }

    #[tokio::test]
    async fn loop_body_branch_and_exitloop_terminal_do_not_bypass_predicate() {
        let mut dsl = counter_loop_canvas(json!(0), 3, 10);
        dsl["components"]["bump"]["downstream"] = json!(["switch"]);
        dsl["components"]["switch"] = json!({
            "obj": {"component_name": "Switch", "params": {
                "conditions": [{
                    "logical_operator": "and",
                    "items": [{
                        "cpn_id": "loop@counter",
                        "operator": "≥",
                        "value": 2
                    }],
                    "to": ["exit"]
                }],
                "end_cpn_ids": ["continue"]
            }},
            "parent_id": "loop",
            "downstream": ["exit", "continue"], "upstream": ["bump"]
        });
        dsl["components"]["exit"] = json!({
            "obj": {"component_name": "ExitLoop", "params": {}},
            "parent_id": "loop",
            "downstream": [], "upstream": ["switch"]
        });
        dsl["components"]["continue"] = json!({
            "obj": {"component_name": "VariableAssigner", "params": {
                "variables": [{
                    "variable": "loop@route",
                    "operator": "set",
                    "parameter": "continue"
                }]
            }},
            "parent_id": "loop",
            "downstream": [], "upstream": ["switch"]
        });

        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        assert!(workflow.unsupported_components().is_empty());
        let result = workflow
            .run(
                None,
                WorkflowRunInput {
                    question: "go",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "counter=3.0");
        let loop_outputs = &result
            .trace
            .iter()
            .find(|trace| trace.component_id == "loop")
            .unwrap()
            .outputs;
        assert_eq!(loop_outputs["counter"], json!(3.0));
        assert_eq!(loop_outputs["route"], json!("continue"));
    }

    #[tokio::test]
    async fn loop_ungrouped_fallback_absorbs_descendants_until_back_edge() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["loop"], "upstream": []
            },
            "loop": {
                "obj": {"component_name": "Loop", "params": {
                    "loop_variables": [],
                    "loop_termination_condition": [{
                        "variable": "sys.query",
                        "operator": "is",
                        "value": "stop",
                        "input_mode": "constant"
                    }],
                    "maximum_loop_count": 5
                }},
                "downstream": ["body"], "upstream": ["begin"]
            },
            "body": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["body"]
                }},
                "downstream": [], "upstream": ["loop"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        assert_eq!(workflow.execution_order, ["begin", "loop"]);
        assert_eq!(workflow.loop_plans["loop"].execution_order, ["body"]);
        let result = workflow
            .run(
                None,
                WorkflowRunInput {
                    question: "stop",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "body");
        assert_eq!(result.path, ["begin", "loop"]);
    }

    #[test]
    fn loop_condition_operator_dispatch_matches_fixed_go_port() {
        for (left, operator, right, expected) in [
            (json!("hello"), "contains", json!("ell"), true),
            (json!("hello"), "not contains", json!("zzz"), true),
            (json!("hello"), "start with", json!("he"), true),
            (json!("hello"), "end with", json!("lo"), true),
            (json!(true), "is", json!(true), true),
            (json!(false), "empty", Value::Null, true),
            (json!(5), "≥", json!(5), true),
            (json!({}), "empty", Value::Null, true),
            (json!([1, 2]), "contains", json!(2), true),
            (Value::Null, "empty", Value::Null, true),
            (Value::Null, "not empty", Value::Null, false),
        ] {
            assert_eq!(
                evaluate_loop_condition_value(&left, operator, &right).unwrap(),
                expected,
                "{left:?} {operator} {right:?}"
            );
        }
        assert!(
            evaluate_loop_condition_value(&json!(1), "bogus", &json!(1))
                .unwrap_err()
                .to_string()
                .contains("invalid operator")
        );
    }

    #[test]
    fn loop_zero_init_and_validation_follow_fixed_build_contract() {
        for (kind, expected) in [
            ("number", json!(0)),
            ("string", json!("")),
            ("boolean", json!(false)),
            ("object<string>", json!({})),
            ("array<string>", json!([])),
            ("unknown", json!("")),
        ] {
            assert_eq!(zero_loop_value(Some(&json!(kind))), expected, "{kind}");
        }

        let mut dsl = counter_loop_canvas(json!(0), 1, 2);
        dsl["components"]["loop"]["obj"]["params"]["loop_variables"][0]
            .as_object_mut()
            .unwrap()
            .remove("value");
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("missing 'value'")
        );
        let mut dsl = counter_loop_canvas(json!(0), 1, 2);
        dsl["components"]["loop"]["obj"]["params"]["logical_operator"] = json!("xor");
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("logical_operator")
        );
        assert_eq!(
            loop_max_iterations(&Map::from_iter([("maximum_loop_count".into(), json!(0))])),
            DEFAULT_LOOP_MAX_ITERATIONS
        );
        let runtime = CanvasRuntime::default();
        assert_eq!(
            get_loop_variable_or_null(&runtime, "missing@value").unwrap(),
            Value::Null
        );
        assert!(get_loop_variable_or_null(&runtime, "").is_err());
    }

    #[tokio::test]
    async fn concurrent_canvas_runs_own_isolated_runtime_state() {
        let workflow = AgentWorkflow::from_value(&branch_canvas())
            .unwrap()
            .unwrap();
        let first_inputs = Map::new();
        let second_inputs = Map::new();
        let first = workflow.run(
            None,
            WorkflowRunInput {
                question: "first",
                user_id: "user-1",
                inputs: &first_inputs,
                history: &[],
                generation: GenerationParamsPatch::default(),
                llm_resolver: None,
                retriever: None,
                fallback_kb_ids: &[],
            },
        );
        let second = workflow.run(
            None,
            WorkflowRunInput {
                question: "second",
                user_id: "user-2",
                inputs: &second_inputs,
                history: &[],
                generation: GenerationParamsPatch::default(),
                llm_resolver: None,
                retriever: None,
                fallback_kb_ids: &[],
            },
        );

        let (first, second) = tokio::join!(first, second);
        assert_eq!(first.unwrap().answer, "score=3.0; q=first");
        assert_eq!(second.unwrap().answer, "score=3.0; q=second");
    }

    #[test]
    fn switch_legacy_python_dialect_matches_routing_and_nil_semantics() {
        let params = json!({
            "conditions": [
                {
                    "logical_operator": "and",
                    "items": [{"cpn_id": "", "operator": "=", "value": "wrong"}],
                    "to": ["wrong"]
                },
                {
                    "logical_operator": "and",
                    "items": [{"cpn_id": "sys.answer", "operator": "contains", "value": null}],
                    "to": ["matched"]
                }
            ],
            "end_cpn_ids": ["fallback"]
        });
        let (next, outputs) =
            run_switch(params, [("sys.answer", Value::String("foobar".into()))]).unwrap();
        assert_eq!(next, ["matched"]);
        assert_eq!(outputs["_next"], json!(["matched"]));
        assert_eq!(outputs["next"], json!(["matched"]));

        let params = json!({
            "conditions": [{
                "logical_operator": "and",
                "items": [{"cpn_id": "sys.answer", "operator": "contains", "value": "foo"}],
                "to": ["wrong"]
            }],
            "end_cpn_ids": ["fallback"]
        });
        let (next, outputs) = run_switch(params, [("sys.answer", Value::Null)]).unwrap();
        assert_eq!(next, ["fallback"]);
        assert_eq!(outputs["next"], json!(["fallback"]));
    }

    #[test]
    fn switch_legacy_python_dialect_coerces_numeric_operands_and_uses_or_fallback() {
        let params = json!({
            "conditions": [{
                "logical_operator": "xor",
                "items": [
                    {"cpn_id": "sys.score", "operator": "=", "value": "3.0"},
                    {"cpn_id": "sys.score", "operator": ">", "value": "9"}
                ],
                "to": ["numeric"]
            }],
            "end_cpn_ids": ["fallback"]
        });
        let (next, _) = run_switch(params, [("sys.score", json!(3))]).unwrap();
        assert_eq!(next, ["numeric"]);

        let params = json!({
            "conditions": [{
                "logical_operator": "and",
                "items": [],
                "to": ["wrong"]
            }],
            "end_cpn_ids": ["fallback"]
        });
        let (next, _) = run_switch(params, std::iter::empty::<(&'static str, Value)>()).unwrap();
        assert_eq!(next, ["fallback"]);
    }

    #[test]
    fn switch_modern_go_dialect_supports_clauses_defaults_and_matched_targets() {
        let params = json!({
            "conditions": [
                {
                    "op": "and",
                    "clauses": [{"left": "{{sys.flag}}", "op": "==", "right": "YES"}],
                    "to": ["first", "also-first"]
                }
            ],
            "default": "fallback"
        });
        let (next, outputs) =
            run_switch(params, [("sys.flag", Value::String("yes".into()))]).unwrap();
        assert_eq!(next, ["first", "also-first"]);
        assert_eq!(outputs, Map::from_iter([("_next".into(), json!(next))]));

        let params = json!({
            "conditions": [{
                "op": "and",
                "clauses": [{"left": "{{sys.score}}", "op": ">=", "right": 5}]
            }],
            "default": "fallback"
        });
        let (next, _) = run_switch(params, [("sys.score", json!(5))]).unwrap();
        assert_eq!(next, ["matched_0"]);

        let params = json!({
            "conditions": [{
                "logical_operator": "and",
                "items": [{"cpn_id": "sys.flag", "operator": "=", "value": "YES"}],
                "to": "normalized-legacy"
            }],
            "default": "fallback"
        });
        let (next, _) = run_switch(params, [("sys.flag", Value::String("yes".into()))]).unwrap();
        assert_eq!(next, ["normalized-legacy"]);
    }

    #[test]
    fn switch_modern_go_dialect_matches_nil_empty_and_unresolved_reference_edges() {
        let params = json!({
            "conditions": [
                {
                    "op": "and",
                    "clauses": [{"left": "{{sys.answer}}", "op": "start with", "right": null}],
                    "to": "nil-match"
                }
            ],
            "default": "fallback"
        });
        let (next, _) = run_switch(params, [("sys.answer", Value::Null)]).unwrap();
        assert_eq!(next, ["nil-match"]);

        let params = json!({
            "conditions": [{
                "op": "and",
                "clauses": [{"left": "{{sys.absent}}", "op": "not empty"}],
                "to": "raw-reference"
            }],
            "default": "fallback"
        });
        let (next, _) = run_switch(params, std::iter::empty::<(&'static str, Value)>()).unwrap();
        assert_eq!(next, ["raw-reference"]);
        assert!(!modern_switch_empty(&Value::Bool(false)));
        assert!(!modern_switch_empty(&json!(0)));

        let params = json!({
            "conditions": [{
                "logical_operator": "and",
                "items": [{"cpn_id": "sys.flag", "operator": "empty"}],
                "to": "wrong"
            }],
            "default": "fallback"
        });
        let (next, _) = run_switch(params, [("sys.flag", Value::Bool(false))]).unwrap();
        assert_eq!(next, ["fallback"]);
    }

    #[tokio::test]
    async fn switch_executes_every_selected_declared_branch_without_fallback() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["switch"], "upstream": []
            },
            "switch": {
                "obj": {"component_name": "Switch", "params": {
                    "conditions": [{
                        "op": "and",
                        "clauses": [{
                            "left": "{{sys.query}}",
                            "op": "contains",
                            "right": "multi"
                        }],
                        "to": ["left", "right"]
                    }],
                    "default": "fallback"
                }},
                "downstream": ["left", "right", "fallback"], "upstream": ["begin"]
            },
            "left": {
                "obj": {"component_name": "Message", "params": {"content": ["left"]}},
                "downstream": [], "upstream": ["switch"]
            },
            "right": {
                "obj": {"component_name": "Message", "params": {"content": ["right"]}},
                "downstream": [], "upstream": ["switch"]
            },
            "fallback": {
                "obj": {"component_name": "Message", "params": {"content": ["fallback"]}},
                "downstream": [], "upstream": ["switch"]
            }
        }});
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "take the multi branch",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();

        assert_eq!(result.path, ["begin", "switch", "left", "right"]);
        assert_eq!(result.answer, "right");
        assert_eq!(
            result
                .trace
                .iter()
                .map(|entry| entry.component_id.as_str())
                .collect::<Vec<_>>(),
            ["begin", "switch", "left", "right"]
        );
        assert!(
            result
                .trace
                .iter()
                .all(|entry| entry.component_id != "fallback")
        );
    }

    #[tokio::test]
    async fn scheduler_waits_for_uneven_diamond_and_executes_the_join_once() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["short", "long"], "upstream": []
            },
            "short": {
                "obj": {"component_name": "Message", "params": {"content": ["short"]}},
                "downstream": ["join"], "upstream": ["begin"]
            },
            "long": {
                "obj": {"component_name": "Message", "params": {"content": ["long"]}},
                "downstream": ["middle"], "upstream": ["begin"]
            },
            "middle": {
                "obj": {"component_name": "Message", "params": {"content": ["middle"]}},
                "downstream": ["join"], "upstream": ["long"]
            },
            "join": {
                "obj": {"component_name": "Message", "params": {"content": ["joined"]}},
                "downstream": [], "upstream": ["short", "middle"]
            }
        }});
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "run",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();

        assert_eq!(result.path, ["begin", "short", "long", "middle", "join"]);
        assert_eq!(result.answer, "joined");
        assert_eq!(
            result
                .trace
                .iter()
                .filter(|entry| entry.component_id == "join")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn string_transform_and_aggregator_keep_native_values() {
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["split"], "upstream": []},
            "split": {"obj": {"component_name": "StringTransform", "params": {"method": "split", "split_ref": "sys.query", "delimiters": [","]}}, "downstream": ["aggregate"], "upstream": ["begin"]},
            "aggregate": {"obj": {"component_name": "VariableAggregator", "params": {"groups": [{"group_name": "picked", "variables": [{"value": "split@result"}]}]}}, "downstream": ["message"], "upstream": ["split"]},
            "message": {"obj": {"component_name": "Message", "params": {"content": ["{aggregate@picked.1}"]}}, "downstream": [], "upstream": ["aggregate"]}
        }});
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "a,b,c",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "b");
    }

    #[tokio::test]
    async fn begin_maps_question_to_a_single_declared_input() {
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {
                "mode": "task",
                "inputs": {"customer_review": {"type": "line", "value": ""}}
            }}, "downstream": ["message"], "upstream": []},
            "message": {"obj": {"component_name": "Message", "params": {
                "content": ["review={begin@customer_review}"]
            }}, "downstream": [], "upstream": ["begin"]}
        }});
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "excellent service",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "review=excellent service");
        assert_eq!(
            result.trace[0].outputs["customer_review"],
            "excellent service"
        );
    }

    #[tokio::test]
    async fn begin_decodes_object_descriptors_and_rejects_file_inputs() {
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {
                "mode": "Webhook",
                "inputs": {"payload": {"type": "object", "value": ""}}
            }}, "downstream": ["message"], "upstream": []},
            "message": {"obj": {"component_name": "Message", "params": {
                "content": ["name={begin@payload.name}"]
            }}, "downstream": [], "upstream": ["begin"]}
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let inputs = Map::from_iter([(
            "payload".into(),
            json!({"type": "object", "value": "{\"name\":\"Ada\"}"}),
        )]);
        let result = workflow
            .run(
                None,
                WorkflowRunInput {
                    question: "ignored",
                    user_id: "user-1",
                    inputs: &inputs,
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "name=Ada");

        let error = resolve_begin_input(
            &workflow.nodes["begin"],
            "attachment",
            json!({"type": "file", "value": "file-id"}),
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requires the file parsing service")
        );
        assert_eq!(
            resolve_begin_input(
                &workflow.nodes["begin"],
                "attachment",
                json!({"type": "file", "optional": true, "value": null}),
            )
            .unwrap(),
            Value::Null
        );
    }

    #[test]
    fn begin_params_are_validated_when_canvas_is_compiled() {
        for params in [
            json!({"mode": "invalid"}),
            json!({"mode": 1}),
            json!({"inputs": []}),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": params}, "downstream": [], "upstream": []}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_err(), "{dsl}");
        }
    }

    #[test]
    fn llm_params_are_normalized_and_validated_when_canvas_is_compiled() {
        let canvas = |params: Value| {
            json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["llm"], "upstream": []},
                "llm": {"obj": {"component_name": "LLM", "params": params}, "downstream": ["message"], "upstream": ["begin"]},
                "message": {"obj": {"component_name": "Message", "params": {"content": ["{llm@content}"]}}, "downstream": [], "upstream": ["llm"]}
            }})
        };
        AgentWorkflow::from_value(&canvas(json!({
            "llm_id": "deepseek-chat",
            "max_tokens": 0,
            "maxTokensEnabled": true
        })))
        .unwrap()
        .unwrap();
        for params in [
            json!({"prompts": []}),
            json!({"prompts": [1]}),
            json!({"prompts": [{"role": "user", "content": 1}]}),
            json!({"message_history_window_size": -1}),
            json!({"max_retries": 1.5}),
            json!({"delay_after_error": -1}),
            json!({"temperature": 1.1}),
            json!({"topPEnabled": "yes"}),
            json!({"max_tokens": 128001}),
        ] {
            assert!(
                AgentWorkflow::from_value(&canvas(params.clone())).is_err(),
                "{params}"
            );
        }
    }

    #[test]
    fn agent_tool_dsl_is_validated_and_indexed_like_the_fixed_python_component() {
        let canvas = |params: Value| {
            json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["agent"], "upstream": []},
                "agent": {"obj": {"component_name": "Agent", "params": params}, "downstream": [], "upstream": ["begin"]}
            }})
        };
        let dsl = canvas(json!({
            "llm_id": "deepseek-chat",
            "max_rounds": 2,
            "mcp": [],
            "tools": [
                {
                    "component_name": "Retrieval",
                    "name": "Knowledge Search",
                    "params": {"dataset_ids": ["kb-1"]}
                },
                {
                    "component_name": "TavilySearch",
                    "name": "Web Search",
                    "params": {"api_key": "configured", "search_depth": "advanced"}
                },
                {
                    "component_name": "TavilyExtract",
                    "name": "Web Extract",
                    "params": {"api_key": "configured", "format": "text"}
                },
                {
                    "component_name": "DuckDuckGo",
                    "name": "Privacy Search",
                    "params": {"channel": "text", "top_n": 6}
                },
                {
                    "component_name": "Wikipedia",
                    "name": "Encyclopedia",
                    "params": {"language": "zh", "top_n": 5}
                },
                {
                    "component_name": "GoogleScholar",
                    "name": "Scholar Search",
                    "params": {"sort_by": "date", "year_low": 2020, "year_high": 2026, "patents": false, "top_n": 6}
                },
                {
                    "component_name": "ArXiv",
                    "name": "Paper Search",
                    "params": {"sort_by": "relevance", "top_n": 7}
                },
                {
                    "component_name": "PubMed",
                    "name": "Biomedical Search",
                    "params": {"email": "reader@example.test", "top_n": 8}
                },
                {
                    "component_name": "GitHub",
                    "name": "Repository Search",
                    "params": {"top_n": 5}
                },
                {
                    "component_name": "YahooFinance",
                    "name": "Market Data",
                    "params": {"info": false, "balance_sheet": true, "news": false}
                }
            ]
        }));
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let tools = load_canvas_agent_tools(&workflow.nodes["agent"]).unwrap();
        assert_eq!(tools.len(), 10);
        assert_eq!(tools[0].function_name, "search_my_dateset_0");
        assert_eq!(tools[0].child_id, "agent-->Knowledge_Search");
        assert_eq!(
            tools[0].definition["function"]["parameters"]["required"],
            json!(["query"])
        );
        assert_eq!(tools[1].function_name, "tavily_search_1");
        assert_eq!(tools[1].child_id, "agent-->Web_Search");
        assert!(
            tools[1].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("Number of keywords in query should be less than 5")
        );
        assert_eq!(
            tools[1].definition["function"]["parameters"]["properties"]["topic"]["enum"],
            json!(["general", "news"])
        );
        assert_eq!(tools[2].function_name, "tavily_extract_2");
        assert!(
            tools[2].definition["function"]["parameters"]["properties"]["extract_depth"]
                ["description"]
                .as_str()
                .unwrap()
                .contains("2 credits per 5 successful URL extractions")
        );
        assert_eq!(
            tools[2].definition["function"]["parameters"]["required"],
            json!(["urls"])
        );
        assert_eq!(tools[3].function_name, "duckduckgo_search_3");
        assert_eq!(tools[3].child_id, "agent-->Privacy_Search");
        assert_eq!(
            tools[3].definition["function"]["parameters"]["properties"]["channel"]["enum"],
            json!(["general", "news"])
        );
        assert_eq!(
            tools[3].definition["function"]["parameters"]["required"],
            json!(["query"])
        );
        assert!(
            tools[3].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("focused on privacy")
        );
        assert_eq!(tools[4].function_name, "wikipedia_search_4");
        assert_eq!(tools[4].child_id, "agent-->Encyclopedia");
        assert_eq!(
            tools[4].definition["function"]["parameters"]["required"],
            json!(["query"])
        );
        assert_eq!(
            tools[4].definition["function"]["parameters"]["properties"]["query"]["description"],
            "The search keyword to execute with wikipedia. The keyword MUST be a specific subject that can match the title."
        );
        assert!(
            tools[4].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("world's largest reference website")
        );
        assert_eq!(tools[5].function_name, "google_scholar_search_5");
        assert_eq!(tools[5].child_id, "agent-->Scholar_Search");
        assert_eq!(
            tools[5].definition["function"]["parameters"]["required"],
            json!(["query"])
        );
        assert_eq!(
            tools[5].definition["function"]["parameters"]["properties"]["query"]["description"],
            "The search keyword to execute with Google Scholar. The keywords should be the most important words/terms(includes synonyms) from the original request."
        );
        assert!(
            tools[5].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("broadly search for scholarly literature")
        );
        assert_eq!(tools[6].function_name, "arxiv_search_6");
        assert_eq!(tools[6].child_id, "agent-->Paper_Search");
        assert_eq!(
            tools[6].definition["function"]["parameters"]["required"],
            json!(["query"])
        );
        assert_eq!(
            tools[6].definition["function"]["parameters"]["properties"]["query"]["description"],
            "The search keywords to execute with arXiv. The keywords should be the most important words/terms(includes synonyms) from the original request."
        );
        assert!(
            tools[6].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("not peer-reviewed by arXiv")
        );
        assert_eq!(tools[7].function_name, "pubmed_search_7");
        assert_eq!(tools[7].child_id, "agent-->Biomedical_Search");
        assert_eq!(
            tools[7].definition["function"]["parameters"]["required"],
            json!(["query"])
        );
        assert_eq!(
            tools[7].definition["function"]["parameters"]["properties"]["query"]["description"],
            "The search keywords to execute with PubMed. The keywords should be the most important words/terms(includes synonyms) from the original request."
        );
        assert!(
            tools[7].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("MEDLINE database")
        );
        assert_eq!(tools[8].function_name, "github_search_8");
        assert_eq!(tools[8].child_id, "agent-->Repository_Search");
        assert_eq!(
            tools[8].definition["function"]["parameters"]["required"],
            json!(["query"])
        );
        assert_eq!(
            tools[8].definition["function"]["parameters"]["properties"]["query"]["description"],
            "The search keywords to execute with GitHub. The keywords should be the most important words/terms(includes synonyms) from the original request."
        );
        assert!(
            tools[8].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("find specific repositories")
        );
        assert_eq!(tools[9].function_name, "yahoo_finance_9");
        assert_eq!(tools[9].child_id, "agent-->Market_Data");
        assert_eq!(
            tools[9].definition["function"]["parameters"]["required"],
            json!(["stock_code"])
        );
        assert_eq!(
            tools[9].definition["function"]["parameters"]["properties"]["stock_code"]["description"],
            "The stock code or company name."
        );
        assert!(
            tools[9].definition["function"]["description"]
                .as_str()
                .unwrap()
                .contains("real-time and historical stock market data")
        );

        for params in [
            json!({"max_rounds": -1}),
            json!({"tools": {}}),
            json!({"tools": [1]}),
            json!({"tools": [{"component_name": "Retrieval", "params": []}]}),
            json!({"tools": [{"component_name": "TavilySearch", "params": {"topic": "academic"}}]}),
            json!({"tools": [{"component_name": "TavilyExtract", "params": {"format": "html"}}]}),
            json!({"tools": [{"component_name": "DuckDuckGo", "params": {"channel": "images"}}]}),
            json!({"tools": [{"component_name": "Wikipedia", "params": {"language": "xx"}}]}),
            json!({"tools": [{"component_name": "GoogleScholar", "params": {"sort_by": "newest"}}]}),
            json!({"tools": [{"component_name": "GitHub", "params": {"top_n": 0}}]}),
            json!({"tools": [{"component_name": "YahooFinance", "params": {"news": "yes"}}]}),
            json!({"tools": [{"component_name": "ArXiv", "params": {"sort_by": "newest"}}]}),
            json!({"tools": [{"component_name": "PubMed", "params": {"email": 1}}]}),
            json!({"tools": [{"component_name": "Google", "params": {}}]}),
            json!({"mcp": [{"mcp_id": "server", "tools": {}}]}),
        ] {
            assert!(
                AgentWorkflow::from_value(&canvas(params.clone())).is_err(),
                "{params}"
            );
        }
    }

    #[tokio::test]
    async fn agent_tool_dispatch_returns_protocol_errors_for_unknown_or_non_object_calls() {
        let node = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "Retrieval",
                    "name": "Search",
                    "params": {"dataset_ids": ["kb"]}
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&node).unwrap();
        let retriever = MockRetriever::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "q",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: Some(&retriever),
            fallback_kb_ids: &[],
        };
        let call = |name: &str, arguments: &str| ToolCall {
            id: "call".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        };
        let mut runtime = CanvasRuntime::default();
        let unknown = execute_canvas_agent_tool(
            &mut runtime,
            &node,
            &tools,
            &call("hallucinated", "{}"),
            &run_input,
        )
        .await
        .unwrap_err();
        assert_eq!(unknown.to_string(), "LLM tool hallucinated does not exist");
        let malformed = execute_canvas_agent_tool(
            &mut runtime,
            &node,
            &tools,
            &call("search_my_dateset_0", "[]"),
            &run_input,
        )
        .await
        .unwrap_err();
        assert!(malformed.to_string().contains("must be a JSON object"));
        assert_eq!(retriever.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn tavily_search_retries_forces_bounded_fields_and_adds_web_references() {
        let node = CanvasNode {
            id: "search".into(),
            component_name: "TavilySearch".into(),
            params: serde_json::from_value(json!({
                "api_key": "static-key",
                "query": "sys.query",
                "search_depth": "advanced",
                "topic": "news",
                "max_results": 3,
                "days": 7,
                "include_answer": true,
                "include_raw_content": true,
                "include_images": true,
                "include_image_descriptions": true,
                "include_domains": ["rust-lang.org", {"value": "docs.rs"}],
                "exclude_domains": ["spam.test"],
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_tavily_search_params(&node).unwrap();
        let provider = MockTavily::default();
        provider
            .search_failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("rust rag"))]),
            ..CanvasRuntime::default()
        };
        execute_tavily_search_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        let searches = provider.searches.lock().unwrap();
        assert_eq!(searches.len(), 2);
        assert_eq!(searches[0].0, "static-key");
        let request = &searches[0].1;
        assert_eq!(request.query, "rust rag");
        assert_eq!(request.include_domains, ["rust-lang.org", "docs.rs"]);
        assert!(request.include_answer);
        assert!(request.include_image_descriptions);
        assert!(!request.include_raw_content);
        assert!(!request.include_images);
        drop(searches);

        let outputs = &runtime.outputs["search"];
        assert_eq!(outputs["json"][0]["title"], "Rust\nGuide");
        assert_eq!(runtime.retrieval["chunks"][0]["content"], "alpha  beta");
        assert!(
            outputs["formalized_content"]
                .as_str()
                .unwrap()
                .contains("├── Title: Rust Guide")
        );
        assert_eq!(runtime.references.len(), 1);
        assert_eq!(
            runtime.references[0].id,
            runtime.retrieval["chunks"][0]["chunk_id"].as_str().unwrap()
        );
        assert_eq!(runtime.references[0].kb_id, runtime.references[0].id);
    }

    #[tokio::test]
    async fn tavily_extract_agent_dispatch_accepts_csv_urls_and_returns_raw_results() {
        let agent = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "TavilyExtract",
                    "name": "Open Pages",
                    "params": {
                        "api_key": "static-key",
                        "extract_depth": "advanced",
                        "format": "text",
                        "max_retries": 1
                    }
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&agent).unwrap();
        let provider = MockTavily::default();
        provider
            .extract_failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "q",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let call = ToolCall {
            id: "call-extract".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: "tavily_extract_0".into(),
                arguments: json!({
                    "urls": "https://example.test/a, https://example.test/b"
                })
                .to_string(),
            },
        };
        let mut runtime = CanvasRuntime::default();
        let response = execute_canvas_agent_tool_with_tavily(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            &provider,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&response).unwrap(),
            json!([{"url": "https://example.test/a", "raw_content": "page body"}])
        );
        let extracts = provider.extracts.lock().unwrap();
        assert_eq!(extracts.len(), 2);
        assert_eq!(
            extracts[0].1.urls,
            ["https://example.test/a", " https://example.test/b"]
        );
        assert_eq!(extracts[0].1.extract_depth, "advanced");
        assert_eq!(extracts[0].1.format, "text");
        assert!(!extracts[0].1.include_images);
        assert_eq!(
            runtime.outputs["agent-->Open_Pages"]["json"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn duckduckgo_search_retries_honors_topic_alias_and_adds_references() {
        let node = CanvasNode {
            id: "duck".into(),
            component_name: "DuckDuckGo".into(),
            params: serde_json::from_value(json!({
                "query": "sys.query",
                "channel": "text",
                "topic": "news",
                "top_n": 3,
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_duckduckgo_params(&node).unwrap();
        let provider = MockDuckDuckGo::default();
        provider
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("rust search"))]),
            ..CanvasRuntime::default()
        };
        execute_duckduckgo_search_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        {
            let searches = provider.searches.lock().unwrap();
            assert_eq!(searches.len(), 2);
            assert_eq!(searches[0].query, "rust search");
            assert_eq!(searches[0].channel, DuckDuckGoChannel::News);
            assert_eq!(searches[0].top_n, 3);
        }
        let outputs = &runtime.outputs["duck"];
        assert_eq!(outputs["json"][0]["title"], "Rust\nSearch");
        assert_eq!(outputs["json"][0]["url"], "https://example.test/rust");
        assert_eq!(runtime.retrieval["chunks"][0]["content"], "alpha  beta");
        assert_eq!(runtime.retrieval["chunks"][0]["similarity"], 1);
        assert!(
            outputs["formalized_content"]
                .as_str()
                .unwrap()
                .contains("├── Title: Rust Search")
        );
        assert_eq!(runtime.references.len(), 1);
        assert_eq!(runtime.references[0].similarity, Some(1.0));

        let empty = CanvasNode {
            id: "empty-duck".into(),
            params: Map::from_iter([("query".into(), json!(""))]),
            ..node
        };
        execute_duckduckgo_search_with_provider(&mut runtime, &empty, &provider)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["empty-duck"]["formalized_content"], "");
        assert_eq!(provider.searches.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn duckduckgo_agent_dispatch_maps_general_news_and_preserves_error_prefix() {
        let agent = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "DuckDuckGo",
                    "name": "Privacy Search",
                    "params": {
                        "channel": "text",
                        "top_n": 4,
                        "max_retries": 0,
                        "delay_after_error": 0
                    }
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&agent).unwrap();
        let tavily = MockTavily::default();
        let wikipedia = MockWikipedia::default();
        let duckduckgo = MockDuckDuckGo::default();
        let google = GoogleClient::default();
        let google_scholar = GoogleScholarClient::default();
        let arxiv = ArxivClient::default();
        let pubmed = PubMedClient::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "ignored",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let call = ToolCall {
            id: "call-duck".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: "duckduckgo_search_0".into(),
                arguments: json!({"query": "Rust", "channel": "news"}).to_string(),
            },
        };
        let mut runtime = CanvasRuntime::default();
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert!(response.contains("├── Title: Rust Search"));
        {
            let searches = duckduckgo.searches.lock().unwrap();
            assert_eq!(searches[0].query, "Rust");
            assert_eq!(searches[0].channel, DuckDuckGoChannel::News);
            assert_eq!(searches[0].top_n, 4);
        }

        duckduckgo
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert_eq!(response, "DuckDuckGo error: temporary DuckDuckGo failure");
    }

    #[tokio::test]
    async fn wikipedia_search_retries_and_publishes_python_chunks_plus_go_json() {
        let node = CanvasNode {
            id: "wiki".into(),
            component_name: "Wikipedia".into(),
            params: serde_json::from_value(json!({
                "query": "sys.query",
                "language": "zh",
                "top_n": 2,
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_wikipedia_params(&node).unwrap();
        let provider = MockWikipedia::default();
        provider
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("rust language"))]),
            ..CanvasRuntime::default()
        };
        execute_wikipedia_search_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        {
            let searches = provider.searches.lock().unwrap();
            assert_eq!(searches.len(), 2);
            assert_eq!(searches[0].query, "rust language");
            assert_eq!(searches[0].language, "zh");
            assert_eq!(searches[0].top_n, 2);
        }
        let outputs = &runtime.outputs["wiki"];
        assert_eq!(outputs["json"]["results"].as_array().unwrap().len(), 2);
        assert_eq!(
            outputs["json"]["results"][0],
            json!({
                "title": "Rust\nLanguage",
                "snippet": "<span>Rust</span> language",
                "url": "https://en.wikipedia.org/wiki/Rust_(programming_language)"
            })
        );
        assert_eq!(runtime.retrieval["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(runtime.retrieval["chunks"][0]["content"], "alpha  beta");
        assert_eq!(runtime.retrieval["chunks"][0]["similarity"], 1);
        assert!(
            outputs["formalized_content"]
                .as_str()
                .unwrap()
                .contains("├── Title: Rust Language")
        );
        assert_eq!(runtime.references.len(), 1);
        assert_eq!(runtime.references[0].similarity, Some(1.0));

        let empty = CanvasNode {
            id: "empty-wiki".into(),
            params: Map::from_iter([("query".into(), json!(""))]),
            ..node
        };
        execute_wikipedia_search_with_provider(&mut runtime, &empty, &provider)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["empty-wiki"]["formalized_content"], "");
        assert_eq!(provider.searches.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn wikipedia_agent_dispatch_uses_child_params_and_preserves_error_prefix() {
        let agent = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "Wikipedia",
                    "name": "Encyclopedia",
                    "params": {
                        "language": "fr",
                        "top_n": 4,
                        "max_retries": 0,
                        "delay_after_error": 0
                    }
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&agent).unwrap();
        let tavily = MockTavily::default();
        let wikipedia = MockWikipedia::default();
        let duckduckgo = MockDuckDuckGo::default();
        let google = GoogleClient::default();
        let google_scholar = GoogleScholarClient::default();
        let arxiv = ArxivClient::default();
        let pubmed = PubMedClient::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "ignored",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let call = ToolCall {
            id: "call-wikipedia".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: "wikipedia_search_0".into(),
                arguments: json!({"query": "Paris"}).to_string(),
            },
        };
        let mut runtime = CanvasRuntime::default();
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert!(response.contains("├── Title: Rust Language"));
        {
            let searches = wikipedia.searches.lock().unwrap();
            assert_eq!(searches[0].query, "Paris");
            assert_eq!(searches[0].language, "fr");
            assert_eq!(searches[0].top_n, 4);
        }

        wikipedia
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert_eq!(response, "Wikipedia error: temporary Wikipedia failure");
    }

    #[tokio::test]
    async fn google_search_retries_and_publishes_python_chunks_and_raw_json() {
        let node = CanvasNode {
            id: "google".into(),
            component_name: "Google".into(),
            params: serde_json::from_value(json!({
                "q": "sys.query",
                "start": 20,
                "num": 12,
                "api_key": "serp-secret",
                "country": "us",
                "language": "en",
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_google_params(&node).unwrap();
        let provider = MockGoogle::default();
        provider
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("rust google"))]),
            ..CanvasRuntime::default()
        };
        execute_google_search_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        {
            let searches = provider.searches.lock().unwrap();
            assert_eq!(searches.len(), 2);
            assert_eq!(searches[0].0, "serp-secret");
            assert_eq!(searches[0].1.query, "rust google");
            assert_eq!(searches[0].1.country, "us");
            assert_eq!(searches[0].1.language, "en");
        }
        let outputs = &runtime.outputs["google"];
        assert_eq!(outputs["json"][0]["position"], 1);
        assert_eq!(runtime.retrieval["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(runtime.retrieval["chunks"][0]["content"], "alpha  beta");
        assert!(
            outputs["formalized_content"]
                .as_str()
                .unwrap()
                .contains("├── Title: Rust Search")
        );
        assert_eq!(runtime.references.len(), 1);

        let empty = CanvasNode {
            id: "empty-google".into(),
            params: Map::from_iter([
                ("q".into(), json!("")),
                ("api_key".into(), json!("serp-secret")),
            ]),
            ..node
        };
        execute_google_search_with_provider(&mut runtime, &empty, &provider)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["empty-google"]["formalized_content"], "");
        assert!(runtime.outputs["empty-google"].get("json").is_none());
        assert_eq!(provider.searches.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn google_agent_metadata_dispatch_and_error_prefix_match_python_tool() {
        let agent = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "Google",
                    "name": "Web Search",
                    "params": {
                        "api_key": "agent-secret",
                        "country": "gb",
                        "language": "en",
                        "max_retries": 0,
                        "delay_after_error": 0
                    }
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&agent).unwrap();
        assert_eq!(tools[0].function_name, "google_search_0");
        assert_eq!(
            tools[0].definition["function"]["parameters"]["required"],
            json!(["q"])
        );
        assert_eq!(
            tools[0].definition["function"]["parameters"]["properties"]["start"]["type"],
            "integer"
        );

        let tavily = MockTavily::default();
        let wikipedia = MockWikipedia::default();
        let duckduckgo = MockDuckDuckGo::default();
        let google = MockGoogle::default();
        let google_scholar = GoogleScholarClient::default();
        let github = GitHubClient::default();
        let arxiv = ArxivClient::default();
        let pubmed = PubMedClient::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "ignored",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let call = ToolCall {
            id: "call-google".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: "google_search_0".into(),
                arguments: json!({"q": "RAG", "start": 40, "num": 100}).to_string(),
            },
        };
        let providers = CanvasAgentToolProviders {
            tavily: &tavily,
            wikipedia: &wikipedia,
            duckduckgo: &duckduckgo,
            google: &google,
            google_scholar: &google_scholar,
            github: &github,
            arxiv: &arxiv,
            pubmed: &pubmed,
        };
        let mut runtime = CanvasRuntime::default();
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            providers,
        )
        .await
        .unwrap();
        assert!(response.contains("├── Title: Rust Search"));
        {
            let searches = google.searches.lock().unwrap();
            assert_eq!(searches[0].0, "agent-secret");
            assert_eq!(searches[0].1.query, "RAG");
            assert_eq!(searches[0].1.country, "gb");
        }

        google
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            providers,
        )
        .await
        .unwrap();
        assert_eq!(response, "Google error: temporary Google failure");
    }

    #[tokio::test]
    async fn google_scholar_search_retries_and_publishes_python_chunks_and_json() {
        let node = CanvasNode {
            id: "scholar".into(),
            component_name: "GoogleScholar".into(),
            params: serde_json::from_value(json!({
                "query": "sys.query",
                "top_n": 3,
                "sort_by": "date",
                "year_low": 2020,
                "year_high": 2026,
                "patents": false,
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_google_scholar_params(&node).unwrap();
        let provider = MockGoogleScholar::default();
        provider
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("rust scholar"))]),
            ..CanvasRuntime::default()
        };
        execute_google_scholar_search_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        {
            let searches = provider.searches.lock().unwrap();
            assert_eq!(searches.len(), 2);
            assert_eq!(searches[0].query, "rust scholar");
            assert_eq!(searches[0].top_n, 3);
            assert_eq!(searches[0].sort_by, GoogleScholarSortBy::Date);
            assert_eq!(searches[0].year_low, Some(2020));
            assert_eq!(searches[0].year_high, Some(2026));
            assert!(!searches[0].patents);
        }
        let outputs = &runtime.outputs["scholar"];
        assert_eq!(outputs["json"].as_array().unwrap().len(), 1);
        assert_eq!(outputs["json"][0]["source"], "PUBLICATION_SEARCH_SNIPPET");
        assert_eq!(outputs["json"][0]["bib"]["author"], json!(["Alice", "Bob"]));
        assert_eq!(runtime.retrieval["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(
            runtime.retrieval["chunks"][0]["content"],
            "\n author: Alice,Bob\n Abstract: alpha  beta"
        );
        assert!(
            outputs["formalized_content"]
                .as_str()
                .unwrap()
                .contains("├── Title: Rust Scholarship")
        );
        assert_eq!(runtime.references.len(), 1);

        let empty = CanvasNode {
            id: "empty-scholar".into(),
            params: Map::from_iter([("query".into(), json!(""))]),
            ..node
        };
        execute_google_scholar_search_with_provider(&mut runtime, &empty, &provider)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["empty-scholar"]["formalized_content"], "");
        assert_eq!(runtime.outputs["empty-scholar"]["json"], json!([]));
        assert_eq!(provider.searches.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn google_scholar_agent_dispatch_uses_child_params_and_error_prefix() {
        let agent = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "GoogleScholar",
                    "name": "Scholar Search",
                    "params": {
                        "sort_by": "relevance",
                        "year_low": 2021,
                        "year_high": null,
                        "patents": true,
                        "top_n": 4,
                        "max_retries": 0,
                        "delay_after_error": 0
                    }
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&agent).unwrap();
        let tavily = MockTavily::default();
        let wikipedia = MockWikipedia::default();
        let duckduckgo = MockDuckDuckGo::default();
        let google = GoogleClient::default();
        let google_scholar = MockGoogleScholar::default();
        let arxiv = ArxivClient::default();
        let pubmed = PubMedClient::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "ignored",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let call = ToolCall {
            id: "call-scholar".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: "google_scholar_search_0".into(),
                arguments: json!({"query": "RAG"}).to_string(),
            },
        };
        let mut runtime = CanvasRuntime::default();
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert!(response.contains("├── Title: Rust Scholarship"));
        {
            let searches = google_scholar.searches.lock().unwrap();
            assert_eq!(searches[0].query, "RAG");
            assert_eq!(searches[0].top_n, 4);
            assert_eq!(searches[0].sort_by, GoogleScholarSortBy::Relevance);
            assert_eq!(searches[0].year_low, Some(2021));
            assert_eq!(searches[0].year_high, None);
            assert!(searches[0].patents);
        }

        google_scholar
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            response,
            "GoogleScholar error: temporary Google Scholar failure"
        );
    }

    #[tokio::test]
    async fn github_search_retries_and_publishes_python_chunks_and_raw_items() {
        let node = CanvasNode {
            id: "github".into(),
            component_name: "GitHub".into(),
            params: serde_json::from_value(json!({
                "query": "sys.query",
                "top_n": 5,
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_github_params(&node).unwrap();
        let provider = MockGitHub::default();
        provider
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("rust rag"))]),
            ..CanvasRuntime::default()
        };
        execute_github_search_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        {
            let searches = provider.searches.lock().unwrap();
            assert_eq!(searches.len(), 2);
            assert_eq!(searches[0].query, "rust rag");
            assert_eq!(searches[0].top_n, 5);
        }
        let outputs = &runtime.outputs["github"];
        assert_eq!(outputs["json"][0]["extra"], "preserved");
        assert_eq!(runtime.retrieval["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(
            runtime.retrieval["chunks"][0]["content"],
            "alpha  beta\n stars:42"
        );
        assert!(
            outputs["formalized_content"]
                .as_str()
                .unwrap()
                .contains("├── Title: rayrag repository")
        );
        assert_eq!(runtime.references.len(), 1);

        let empty = CanvasNode {
            id: "empty-github".into(),
            params: Map::from_iter([("query".into(), json!(""))]),
            ..node
        };
        execute_github_search_with_provider(&mut runtime, &empty, &provider)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["empty-github"]["formalized_content"], "");
        assert!(runtime.outputs["empty-github"].get("json").is_none());
        assert_eq!(provider.searches.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn yahoo_finance_retries_resolves_stock_code_and_preserves_hidden_flags() {
        let node = CanvasNode {
            id: "yahoo".into(),
            component_name: "YahooFinance".into(),
            params: serde_json::from_value(json!({
                "stock_code": "sys.query",
                "info": false,
                "history": true,
                "count": true,
                "financials": true,
                "income_stmt": true,
                "balance_sheet": true,
                "cash_flow_statement": true,
                "news": false,
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_yahoo_finance_params(&node).unwrap();
        let provider = MockYahooFinance::default();
        provider
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("0005.HK"))]),
            ..CanvasRuntime::default()
        };
        execute_yahoo_finance_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        {
            let reports = provider.reports.lock().unwrap();
            assert_eq!(reports.len(), 2);
            assert_eq!(reports[0].stock_code, "0005.HK");
            assert!(!reports[0].info);
            assert!(reports[0].history);
            assert!(reports[0].count);
            assert!(reports[0].financials);
            assert!(reports[0].income_stmt);
            assert!(reports[0].balance_sheet);
            assert!(reports[0].cash_flow_statement);
            assert!(!reports[0].news);
        }
        assert_eq!(
            runtime.outputs["yahoo"]["report"],
            "# Information:\n0005.HK"
        );

        let empty = CanvasNode {
            id: "empty-yahoo".into(),
            params: Map::from_iter([("stock_code".into(), json!(""))]),
            ..node
        };
        execute_yahoo_finance_with_provider(&mut runtime, &empty, &provider)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["empty-yahoo"]["report"], "");
        assert_eq!(provider.reports.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn github_agent_dispatch_uses_frontend_limit_and_error_prefix() {
        let agent = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "GitHub",
                    "name": "Repository Search",
                    "params": {
                        "top_n": 5,
                        "max_retries": 0,
                        "delay_after_error": 0
                    }
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&agent).unwrap();
        let tavily = MockTavily::default();
        let wikipedia = MockWikipedia::default();
        let duckduckgo = MockDuckDuckGo::default();
        let google = GoogleClient::default();
        let google_scholar = GoogleScholarClient::default();
        let github = MockGitHub::default();
        let arxiv = ArxivClient::default();
        let pubmed = PubMedClient::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "ignored",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let call = ToolCall {
            id: "call-github".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: "github_search_0".into(),
                arguments: json!({"query": "RayRAG"}).to_string(),
            },
        };
        let mut runtime = CanvasRuntime::default();
        let providers = CanvasAgentToolProviders {
            tavily: &tavily,
            wikipedia: &wikipedia,
            duckduckgo: &duckduckgo,
            google: &google,
            google_scholar: &google_scholar,
            github: &github,
            arxiv: &arxiv,
            pubmed: &pubmed,
        };
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            providers,
        )
        .await
        .unwrap();
        assert!(response.contains("├── Title: rayrag repository"));
        {
            let searches = github.searches.lock().unwrap();
            assert_eq!(searches[0].query, "RayRAG");
            assert_eq!(searches[0].top_n, 5);
        }

        github
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            providers,
        )
        .await
        .unwrap();
        assert_eq!(response, "GitHub error: temporary GitHub failure");
    }

    #[tokio::test]
    async fn arxiv_search_retries_and_publishes_python_chunks_plus_go_json() {
        let node = CanvasNode {
            id: "arxiv".into(),
            component_name: "ArXiv".into(),
            params: serde_json::from_value(json!({
                "query": "sys.query",
                "top_n": 3,
                "sort_by": "lastUpdatedDate",
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_arxiv_params(&node).unwrap();
        let provider = MockArxiv::default();
        provider
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("rust retrieval"))]),
            ..CanvasRuntime::default()
        };
        execute_arxiv_search_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        {
            let searches = provider.searches.lock().unwrap();
            assert_eq!(searches.len(), 2);
            assert_eq!(searches[0].query, "rust retrieval");
            assert_eq!(searches[0].top_n, 3);
            assert_eq!(searches[0].sort_by, ArxivSortBy::LastUpdatedDate);
        }
        let outputs = &runtime.outputs["arxiv"];
        assert_eq!(outputs["json"]["results"].as_array().unwrap().len(), 2);
        assert_eq!(
            outputs["json"]["results"][0]["authors"],
            json!(["Alice", "Bob"])
        );
        assert_eq!(
            outputs["json"]["results"][1]["pdf_url"],
            "http://arxiv.org/pdf/2409.99999v2"
        );
        assert_eq!(runtime.retrieval["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(runtime.retrieval["chunks"][0]["content"], "alpha  beta");
        assert_eq!(runtime.retrieval["chunks"][0]["similarity"], 1);
        assert!(
            outputs["formalized_content"]
                .as_str()
                .unwrap()
                .contains("├── Title: Rust Retrieval")
        );
        assert_eq!(runtime.references.len(), 1);
        assert_eq!(runtime.references[0].similarity, Some(1.0));

        let empty = CanvasNode {
            id: "empty-arxiv".into(),
            params: Map::from_iter([("query".into(), json!(""))]),
            ..node
        };
        execute_arxiv_search_with_provider(&mut runtime, &empty, &provider)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["empty-arxiv"]["formalized_content"], "");
        assert_eq!(provider.searches.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn arxiv_agent_dispatch_uses_child_params_and_preserves_error_prefix() {
        let agent = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "ArXiv",
                    "name": "Paper Search",
                    "params": {
                        "sort_by": "relevance",
                        "top_n": 4,
                        "max_retries": 0,
                        "delay_after_error": 0
                    }
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&agent).unwrap();
        let tavily = MockTavily::default();
        let wikipedia = MockWikipedia::default();
        let duckduckgo = MockDuckDuckGo::default();
        let google = GoogleClient::default();
        let google_scholar = GoogleScholarClient::default();
        let arxiv = MockArxiv::default();
        let pubmed = PubMedClient::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "ignored",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let call = ToolCall {
            id: "call-arxiv".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: "arxiv_search_0".into(),
                arguments: json!({"query": "RAG"}).to_string(),
            },
        };
        let mut runtime = CanvasRuntime::default();
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert!(response.contains("├── Title: Rust Retrieval"));
        {
            let searches = arxiv.searches.lock().unwrap();
            assert_eq!(searches[0].query, "RAG");
            assert_eq!(searches[0].top_n, 4);
            assert_eq!(searches[0].sort_by, ArxivSortBy::Relevance);
        }

        arxiv.failures.store(1, std::sync::atomic::Ordering::SeqCst);
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert_eq!(response, "ArXiv error: temporary ArXiv failure");
    }

    #[tokio::test]
    async fn pubmed_search_retries_and_publishes_python_chunks_plus_go_json() {
        let node = CanvasNode {
            id: "pubmed".into(),
            component_name: "PubMed".into(),
            params: serde_json::from_value(json!({
                "query": "sys.query",
                "top_n": 3,
                "email": "reader@example.test",
                "max_retries": 1,
                "delay_after_error": 0
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        validate_pubmed_params(&node).unwrap();
        let provider = MockPubMed::default();
        provider
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let mut runtime = CanvasRuntime {
            sys: Map::from_iter([("query".into(), json!("rust medicine"))]),
            ..CanvasRuntime::default()
        };
        execute_pubmed_search_with_provider(&mut runtime, &node, &provider)
            .await
            .unwrap();

        {
            let searches = provider.searches.lock().unwrap();
            assert_eq!(searches.len(), 2);
            assert_eq!(searches[0].query, "rust medicine");
            assert_eq!(searches[0].top_n, 3);
            assert_eq!(searches[0].email, "reader@example.test");
        }
        let outputs = &runtime.outputs["pubmed"];
        assert_eq!(outputs["json"]["results"][0]["pmid"], "31415926");
        assert_eq!(outputs["json"]["results"][0]["year"], "2024");
        assert_eq!(
            outputs["json"]["results"][0]["authors"],
            "Alice Smith, Bob Jones"
        );
        assert_eq!(runtime.retrieval["chunks"].as_array().unwrap().len(), 1);
        assert_eq!(runtime.retrieval["chunks"][0]["similarity"], 1);
        assert!(
            runtime.retrieval["chunks"][0]["content"]
                .as_str()
                .unwrap()
                .contains("Abstract: alpha  beta")
        );
        assert!(
            outputs["formalized_content"]
                .as_str()
                .unwrap()
                .contains("├── Title: Rust Biomedical Retrieval")
        );
        assert_eq!(runtime.references.len(), 1);

        let empty = CanvasNode {
            id: "empty-pubmed".into(),
            params: Map::from_iter([("query".into(), json!(""))]),
            ..node
        };
        execute_pubmed_search_with_provider(&mut runtime, &empty, &provider)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["empty-pubmed"]["formalized_content"], "");
        assert_eq!(provider.searches.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn pubmed_agent_dispatch_uses_child_params_and_preserves_error_prefix() {
        let agent = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "tools": [{
                    "component_name": "PubMed",
                    "name": "Biomedical Search",
                    "params": {
                        "email": "agent@example.test",
                        "top_n": 4,
                        "max_retries": 0,
                        "delay_after_error": 0
                    }
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let tools = load_canvas_agent_tools(&agent).unwrap();
        let tavily = MockTavily::default();
        let wikipedia = MockWikipedia::default();
        let duckduckgo = MockDuckDuckGo::default();
        let google = GoogleClient::default();
        let google_scholar = GoogleScholarClient::default();
        let arxiv = MockArxiv::default();
        let pubmed = MockPubMed::default();
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "ignored",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        let call = ToolCall {
            id: "call-pubmed".into(),
            kind: "function".into(),
            function: crate::llm::ToolFunctionCall {
                name: "pubmed_search_0".into(),
                arguments: json!({"query": "RAG medicine"}).to_string(),
            },
        };
        let mut runtime = CanvasRuntime::default();
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert!(response.contains("├── Title: Rust Biomedical Retrieval"));
        {
            let searches = pubmed.searches.lock().unwrap();
            assert_eq!(searches[0].query, "RAG medicine");
            assert_eq!(searches[0].top_n, 4);
            assert_eq!(searches[0].email, "agent@example.test");
        }

        pubmed
            .failures
            .store(1, std::sync::atomic::Ordering::SeqCst);
        let response = execute_canvas_agent_tool_with_providers(
            &mut runtime,
            &agent,
            &tools,
            &call,
            &run_input,
            CanvasAgentToolProviders {
                tavily: &tavily,
                wikipedia: &wikipedia,
                duckduckgo: &duckduckgo,
                google: &google,
                google_scholar: &google_scholar,
                github: &GitHubClient::default(),
                arxiv: &arxiv,
                pubmed: &pubmed,
            },
        )
        .await
        .unwrap();
        assert_eq!(response, "PubMed error: temporary PubMed failure");
    }

    #[test]
    fn web_tool_hash_and_validation_match_fixed_python_boundaries() {
        assert_eq!(hash_str2int("abc", 100_000_000), 66_479_517);
        assert_eq!(hash_str2int("123456", 500), 319);
        for (component_name, params) in [
            ("TavilySearch", json!({"max_results": 0})),
            ("TavilySearch", json!({"days": 1.5})),
            ("TavilySearch", json!({"include_domains": [1]})),
            ("TavilyExtract", json!({"urls": {}})),
            ("TavilyExtract", json!({"extract_depth": "deep"})),
            ("DuckDuckGo", json!({"top_n": 0})),
            ("DuckDuckGo", json!({"top_n": 1.5})),
            ("DuckDuckGo", json!({"channel": "images"})),
            ("DuckDuckGo", json!({"topic": "academic"})),
            ("DuckDuckGo", json!({"max_retries": -1})),
            ("DuckDuckGo", json!({"delay_after_error": -1})),
            ("Wikipedia", json!({"top_n": 0})),
            ("Wikipedia", json!({"top_n": 1.5})),
            ("Wikipedia", json!({"language": "xx"})),
            ("Wikipedia", json!({"max_retries": -1})),
            ("Wikipedia", json!({"delay_after_error": -1})),
            ("GoogleScholar", json!({"top_n": 0})),
            ("GoogleScholar", json!({"top_n": 1.5})),
            ("GoogleScholar", json!({"sort_by": "newest"})),
            ("GoogleScholar", json!({"year_low": 2020.5})),
            ("GoogleScholar", json!({"patents": "yes"})),
            ("GoogleScholar", json!({"max_retries": -1})),
            ("GoogleScholar", json!({"delay_after_error": -1})),
            ("GitHub", json!({"top_n": 0})),
            ("GitHub", json!({"top_n": 1.5})),
            ("GitHub", json!({"query": 1})),
            ("GitHub", json!({"max_retries": -1})),
            ("GitHub", json!({"delay_after_error": -1})),
            ("YahooFinance", json!({"stock_code": 1})),
            ("YahooFinance", json!({"info": "yes"})),
            ("YahooFinance", json!({"count": 1})),
            ("YahooFinance", json!({"income_stmt": null})),
            ("YahooFinance", json!({"max_retries": -1})),
            ("YahooFinance", json!({"delay_after_error": -1})),
            ("ArXiv", json!({"top_n": 0})),
            ("ArXiv", json!({"top_n": 1.5})),
            ("ArXiv", json!({"sort_by": "newest"})),
            ("ArXiv", json!({"max_retries": -1})),
            ("ArXiv", json!({"delay_after_error": -1})),
            ("PubMed", json!({"top_n": 0})),
            ("PubMed", json!({"top_n": 1.5})),
            ("PubMed", json!({"email": 1})),
            ("PubMed", json!({"max_retries": -1})),
            ("PubMed", json!({"delay_after_error": -1})),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["tool"], "upstream": []},
                "tool": {"obj": {"component_name": component_name, "params": params}, "downstream": [], "upstream": ["begin"]}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_err(), "{dsl}");
        }
    }

    #[test]
    fn llm_prompt_history_and_generation_flags_follow_fixed_component_contract() {
        let mut runtime = CanvasRuntime::default();
        runtime
            .sys
            .insert("query".into(), Value::String("current question".into()));
        let node = CanvasNode {
            id: "llm".into(),
            component_name: "LLM".into(),
            params: Map::from_iter([
                ("sys_prompt".into(), json!("system for {sys.query}")),
                ("message_history_window_size".into(), json!(1)),
                ("temperature".into(), json!(0.9)),
                ("temperatureEnabled".into(), json!(false)),
                ("top_p".into(), json!(0.7)),
                ("max_tokens".into(), json!(0)),
                ("maxTokensEnabled".into(), json!(true)),
            ]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let history = vec![
            ChatMessage::new("assistant", "discarded"),
            ChatMessage::new("assistant", "recent reply"),
            ChatMessage::new("user", "current history placeholder"),
        ];
        let messages = llm_messages(&runtime, &node, &history).unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, "system");
        assert_eq!(messages[0].content, "system for current question");
        assert_eq!(messages[1].content, "recent reply");
        assert_eq!(messages[2].role, "user");
        assert_eq!(messages[2].content, "current question");

        let component = llm_component_generation_patch(&node).unwrap();
        assert_eq!(component.temperature, None);
        assert_eq!(component.top_p, Some(0.7));
        assert_eq!(component.max_tokens, None);
        let merged = merge_generation_patch(
            GenerationParamsPatch {
                temperature: Some(0.4),
                max_tokens: Some(2048),
                ..GenerationParamsPatch::default()
            },
            component,
        );
        assert_eq!(merged.temperature, Some(0.4));
        assert_eq!(merged.top_p, Some(0.7));
        assert_eq!(merged.max_tokens, Some(2048));
    }

    #[test]
    fn structured_content_strips_thinking_and_json_fences() {
        assert_eq!(
            parse_structured_content(
                "<think>private reasoning</think>\n```json\n{\"name\":\"Ada\"}\n```"
            ),
            Some(json!({"name": "Ada"}))
        );
        assert_eq!(
            parse_structured_content("prefix without a closing think block"),
            None
        );
    }

    #[tokio::test]
    async fn llm_structured_output_retries_and_publishes_fixed_output_key() {
        use axum::{Json, Router, routing::post};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let calls = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let calls = calls.clone();
                let requests = requests.clone();
                move |Json(request): Json<Value>| {
                    let calls = calls.clone();
                    let requests = requests.clone();
                    async move {
                        requests.lock().unwrap().push(request);
                        let call = calls.fetch_add(1, AtomicOrdering::SeqCst);
                        let content = if call == 0 {
                            "not json".to_owned()
                        } else {
                            "<think>hidden</think>```json\n{\"name\":\"Ada\"}\n```".to_owned()
                        };
                        Json(json!({
                            "choices": [{"message": {"content": content}}],
                            "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6}
                        }))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let llm = LlmClient::new(crate::llm::LlmConfig {
            api_base: format!("http://{address}"),
            api_key: "test".into(),
            model: "test".into(),
            ..crate::llm::LlmConfig::default()
        });
        let dsl = json!({"components": {
            "llm": {"obj": {"component_name": "LLM", "params": {
                "llm_id": "test",
                "sys_prompt": "Return a person.",
                "prompts": [{"role": "user", "content": "{sys.query}"}],
                "max_retries": 1,
                "temperature": 0.9,
                "temperatureEnabled": false,
                "top_p": 0.7,
                "topPEnabled": true,
                "outputs": {"structured": {
                    "type": "object",
                    "properties": {"name": {"type": "string"}},
                    "required": ["name"]
                }}
            }}}
        }});
        let node = CanvasNode {
            id: "llm".into(),
            component_name: "LLM".into(),
            params: extract_component_params(&dsl, "llm")
                .unwrap()
                .unwrap()
                .clone(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let mut runtime = CanvasRuntime::default();
        runtime
            .sys
            .insert("query".into(), Value::String("Who?".into()));
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "Who?",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };
        execute_node(&mut runtime, &node, Some(&llm), &run_input)
            .await
            .unwrap();
        let output = runtime.outputs.remove("llm").unwrap();
        server.abort();

        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(runtime.provider_calls, 2);
        assert_eq!(runtime.usage.total_tokens, 12);
        assert_eq!(output["structured"], json!({"name": "Ada"}));
        assert!(!output.contains_key("structured_content"));
        assert!(!output.contains_key("content"));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["messages"].as_array().unwrap().len(), 2);
        assert_eq!(requests[0]["messages"][1]["content"], "Who?");
        assert!(
            requests[0]["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("Here is the JSON schema:")
        );
        assert!((requests[0]["temperature"].as_f64().unwrap() - 0.1).abs() < 1e-6);
        assert!((requests[0]["top_p"].as_f64().unwrap() - 0.7).abs() < 1e-6);
    }

    #[tokio::test]
    async fn agent_react_loop_dispatches_retrieval_and_preserves_openai_history() {
        use axum::{Json, Router, routing::post};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let calls = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let calls = calls.clone();
                let requests = requests.clone();
                move |Json(request): Json<Value>| {
                    let calls = calls.clone();
                    let requests = requests.clone();
                    async move {
                        requests.lock().unwrap().push(request);
                        let call = calls.fetch_add(1, AtomicOrdering::SeqCst);
                        if call == 0 {
                            Json(json!({
                                "choices": [{"message": {
                                    "content": null,
                                    "tool_calls": [{
                                        "id": "call_search",
                                        "type": "function",
                                        "function": {
                                            "name": "search_my_dateset_0",
                                            "arguments": "{\"query\":\"rust ownership\"}"
                                        }
                                    }]
                                }}],
                                "usage": {"prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6}
                            }))
                        } else {
                            Json(json!({
                                "choices": [{"message": {"content": "Use the ownership guide."}}],
                                "usage": {"prompt_tokens": 5, "completion_tokens": 1, "total_tokens": 6}
                            }))
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let llm = LlmClient::new(crate::llm::LlmConfig {
            api_base: format!("http://{address}"),
            api_key: "test".into(),
            model: "tool-model".into(),
            ..crate::llm::LlmConfig::default()
        });
        let node = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "sys_prompt": "Use tools when useful.",
                "prompts": [{"role": "user", "content": "{sys.query}"}],
                "max_rounds": 2,
                "tools": [{
                    "component_name": "Retrieval",
                    "name": "Knowledge Search",
                    "params": {"dataset_ids": []}
                }],
                "mcp": []
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let retriever = MockRetriever::default();
        let mut runtime = CanvasRuntime::default();
        runtime
            .sys
            .insert("query".into(), json!("Explain Rust ownership"));
        let inputs = Map::new();
        let fallback_kb_ids = vec!["fallback-kb".to_owned()];
        let run_input = WorkflowRunInput {
            question: "Explain Rust ownership",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: Some(&retriever),
            fallback_kb_ids: &fallback_kb_ids,
        };
        execute_node(&mut runtime, &node, Some(&llm), &run_input)
            .await
            .unwrap();
        server.abort();

        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        assert_eq!(retriever.calls.load(AtomicOrdering::SeqCst), 1);
        let retrieval_request = retriever.request.lock().unwrap().clone().unwrap();
        assert_eq!(retrieval_request.query, "rust ownership");
        assert_eq!(retrieval_request.kb_ids, ["fallback-kb"]);
        assert_eq!(runtime.usage.total_tokens, 12);
        assert_eq!(runtime.provider_calls, 2);
        assert_eq!(
            runtime.outputs["agent"]["content"],
            "Use the ownership guide."
        );
        assert_eq!(
            runtime.outputs["agent"]["tool_calls"][0]["name"],
            "search_my_dateset_0"
        );
        assert_eq!(runtime.references.len(), 1);

        let requests = requests.lock().unwrap();
        assert_eq!(requests[0]["tool_choice"], "auto");
        assert_eq!(
            requests[0]["tools"][0]["function"]["name"],
            "search_my_dateset_0"
        );
        assert_eq!(
            requests[0]["tools"][0]["function"]["parameters"]["required"],
            json!(["query"])
        );
        let second_messages = requests[1]["messages"].as_array().unwrap();
        assert_eq!(second_messages.len(), 4);
        assert_eq!(second_messages[2]["role"], "assistant");
        assert!(second_messages[2].get("content").is_none());
        assert_eq!(second_messages[2]["tool_calls"][0]["id"], "call_search");
        assert_eq!(second_messages[3]["role"], "tool");
        assert_eq!(second_messages[3]["tool_call_id"], "call_search");
        assert!(
            second_messages[3]["content"]
                .as_str()
                .unwrap()
                .contains("Rust ownership guide")
        );
    }

    #[tokio::test]
    async fn agent_react_max_rounds_adds_notice_and_uses_tool_free_fallback() {
        use axum::{Json, Router, routing::post};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

        let calls = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let calls = calls.clone();
                let requests = requests.clone();
                move |Json(request): Json<Value>| {
                    let calls = calls.clone();
                    let requests = requests.clone();
                    async move {
                        requests.lock().unwrap().push(request);
                        let call = calls.fetch_add(1, AtomicOrdering::SeqCst);
                        if call == 0 {
                            Json(json!({
                                "choices": [{"message": {
                                    "content": null,
                                    "tool_calls": [{
                                        "id": "call_loop",
                                        "type": "function",
                                        "function": {
                                            "name": "search_my_dateset_0",
                                            "arguments": "{\"query\":\"loop query\"}"
                                        }
                                    }]
                                }}],
                                "usage": {"total_tokens": 3}
                            }))
                        } else {
                            Json(json!({
                                "choices": [{"message": {"content": "bounded fallback"}}],
                                "usage": {"total_tokens": 4}
                            }))
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let llm = LlmClient::new(crate::llm::LlmConfig {
            api_base: format!("http://{address}"),
            api_key: "test".into(),
            model: "tool-model".into(),
            ..crate::llm::LlmConfig::default()
        });
        let node = CanvasNode {
            id: "agent".into(),
            component_name: "Agent".into(),
            params: serde_json::from_value(json!({
                "prompts": [{"role": "user", "content": "{sys.query}"}],
                "max_rounds": 0,
                "tools": [{
                    "component_name": "Retrieval",
                    "name": "Search",
                    "params": {"dataset_ids": ["kb"]}
                }]
            }))
            .unwrap(),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        let retriever = MockRetriever::default();
        let mut runtime = CanvasRuntime::default();
        runtime.sys.insert("query".into(), json!("question"));
        let inputs = Map::new();
        let run_input = WorkflowRunInput {
            question: "question",
            user_id: "owner",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: Some(&retriever),
            fallback_kb_ids: &[],
        };
        execute_node(&mut runtime, &node, Some(&llm), &run_input)
            .await
            .unwrap();
        server.abort();

        assert_eq!(runtime.outputs["agent"]["content"], "bounded fallback");
        assert_eq!(runtime.usage.total_tokens, 7);
        assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
        let requests = requests.lock().unwrap();
        assert!(requests[0].get("tools").is_some());
        assert!(requests[1].get("tools").is_none());
        assert!(requests[1].get("tool_choice").is_none());
        assert_eq!(
            requests[1]["messages"].as_array().unwrap().last().unwrap(),
            &json!({"role": "user", "content": "Exceed max rounds: 0"})
        );
    }

    #[test]
    fn template_fast_path_matches_reference_forms_and_extracts_unique_refs() {
        let mut runtime = CanvasRuntime::default();
        runtime.outputs.insert(
            "llm_0".into(),
            Map::from_iter([("content".into(), json!("hello world"))]),
        );
        runtime
            .globals
            .insert("sys.query".into(), json!("what is ragflow"));
        runtime.globals.insert("env.max_tokens".into(), json!(1024));
        runtime.globals.insert("__item__".into(), json!("alpha"));
        runtime.globals.insert("__index__".into(), json!(3));

        for (template, expected) in [
            ("{{llm_0@content}}", "hello world"),
            ("{{{llm_0@content}}}", "hello world"),
            ("{llm_0@content}", "hello world"),
            (
                "{{sys.query}} :: {{llm_0@content}} :: {{env.max_tokens}}",
                "what is ragflow :: hello world :: 1024",
            ),
            ("{{item}}/{{index}}", "alpha/3"),
            ("{{garbage}}", "{{garbage}}"),
            ("", ""),
        ] {
            assert_eq!(resolve_template(&runtime, template).unwrap(), expected);
        }
        assert_eq!(
            extract_template_refs("{{llm_0@content}} {{sys.query}} {{llm_0@content}} {{item}}"),
            ["llm_0@content", "sys.query", "item"]
        );
    }

    #[test]
    fn template_loud_and_display_paths_share_partial_resolution() {
        let mut runtime = CanvasRuntime::default();
        runtime
            .globals
            .insert("sys.query".into(), json!("resolved"));
        runtime.outputs.insert(
            "null_node".into(),
            Map::from_iter([("value".into(), Value::Null)]),
        );
        let template =
            "x={{missing@thing}}; q={{sys.query}}; n={{null_node@value}}; z={{env.nope}}";
        let (partial, error) = resolve_template_with_partial(&runtime, template);
        assert_eq!(partial, "x=; q=resolved; n=; z=");
        let error = error.unwrap().to_string();
        assert!(error.contains("missing@thing"), "{error}");
        assert!(resolve_template(&runtime, template).is_err());
        assert_eq!(resolve_template_for_display(&runtime, template), partial);
    }

    #[tokio::test]
    async fn message_uses_display_template_soft_fail_semantics() {
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["message"], "upstream": []},
            "message": {"obj": {"component_name": "Message", "params": {
                "content": ["hello {{missing@content}}/{{sys.query}}"]
            }}, "downstream": [], "upstream": ["begin"]}
        }});
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "question",
                    user_id: "user",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "hello /question");
    }

    #[tokio::test]
    async fn canvas_runs_record_success_and_error_metrics() {
        use crate::metrics::{CanvasRunOutcome, canvas_run_count};

        let success_before = canvas_run_count(CanvasRunOutcome::Success);
        let error_before = canvas_run_count(CanvasRunOutcome::Error);
        let success = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["message"], "upstream": []},
            "message": {"obj": {"component_name": "Message", "params": {"content": ["ok"]}}, "downstream": [], "upstream": ["begin"]}
        }});
        AgentWorkflow::from_value(&success)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "question",
                    user_id: "user",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();

        let unsupported = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["browser"], "upstream": []},
            "browser": {"obj": {"component_name": "Browser", "params": {}}, "downstream": [], "upstream": ["begin"]}
        }});
        assert!(
            AgentWorkflow::from_value(&unsupported)
                .unwrap()
                .unwrap()
                .run(
                    None,
                    WorkflowRunInput {
                        question: "question",
                        user_id: "user",
                        inputs: &Map::new(),
                        history: &[],
                        generation: GenerationParamsPatch::default(),
                        llm_resolver: None,
                        retriever: None,
                        fallback_kb_ids: &[],
                    },
                )
                .await
                .is_err()
        );

        assert!(canvas_run_count(CanvasRunOutcome::Success) >= success_before + 1.0);
        assert!(canvas_run_count(CanvasRunOutcome::Error) >= error_before + 1.0);
    }

    #[test]
    fn string_transform_matches_split_type_and_empty_segment_semantics() {
        let mut runtime = CanvasRuntime::default();
        runtime
            .globals
            .insert("sys.source".into(), Value::String(",a;b,".into()));
        let node = CanvasNode {
            id: "split".into(),
            component_name: "StringTransform".into(),
            params: Map::from_iter([
                ("method".into(), json!("split")),
                ("split_ref".into(), json!("sys.source")),
                ("delimiters".into(), json!([",", ";"])),
            ]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };

        execute_string_transform(&mut runtime, &node).unwrap();
        assert_eq!(
            runtime.outputs["split"]["result"],
            json!(["", "a", "b", ""])
        );

        runtime
            .globals
            .insert("sys.source".into(), Value::Number(Number::from(7)));
        assert!(
            execute_string_transform(&mut runtime, &node)
                .unwrap_err()
                .to_string()
                .contains("split input is not a string: number")
        );

        runtime
            .globals
            .insert("sys.source".into(), Value::Bool(false));
        execute_string_transform(&mut runtime, &node).unwrap();
        assert_eq!(runtime.outputs["split"]["result"], json!([""]));
    }

    #[test]
    fn string_transform_merge_joins_arrays_and_blanks_missing_references() {
        let mut runtime = CanvasRuntime::default();
        runtime.outputs.insert(
            "split".into(),
            Map::from_iter([("result".into(), json!(["alpha", "beta"]))]),
        );
        let node = CanvasNode {
            id: "merge".into(),
            component_name: "StringTransform".into(),
            params: Map::from_iter([
                ("method".into(), json!("merge")),
                ("script".into(), json!("{split@result}/{missing@result}")),
                ("delimiters".into(), json!(["|"])),
            ]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };

        execute_string_transform(&mut runtime, &node).unwrap();
        assert_eq!(runtime.outputs["merge"]["result"], "alpha|beta/");
    }

    #[test]
    fn string_transform_merge_renders_jinja_filters_conditionals_and_nested_values() {
        let mut runtime = CanvasRuntime::default();
        runtime.outputs.insert(
            "begin_0".into(),
            Map::from_iter([
                ("content".into(), json!("<hello>")),
                ("enabled".into(), json!(true)),
                ("user".into(), json!({"name": "alice"})),
            ]),
        );
        let node = CanvasNode {
            id: "merge".into(),
            component_name: "StringTransform".into(),
            params: Map::from_iter([
                ("method".into(), json!("merge")),
                (
                    "script".into(),
                    json!(
                        "{% if begin_0.enabled %}{{ begin_0.content | upper }} / {{ begin_0.user.name }}{% else %}no{% endif %}"
                    ),
                ),
                ("delimiters".into(), json!([","])),
            ]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };

        execute_string_transform(&mut runtime, &node).unwrap();
        assert_eq!(
            runtime.outputs["merge"]["result"], "<HELLO> / alice",
            "StringTransform text output must not be HTML escaped"
        );
    }

    #[test]
    fn string_transform_merge_renders_loops_comments_globals_and_missing_values() {
        let mut runtime = CanvasRuntime::default();
        runtime.outputs.insert(
            "begin_0".into(),
            Map::from_iter([("items".into(), json!(["a", "b", "c"]))]),
        );
        runtime.globals.insert("env.enabled".into(), json!(true));
        runtime.globals.insert("sys.suffix".into(), json!("done"));
        let node = CanvasNode {
            id: "merge".into(),
            component_name: "StringTransform".into(),
            params: Map::from_iter([
                ("method".into(), json!("merge")),
                (
                    "script".into(),
                    json!(
                        "{# hidden #}{% if env.enabled %}{% for item in begin_0.items %}{{ item }}{% if not loop.last %}|{% endif %}{% endfor %}/{{ sys.suffix }}/{{ missing.deep }}{% endif %}"
                    ),
                ),
                ("delimiters".into(), json!([","])),
            ]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };

        execute_string_transform(&mut runtime, &node).unwrap();
        assert_eq!(runtime.outputs["merge"]["result"], "a|b|c/done/");
    }

    #[test]
    fn string_transform_merge_normalizes_selectors_around_jinja_statements() {
        let mut runtime = CanvasRuntime::default();
        runtime.outputs.insert(
            "split:0".into(),
            Map::from_iter([("result".into(), json!(["alpha", "beta"]))]),
        );
        runtime
            .globals
            .insert("sys.query".into(), json!("question"));
        let node = CanvasNode {
            id: "merge".into(),
            component_name: "StringTransform".into(),
            params: Map::from_iter([
                ("method".into(), json!("merge")),
                (
                    "script".into(),
                    json!("{% if true %}{split:0@result}/{{sys.query}}{% endif %}"),
                ),
                ("delimiters".into(), json!(["|"])),
            ]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };

        execute_string_transform(&mut runtime, &node).unwrap();
        assert_eq!(runtime.outputs["merge"]["result"], "alpha|beta/question");
    }

    #[test]
    fn string_transform_accepts_upstream_line_and_merge_inputs() {
        let mut runtime = CanvasRuntime::default();
        runtime.outputs.insert(
            "begin".into(),
            Map::from_iter([
                ("line".into(), json!("a,b")),
                ("x".into(), json!("foo")),
                ("y".into(), json!("bar")),
            ]),
        );
        let split = CanvasNode {
            id: "split".into(),
            component_name: "StringTransform".into(),
            params: Map::from_iter([
                ("method".into(), json!("split")),
                ("split_ref".into(), json!("missing@line")),
                ("delimiters".into(), json!([","])),
            ]),
            downstream: Vec::new(),
            upstream: vec!["begin".into()],
            parent_id: None,
        };
        execute_string_transform(&mut runtime, &split).unwrap();
        assert_eq!(runtime.outputs["split"]["result"], json!(["a", "b"]));

        let merge = CanvasNode {
            id: "merge".into(),
            component_name: "StringTransform".into(),
            params: Map::from_iter([
                ("method".into(), json!("merge")),
                ("script".into(), json!("{{x}} and {{y}}")),
                ("delimiters".into(), json!([","])),
            ]),
            downstream: Vec::new(),
            upstream: vec!["begin".into()],
            parent_id: None,
        };
        execute_string_transform(&mut runtime, &merge).unwrap();
        assert_eq!(runtime.outputs["merge"]["result"], "foo and bar");
    }

    #[test]
    fn string_transform_resolves_iteration_aliases() {
        let mut runtime = CanvasRuntime::default();
        runtime.globals.insert("__item__".into(), json!("beta"));
        runtime.globals.insert("__index__".into(), json!(1));
        runtime
            .globals
            .insert("__result__".into(), json!(["a", "b"]));

        assert_eq!(
            resolve_string_transform_template(&runtime, "{index}: {item}/{result}", "|"),
            "1: beta/a|b"
        );
    }

    #[test]
    fn string_transform_jinja_is_detected_and_parse_errors_fail_soft() {
        for template in [
            "{{ name }}",
            "{% if value %}yes{% endif %}",
            "{# comment #}",
            "{{ value | upper }}",
        ] {
            assert!(contains_jinja_syntax(template), "{template}");
        }
        assert!(!contains_jinja_syntax("{begin@content}"));

        let mut runtime = CanvasRuntime::default();
        runtime.outputs.insert(
            "begin".into(),
            Map::from_iter([("content".into(), json!("resolved"))]),
        );
        assert!(render_sandboxed_jinja("{% invalid %}", Map::new()).is_err());
        assert_eq!(
            resolve_string_transform_template(&runtime, "{% invalid %}{begin@content}", ","),
            "{% invalid %}resolved"
        );
    }

    #[test]
    fn string_transform_jinja_fuel_exhaustion_fails_soft() {
        let template = "{% for item in begin_0.items %}x{% endfor %}";
        let mut runtime = CanvasRuntime::default();
        runtime.outputs.insert(
            "begin_0".into(),
            Map::from_iter([("items".into(), Value::Array(vec![Value::Null; 100_000]))]),
        );

        assert_eq!(
            resolve_string_transform_template(&runtime, template, ","),
            template
        );
    }

    #[test]
    fn string_transform_params_are_validated_when_canvas_is_compiled() {
        for params in [
            json!({"method": "unknown", "delimiters": [","]}),
            json!({"method": "merge", "delimiters": []}),
            json!({"method": "split", "delimiters": [1]}),
            json!({"method": "merge", "script": 42}),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["transform"], "upstream": []},
                "transform": {"obj": {"component_name": "StringTransform", "params": params}, "downstream": [], "upstream": ["begin"]}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_err(), "{dsl}");
        }
    }

    #[tokio::test]
    async fn data_operations_runs_frontend_query_shape_in_a_canvas() {
        let dsl = json!({
            "components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["data"], "upstream": []},
                "data": {"obj": {"component_name": "DataOperations", "params": {
                    "query": [{"input": "env.items"}],
                    "operations": "select_keys",
                    "select_keys": [{"name": "name"}]
                }}, "downstream": ["message"], "upstream": ["begin"]},
                "message": {"obj": {"component_name": "Message", "params": {
                    "content": ["{data@result}"]
                }}, "downstream": [], "upstream": ["data"]}
            },
            "globals": {"env.items": [
                {"name": "alpha", "secret": "hidden"},
                {"name": "beta", "secret": "hidden"}
            ]}
        });
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "ignored",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, r#"[{"name":"alpha"},{"name":"beta"}]"#);
        assert_eq!(
            result.trace[1].outputs["result"],
            json!([{"name": "alpha"}, {"name": "beta"}])
        );
    }

    #[test]
    fn data_operations_select_remove_and_rename_keys() {
        let items = json!([
            {"name": "alpha", "secret": "s1", "old": 1},
            {"name": "beta", "secret": "s2", "old": 2}
        ]);
        let selected = run_data_operation(
            json!({
                "query": "sys.items",
                "operations": "select_keys",
                "select_keys": "name, old"
            }),
            [("sys.items", items.clone())],
        )
        .unwrap();
        assert_eq!(
            selected,
            json!([{"name": "alpha", "old": 1}, {"name": "beta", "old": 2}])
        );

        let removed = run_data_operation(
            json!({
                "query": ["sys.items"],
                "operations": "remove_keys",
                "remove_keys": [{"name": "secret"}]
            }),
            [("sys.items", items.clone())],
        )
        .unwrap();
        assert_eq!(
            removed,
            json!([
                {"name": "alpha", "old": 1},
                {"name": "beta", "old": 2}
            ])
        );

        let renamed = run_data_operation(
            json!({
                "query": ["sys.items"],
                "operations": "rename_keys",
                "rename_keys": [
                    {"old_key": "old", "new_key": "key"},
                    {"old_key": "", "new_key": "ignored"}
                ]
            }),
            [("sys.items", items)],
        )
        .unwrap();
        assert_eq!(
            renamed,
            json!([
                {"name": "alpha", "secret": "s1", "key": 1},
                {"name": "beta", "secret": "s2", "key": 2}
            ])
        );
    }

    #[test]
    fn data_operations_literal_eval_walks_json_shaped_values() {
        let result = run_data_operation(
            json!({"query": ["sys.items"], "operations": "literal_eval"}),
            [(
                "sys.items",
                json!([{
                    "plain": "hello",
                    "object": "{\"key\":1,\"nested\":[2,3]}",
                    "list": "[true, null, 3]",
                    "number": "42",
                    "bool": "true",
                    "python_bool": "False",
                    "python_none": "None",
                    "leading_decimal": ".5",
                    "invalid": "{broken"
                }]),
            )],
        )
        .unwrap();
        assert_eq!(
            result,
            json!([{
                "plain": "hello",
                "object": {"key": 1, "nested": [2, 3]},
                "list": [true, null, 3],
                "number": 42,
                "bool": true,
                "python_bool": false,
                "python_none": null,
                "leading_decimal": 0.5,
                "invalid": "{broken"
            }])
        );
    }

    #[test]
    fn data_operations_combine_flattens_conflicting_arrays() {
        let result = run_data_operation(
            json!({
                "query": ["sys.first", "sys.second", "sys.third"],
                "operations": "combine"
            }),
            [
                ("sys.first", json!({"key": "first", "list": [1]})),
                ("sys.second", json!({"key": [2, 3], "list": 2})),
                ("sys.third", json!({"key": 4, "list": [3, 4]})),
            ],
        )
        .unwrap();
        assert_eq!(
            result,
            json!({"key": ["first", 2, 3, 4], "list": [1, 2, 3, 4]})
        );
    }

    #[test]
    fn data_operations_filter_and_update_resolve_canvas_values() {
        let items = json!([
            {"name": "alpha-tenant-7", "score": 1},
            {"name": "beta", "score": 2}
        ]);
        let filtered = run_data_operation(
            json!({
                "query": ["sys.items"],
                "operations": "filter_values",
                "filter_values": [
                    {"key": "name", "operator": "contains", "value": "{{sys.user_id}}"}
                ]
            }),
            [
                ("sys.items", items.clone()),
                ("sys.user_id", json!("tenant-7")),
            ],
        )
        .unwrap();
        assert_eq!(filtered, json!([{"name": "alpha-tenant-7", "score": 1}]));

        let updated = run_data_operation(
            json!({
                "query": ["sys.items"],
                "operations": "append_or_update",
                "updates": [
                    {"key": "owner", "value": "sys.user_id"},
                    {"key": "label", "value": "owner={{sys.user_id}}"},
                    {"key": "", "value": "ignored"}
                ]
            }),
            [("sys.items", items), ("sys.user_id", json!("tenant-7"))],
        )
        .unwrap();
        assert_eq!(updated[0]["owner"], "tenant-7");
        assert_eq!(updated[0]["label"], "owner=tenant-7");
        assert_eq!(updated[1]["owner"], "tenant-7");
    }

    #[test]
    fn data_operations_collects_only_objects_and_rejects_malformed_params() {
        let result = run_data_operation(
            json!({
                "query": ["sys.object", "sys.values", "sys.scalar"],
                "operations": ""
            }),
            [
                ("sys.object", json!({"value": "1"})),
                ("sys.values", json!([{"value": "2"}, 3, null])),
                ("sys.scalar", json!("ignored")),
            ],
        )
        .unwrap();
        assert_eq!(result, json!([{"value": 1}, {"value": 2}]));

        for params in [
            json!({"query": [], "operations": "bogus"}),
            json!({"query": [{"missing": "input"}]}),
            json!({"query": [1]}),
            json!({"select_keys": [1]}),
            json!({"filter_values": {}}),
            json!({"updates": [1]}),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["data"], "upstream": []},
                "data": {"obj": {"component_name": "DataOperations", "params": params}, "downstream": [], "upstream": ["begin"]}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_err(), "{dsl}");
        }
    }

    #[test]
    fn excel_processor_write_then_read_matches_go_round_trip_contract() {
        let written = run_excel_operation(
            json!({
                "operation": "write",
                "output_data": [["a", "b"], [1, 2]]
            }),
            [],
        )
        .unwrap();
        let encoded = written["bytes"].as_str().unwrap();
        let bytes = BASE64_STANDARD.decode(encoded).unwrap();
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        assert_eq!(written["size"], bytes.len());
        assert_eq!(written["sheet_names"], json!(["Sheet1"]));

        let read = run_excel_operation(json!({"operation": "read", "bytes": encoded}), []).unwrap();
        assert_eq!(read["rows"], json!([["a", "b"], ["1", "2"]]));
        assert_eq!(read["sheet_names"], json!(["Sheet1"]));
        assert_eq!(read["size"], 2);
    }

    #[test]
    fn excel_processor_output_reads_selected_multi_sheet_records() {
        let output = run_excel_operation(
            json!({
                "operation": "output",
                "transform_data": "sys.tables",
                "output_filename": "report"
            }),
            [(
                "sys.tables",
                json!({
                    "Alpha": [{"name": "Ada", "score": 9}],
                    "Beta": [{"name": "Lin", "score": 8}]
                }),
            )],
        )
        .unwrap();
        assert_eq!(output["sheet_names"], json!(["Alpha", "Beta"]));
        assert_eq!(output["attachment"]["file_name"], "report.xlsx");
        assert!(output["attachment"].get("doc_id").is_none());

        let read = run_excel_operation(
            json!({
                "operation": "read",
                "input_files": [{
                    "input": "sys.workbook",
                    "name": "report.xlsx"
                }],
                "sheet_selection": "Beta"
            }),
            [("sys.workbook", output["bytes"].clone())],
        )
        .unwrap();
        assert_eq!(read["sheet_names"], json!(["Alpha", "Beta"]));
        assert_eq!(
            read["data"]["report.xlsx"],
            json!([{"name": "Lin", "score": "8"}])
        );
        assert!(read["markdown"].as_str().unwrap().contains("Lin"));
    }

    #[tokio::test]
    async fn excel_processor_runs_frontend_csv_shape_in_a_canvas() {
        let csv = BASE64_STANDARD.encode(b"name,score\nAda,9\nLin,8\n");
        let dsl = json!({
            "components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["excel"], "upstream": []},
                "excel": {"obj": {"component_name": "ExcelProcessor", "params": {
                    "operation": "read",
                    "input_files": [{"input": "env.csv", "name": "scores.csv"}],
                    "sheet_selection": "all"
                }}, "downstream": ["message"], "upstream": ["begin"]},
                "message": {"obj": {"component_name": "Message", "params": {
                    "content": ["{excel@summary}"]
                }}, "downstream": [], "upstream": ["excel"]}
            },
            "globals": {"env.csv": csv}
        });
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "ignored",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.path, ["begin", "excel", "message"]);
        assert!(result.answer.contains("2 rows, 2 columns"));
        assert_eq!(
            result.trace[1].outputs["data"]["scores.csv"],
            json!([
                {"name": "Ada", "score": "9"},
                {"name": "Lin", "score": "8"}
            ])
        );
    }

    #[test]
    fn excel_processor_merges_workbooks_and_transforms_native_records() {
        let first = run_excel_operation(
            json!({"operation": "write", "output_data": [["id", "left"], [1, "a"]]}),
            [],
        )
        .unwrap();
        let second = run_excel_operation(
            json!({"operation": "write", "output_data": [["id", "right"], [1, "b"], [2, "c"]]}),
            [],
        )
        .unwrap();
        let merged = run_excel_operation(
            json!({
                "operation": "merge",
                "file_refs": [first["bytes"].clone(), second["bytes"].clone()],
                "merge_strategy": "join",
                "join_on": "id"
            }),
            [],
        )
        .unwrap();
        assert_eq!(merged["size"], 5);
        assert_eq!(
            merged["data"]["merged"],
            json!([
                {"id": "1", "left": "a", "right": "b"},
                {"id": "2", "right": "c"}
            ])
        );

        let transformed = run_excel_operation(
            json!({"operation": "transform", "transform_data": "sys.records"}),
            [("sys.records", json!([{"name": "Ada", "active": true}]))],
        )
        .unwrap();
        assert_eq!(
            transformed["data"],
            json!([{"name": "Ada", "active": true}])
        );
        assert!(transformed["markdown"].as_str().unwrap().contains("Ada"));
    }

    #[test]
    fn excel_processor_empty_and_invalid_inputs_are_explicit() {
        let written =
            run_excel_operation(json!({"operation": "write", "output_data": []}), []).unwrap();
        assert_eq!(written["rows"], json!([]));
        let read = run_excel_operation(
            json!({"operation": "read", "file_ref": written["bytes"].clone()}),
            [],
        )
        .unwrap();
        assert_eq!(read["rows"], json!([]));
        assert_eq!(read["sheet_names"], json!(["Sheet1"]));

        assert!(run_excel_operation(json!({"operation": "read"}), []).is_err());
        assert!(
            run_excel_operation(json!({"operation": "read", "file_ref": "not-base64"}), [])
                .unwrap_err()
                .to_string()
                .contains("not valid base64")
        );
        assert!(
            run_excel_operation(
                json!({"operation": "read", "file_ref": {"id": "external-only"}}),
                []
            )
            .unwrap_err()
            .to_string()
            .contains("external file service")
        );
        for params in [
            json!({"operation": "bogus"}),
            json!({"operation": "read", "input_files": {}}),
            json!({"operation": "output", "output_format": "pdf"}),
            json!({"operation": "merge", "merge_strategy": "zip"}),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["excel"], "upstream": []},
                "excel": {"obj": {"component_name": "ExcelProcessor", "params": params}, "downstream": [], "upstream": ["begin"]}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_err(), "{dsl}");
        }
    }

    #[test]
    fn doc_generator_writes_all_fixed_formats_and_both_component_names() {
        for (component_name, format, mime, magic) in [
            (
                "DocGenerator",
                "pdf",
                "application/pdf",
                b"%PDF-".as_slice(),
            ),
            (
                "DocsGenerator",
                "docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                b"PK\x03\x04".as_slice(),
            ),
            (
                "DocGenerator",
                "txt",
                "text/plain",
                b"Generated:".as_slice(),
            ),
            (
                "DocsGenerator",
                "markdown",
                "text/markdown",
                b"<!-- generated:".as_slice(),
            ),
            (
                "DocGenerator",
                "html",
                "text/html",
                b"<!DOCTYPE html>".as_slice(),
            ),
        ] {
            let output = run_doc_generator(
                component_name,
                json!({
                    "content": "<think>secret</think>Report {env.name}",
                    "filename": " bad/name.old ",
                    "output_format": format,
                    "add_page_numbers": true,
                    "add_timestamp": true,
                    "font_size": 12
                }),
                [("env.name", json!("中文"))],
            )
            .unwrap();
            assert_eq!(output["mime_type"], mime, "{format}");
            assert_eq!(
                output["filename"],
                format!(
                    "bad name.{}",
                    if format == "markdown" { "md" } else { format }
                )
            );
            let bytes = BASE64_STANDARD
                .decode(output["bytes"].as_str().unwrap())
                .unwrap();
            assert!(
                bytes.starts_with(magic),
                "{format}: {:x?}",
                &bytes[..bytes.len().min(16)]
            );
            assert_eq!(output["size"], bytes.len(), "{format}");
            assert!(
                !String::from_utf8_lossy(&bytes).contains("secret"),
                "{format}"
            );
            let download: Value =
                serde_json::from_str(output["download"].as_str().unwrap()).unwrap();
            assert_eq!(download["filename"], output["filename"], "{format}");
            assert_eq!(download["base64"], output["bytes"], "{format}");
        }
    }

    #[tokio::test]
    async fn doc_generator_runs_in_canvas_and_message_consumes_download_descriptor() {
        let dsl = json!({
            "components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["docs"], "upstream": []},
                "docs": {"obj": {"component_name": "DocGenerator", "params": {
                    "content": "Hello {env.name}",
                    "filename": "greeting.txt",
                    "output_format": "txt",
                    "add_page_numbers": false,
                    "add_timestamp": false,
                    "include_download_info_in_content": true,
                    "font_size": 12
                }}, "downstream": ["message"], "upstream": ["begin"]},
                "message": {"obj": {"component_name": "Message", "params": {
                    "content": ["{docs@download}"]
                }}, "downstream": [], "upstream": ["docs"]}
            },
            "globals": {"env.name": "RayRAG"}
        });
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "ignored",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.path, ["begin", "docs", "message"]);
        let answer: Value = serde_json::from_str(&result.answer).unwrap();
        assert_eq!(answer["filename"], "greeting.txt");
        assert_eq!(answer["mime_type"], "text/plain");
        assert!(
            answer["base64"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(answer.get("include_download_info_in_content").is_none());
        let bytes = BASE64_STANDARD
            .decode(result.trace[1].outputs["bytes"].as_str().unwrap())
            .unwrap();
        assert_eq!(bytes, b"\nHello RayRAG");
    }

    #[test]
    fn doc_generator_rejects_invalid_static_params() {
        for params in [
            json!({}),
            json!({"content": ""}),
            json!({"content": 1}),
            json!({"content": "x", "output_format": "xlsx"}),
            json!({"content": "x", "font_size": 11}),
            json!({"content": "x", "add_timestamp": "yes"}),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["docs"], "upstream": []},
                "docs": {"obj": {"component_name": "DocGenerator", "params": params}, "downstream": [], "upstream": ["begin"]}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_err(), "{dsl}");
        }
    }

    #[tokio::test]
    async fn list_operations_runs_in_a_canvas_and_preserves_native_outputs() {
        let dsl = json!({
            "components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["list"], "upstream": []},
                "list": {"obj": {"component_name": "ListOperations", "params": {
                    "query": "env.items", "operations": "topN", "n": "2"
                }}, "downstream": ["message"], "upstream": ["begin"]},
                "message": {"obj": {"component_name": "Message", "params": {
                    "content": ["{list@first}/{list@last}"]
                }}, "downstream": [], "upstream": ["list"]}
            },
            "globals": {"env.items": ["alpha", "beta", "gamma"]}
        });
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "ignored",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "alpha/beta");
        assert_eq!(result.trace[1].outputs["result"], json!(["alpha", "beta"]));
    }

    #[test]
    fn list_operations_match_nth_head_tail_and_strict_ranges() {
        let items = json!(["a", "b", "c", "d", "e"]);
        let nth = run_list_operation(
            json!({"query": "sys.items", "operations": "nth", "n": -1}),
            items.clone(),
        )
        .unwrap();
        assert_eq!(nth["result"], json!(["e"]));
        assert_eq!(nth["first"], "e");
        assert_eq!(nth["last"], "e");

        let head = run_list_operation(
            json!({"query": "sys.items", "operations": "topN", "n": true}),
            items.clone(),
        )
        .unwrap();
        assert_eq!(head["result"], json!(["a"]));

        let tail = run_list_operation(
            json!({"query": "sys.items", "operations": "tail", "n": 99}),
            items.clone(),
        )
        .unwrap();
        assert_eq!(tail["result"], items);

        let error = run_list_operation(
            json!({"query": "sys.items", "operations": "nth", "n": 0, "strict": "YES"}),
            json!([1, 2, 3]),
        )
        .unwrap_err();
        assert!(error.to_string().contains("nth requires n"));
    }

    #[test]
    fn list_operations_filter_sort_and_deduplicate_match_upstream_contract() {
        let filtered = run_list_operation(
            json!({
                "query": "sys.items",
                "operations": "filter",
                "filter": {"operator": "=", "value": "True"}
            }),
            json!([true, false, true, "True"]),
        )
        .unwrap();
        assert_eq!(filtered["result"], json!([true, true, "True"]));

        let sorted = run_list_operation(
            json!({
                "query": "sys.items",
                "operations": "sort",
                "sort_method": "desc",
                "sort_by": "score,title"
            }),
            json!([
                {"id": 3, "score": 0.76, "title": "Gamma"},
                {"id": 2, "score": 0.88, "title": "Beta"},
                {"id": 1, "score": 0.91, "title": "Alpha"}
            ]),
        )
        .unwrap();
        assert_eq!(sorted["first"]["id"], 1);
        assert_eq!(sorted["last"]["id"], 3);

        let legacy_sorted = run_list_operation(
            json!({"query": "sys.items", "operations": "sort", "sort_method": "desc"}),
            json!([
                {"id": 1, "score": 0.91},
                {"id": 3, "score": 0.76},
                {"id": 2, "score": 0.88}
            ]),
        )
        .unwrap();
        assert_eq!(legacy_sorted["result"][0]["id"], 3);

        let mut first = Map::new();
        first.insert("b".into(), json!([2, 3]));
        first.insert("a".into(), json!(1));
        let mut same = Map::new();
        same.insert("a".into(), json!(1));
        same.insert("b".into(), json!([2, 3]));
        let deduplicated = run_list_operation(
            json!({"query": "sys.items", "operations": "drop_duplicates"}),
            Value::Array(vec![Value::Object(first.clone()), Value::Object(same)]),
        )
        .unwrap();
        assert_eq!(
            deduplicated["result"],
            Value::Array(vec![Value::Object(first)])
        );
    }

    #[test]
    fn list_operations_reject_invalid_params_and_non_array_inputs() {
        for params in [
            json!({"operations": "head", "n": 1}),
            json!({"query": "sys.items", "operations": "reverse"}),
            json!({"query": "sys.items", "sort_by": ["score", 1]}),
            json!({"query": "sys.items", "filter": []}),
        ] {
            let dsl = json!({"components": {
                "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["list"], "upstream": []},
                "list": {"obj": {"component_name": "ListOperations", "params": params}, "downstream": [], "upstream": ["begin"]}
            }});
            assert!(AgentWorkflow::from_value(&dsl).is_err(), "{dsl}");
        }

        let error = run_list_operation(
            json!({"query": "sys.items", "operations": "head", "n": 1}),
            json!("not-an-array"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("input should be an array"));
    }

    #[test]
    fn variable_aggregator_rejects_empty_groups_and_empty_candidates() {
        let mut runtime = CanvasRuntime::default();
        let empty_groups = CanvasNode {
            id: "aggregate".into(),
            component_name: "VariableAggregator".into(),
            params: Map::from_iter([("groups".into(), json!([]))]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        assert!(
            execute_variable_aggregator(&mut runtime, &empty_groups)
                .unwrap_err()
                .to_string()
                .contains("groups cannot be empty")
        );

        let empty_candidates = CanvasNode {
            params: Map::from_iter([(
                "groups".into(),
                json!([{"group_name": "answer", "variables": []}]),
            )]),
            ..empty_groups
        };
        assert!(
            execute_variable_aggregator(&mut runtime, &empty_candidates)
                .unwrap_err()
                .to_string()
                .contains("variables of group 'answer' cannot be empty")
        );
    }

    #[test]
    fn variable_assigner_matches_selector_type_and_error_semantics() {
        let mut runtime = CanvasRuntime {
            globals: Map::from_iter([
                ("source".into(), json!([2, 3])),
                ("target".into(), json!([1])),
                ("text".into(), json!("old")),
                ("env.number".into(), json!(8)),
                ("flag".into(), json!(true)),
            ]),
            ..CanvasRuntime::default()
        };
        let node = CanvasNode {
            id: "assign".into(),
            component_name: "VariableAssigner".into(),
            params: Map::from_iter([(
                "variables".into(),
                json!([
                    {"variable": "target", "operator": "extend", "parameter": "source"},
                    {"variable": "text", "operator": "set", "parameter": "value={env.number}"},
                    {"variable": "env.number", "operator": "/=", "parameter": 0},
                    {"variable": "flag", "operator": "+=", "parameter": 1}
                ]),
            )]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };

        execute_variable_assigner(&mut runtime, &node).unwrap();
        assert_eq!(runtime.globals["target"], json!([1, 2, 3]));
        assert_eq!(runtime.globals["text"], json!("value=8"));
        assert_eq!(runtime.globals["env.number"], json!("ERROR:DIVIDE_BY_ZERO"));
        assert_eq!(
            runtime.globals["flag"],
            json!("ERROR:VARIABLE_NOT_NUMBER or PARAMETER_NOT_NUMBER")
        );
    }

    #[test]
    fn variable_assigner_requires_parameters_except_for_parameterless_operators() {
        let mut runtime = CanvasRuntime {
            globals: Map::from_iter([("items".into(), json!([1, 2]))]),
            ..CanvasRuntime::default()
        };
        let incomplete = CanvasNode {
            id: "assign".into(),
            component_name: "VariableAssigner".into(),
            params: Map::from_iter([(
                "variables".into(),
                json!([{"variable": "items", "operator": "append"}]),
            )]),
            downstream: Vec::new(),
            upstream: Vec::new(),
            parent_id: None,
        };
        assert!(
            execute_variable_assigner(&mut runtime, &incomplete)
                .unwrap_err()
                .to_string()
                .contains("variable is not complete")
        );

        let remove = CanvasNode {
            params: Map::from_iter([(
                "variables".into(),
                json!([{"variable": "items", "operator": "remove_last"}]),
            )]),
            ..incomplete
        };
        execute_variable_assigner(&mut runtime, &remove).unwrap();
        assert_eq!(runtime.globals["items"], json!([1]));
    }

    #[tokio::test]
    async fn retrieval_uses_backend_fallback_kb_and_returns_references() {
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["retrieval"], "upstream": []},
            "retrieval": {"obj": {"component_name": "Retrieval", "params": {
                "query": "sys.query",
                "similarity_threshold": 0.3,
                "keywords_similarity_weight": 0.7,
                "top_n": 3,
                "top_k": 20
            }}, "downstream": ["message"], "upstream": ["begin"]},
            "message": {"obj": {"component_name": "Message", "params": {
                "content": ["{retrieval@formalized_content}"]
            }}, "downstream": [], "upstream": ["retrieval"]}
        }});
        let retriever = MockRetriever::default();
        let fallback_kb_ids = vec!["fallback-kb".to_string()];
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                None,
                WorkflowRunInput {
                    question: "USER: How does Rust ownership work?",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: Some(&retriever),
                    fallback_kb_ids: &fallback_kb_ids,
                },
            )
            .await
            .unwrap();
        let request = retriever.request.lock().unwrap().clone().unwrap();
        assert_eq!(request.query, "How does Rust ownership work?");
        assert_eq!(request.kb_ids, fallback_kb_ids);
        assert_eq!(request.top_n, 3);
        assert_eq!(request.top_k, 20);
        assert_eq!(
            result.answer,
            "Reference 1 | Guide.md\nRust ownership guide"
        );
        assert_eq!(result.references.len(), 1);
        assert_eq!(result.references[0].id, "chunk-1");
    }

    #[tokio::test]
    async fn workflow_observer_emits_node_start_before_async_component_finishes() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["retrieval"], "upstream": []
            },
            "retrieval": {
                "obj": {"component_name": "Retrieval", "params": {
                    "query": "sys.query",
                    "kb_ids": ["kb-1"]
                }},
                "downstream": ["message"], "upstream": ["begin"]
            },
            "message": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["{retrieval@formalized_content}"]
                }},
                "downstream": [], "upstream": ["retrieval"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let retriever = BlockingRetriever::default();
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let observer = ChannelWorkflowObserver(events);
        let inputs = Map::new();
        let fallback_kb_ids = Vec::new();
        let run = workflow.run_interactive_observed(
            None,
            WorkflowRunInput {
                question: "live query",
                user_id: "user-1",
                inputs: &inputs,
                history: &[],
                generation: GenerationParamsPatch::default(),
                llm_resolver: None,
                retriever: Some(&retriever),
                fallback_kb_ids: &fallback_kb_ids,
            },
            &observer,
        );
        tokio::pin!(run);

        loop {
            tokio::select! {
                event = received.recv() => {
                    let event = event.unwrap();
                    if event.event == "node_started"
                        && event.data["component_id"] == "retrieval"
                    {
                        assert_eq!(event.data["component_name"], "Retrieval");
                        assert_eq!(event.data["inputs"]["query"], "live query");
                        break;
                    }
                }
                result = &mut run => {
                    panic!("workflow completed before blocked Retrieval start: {result:?}");
                }
            }
        }

        retriever.release.notify_one();
        let result = run.await.unwrap();
        assert!(matches!(
            result,
            WorkflowRunOutcome::Completed(WorkflowRunResult { answer, .. })
                if answer == "live result"
        ));
        let remaining: Vec<_> = std::iter::from_fn(|| received.try_recv().ok()).collect();
        assert!(remaining.iter().any(|event| {
            event.event == "node_finished" && event.data["component_id"] == "retrieval"
        }));
    }

    #[tokio::test]
    async fn categorize_routes_to_the_llm_selected_destination() {
        use axum::{Json, Router, routing::post};

        let app = Router::new().route(
            "/chat/completions",
            post(|| async {
                Json(json!({
                    "choices": [{"message": {"content": "product"}}],
                    "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let llm = LlmClient::new(crate::llm::LlmConfig {
            api_base: format!("http://{address}"),
            api_key: "test".into(),
            model: "test".into(),
            ..crate::llm::LlmConfig::default()
        });
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["categorize"], "upstream": []},
            "categorize": {"obj": {"component_name": "Categorize", "params": {
                "query": "sys.query",
                "category_description": {
                    "other": {"description": "anything else", "to": ["message-other"]},
                    "product": {"description": "product questions", "examples": ["What is RayRAG?"], "to": ["message-product"]}
                }
            }}, "downstream": ["message-other", "message-product"], "upstream": ["begin"]},
            "message-other": {"obj": {"component_name": "Message", "params": {"content": ["other"]}}, "downstream": [], "upstream": ["categorize"]},
            "message-product": {"obj": {"component_name": "Message", "params": {"content": ["product"]}}, "downstream": [], "upstream": ["categorize"]}
        }});
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                Some(&llm),
                WorkflowRunInput {
                    question: "Tell me about RayRAG",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        server.abort();
        assert_eq!(result.answer, "product");
        assert_eq!(result.path, ["begin", "categorize", "message-product"]);
        assert_eq!(result.usage.unwrap().total_tokens, 11);
    }

    #[tokio::test]
    async fn llm_and_categorize_nodes_resolve_their_own_tenant_models() {
        use axum::{Json, Router, routing::post};

        let requested_models = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let requested_models = requested_models.clone();
                move |Json(request): Json<Value>| {
                    let requested_models = requested_models.clone();
                    async move {
                        let model = request["model"].as_str().unwrap().to_owned();
                        requested_models.lock().unwrap().push(model.clone());
                        let content = match model.as_str() {
                            "category-model" => "product",
                            "answer-model" => "node-specific answer",
                            _ => "default model was used",
                        };
                        Json(json!({
                            "choices": [{"message": {"content": content}}],
                            "usage": {"prompt_tokens": 2, "completion_tokens": 1, "total_tokens": 3}
                        }))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let api_base = format!("http://{address}");
        let default_llm = LlmClient::new(crate::llm::LlmConfig {
            api_base: api_base.clone(),
            api_key: "default-secret".into(),
            model: "default-model".into(),
            ..crate::llm::LlmConfig::default()
        });
        let resolved_selectors = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let resolver = RecordingLlmResolver {
            api_base,
            selectors: resolved_selectors.clone(),
        };
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["categorize"], "upstream": []},
            "categorize": {"obj": {"component_name": "Categorize", "params": {
                "llm_id": "category-model@default@TestProvider",
                "category_description": {
                    "other": {"description": "anything else", "to": ["message-other"]},
                    "product": {"description": "product questions", "to": ["llm"]}
                }
            }}, "downstream": ["message-other", "llm"], "upstream": ["begin"]},
            "llm": {"obj": {"component_name": "LLM", "params": {
                "llm_id": "answer-model@default@TestProvider",
                "prompts": [{"role": "user", "content": "{sys.query}"}]
            }}, "downstream": ["message-answer"], "upstream": ["categorize"]},
            "message-other": {"obj": {"component_name": "Message", "params": {"content": ["other"]}}, "downstream": [], "upstream": ["categorize"]},
            "message-answer": {"obj": {"component_name": "Message", "params": {"content": ["{llm@content}"]}}, "downstream": [], "upstream": ["llm"]}
        }});
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                Some(&default_llm),
                WorkflowRunInput {
                    question: "Tell me about RayRAG",
                    user_id: "tenant-a",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: Some(&resolver),
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        server.abort();

        assert_eq!(result.answer, "node-specific answer");
        assert_eq!(
            *resolved_selectors.lock().unwrap(),
            [
                "category-model@default@TestProvider",
                "answer-model@default@TestProvider"
            ]
        );
        assert_eq!(
            *requested_models.lock().unwrap(),
            ["category-model", "answer-model"]
        );
    }

    #[tokio::test]
    #[ignore = "requires opt-in GPU model endpoints"]
    async fn gpu_model_endpoints_execute_categorize_embedding_and_reranking() {
        use crate::embed::{Embedder, OpenAIEmbedder};
        use crate::rerank::{RemoteReranker, Reranker};

        let llm_base = std::env::var("RAYRAG_TEST_LLM_BASE").unwrap();
        let embedding_base = std::env::var("RAYRAG_TEST_EMBEDDING_BASE").unwrap();
        let embedding_model = std::env::var("RAYRAG_TEST_EMBEDDING_MODEL").unwrap();
        let reranker_base = std::env::var("RAYRAG_TEST_RERANKER_BASE").unwrap();

        let embeddings = OpenAIEmbedder::new(&embedding_base, "test", &embedding_model)
            .embed(&["Rust ownership", "Bananas are yellow"])
            .await
            .unwrap();
        assert_eq!(embeddings.len(), 2);
        assert!(!embeddings[0].is_empty());
        assert_eq!(embeddings[0].len(), embeddings[1].len());
        assert!(embeddings.iter().flatten().all(|value| value.is_finite()));

        let mut reranker = RemoteReranker::new(&reranker_base);
        if let Some(key) = std::env::var("RAYRAG_TEST_RERANKER_KEY")
            .ok()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
        {
            reranker = reranker.with_api_key(&key);
        }
        let ranked = reranker
            .rerank(
                "什么是RAGFlow",
                &[
                    "RAGFlow是一个开源的RAG引擎".into(),
                    "这是一段与查询无关的文本".into(),
                ],
                2,
            )
            .await
            .unwrap();
        assert_eq!(ranked.len(), 2);
        // mxbai-rerank-large-v2 puts the RAGFlow-related document first.
        assert_eq!(ranked[0].0, 0);

        let llm = LlmClient::new(crate::llm::LlmConfig {
            api_base: llm_base,
            api_key: "test".into(),
            model: std::env::var("RAYRAG_TEST_LLM_MODEL").unwrap_or_else(|_| "default".into()),
            ..crate::llm::LlmConfig::default()
        });
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["categorize"], "upstream": []},
            "categorize": {"obj": {"component_name": "Categorize", "params": {
                "category_description": {
                    "rust": {"description": "questions about the Rust programming language", "examples": ["Explain Rust borrowing"], "to": ["message-rust"]},
                    "weather": {"description": "weather forecasts", "to": ["message-weather"]},
                    "other": {"description": "anything else", "to": ["message-other"]}
                }
            }}, "downstream": ["message-rust", "message-weather", "message-other"], "upstream": ["begin"]},
            "message-rust": {"obj": {"component_name": "Message", "params": {"content": ["rust"]}}, "downstream": [], "upstream": ["categorize"]},
            "message-weather": {"obj": {"component_name": "Message", "params": {"content": ["weather"]}}, "downstream": [], "upstream": ["categorize"]},
            "message-other": {"obj": {"component_name": "Message", "params": {"content": ["other"]}}, "downstream": [], "upstream": ["categorize"]}
        }});
        let result = AgentWorkflow::from_value(&dsl)
            .unwrap()
            .unwrap()
            .run(
                Some(&llm),
                WorkflowRunInput {
                    question: "How does ownership prevent memory bugs in Rust?",
                    user_id: "gpu-test",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        assert_eq!(result.answer, "rust");
        assert_eq!(result.path, ["begin", "categorize", "message-rust"]);
    }

    #[tokio::test]
    #[ignore = "requires opt-in GPU vision endpoint"]
    async fn gpu_vision_endpoint_describes_image_with_prompt() {
        use crate::vision::{VisionClient, VisionConfig};

        let vision_base = std::env::var("RAYRAG_TEST_VISION_BASE").unwrap();
        let vision_model =
            std::env::var("RAYRAG_TEST_VISION_MODEL").unwrap_or_else(|_| "default".into());

        // A tiny white image with a black rectangle, generated in pure Rust
        // as an RGB PNG (llama.cpp vision accepts PNG/JPEG, not PPM).
        fn encode_png(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
            use flate2::Compression;
            use flate2::write::ZlibEncoder;
            use std::io::Write;
            let mut raw = Vec::with_capacity((width * height * 3 + height) as usize);
            for y in 0..height {
                raw.push(0); // filter: none
                for x in 0..width {
                    let i = ((y * width + x) * 3) as usize;
                    raw.extend_from_slice(&pixels[i..i + 3]);
                }
            }
            let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
            z.write_all(&raw).unwrap();
            let idat = z.finish().unwrap();
            fn chunk(tag: &[u8; 4], data: &[u8]) -> Vec<u8> {
                // PNG chunk CRC-32 (IEEE 802.3, poly 0xEDB88320), no external crate.
                fn crc32(bytes: &[u8]) -> u32 {
                    let mut table = [0u32; 256];
                    for (i, entry) in table.iter_mut().enumerate() {
                        let mut c = i as u32;
                        for _ in 0..8 {
                            c = if c & 1 != 0 {
                                0xEDB8_8320 ^ (c >> 1)
                            } else {
                                c >> 1
                            };
                        }
                        *entry = c;
                    }
                    let mut crc = 0xFFFF_FFFFu32;
                    for &b in bytes {
                        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
                    }
                    crc ^ 0xFFFF_FFFF
                }
                let mut out = Vec::new();
                out.extend_from_slice(&(data.len() as u32).to_be_bytes());
                out.extend_from_slice(tag);
                out.extend_from_slice(data);
                out.extend_from_slice(&crc32(&[tag, data].concat()).to_be_bytes());
                out
            }
            let mut png = Vec::new();
            png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
            let mut ihdr = Vec::new();
            ihdr.extend_from_slice(&width.to_be_bytes());
            ihdr.extend_from_slice(&height.to_be_bytes());
            ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, RGB, deflate, adaptive, none
            png.extend_from_slice(&chunk(b"IHDR", &ihdr));
            png.extend_from_slice(&chunk(b"IDAT", &idat));
            png.extend_from_slice(&chunk(b"IEND", &[]));
            png
        }

        let (w, h) = (64u32, 24u32);
        let mut pixels = vec![255u8; (w * h * 3) as usize];
        for y in 8..16 {
            for x in 8..56 {
                let i = ((y * w + x) * 3) as usize;
                pixels[i] = 0;
                pixels[i + 1] = 0;
                pixels[i + 2] = 0;
            }
        }
        let data_url = VisionClient::normalize_image(&encode_png(w, h, &pixels), "image/png");

        let client = VisionClient::new(VisionConfig {
            api_base: vision_base,
            api_key: String::new(),
            model: vision_model,
            lang: "Chinese".into(),
        });
        let out = client
            .describe_with_prompt(
                &data_url,
                "What color is the rectangle? Answer in one word.",
            )
            .await
            .unwrap();
        assert!(!out.content.is_empty());
        // The black rectangle on white should be described as black/dark.
        assert!(
            out.content.to_ascii_lowercase().contains("black")
                || out.content.to_ascii_lowercase().contains("dark")
                || out.content.contains("黑")
        );
    }

    #[test]
    fn malformed_edges_and_switch_destinations_are_rejected() {
        let mut dsl = branch_canvas();
        dsl["components"]["begin"]["downstream"] = json!(["missing"]);
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("missing component")
        );

        let mut dsl = branch_canvas();
        dsl["components"]["switch:0"]["obj"]["params"]["conditions"][0]["to"] = json!(["missing"]);
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("missing destination")
        );

        let mut dsl = branch_canvas();
        dsl["components"]["switch:0"]["obj"]["params"]["conditions"][0]["to"] = json!(["assign:0"]);
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("not a declared downstream")
        );

        let mut dsl = branch_canvas();
        dsl["components"]["message:yes"]["downstream"] = json!(["assign:0"]);
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("contains a cycle")
        );
    }

    #[test]
    fn switch_dialects_validate_required_and_inferred_destinations() {
        let mut dsl = branch_canvas();
        dsl["components"]["switch:0"]["obj"]["params"]["conditions"] = json!([]);
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("conditions cannot be empty")
        );

        let mut dsl = branch_canvas();
        dsl["components"]["switch:0"]["obj"]["params"]["conditions"][0]["to"] = json!([]);
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("condition.to cannot be empty")
        );

        let mut dsl = branch_canvas();
        dsl["components"]["switch:0"]["obj"]["params"] = json!({
            "conditions": [{
                "op": "and",
                "clauses": [{"left": "{{env.score}}", "op": ">=", "right": 3}],
                "to": "message:yes"
            }],
            "default": "message:no"
        });
        AgentWorkflow::from_value(&dsl).unwrap().unwrap();

        dsl["components"]["switch:0"]["obj"]["params"]["default"] = json!("missing");
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("missing destination")
        );
    }

    #[test]
    fn retrieval_rejects_unimplemented_ragflow_branches_during_validation() {
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["retrieval"], "upstream": []},
            "retrieval": {"obj": {"component_name": "Retrieval", "params": {
                "retrieval_from": "memory", "memory_ids": ["memory-1"]
            }}, "downstream": [], "upstream": ["begin"]}
        }});
        assert!(
            AgentWorkflow::from_value(&dsl)
                .unwrap_err()
                .to_string()
                .contains("only dataset retrieval is implemented")
        );
    }

    #[test]
    fn legacy_prompt_only_dsl_remains_compatible() {
        assert!(AgentWorkflow::from_value(&json!({})).unwrap().is_none());
        assert!(
            AgentWorkflow::from_value(&json!({"components": []}))
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn user_fill_up_consumes_one_initial_scalar_without_interrupting() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["fill"], "upstream": []
            },
            "fill": {
                "obj": {"component_name": "UserFillUp", "params": {
                    "inputs": {"answer": {"type": "line"}},
                    "tips": "Answer the question"
                }},
                "downstream": ["done"], "upstream": ["begin"]
            },
            "done": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["answer={fill@answer}"]
                }},
                "downstream": [], "upstream": ["fill"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let outcome = workflow
            .run_interactive(
                None,
                WorkflowRunInput {
                    question: "forty-two",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        let WorkflowRunOutcome::Completed(result) = outcome else {
            panic!("single declared field should consume the initial query");
        };
        assert_eq!(result.answer, "answer=forty-two");
        assert_eq!(result.path, ["begin", "fill", "done"]);
        assert_eq!(result.trace[1].outputs["user_input"], "forty-two");
        assert_eq!(result.trace[1].outputs["answer"], "forty-two");
    }

    #[tokio::test]
    async fn user_fill_up_checkpoint_roundtrip_resumes_at_the_exact_cursor() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["fill"], "upstream": []
            },
            "fill": {
                "obj": {"component_name": "UserFillUp", "params": {
                    "enable_tips": true,
                    "inputs": {
                        "age": {"type": "line"},
                        "name": {"type": "line"}
                    },
                    "tips": "Complete the form"
                }},
                "downstream": ["done"], "upstream": ["begin"]
            },
            "done": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["{fill@name}:{fill@age}"]
                }},
                "downstream": [], "upstream": ["fill"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let first = workflow
            .run_interactive(
                None,
                WorkflowRunInput {
                    question: "not a structured form",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        let WorkflowRunOutcome::WaitingForUser(waiting) = first else {
            panic!("two fields require an interrupt for scalar initial input");
        };
        assert_eq!(waiting.component_id, "fill");
        assert_eq!(waiting.tips.as_deref(), Some("Complete the form"));
        assert_eq!(waiting.path, ["begin"]);
        assert_eq!(
            waiting
                .inputs
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["age", "name"]
        );
        let checkpoint: AgentWorkflowCheckpoint =
            serde_json::from_slice(&serde_json::to_vec(&waiting.checkpoint).unwrap()).unwrap();
        let resumed = workflow
            .resume_interactive(
                None,
                WorkflowRunInput {
                    question: "form response",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
                checkpoint,
                json!({"name": "Ada", "age": "37"}),
            )
            .await
            .unwrap();
        let WorkflowRunOutcome::Completed(result) = resumed else {
            panic!("one resume payload should complete this workflow");
        };
        assert_eq!(result.answer, "Ada:37");
        assert_eq!(result.path, ["begin", "fill", "done"]);
        assert_eq!(result.trace[1].outputs["name"], "Ada");
        assert_eq!(result.trace[1].outputs["age"], "37");
    }

    #[tokio::test]
    async fn user_fill_up_resume_does_not_rerun_upstream_retrieval() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["retrieve"], "upstream": []
            },
            "retrieve": {
                "obj": {"component_name": "Retrieval", "params": {
                    "query": "sys.query", "kb_ids": []
                }},
                "downstream": ["fill"], "upstream": ["begin"]
            },
            "fill": {
                "obj": {"component_name": "UserFillUp", "params": {
                    "inputs": {
                        "answer": {"type": "line"},
                        "reason": {"type": "line"}
                    }
                }},
                "downstream": ["done"], "upstream": ["retrieve"]
            },
            "done": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["{retrieve@formalized_content}|{fill@answer}"]
                }},
                "downstream": [], "upstream": ["fill"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let retriever = MockRetriever::default();
        let first = workflow
            .run_interactive(
                None,
                WorkflowRunInput {
                    question: "Rust ownership",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: Some(&retriever),
                    fallback_kb_ids: &["kb-1".into()],
                },
            )
            .await
            .unwrap();
        let WorkflowRunOutcome::WaitingForUser(waiting) = first else {
            panic!("form should interrupt");
        };
        assert_eq!(retriever.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let resumed = workflow
            .resume_interactive(
                None,
                WorkflowRunInput {
                    question: "resume",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: Some(&retriever),
                    fallback_kb_ids: &["kb-1".into()],
                },
                waiting.checkpoint,
                json!({"answer": "yes", "reason": "evidence"}),
            )
            .await
            .unwrap();
        assert!(matches!(resumed, WorkflowRunOutcome::Completed(_)));
        assert_eq!(retriever.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn begin_and_user_fill_up_share_the_initial_query_consumption_flag() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {
                    "inputs": {"seed": {"type": "line"}}
                }},
                "downstream": ["fill"], "upstream": []
            },
            "fill": {
                "obj": {"component_name": "UserFillUp", "params": {
                    "inputs": {"answer": {"type": "line"}}
                }},
                "downstream": ["done"], "upstream": ["begin"]
            },
            "done": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["{fill@answer}"]
                }},
                "downstream": [], "upstream": ["fill"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let outcome = workflow
            .run_interactive(
                None,
                WorkflowRunInput {
                    question: "consume once",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        let WorkflowRunOutcome::WaitingForUser(waiting) = outcome else {
            panic!("UserFillUp must not consume the Begin input a second time");
        };
        assert_eq!(waiting.path, ["begin"]);
        assert_eq!(
            waiting
                .checkpoint
                .runtime
                .sys
                .get("__initial_user_input_consumed__"),
            Some(&Value::Bool(true))
        );
    }

    #[tokio::test]
    async fn user_fill_up_file_service_gap_fails_closed() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["fill"], "upstream": []
            },
            "fill": {
                "obj": {"component_name": "UserFillUp", "params": {
                    "inputs": {"upload": {"type": "file", "optional": false}}
                }},
                "downstream": ["done"], "upstream": ["begin"]
            },
            "done": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["done"]
                }},
                "downstream": [], "upstream": ["fill"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let outcome = workflow
            .run_interactive(
                None,
                WorkflowRunInput {
                    question: "",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
            )
            .await
            .unwrap();
        let WorkflowRunOutcome::WaitingForUser(waiting) = outcome else {
            panic!("empty initial query should interrupt");
        };
        let error = workflow
            .resume_interactive(
                None,
                WorkflowRunInput {
                    question: "file",
                    user_id: "user-1",
                    inputs: &Map::new(),
                    history: &[],
                    generation: GenerationParamsPatch::default(),
                    llm_resolver: None,
                    retriever: None,
                    fallback_kb_ids: &[],
                },
                waiting.checkpoint,
                json!({"upload": {
                    "type": "file", "optional": false, "value": "file-id"
                }}),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("file parsing service"));
    }

    #[tokio::test]
    async fn loop_user_fill_up_resumes_the_same_iteration_across_restart() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["loop"], "upstream": []
            },
            "loop": {
                "obj": {"component_name": "Loop", "params": {
                    "loop_variables": [{
                        "variable": "counter",
                        "input_mode": "constant",
                        "value": 0,
                        "type": "number"
                    }],
                    "loop_termination_condition": [{
                        "variable": "counter",
                        "operator": "≥",
                        "value": 2,
                        "input_mode": "constant"
                    }],
                    "maximum_loop_count": 5
                }},
                "downstream": ["bump", "done"], "upstream": ["begin"]
            },
            "bump": {
                "obj": {"component_name": "VariableAssigner", "params": {
                    "variables": [{
                        "variable": "loop@counter",
                        "operator": "+=",
                        "parameter": 1
                    }]
                }},
                "parent_id": "loop",
                "downstream": ["fill"], "upstream": ["loop"]
            },
            "fill": {
                "obj": {"component_name": "UserFillUp", "params": {
                    "enable_tips": true,
                    "tips": "iteration {loop@counter}",
                    "inputs": {"answer": {"type": "line", "optional": false}}
                }},
                "parent_id": "loop",
                "downstream": [], "upstream": ["bump"]
            },
            "done": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["{fill@answer}:{loop@counter}"]
                }},
                "downstream": [], "upstream": ["loop"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let inputs = Map::new();
        let input = || WorkflowRunInput {
            question: "",
            user_id: "user-1",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };

        let first = workflow.run_interactive(None, input()).await.unwrap();
        let WorkflowRunOutcome::WaitingForUser(first) = first else {
            panic!("first loop iteration should interrupt");
        };
        assert_eq!(first.component_id, "fill");
        assert_eq!(first.interrupt_id, "loop:loop:1:fill");
        assert_eq!(first.tips.as_deref(), Some("iteration 1.0"));
        assert_eq!(first.path, ["begin"]);
        assert_eq!(first.checkpoint.runtime.outputs["loop"]["counter"], 1.0);
        let checkpoint: AgentWorkflowCheckpoint =
            serde_json::from_value(serde_json::to_value(first.checkpoint).unwrap()).unwrap();

        let second = workflow
            .resume_interactive(None, input(), checkpoint, json!("first"))
            .await
            .unwrap();
        let WorkflowRunOutcome::WaitingForUser(second) = second else {
            panic!("second loop iteration should interrupt");
        };
        assert_eq!(second.interrupt_id, "loop:loop:2:fill");
        assert_eq!(second.tips.as_deref(), Some("iteration 2.0"));
        assert_eq!(second.path, ["begin"]);
        assert_eq!(second.checkpoint.runtime.outputs["loop"]["counter"], 2.0);
        assert_eq!(second.checkpoint.runtime.outputs["fill"]["answer"], "first");
        let checkpoint: AgentWorkflowCheckpoint =
            serde_json::from_value(serde_json::to_value(second.checkpoint).unwrap()).unwrap();

        let completed = workflow
            .resume_interactive(None, input(), checkpoint, json!("second"))
            .await
            .unwrap();
        let WorkflowRunOutcome::Completed(completed) = completed else {
            panic!("loop should complete after the second response");
        };
        assert_eq!(completed.answer, "second:2.0");
        assert_eq!(completed.path, ["begin", "loop", "done"]);
    }

    #[tokio::test]
    async fn parallel_user_fill_up_snapshots_all_items_and_resumes_each_once() {
        let dsl = json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["parallel"], "upstream": []
            },
            "parallel": {
                "obj": {"component_name": "Parallel", "params": {
                    "items_ref": "begin@items",
                    "max_concurrency": 3,
                    "outputs": {
                        "answers": {"ref": "fill@answer"},
                        "items": {"ref": "item"}
                    }
                }},
                "downstream": ["done"], "upstream": ["begin"]
            },
            "fill": {
                "obj": {"component_name": "UserFillUp", "params": {
                    "inputs": {"answer": {"type": "line", "optional": false}}
                }},
                "parent_id": "parallel",
                "downstream": [], "upstream": ["parallel"]
            },
            "done": {
                "obj": {"component_name": "Message", "params": {
                    "content": ["{parallel@answers}"]
                }},
                "downstream": [], "upstream": ["parallel"]
            }
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        let inputs = Map::from_iter([("items".into(), json!(["one", "two", "three"]))]);
        let input = || WorkflowRunInput {
            question: "",
            user_id: "user-1",
            inputs: &inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        };

        let mut outcome = workflow.run_interactive(None, input()).await.unwrap();
        for (index, answer) in ["alpha", "beta", "gamma"].into_iter().enumerate() {
            let WorkflowRunOutcome::WaitingForUser(waiting) = outcome else {
                panic!("parallel item {index} should be waiting");
            };
            assert_eq!(waiting.component_id, "fill");
            assert_eq!(
                waiting.interrupt_id,
                format!("parallel:parallel:{index}:fill")
            );
            assert_eq!(waiting.path, ["begin"]);
            let Some(CompositeCheckpoint::Parallel(state)) = waiting.checkpoint.composite.as_ref()
            else {
                panic!("parallel wait must persist composite state");
            };
            assert_eq!(state.active_index, index);
            assert_eq!(state.completed_items.len(), index);
            assert_eq!(state.pending_items.len(), 3 - index);
            assert_eq!(
                Value::Array(state.original_items.clone()),
                json!(["one", "two", "three"])
            );
            if index == 0 {
                let mut malformed = waiting.checkpoint.clone();
                malformed.waiting_component_id = "done".into();
                let error = workflow
                    .resume_interactive(None, input(), malformed, json!("wrong"))
                    .await
                    .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("composite checkpoint leaf does not match")
                );
            }
            let checkpoint: AgentWorkflowCheckpoint =
                serde_json::from_value(serde_json::to_value(waiting.checkpoint).unwrap()).unwrap();
            outcome = workflow
                .resume_interactive(None, input(), checkpoint, json!(answer))
                .await
                .unwrap();
        }
        let WorkflowRunOutcome::Completed(completed) = outcome else {
            panic!("parallel should complete after one response per item");
        };
        assert_eq!(completed.answer, r#"["alpha","beta","gamma"]"#);
        assert_eq!(completed.path, ["begin", "parallel", "done"]);
        let parallel = completed
            .trace
            .iter()
            .find(|trace| trace.component_id == "parallel")
            .unwrap();
        assert_eq!(parallel.outputs["items"], json!(["one", "two", "three"]));
        assert_eq!(
            parallel.outputs["answers"],
            json!(["alpha", "beta", "gamma"])
        );
    }

    #[derive(Default)]
    struct StubInvokeNetwork {
        addresses: std::sync::Mutex<BTreeMap<String, Vec<IpAddr>>>,
        outcomes:
            std::sync::Mutex<std::collections::VecDeque<std::result::Result<Vec<u8>, String>>>,
        resolves: std::sync::Mutex<Vec<(String, u16)>>,
        requests: std::sync::Mutex<Vec<InvokeHttpRequest>>,
    }

    impl StubInvokeNetwork {
        fn with_outcomes(
            outcomes: impl IntoIterator<Item = std::result::Result<Vec<u8>, String>>,
        ) -> Self {
            Self {
                outcomes: std::sync::Mutex::new(outcomes.into_iter().collect()),
                ..Self::default()
            }
        }

        fn resolve_to(&self, hostname: &str, addresses: Vec<IpAddr>) {
            self.addresses
                .lock()
                .unwrap()
                .insert(hostname.to_owned(), addresses);
        }
    }

    #[async_trait::async_trait]
    impl InvokeNetwork for StubInvokeNetwork {
        async fn resolve(&self, hostname: &str, port: u16) -> Result<Vec<IpAddr>> {
            self.resolves
                .lock()
                .unwrap()
                .push((hostname.to_owned(), port));
            Ok(self
                .addresses
                .lock()
                .unwrap()
                .get(hostname)
                .cloned()
                .unwrap_or_else(|| vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]))
        }

        async fn send(&self, request: InvokeHttpRequest) -> Result<Vec<u8>> {
            self.requests.lock().unwrap().push(request);
            match self.outcomes.lock().unwrap().pop_front() {
                Some(Ok(body)) => Ok(body),
                Some(Err(error)) => Err(anyhow!(error)),
                None => Ok(b"ok".to_vec()),
            }
        }
    }

    fn invoke_canvas(params: Value) -> Value {
        json!({"components": {
            "begin": {
                "obj": {"component_name": "Begin", "params": {}},
                "downstream": ["invoke"],
                "upstream": []
            },
            "invoke": {
                "obj": {"component_name": "Invoke", "params": params},
                "downstream": [],
                "upstream": ["begin"]
            }
        }})
    }

    fn invoke_node(params: Value) -> CanvasNode {
        CanvasNode {
            id: "invoke".into(),
            component_name: "Invoke".into(),
            params: params.as_object().cloned().unwrap(),
            downstream: Vec::new(),
            upstream: vec!["begin".into()],
            parent_id: None,
        }
    }

    fn invoke_run_input(inputs: &Map<String, Value>) -> WorkflowRunInput<'_> {
        WorkflowRunInput {
            question: "",
            user_id: "user",
            inputs,
            history: &[],
            generation: GenerationParamsPatch::default(),
            llm_resolver: None,
            retriever: None,
            fallback_kb_ids: &[],
        }
    }

    #[test]
    fn invoke_compile_validation_matches_the_fixed_python_param_contract() {
        let valid = json!({
            "url": "api.example.test/{{sys.user}}",
            "method": "PUT",
            "timeout": 60,
            "headers": "{}",
            "proxy": "",
            "clean_html": false,
            "datatype": "formdata",
            "variables": [{"key": "q", "ref": "sys.query", "value": "fallback"}],
            "max_retries": 2,
            "delay_after_error": 0.25
        });
        let workflow = AgentWorkflow::from_value(&invoke_canvas(valid.clone()))
            .unwrap()
            .unwrap();
        assert!(workflow.unsupported_components().is_empty());

        for (field, value, message) in [
            (
                "method",
                json!("DELETE"),
                "method must be GET, POST, or PUT",
            ),
            ("url", json!(""), "url cannot be empty"),
            ("timeout", json!(0), "timeout must be a positive integer"),
            ("timeout", json!(1.5), "timeout must be a positive integer"),
            ("clean_html", json!("false"), "clean_html must be a boolean"),
            (
                "datatype",
                json!("text"),
                "datatype must be 'json' or 'formdata'",
            ),
            ("variables", json!({}), "variables must be an array"),
            (
                "max_retries",
                json!(-1),
                "max_retries must be a non-negative integer",
            ),
        ] {
            let mut params = valid.clone();
            params[field] = value;
            let error = AgentWorkflow::from_value(&invoke_canvas(params)).unwrap_err();
            assert!(error.to_string().contains(message), "{field}: {error}");
        }
    }

    #[tokio::test]
    async fn invoke_resolves_templates_headers_and_json_arguments_without_live_network() {
        let node = invoke_node(json!({
            "url": "api.example.test/v1/{{sys.user}}",
            "method": "GET",
            "timeout": 15,
            "headers": r#"{
                "Authorization":"Bearer {token}",
                "X-Environment":"{env.api_key}",
                "X-Missing":"prefix-{missing}"
            }"#,
            "clean_html": true,
            "datatype": "json",
            "variables": [
                {"key":"payload", "ref":"begin@payload", "value":"fallback"},
                {"key":"count", "value":"{{sys.count}}"},
                {"key":"fallback", "ref":"missing@value", "value":"user={sys.user}"}
            ]
        }));
        let network = StubInvokeNetwork::with_outcomes([Ok(
            b"<h1>Hello</h1>\n<script>bad()</script><p>World</p>".to_vec(),
        )]);
        let mut runtime = CanvasRuntime::default();
        runtime.sys.insert("user".into(), json!("alice"));
        runtime.sys.insert("count".into(), json!(3));
        runtime.env.insert("api_key".into(), json!("env-secret"));
        runtime.outputs.insert(
            "begin".into(),
            Map::from_iter([("payload".into(), json!(r#"{"ok":true}"#))]),
        );
        let inputs = Map::from_iter([("token".into(), json!("request-token"))]);

        execute_invoke_with_network(&mut runtime, &node, &invoke_run_input(&inputs), &network)
            .await
            .unwrap();

        // RAGFlow html_parser semantics: h1 gets a markdown # prefix,
        // <script> is stripped, <p> text is kept.
        assert_eq!(runtime.outputs["invoke"]["result"], "# Hello\nWorld");
        let requests = network.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.method, "get");
        assert_eq!(request.url.as_str(), "http://api.example.test/v1/alice");
        assert_eq!(request.timeout, Duration::from_secs(15));
        assert_eq!(
            request.target,
            InvokeEndpointPin {
                hostname: "api.example.test".into(),
                ip: IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))
            }
        );
        assert_eq!(request.headers["Authorization"], "Bearer request-token");
        assert_eq!(request.headers["X-Environment"], "env-secret");
        assert_eq!(request.headers["X-Missing"], "prefix-");
        assert_eq!(request.arguments["payload"], json!({"ok": true}));
        assert_eq!(request.arguments["count"], json!(3));
        assert_eq!(request.arguments["fallback"], "user=alice");
    }

    #[tokio::test]
    async fn invoke_retries_form_requests_and_records_terminal_transport_errors() {
        let node = invoke_node(json!({
            "url": "https://8.8.8.8/submit",
            "method": "POST",
            "datatype": "formdata",
            "variables": [{"key":"tag", "value":["a", "b"]}],
            "max_retries": 1,
            "delay_after_error": 0
        }));
        let network = StubInvokeNetwork::with_outcomes([
            Err("temporary failure".into()),
            Ok(b"accepted".to_vec()),
        ]);
        let mut runtime = CanvasRuntime::default();
        let inputs = Map::new();
        execute_invoke_with_network(&mut runtime, &node, &invoke_run_input(&inputs), &network)
            .await
            .unwrap();
        assert_eq!(runtime.outputs["invoke"]["result"], "accepted");
        {
            let requests = network.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(requests.iter().all(|request| {
                request.method == "post" && request.datatype == InvokeDataType::FormData
            }));
            assert_eq!(
                invoke_form_pairs(&requests[0].arguments),
                [("tag".into(), "a".into()), ("tag".into(), "b".into())]
            );
        }

        let terminal = StubInvokeNetwork::with_outcomes([Err("connection refused".into())]);
        let mut terminal_runtime = CanvasRuntime::default();
        let mut terminal_node = node.clone();
        terminal_node.params.insert("max_retries".into(), json!(0));
        execute_invoke_with_network(
            &mut terminal_runtime,
            &terminal_node,
            &invoke_run_input(&inputs),
            &terminal,
        )
        .await
        .unwrap();
        assert_eq!(
            terminal_runtime.outputs["invoke"]["_ERROR"],
            "connection refused"
        );
    }

    #[tokio::test]
    async fn invoke_ssrf_guard_checks_every_address_and_both_proxy_hops() {
        let inputs = Map::new();
        for url in [
            "http://127.0.0.1:9380/private",
            "169.254.169.254/latest/meta-data/",
            "http://[::ffff:127.0.0.1]/private",
        ] {
            let network = StubInvokeNetwork::default();
            let node = invoke_node(json!({"url": url}));
            let mut runtime = CanvasRuntime::default();
            execute_invoke_with_network(&mut runtime, &node, &invoke_run_input(&inputs), &network)
                .await
                .unwrap();
            assert_eq!(runtime.outputs["invoke"]["_ERROR"], "URL not valid");
            assert!(network.requests.lock().unwrap().is_empty());
        }

        let mixed_dns = StubInvokeNetwork::default();
        mixed_dns.resolve_to(
            "mixed.example.test",
            vec![
                IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
            ],
        );
        let mut runtime = CanvasRuntime::default();
        execute_invoke_with_network(
            &mut runtime,
            &invoke_node(json!({"url":"https://mixed.example.test"})),
            &invoke_run_input(&inputs),
            &mixed_dns,
        )
        .await
        .unwrap();
        assert_eq!(runtime.outputs["invoke"]["_ERROR"], "URL not valid");
        assert!(mixed_dns.requests.lock().unwrap().is_empty());

        let unsafe_proxy = StubInvokeNetwork::default();
        let mut runtime = CanvasRuntime::default();
        execute_invoke_with_network(
            &mut runtime,
            &invoke_node(json!({
                "url":"https://8.8.8.8/api",
                "proxy":"http://127.0.0.1:8080"
            })),
            &invoke_run_input(&inputs),
            &unsafe_proxy,
        )
        .await
        .unwrap();
        assert_eq!(runtime.outputs["invoke"]["_ERROR"], "URL not valid");
        assert!(unsafe_proxy.requests.lock().unwrap().is_empty());

        let hostname_through_proxy = StubInvokeNetwork::default();
        let mut runtime = CanvasRuntime::default();
        execute_invoke_with_network(
            &mut runtime,
            &invoke_node(json!({
                "url":"https://api.example.test/data",
                "proxy":"proxy.example.test:8080"
            })),
            &invoke_run_input(&inputs),
            &hostname_through_proxy,
        )
        .await
        .unwrap();
        assert_eq!(runtime.outputs["invoke"]["_ERROR"], "URL not valid");
        assert!(hostname_through_proxy.requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invoke_rejects_malformed_headers_before_network_and_caps_response() {
        let inputs = Map::new();
        let network = StubInvokeNetwork::default();
        let mut runtime = CanvasRuntime::default();
        execute_invoke_with_network(
            &mut runtime,
            &invoke_node(json!({"url":"https://8.8.8.8", "headers":"[]"})),
            &invoke_run_input(&inputs),
            &network,
        )
        .await
        .unwrap();
        assert_eq!(
            runtime.outputs["invoke"]["_ERROR"],
            "Invoke headers must be a JSON object."
        );
        assert!(network.requests.lock().unwrap().is_empty());

        let oversized =
            StubInvokeNetwork::with_outcomes([Ok(vec![b'x'; MAX_INVOKE_RESPONSE_BODY + 32])]);
        let mut capped_runtime = CanvasRuntime::default();
        execute_invoke_with_network(
            &mut capped_runtime,
            &invoke_node(json!({"url":"https://8.8.8.8"})),
            &invoke_run_input(&inputs),
            &oversized,
        )
        .await
        .unwrap();
        assert_eq!(
            capped_runtime.outputs["invoke"]["result"]
                .as_str()
                .unwrap()
                .len(),
            MAX_INVOKE_RESPONSE_BODY
        );
    }

    #[test]
    fn invoke_public_address_allowlist_blocks_special_purpose_ranges() {
        for address in [
            "0.1.2.3",
            "10.0.0.1",
            "100.64.0.1",
            "127.0.0.1",
            "169.254.169.254",
            "172.16.0.1",
            "192.168.1.1",
            "198.51.100.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "fc00::1",
            "fe80::1",
        ] {
            assert!(
                !invoke_ip_is_public(address.parse().unwrap()),
                "{address} must not be reachable by Invoke"
            );
        }
        for address in ["8.8.8.8", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(invoke_ip_is_public(address.parse().unwrap()), "{address}");
        }
    }

    #[test]
    fn unsupported_components_are_reported_before_execution() {
        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["browser"], "upstream": []},
            "browser": {"obj": {"component_name": "Browser", "params": {}}, "downstream": [], "upstream": ["begin"]}
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        assert_eq!(workflow.unsupported_components(), ["browser (Browser)"]);

        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["fill"], "upstream": []},
            "fill": {"obj": {"component_name": "UserFillUp", "params": {
                "inputs": {"answer": {"type": "line"}},
                "tips": "answer required"
            }}, "downstream": [], "upstream": ["begin"]}
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        assert!(workflow.unsupported_components().is_empty());

        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["loop"], "upstream": []},
            "loop": {"obj": {"component_name": "Loop", "params": {
                "loop_variables": [{
                    "variable": "counter",
                    "input_mode": "constant",
                    "value": 0,
                    "type": "number"
                }],
                "loop_termination_condition": [{
                    "variable": "counter",
                    "operator": "≥",
                    "value": 3,
                    "input_mode": "constant"
                }],
                "maximum_loop_count": 10
            }}, "downstream": [], "upstream": ["begin"]}
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        assert!(workflow.unsupported_components().is_empty());

        let dsl = json!({"components": {
            "begin": {"obj": {"component_name": "Begin", "params": {}}, "downstream": ["parallel"], "upstream": []},
            "parallel": {"obj": {"component_name": "Parallel", "params": {
                "items_ref": "sys.items",
                "max_concurrency": 4,
                "outputs": {"values": {"ref": "body@result"}}
            }}, "downstream": [], "upstream": ["begin"]},
            "body": {"obj": {"component_name": "StringTransform", "params": {
                "method": "merge",
                "script": "{item}",
                "delimiters": ["|"]
            }}, "parent_id": "parallel", "downstream": [], "upstream": ["parallel"]}
        }});
        let workflow = AgentWorkflow::from_value(&dsl).unwrap().unwrap();
        assert!(workflow.unsupported_components().is_empty());
    }
}

// ---------------------------------------------------------------------------
// canvas.py remaining semantics (appended; pure helpers, no runtime changes).
//
// These functions mirror the parts of `agent/canvas.py` that were not yet
// ported: `Canvas.reset` globals handling (`sys.*` typed resets and `env.*`
// restoration from the canvas variables table), `get_history` windowing,
// `is_reff`, the retrieval reference accumulator (`add_reference` /
// `_has_reference` / `_build_message_end`) and the tool-use trace merge from
// `tool_use_callback`. All are standalone so they can be tested in isolation
// and reused by sync and async execution paths alike.
// ---------------------------------------------------------------------------

/// Empty retrieval slot used by `add_retrieval_reference` when no reference
/// was recorded yet (`canvas.py`: `self.retrieval = [{"chunks": {}, "doc_aggs": {}}]`).
pub fn empty_retrieval_slot() -> Map<String, Value> {
    let mut slot = Map::new();
    slot.insert("chunks".to_owned(), Value::Object(Map::new()));
    slot.insert("doc_aggs".to_owned(), Value::Object(Map::new()));
    slot
}

/// `Canvas.reset(mem=False)` for `sys.*` globals: string → "", int/bool → 0,
/// float → 0, list → [], dict → {}, anything else → null. Mirrors the Python
/// `isinstance` dispatch (bool is an int subclass in Python, hence 0).
pub fn reset_sys_globals(globals: &mut Map<String, Value>) {
    let keys: Vec<String> = globals
        .keys()
        .filter(|key| key.starts_with("sys."))
        .cloned()
        .collect();
    for key in keys {
        let reset = match globals.get(&key) {
            Some(Value::String(_)) => Value::String(String::new()),
            Some(Value::Number(_)) => Value::Number(0.into()),
            Some(Value::Bool(_)) => Value::Number(0.into()),
            Some(Value::Array(_)) => Value::Array(Vec::new()),
            Some(Value::Object(_)) => Value::Object(Map::new()),
            _ => Value::Null,
        };
        globals.insert(key, reset);
    }
}

/// `Canvas.reset(mem=False)` for `env.*` globals: restore each variable from
/// the canvas variables table — the declared `value` wins when present,
/// otherwise the type-based default (`number` → 0, `boolean` → false,
/// `object` → {}, `array*` → [], anything else → ""). Variables missing from
/// the table reset to "".
pub fn reset_env_globals(globals: &mut Map<String, Value>, variables: &Map<String, Value>) {
    let keys: Vec<String> = globals
        .keys()
        .filter(|key| key.starts_with("env."))
        .cloned()
        .collect();
    for key in keys {
        let var_key = &key[4..];
        let reset = match variables.get(var_key) {
            Some(variable) => match variable.get("value") {
                Some(value) if !value.is_null() => value.clone(),
                _ => {
                    let var_type = variable.get("type").and_then(Value::as_str).unwrap_or("");
                    match var_type {
                        "number" => Value::Number(0.into()),
                        "boolean" => Value::Bool(false),
                        "object" => Value::Object(Map::new()),
                        kind if kind.starts_with("array") => Value::Array(Vec::new()),
                        _ => Value::String(String::new()),
                    }
                }
            },
            None => Value::String(String::new()),
        };
        globals.insert(key, reset);
    }
}

/// `Canvas.get_history(window_size)`: last `window_size * 2` history entries
/// as `{"role", "content"}` objects. Dict entries expose their `content`
/// field (default ""), other values are stringified. `window_size <= 0`
/// returns an empty list.
pub fn get_history_window(history: &[(String, Value)], window_size: usize) -> Vec<Value> {
    if window_size == 0 {
        return Vec::new();
    }
    let start = history.len().saturating_sub(window_size * 2);
    history[start..]
        .iter()
        .map(|(role, obj)| {
            let content = match obj {
                Value::Object(map) => map
                    .get("content")
                    .cloned()
                    .unwrap_or_else(|| Value::String(String::new())),
                Value::String(text) => Value::String(text.clone()),
                other => Value::String(other.to_string()),
            };
            let mut entry = Map::new();
            entry.insert("role".to_owned(), Value::String(role.clone()));
            entry.insert("content".to_owned(), content);
            Value::Object(entry)
        })
        .collect()
}

/// `Canvas.is_reff(exp)`: true when `exp` is a resolvable canvas reference —
/// a bare global name present in `globals`, or exactly `component@var` with a
/// known component id. Mirrors the Python `strip("{")`/`strip("}")` handling
/// and the strict two-part `@` split.
pub fn is_canvas_reference(
    expression: &str,
    globals: &Map<String, Value>,
    component_ids: &BTreeSet<String>,
) -> bool {
    let expression = expression.trim_matches('{').trim_matches('}');
    let Some((component_id, _)) = expression.split_once('@') else {
        return globals.contains_key(expression);
    };
    if expression.split('@').count() != 2 {
        return false;
    }
    component_ids.contains(component_id)
}

/// `Canvas.add_reference(chunks, doc_infos)`: merge formatted chunks and doc
/// infos into the latest retrieval slot. Chunk ids are hashed with
/// `hash_str2int(id, 500)` and deduplicated; doc aggs deduplicate by
/// `doc_name`. A missing retrieval list is seeded with an empty slot.
pub fn add_retrieval_reference(
    retrieval: &mut Vec<Map<String, Value>>,
    chunks: &[Value],
    doc_infos: &[Value],
) {
    if retrieval.is_empty() {
        retrieval.push(empty_retrieval_slot());
    }
    let slot = retrieval
        .last_mut()
        .expect("retrieval slot was just seeded");
    let chunks_map = slot
        .entry("chunks".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(chunks_map) = chunks_map {
        for chunk in chunks {
            let Some(id) = chunk.get("id").and_then(Value::as_str) else {
                continue;
            };
            let cid = hash_str2int(id, 500).to_string();
            if !chunks_map.contains_key(&cid) {
                chunks_map.insert(cid, chunk.clone());
            }
        }
    }
    let doc_aggs = slot
        .entry("doc_aggs".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if let Value::Object(doc_aggs) = doc_aggs {
        for doc in doc_infos {
            let Some(name) = doc.get("doc_name").and_then(Value::as_str) else {
                continue;
            };
            if !doc_aggs.contains_key(name) {
                doc_aggs.insert(name.to_owned(), doc.clone());
            }
        }
    }
}

/// `Canvas._has_reference()`: the latest retrieval slot carries at least one
/// chunk or doc aggregation.
pub fn retrieval_has_reference(retrieval: &[Map<String, Value>]) -> bool {
    let Some(slot) = retrieval.last() else {
        return false;
    };
    let non_empty = |key: &str| {
        slot.get(key)
            .and_then(Value::as_object)
            .is_some_and(|entries| !entries.is_empty())
    };
    non_empty("chunks") || non_empty("doc_aggs")
}

/// `Canvas._build_message_end(cpn_obj)`: status (when set), attachment (only
/// when it is an object) and the latest retrieval reference (when present).
pub fn build_message_end(
    status: Option<&str>,
    attachment: Option<&Value>,
    retrieval: &[Map<String, Value>],
) -> Map<String, Value> {
    let mut end = Map::new();
    if let Some(status) = status {
        end.insert("status".to_owned(), Value::String(status.to_owned()));
    }
    if let Some(Value::Object(attachment)) = attachment {
        end.insert("attachment".to_owned(), Value::Object(attachment.clone()));
    }
    if retrieval_has_reference(retrieval) {
        end.insert(
            "reference".to_owned(),
            Value::Object(
                retrieval
                    .last()
                    .expect("has reference implies non-empty")
                    .clone(),
            ),
        );
    }
    end
}

/// One tool-use trace entry appended by `tool_use_callback`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolUseTrace {
    pub path: String,
    pub tool_name: String,
    pub arguments: Value,
    pub result: Value,
    pub elapsed_time: Option<f64>,
}

impl ToolUseTrace {
    pub fn to_value(&self) -> Value {
        let mut entry = Map::new();
        entry.insert("path".to_owned(), Value::String(self.path.clone()));
        entry.insert(
            "tool_name".to_owned(),
            Value::String(self.tool_name.clone()),
        );
        entry.insert("arguments".to_owned(), self.arguments.clone());
        entry.insert("result".to_owned(), self.result.clone());
        entry.insert(
            "elapsed_time".to_owned(),
            self.elapsed_time.map(Value::from).unwrap_or(Value::Null),
        );
        Value::Object(entry)
    }
}

/// `Canvas.tool_use_callback` merge logic (the Redis persistence layer is the
/// caller's concern): append the trace to the last log entry when it belongs
/// to the same component id, otherwise start a new entry.
pub fn merge_tool_use_trace(logs: &mut Vec<Value>, component_id: &str, trace: &ToolUseTrace) {
    let trace_value = trace.to_value();
    if let Some(last) = logs.last_mut()
        && let Some(last_id) = last.get("component_id").and_then(Value::as_str)
        && last_id == component_id
        && let Some(trace_arr) = last.get_mut("trace").and_then(Value::as_array_mut)
    {
        trace_arr.push(trace_value);
        return;
    }
    let mut entry = Map::new();
    entry.insert(
        "component_id".to_owned(),
        Value::String(component_id.to_owned()),
    );
    entry.insert("trace".to_owned(), Value::Array(vec![trace_value]));
    logs.push(Value::Object(entry));
}

/// `Canvas.tts` text cleaning (`clean_tts_text`): drop C0 control characters,
/// drop emoji blocks, collapse whitespace, cap at 500 chars. Returns the
/// cleaned text (empty input stays empty).
pub fn clean_tts_text(text: &str) -> String {
    const MAX_LEN: usize = 500;
    let mut cleaned = String::with_capacity(text.len());
    for ch in text.chars() {
        let code = ch as u32;
        let is_control = code <= 0x08
            || (0x0B..=0x0C).contains(&code)
            || (0x0E..=0x1F).contains(&code)
            || code == 0x7F;
        let is_emoji = (0x1F600..=0x1F64F).contains(&code)
            || (0x1F300..=0x1F5FF).contains(&code)
            || (0x1F680..=0x1F6FF).contains(&code)
            || (0x1F1E0..=0x1F1FF).contains(&code)
            || (0x2700..=0x27BF).contains(&code)
            || (0x1F900..=0x1F9FF).contains(&code)
            || (0x1FA70..=0x1FAFF).contains(&code);
        if is_control || is_emoji {
            continue;
        }
        cleaned.push(ch);
    }
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > MAX_LEN {
        collapsed.chars().take(MAX_LEN).collect()
    } else {
        collapsed
    }
}

#[cfg(test)]
mod canvas_semantics_tests {
    use super::*;
    use serde_json::json;

    fn map_of(entries: &[(&str, Value)]) -> Map<String, Value> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn reset_globals_restores_sys_types_and_env_variable_defaults() {
        let mut globals = map_of(&[
            ("sys.query", json!("hello")),
            ("sys.conversation_turns", json!(3)),
            ("sys.files", json!(["f1"])),
            ("sys.history", json!(["user: hi"])),
            ("env.api_key", json!("old")),
            ("env.limit", json!(42)),
            ("env.debug", json!(true)),
            ("env.profile", json!({"a": 1})),
            ("env.tags", json!(["x"])),
            ("env.missing_src", json!("v")),
        ]);
        let variables = map_of(&[
            ("api_key", json!({"type": "string", "value": "new-key"})),
            ("limit", json!({"type": "number"})),
            ("debug", json!({"type": "boolean"})),
            ("profile", json!({"type": "object"})),
            ("tags", json!({"type": "array<string>"})),
            ("missing_src", json!({"type": "string"})),
        ]);
        reset_sys_globals(&mut globals);
        assert_eq!(globals["sys.query"], json!(""));
        assert_eq!(globals["sys.conversation_turns"], json!(0));
        assert_eq!(globals["sys.files"], json!([]));
        assert_eq!(globals["sys.history"], json!([]));
        // A boolean sys.* global resets to 0 (bool is an int in Python).
        assert_eq!(globals["sys.conversation_turns"], json!(0));

        reset_env_globals(&mut globals, &variables);
        // Declared value wins.
        assert_eq!(globals["env.api_key"], json!("new-key"));
        // Type-based defaults when no value is declared.
        assert_eq!(globals["env.limit"], json!(0));
        assert_eq!(globals["env.debug"], json!(false));
        assert_eq!(globals["env.profile"], json!({}));
        assert_eq!(globals["env.tags"], json!([]));
        assert_eq!(globals["env.missing_src"], json!(""));
        // Unknown variable name → "".
        globals.insert("env.ghost".to_owned(), json!("x"));
        reset_env_globals(&mut globals, &variables);
        assert_eq!(globals["env.ghost"], json!(""));
    }

    #[test]
    fn history_windowing_and_reference_checks_match_canvas() {
        let history: Vec<(String, Value)> = vec![
            ("user".into(), json!("q1")),
            ("assistant".into(), json!({"content": "a1"})),
            ("user".into(), json!("q2")),
            ("assistant".into(), json!("a2")),
        ];
        // window_size 0 → empty.
        assert!(get_history_window(&history, 0).is_empty());
        // window_size 1 → last 2 entries.
        let window = get_history_window(&history, 1);
        assert_eq!(window.len(), 2);
        assert_eq!(window[0], json!({"role": "user", "content": "q2"}));
        assert_eq!(window[1], json!({"role": "assistant", "content": "a2"}));
        // window_size larger than history → everything, dict content extracted.
        let window = get_history_window(&history, 10);
        assert_eq!(window.len(), 4);
        assert_eq!(window[1], json!({"role": "assistant", "content": "a1"}));

        let globals = map_of(&[("sys.query", json!("q"))]);
        let ids: BTreeSet<String> = ["begin".to_owned(), "retrieval_0".to_owned()]
            .into_iter()
            .collect();
        assert!(is_canvas_reference("{{sys.query}}", &globals, &ids));
        assert!(is_canvas_reference("retrieval_0@content", &globals, &ids));
        assert!(!is_canvas_reference("retrieval_0@a@b", &globals, &ids));
        assert!(!is_canvas_reference("missing@content", &globals, &ids));
        assert!(!is_canvas_reference("sys.nope", &globals, &ids));
    }

    #[test]
    fn reference_accumulation_dedupes_and_builds_message_end() {
        let mut retrieval: Vec<Map<String, Value>> = vec![];
        let chunks = vec![
            json!({"id": "chunk-1", "content": "c1"}),
            json!({"id": "chunk-1", "content": "c1-dup"}),
            json!({"id": "chunk-2", "content": "c2"}),
        ];
        let docs = vec![
            json!({"doc_name": "doc-a", "count": 1}),
            json!({"doc_name": "doc-a", "count": 2}),
        ];
        add_retrieval_reference(&mut retrieval, &chunks, &docs);
        let slot = retrieval.last().unwrap();
        let chunks_map = slot["chunks"].as_object().unwrap();
        assert_eq!(chunks_map.len(), 2, "duplicate chunk id collapses");
        let cid1 = hash_str2int("chunk-1", 500).to_string();
        let cid2 = hash_str2int("chunk-2", 500).to_string();
        assert!(chunks_map.contains_key(&cid1));
        assert!(chunks_map.contains_key(&cid2));
        assert_eq!(chunks_map[&cid1]["content"], json!("c1"));
        let doc_aggs = slot["doc_aggs"].as_object().unwrap();
        assert_eq!(doc_aggs.len(), 1, "doc aggs dedupe by doc_name");
        assert_eq!(doc_aggs["doc-a"]["count"], json!(1));
        assert!(retrieval_has_reference(&retrieval));

        let end = build_message_end(Some("finished"), Some(&json!({"type": "pdf"})), &retrieval);
        assert_eq!(end["status"], json!("finished"));
        assert_eq!(end["attachment"], json!({"type": "pdf"}));
        assert!(end.contains_key("reference"));
        assert!(end["reference"].is_object());

        let end = build_message_end(None, Some(&json!("plain-text")), &[]);
        assert!(!end.contains_key("status"));
        assert!(
            !end.contains_key("attachment"),
            "non-object attachment skipped"
        );
        assert!(
            !end.contains_key("reference"),
            "no retrieval → no reference"
        );
    }

    #[test]
    fn tool_trace_merge_and_tts_cleaning_follow_python() {
        let trace = |name: &str| ToolUseTrace {
            path: format!("Agent-->{name}"),
            tool_name: name.to_owned(),
            arguments: json!({"q": "x"}),
            result: json!("ok"),
            elapsed_time: Some(0.5),
        };
        let mut logs: Vec<Value> = vec![];
        merge_tool_use_trace(&mut logs, "agent_0", &trace("retrieval"));
        merge_tool_use_trace(&mut logs, "agent_0", &trace("generate"));
        // Same component → same entry, two traces.
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0]["component_id"], json!("agent_0"));
        assert_eq!(logs[0]["trace"].as_array().unwrap().len(), 2);
        // Different component → new entry.
        merge_tool_use_trace(&mut logs, "agent_1", &trace("search"));
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[1]["component_id"], json!("agent_1"));
        assert_eq!(logs[1]["trace"][0]["tool_name"], json!("search"));

        assert_eq!(clean_tts_text(""), "");
        assert_eq!(clean_tts_text("plain text"), "plain text");
        assert_eq!(clean_tts_text("a\u{0007}b\u{007f}c"), "abc");
        assert_eq!(clean_tts_text("hi 😀 there 🚀"), "hi there");
        assert_eq!(
            clean_tts_text("  multi \n  line \t text "),
            "multi line text"
        );
        let long = "x".repeat(600);
        assert_eq!(clean_tts_text(&long).chars().count(), 500);
    }
}
