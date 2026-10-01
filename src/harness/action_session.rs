//! Slot-table research primitives + the graph-edge action session —
//! RAGFlow v0.27.2 `rag/advanced_rag/harness/action_session.py`.
//!
//! Slice 1 (this batch): the slot-table models ([`Variable`], [`SlotState`],
//! [`SessionResult`]), the unified tool-result contract ([`ToolOutcome`]), the
//! near-duplicate search-query detection and the module constants. The tool
//! registry, per-turn dispatch and the policy gates land on top of these.

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;

use regex::Regex;
use serde_json::Value;

/// `_INIT_TIMEOUT_S` / `_ACTION_TIMEOUT_S`.
pub const INIT_TIMEOUT_S: f64 = 45.0;
pub const ACTION_TIMEOUT_S: f64 = 75.0;
/// `_SNIPPETS_PER_QUERY` / `_MAX_TOOL_RESPONSE_CHARS`.
pub const SNIPPETS_PER_QUERY: usize = 4;
pub const MAX_TOOL_RESPONSE_CHARS: usize = 12000;
/// Dataset-level empty results a compiled-structure tool accumulates before it
/// is disabled for the rest of the session.
pub const EMPTY_STRIKES: usize = 2;
/// Near-duplicate search-query Jaccard threshold.
pub const NEAR_DUP_JACCARD: f64 = 0.8;
pub const RETRIEVAL_TOOLS: [&str; 3] = ["search_chunks", "grep_chunks", "grep_search"];
/// Hard cap on the shared evidence pool (`tools.kbinfos["chunks"]`).
pub const EVIDENCE_POOL_CAP: usize = 60;
/// Emit the "pool FULL" log line once per fill, not once per rejected chunk.
pub static EVIDENCE_POOL_FULL_LOGGED: AtomicBool = AtomicBool::new(false);

/// `_search_tokens`: lowercased alphanumeric tokens of a query.
pub fn search_tokens(query: &str) -> HashSet<String> {
    Regex::new(r"[a-z0-9]{2,}")
        .unwrap()
        .find_iter(&query.to_lowercase())
        .map(|found| found.as_str().to_string())
        .collect()
}

/// `_is_near_dup`: true when `query` shares >= [`NEAR_DUP_JACCARD`] of its
/// tokens with any already-run query.
pub fn is_near_dup(query: &str, seen: &[String]) -> bool {
    if query.is_empty() || seen.is_empty() {
        return false;
    }
    let tokens = search_tokens(query);
    if tokens.len() < 2 {
        return false;
    }
    for previous in seen {
        let other = search_tokens(previous);
        let intersection = tokens.intersection(&other).count();
        let union = tokens.union(&other).count();
        if union > 0 && intersection as f64 / union as f64 >= NEAR_DUP_JACCARD {
            return true;
        }
    }
    false
}

/// `Variable`: one unknown entity to resolve. `id` is immutable across patches.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Variable {
    pub id: i64,
    pub r#type: String,
    pub question_clues: Vec<String>,
    pub discovered_clues: Vec<String>,
    pub candidate: Option<String>,
    pub candidate_strength: Option<f64>,
}

impl Variable {
    /// `Variable.brief`.
    pub fn brief(&self) -> String {
        if self.filled() {
            let strength = self
                .candidate_strength
                .map(|value| format!("{value:.2}"))
                .unwrap_or_else(|| "?".to_string());
            return format!(
                "[{}] {}: {} ({})",
                self.id,
                self.r#type,
                self.candidate.as_deref().unwrap_or(""),
                strength
            );
        }
        format!("[{}] {}: EMPTY", self.id, self.r#type)
    }

    /// `Variable.filled` (Python truthiness: an empty candidate is unfilled).
    pub fn filled(&self) -> bool {
        self.candidate
            .as_deref()
            .map(|candidate| !candidate.is_empty())
            .unwrap_or(false)
    }
}

/// `State`: one slot table at a search depth.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SlotState {
    pub state: Vec<Variable>,
    pub depth: usize,
    pub id: String,
    pub retrieved_evidence_ids: Vec<String>,
}

impl SlotState {
    /// Build a state, generating the `__post_init__` id when empty.
    pub fn new(state: Vec<Variable>, depth: usize) -> Self {
        let id = format!(
            "{:03x}_{:08x}{:02x}",
            depth,
            now_millis() % 100_000_000,
            rand::random::<u8>()
        );
        Self {
            state,
            depth,
            id,
            retrieved_evidence_ids: Vec::new(),
        }
    }

    /// `State.unresolved`: variables without a candidate.
    pub fn unresolved(&self) -> Vec<&Variable> {
        self.state
            .iter()
            .filter(|variable| !variable.filled())
            .collect()
    }

    /// `State.by_id`.
    pub fn by_id(&self, id: i64) -> Option<&Variable> {
        self.state.iter().find(|variable| variable.id == id)
    }

    /// `State.brief`: `d{depth}(+..)` marks.
    pub fn brief(&self) -> String {
        let marks: String = self
            .state
            .iter()
            .map(|variable| if variable.filled() { '+' } else { '.' })
            .collect();
        format!("d{}({marks})", self.depth)
    }

    /// `State.render_slots`.
    pub fn render_slots(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for variable in &self.state {
            let mut line = format!("- id={} type={}", variable.id, variable.r#type);
            if !variable.question_clues.is_empty() {
                line.push_str("\n  question_clues: ");
                line.push_str(&variable.question_clues.join("; "));
            }
            if !variable.discovered_clues.is_empty() {
                let tail =
                    &variable.discovered_clues[variable.discovered_clues.len().saturating_sub(4)..];
                line.push_str("\n  discovered_clues: ");
                line.push_str(&tail.join("; "));
            }
            if variable.filled() {
                let strength = variable
                    .candidate_strength
                    .map(|value| format!("{value:.2}"))
                    .unwrap_or_else(|| "?".to_string());
                line.push_str(&format!(
                    "\n  CANDIDATE: {} (strength={})",
                    variable.candidate.as_deref().unwrap_or(""),
                    strength
                ));
            }
            lines.push(line);
        }
        lines.join("\n")
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// `Result`: outcome of ONE `run_action` session.
#[derive(Debug, Clone, Default)]
pub struct SessionResult {
    pub messages: Vec<Value>,
    pub new_states: Vec<SlotState>,
    pub found_answer: Option<String>,
    pub retrieved_evidence_ids: Vec<String>,
    pub terminal_type: Option<String>,
    pub terminal_payload: Option<Value>,
}

/// Unified tool-result status constants.
pub const OUTCOME_OK: &str = "ok";
/// Dataset-level: no such compiled structure exists here.
pub const OUTCOME_EMPTY: &str = "empty";
/// Query-level: nothing matched THIS query; the tool is still valid.
pub const OUTCOME_MISS: &str = "miss";
/// Produced output, but too weak to be useful.
pub const OUTCOME_POOR: &str = "poor";
/// Ran fine, but added no NEW evidence.
pub const OUTCOME_REDUNDANT: &str = "redundant";
/// Infra / provider failure.
pub const OUTCOME_ERROR: &str = "error";

/// `ToolOutcome`: result of ONE tool call — the signal `_tool_node` acts on.
#[derive(Debug, Clone, Default)]
pub struct ToolOutcome {
    /// What the model sees (a list of passage dicts).
    pub payload: Vec<Value>,
    /// Doc/chunk ids worth tracking.
    pub evidence_ids: Vec<String>,
    /// One of the status constants above.
    pub status: String,
    /// Machine-readable cause: `""` / `no_structure` / `no_doc` / `infra` /
    /// `bad_args`. Only `no_structure` is DATASET-level and may disable a
    /// tool; `no_doc` is QUERY-level and must NOT.
    pub reason: String,
    /// Numeric signals for policy decisions (`hits`, `new_evidence`, ...).
    pub metrics: serde_json::Map<String, Value>,
}

impl ToolOutcome {
    pub fn ok(payload: Vec<Value>, evidence_ids: Vec<String>) -> Self {
        Self {
            payload,
            evidence_ids,
            status: OUTCOME_OK.to_string(),
            ..Self::default()
        }
    }

    pub fn empty(reason: &str) -> Self {
        Self {
            status: OUTCOME_EMPTY.to_string(),
            reason: reason.to_string(),
            ..Self::default()
        }
    }

    pub fn miss() -> Self {
        Self {
            status: OUTCOME_MISS.to_string(),
            ..Self::default()
        }
    }
}

// ── Evidence admission + tool executors (part 2) ────────────────────────────

use std::sync::atomic::Ordering;

use crate::harness::chunk_utils;
use crate::harness::orchestrator::direct::Kbinfos;
use crate::harness::tools::search::is_table_chunk;

/// `_admit_evidence`: register one chunk into the session output AND the shared
/// evidence pool. Returns `true` when the chunk was NEW to the shared pool; a
/// retrieval whose every hit was already known surfaces as `redundant`.
pub fn admit_evidence(
    kbinfos: &mut Kbinfos,
    kb_seen: &mut HashSet<String>,
    chunk: &Value,
    out: &mut Vec<Value>,
    ids: &mut Vec<String>,
    seen: &mut HashSet<String>,
    include_doc_id: bool,
) -> bool {
    // Early-stop: the shared evidence pool is hard-capped (the SCA view and
    // compose can only consume the first 60 anyway).
    if kbinfos.chunks.len() >= EVIDENCE_POOL_CAP {
        if !EVIDENCE_POOL_FULL_LOGGED.swap(true, Ordering::Relaxed) {
            // The "pool FULL" line is emitted once per fill, not per chunk.
        }
        return false;
    }
    let cid = chunk_utils::chunk_id(chunk);
    if seen.contains(&cid) {
        return false;
    }
    seen.insert(cid.clone());
    ids.push(cid.clone());
    // Table chunks pass through UN-truncated (the 1200-char cap hides answer
    // rows in the mid/late table).
    let text = chunk_utils::chunk_text(chunk);
    let mut entry = if is_table_chunk(chunk) {
        serde_json::json!({"id": cid, "content": text})
    } else {
        serde_json::json!({"id": cid, "content": text.chars().take(1200).collect::<String>()})
    };
    if include_doc_id {
        if let Some(object) = entry.as_object_mut() {
            object.insert(
                "doc_id".to_string(),
                Value::String(chunk_utils::doc_id(chunk)),
            );
        }
    }
    out.push(entry);
    if chunk.is_object() && !kb_seen.contains(&cid) {
        kb_seen.insert(cid);
        kbinfos.chunks.push(chunk.clone());
        return true;
    }
    false
}

/// `_search_outcome`: wrap a query-based retrieval result. A run that surfaced
/// hits but admitted NOTHING NEW is `redundant` rather than `ok`.
pub fn search_outcome(payload: &[Value], ids: &[String], new_evidence: usize) -> ToolOutcome {
    if payload.is_empty() {
        return ToolOutcome {
            payload: Vec::new(),
            evidence_ids: Vec::new(),
            status: OUTCOME_MISS.to_string(),
            reason: "no_doc".to_string(),
            metrics: [
                ("hits".to_string(), serde_json::json!(0)),
                ("new_evidence".to_string(), serde_json::json!(0)),
            ]
            .into_iter()
            .collect(),
        };
    }
    ToolOutcome {
        payload: payload.to_vec(),
        evidence_ids: ids.to_vec(),
        status: if new_evidence == 0 {
            OUTCOME_REDUNDANT.to_string()
        } else {
            OUTCOME_OK.to_string()
        },
        reason: String::new(),
        metrics: [
            ("hits".to_string(), serde_json::json!(payload.len())),
            ("new_evidence".to_string(), serde_json::json!(new_evidence)),
        ]
        .into_iter()
        .collect(),
    }
}

/// `_arg_query_list`: normalize a tool call's `query` argument (string | list |
/// absent) and cap it.
pub fn arg_query_list(args: &Value, max_q: usize) -> Vec<String> {
    let query = args.get("query");
    let list: Vec<Value> = match query {
        Some(Value::String(text)) => vec![Value::String(text.clone())],
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    list.into_iter()
        .take(max_q)
        .map(|item| match item {
            Value::String(text) => text,
            other => other.to_string(),
        })
        .collect()
}

/// The injected retrieval backends the executors drive.
#[async_trait]
pub trait ActionSearchBackend: Send + Sync {
    /// `grep_search` (exact-term locate; `doc_scope` + soft `keywords` hint).
    async fn grep_search(
        &self,
        query: &str,
        top_n: usize,
        doc_scope: Option<&[String]>,
        keywords: Option<&str>,
    ) -> Result<Vec<Value>, String>;
    /// `hybrid_search` (semantic; optional compiled expansion).
    async fn hybrid_search(
        &self,
        query: &str,
        top_n: usize,
        use_compiled: bool,
    ) -> Result<Vec<Value>, String>;
    /// `list_chunks` (full document read).
    async fn list_chunks(&self, doc_id: &str) -> Result<Vec<Value>, String>;
    /// Web provider availability (`tools.web_search`).
    fn has_web(&self) -> bool;
    /// Web retrieval (chunks share the RAGFlow chunk shape).
    async fn web_search(&self, query: &str) -> Result<Vec<Value>, String>;
}

struct SearchRun {
    out: Vec<Value>,
    ids: Vec<String>,
    new_evidence: usize,
}

fn admit_candidates(
    kbinfos: &mut Kbinfos,
    kb_seen: &mut HashSet<String>,
    candidates: &[Value],
    out: &mut Vec<Value>,
    ids: &mut Vec<String>,
    seen: &mut HashSet<String>,
    include_doc_id: bool,
) -> usize {
    let mut new_evidence = 0usize;
    for chunk in candidates.iter().take(SNIPPETS_PER_QUERY) {
        if admit_evidence(kbinfos, kb_seen, chunk, out, ids, seen, include_doc_id) {
            new_evidence += 1;
        }
    }
    new_evidence
}

/// `_exec_retrieve`: corpus search via grep (exact-term locate).
/// `nav_hint` is a SOFT retrieval hint (BM25 keywords), never a constraint.
pub async fn exec_retrieve(
    backend: &dyn ActionSearchBackend,
    kbinfos: &mut Kbinfos,
    queries: &[String],
    doc_scope: Option<&[String]>,
    nav_hint: &str,
) -> ToolOutcome {
    let mut out: Vec<Value> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut new_evidence = 0usize;
    let mut kb_seen: HashSet<String> = kbinfos.chunks.iter().map(chunk_utils::chunk_id).collect();
    for query in queries.iter().take(3) {
        let Ok(candidates) = backend
            .grep_search(
                query,
                10,
                doc_scope,
                if nav_hint.is_empty() {
                    None
                } else {
                    Some(nav_hint)
                },
            )
            .await
        else {
            continue;
        };
        new_evidence += admit_candidates(
            kbinfos,
            &mut kb_seen,
            &candidates,
            &mut out,
            &mut ids,
            &mut seen,
            true,
        );
    }
    search_outcome(&out, &ids, new_evidence)
}

/// `_exec_search_chunks`: semantic retrieval (hybrid vector+BM25).
pub async fn exec_search_chunks(
    backend: &dyn ActionSearchBackend,
    kbinfos: &mut Kbinfos,
    queries: &[String],
    use_compiled: bool,
) -> ToolOutcome {
    let mut out: Vec<Value> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut new_evidence = 0usize;
    let mut kb_seen: HashSet<String> = kbinfos.chunks.iter().map(chunk_utils::chunk_id).collect();
    for query in queries.iter().take(2) {
        let Ok(candidates) = backend.hybrid_search(query, 20, use_compiled).await else {
            continue;
        };
        new_evidence += admit_candidates(
            kbinfos,
            &mut kb_seen,
            &candidates,
            &mut out,
            &mut ids,
            &mut seen,
            true,
        );
    }
    search_outcome(&out, &ids, new_evidence)
}

/// `_exec_web_search`: open-web retrieval (≤2 queries, ≤8 chunks each).
pub async fn exec_web_search(
    backend: &dyn ActionSearchBackend,
    kbinfos: &mut Kbinfos,
    queries: &[String],
) -> ToolOutcome {
    if !backend.has_web() {
        // No web provider: emit an explicit, actionable note (never an empty
        // payload the model would just retry).
        return ToolOutcome {
            payload: vec![serde_json::json!({
                "kind": "web_search",
                "note": "Web search is NOT configured for this session. Do not use this tool again; use the corpus tools (retrieve / search_chunks / navigate_*) instead."
            })],
            evidence_ids: Vec::new(),
            status: OUTCOME_ERROR.to_string(),
            reason: "infra".to_string(),
            metrics: serde_json::Map::new(),
        };
    }
    let mut out: Vec<Value> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut new_evidence = 0usize;
    let mut kb_seen: HashSet<String> = kbinfos.chunks.iter().map(chunk_utils::chunk_id).collect();
    for query in queries.iter().take(2) {
        let Ok(candidates) = backend.web_search(query).await else {
            continue;
        };
        for chunk in candidates.iter().take(8) {
            let cid = chunk_utils::chunk_id(chunk);
            if cid.is_empty() || seen.contains(&cid) {
                continue;
            }
            if admit_evidence(
                kbinfos,
                &mut kb_seen,
                chunk,
                &mut out,
                &mut ids,
                &mut seen,
                false,
            ) {
                new_evidence += 1;
            }
        }
    }
    search_outcome(&out, &ids, new_evidence)
}

/// `_exec_list_chunks`: deep-read one document (≤30 chunks). An unknown/blank
/// `doc_id` is a QUERY-level miss, never a dataset fact.
pub async fn exec_list_chunks(
    backend: &dyn ActionSearchBackend,
    kbinfos: &mut Kbinfos,
    doc_id: &str,
) -> ToolOutcome {
    let chunks = match backend.list_chunks(doc_id).await {
        Ok(chunks) => chunks,
        Err(_) => {
            return ToolOutcome {
                payload: Vec::new(),
                evidence_ids: Vec::new(),
                status: OUTCOME_ERROR.to_string(),
                reason: "infra".to_string(),
                metrics: serde_json::Map::new(),
            };
        }
    };
    let mut out: Vec<Value> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut new_evidence = 0usize;
    let mut kb_seen: HashSet<String> = kbinfos.chunks.iter().map(chunk_utils::chunk_id).collect();
    for chunk in chunks.iter().take(30) {
        let cid = chunk_utils::chunk_id(chunk);
        if cid.is_empty() {
            continue;
        }
        if admit_evidence(
            kbinfos,
            &mut kb_seen,
            chunk,
            &mut out,
            &mut ids,
            &mut seen,
            false,
        ) {
            new_evidence += 1;
        }
    }
    let _ = SearchRun {
        out: Vec::new(),
        ids: Vec::new(),
        new_evidence: 0,
    };
    search_outcome(&out, &ids, new_evidence)
}

#[cfg(test)]
mod executor_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockBackend {
        grep: Vec<Value>,
        hybrid: Vec<Value>,
        document: Vec<Value>,
        web: Vec<Value>,
        web_enabled: bool,
        fail: bool,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl Default for MockBackend {
        fn default() -> Self {
            Self {
                grep: Vec::new(),
                hybrid: Vec::new(),
                document: Vec::new(),
                web: Vec::new(),
                web_enabled: false,
                fail: false,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ActionSearchBackend for MockBackend {
        async fn grep_search(
            &self,
            query: &str,
            _top_n: usize,
            _doc_scope: Option<&[String]>,
            keywords: Option<&str>,
        ) -> Result<Vec<Value>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("grep:{query}:{keywords:?}"));
            if self.fail {
                return Err("boom".to_string());
            }
            Ok(self.grep.clone())
        }
        async fn hybrid_search(
            &self,
            query: &str,
            _top_n: usize,
            use_compiled: bool,
        ) -> Result<Vec<Value>, String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("hybrid:{query}:{use_compiled}"));
            if self.fail {
                return Err("boom".to_string());
            }
            Ok(self.hybrid.clone())
        }
        async fn list_chunks(&self, doc_id: &str) -> Result<Vec<Value>, String> {
            self.calls.lock().unwrap().push(format!("list:{doc_id}"));
            if self.fail {
                return Err("boom".to_string());
            }
            Ok(self.document.clone())
        }
        fn has_web(&self) -> bool {
            self.web_enabled
        }
        async fn web_search(&self, query: &str) -> Result<Vec<Value>, String> {
            self.calls.lock().unwrap().push(format!("web:{query}"));
            if self.fail {
                return Err("boom".to_string());
            }
            Ok(self.web.clone())
        }
    }

    fn chunk(id: &str, content: &str, doc_id: &str) -> Value {
        json!({"chunk_id": id, "content_with_weight": content, "doc_id": doc_id})
    }

    #[test]
    fn admission_caps_and_table_passthrough() {
        let mut kbinfos = Kbinfos::default();
        let mut kb_seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Value> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        let table =
            json!({"chunk_id": "t1", "content_with_weight": "| a |\n| - |\n| 1 |", "doc_id": "d1"});
        assert!(!is_table_chunk(
            &json!({"content_with_weight": "plain prose"})
        ));
        assert!(admit_evidence(
            &mut kbinfos,
            &mut kb_seen,
            &table,
            &mut out,
            &mut ids,
            &mut seen,
            true
        ));
        assert_eq!(
            out[0]["content"],
            json!("| a |\n| - |\n| 1 |"),
            "tables stay whole"
        );
        assert_eq!(out[0]["doc_id"], json!("d1"));

        let long = "x".repeat(2000);
        assert!(admit_evidence(
            &mut kbinfos,
            &mut kb_seen,
            &chunk("c2", &long, "d1"),
            &mut out,
            &mut ids,
            &mut seen,
            false
        ));
        assert_eq!(out[1]["content"].as_str().unwrap().chars().count(), 1200);
        assert!(out[1].get("doc_id").is_none());

        // Already-seen chunk -> not new, not duplicated.
        assert!(!admit_evidence(
            &mut kbinfos,
            &mut kb_seen,
            &chunk("c2", &long, "d1"),
            &mut out,
            &mut ids,
            &mut seen,
            false
        ));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn admission_early_stops_at_pool_cap() {
        let mut kbinfos = Kbinfos::default();
        for index in 0..EVIDENCE_POOL_CAP {
            kbinfos.chunks.push(chunk(&format!("pre{index}"), "x", "d"));
        }
        let mut kb_seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Value> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        assert!(!admit_evidence(
            &mut kbinfos,
            &mut kb_seen,
            &chunk("late", "y", "d"),
            &mut out,
            &mut ids,
            &mut seen,
            true
        ));
        assert!(out.is_empty(), "pool cap blocks further admits");
    }

    #[test]
    fn outcome_and_query_args_mirror_upstream() {
        let miss = search_outcome(&[], &[], 0);
        assert_eq!(miss.status, OUTCOME_MISS);
        assert_eq!(miss.reason, "no_doc");
        let redundant = search_outcome(&[json!({"id": "c1"})], &["c1".to_string()], 0);
        assert_eq!(redundant.status, OUTCOME_REDUNDANT);
        let ok = search_outcome(&[json!({"id": "c2"})], &["c2".to_string()], 1);
        assert_eq!(ok.status, OUTCOME_OK);
        assert_eq!(ok.metrics["hits"], json!(1));

        assert_eq!(arg_query_list(&json!({"query": "one"}), 3), vec!["one"]);
        assert_eq!(
            arg_query_list(&json!({"query": ["a", "b", "c", "d"]}), 3),
            vec!["a", "b", "c"]
        );
        assert!(arg_query_list(&json!({}), 3).is_empty());
    }

    #[tokio::test]
    async fn retrieve_and_search_executors() {
        let backend = MockBackend {
            grep: vec![chunk("c1", "alpha", "d1"), chunk("c2", "beta", "d1")],
            hybrid: vec![chunk("c1", "alpha", "d1"), chunk("c3", "gamma", "d1")],
            ..Default::default()
        };
        let mut kbinfos = Kbinfos::default();
        let outcome = exec_retrieve(
            &backend,
            &mut kbinfos,
            &["q1".to_string()],
            None,
            "nav hint",
        )
        .await;
        assert_eq!(outcome.status, OUTCOME_OK);
        assert_eq!(
            outcome.evidence_ids,
            vec!["c1".to_string(), "c2".to_string()]
        );
        assert_eq!(kbinfos.chunks.len(), 2);
        assert!(backend.calls.lock().unwrap()[0].contains("Some(\"nav hint\")"));

        // Second search over the same chunks -> redundant.
        let outcome = exec_search_chunks(&backend, &mut kbinfos, &["q2".to_string()], true).await;
        assert_eq!(outcome.status, OUTCOME_OK, "c3 is new");
        let outcome = exec_search_chunks(&backend, &mut kbinfos, &["q3".to_string()], false).await;
        assert_eq!(outcome.status, OUTCOME_REDUNDANT);

        // Backend failure -> continue with an empty (miss) outcome.
        let failing = MockBackend {
            fail: true,
            ..Default::default()
        };
        let outcome = exec_retrieve(&failing, &mut kbinfos, &["q".to_string()], None, "").await;
        assert_eq!(outcome.status, OUTCOME_MISS);
    }

    #[tokio::test]
    async fn web_and_list_chunks_contracts() {
        let mut kbinfos = Kbinfos::default();
        let no_web = MockBackend::default();
        let outcome = exec_web_search(&no_web, &mut kbinfos, &["q".to_string()]).await;
        assert_eq!(outcome.status, OUTCOME_ERROR);
        assert_eq!(outcome.reason, "infra");
        assert!(
            outcome.payload[0]["note"]
                .as_str()
                .unwrap()
                .contains("NOT configured")
        );

        let web = MockBackend {
            web_enabled: true,
            web: vec![chunk("w1", "web hit", "w")],
            ..Default::default()
        };
        let outcome = exec_web_search(&web, &mut kbinfos, &["q".to_string()]).await;
        assert_eq!(outcome.status, OUTCOME_OK);
        assert!(
            outcome.payload[0].get("doc_id").is_none(),
            "web entries omit doc_id"
        );

        let doc = MockBackend {
            document: vec![chunk("d1", "doc text", "doc")],
            ..Default::default()
        };
        let outcome = exec_list_chunks(&doc, &mut kbinfos, "doc").await;
        assert_eq!(outcome.status, OUTCOME_OK);
        assert_eq!(outcome.evidence_ids, vec!["d1".to_string()]);
    }
}

// ── Terminal parsing + tool surface (part 3a) ──────────────────────────────

/// The tool registry order (`_TOOL_MAP` insertion order).
/// (`retrieve`, `search_chunks`, `list_chunks`, `navigate_tree`,
/// `navigate_structure`, `calculate`, `graph_explore`, `web_search`.)
pub const TOOL_MAP_NAMES: [&str; 8] = [
    "retrieve",
    "search_chunks",
    "list_chunks",
    "navigate_tree",
    "navigate_structure",
    "calculate",
    "graph_explore",
    "web_search",
];

/// `_reason_status`: single source of truth for the cause→status mapping.
pub fn reason_status(reason: &str) -> &'static str {
    match reason {
        "no_structure" => OUTCOME_EMPTY,
        "bad_args" | "infra" => OUTCOME_ERROR,
        // no_doc: this query reached nothing, the tool itself is fine.
        _ => OUTCOME_MISS,
    }
}

/// `_active_tool_specs` (name level): the tool names exposed for THIS mode.
/// * the per-mode tool set comes from `config.THINKING_MODES` (ultra adds
///   `graph_explore`);
/// * `web_search` is hidden when NO web provider is configured;
/// * disabled compile-only tools are removed for the rest of the session.
pub fn active_tool_specs(
    mode_tools: &std::collections::BTreeSet<String>,
    has_web: bool,
    disabled: &HashSet<String>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in TOOL_MAP_NAMES {
        if !mode_tools.contains(name) {
            continue;
        }
        if name == "web_search" && !has_web {
            continue;
        }
        if disabled.contains(name) {
            continue;
        }
        out.push(name.to_string());
    }
    out
}

/// `extract_json`: the first parseable JSON object found in `text` (tries the
/// remainder from each `{`, then the balanced object starting there).
pub fn extract_json(text: &str) -> Option<Value> {
    for (index, ch) in text.char_indices() {
        if ch != '{' {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&text[index..]) {
            if value.is_object() {
                return Some(value);
            }
        }
        if let Some(end) = balanced_object_end(&text[index..]) {
            if let Ok(value) = serde_json::from_str::<Value>(&text[index..index + end]) {
                if value.is_object() {
                    return Some(value);
                }
            }
        }
    }
    None
}

fn balanced_object_end(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

/// `extract_tag`: exact-tag extraction first; then lenient fallbacks for
/// models that wrap the JSON in code fences or emit bare objects.
pub fn extract_tag(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let (Some(start), Some(end)) = (text.rfind(&open), text.rfind(&close)) {
        if end > start {
            return Some(text[start + open.len()..end].trim().to_string());
        }
    }
    let fenced_re = Regex::new(r"(?s)```(?:json)?\s*(\{.*?\}|\[.*?\])\s*```").unwrap();
    let fenced: Vec<String> = fenced_re
        .captures_iter(text)
        .filter_map(|capture| capture.get(1).map(|m| m.as_str().to_string()))
        .collect();
    let tag_re = Regex::new(&format!(r"<{tag}\b")).unwrap();
    if fenced.is_empty() && !tag_re.is_match(text) {
        return None;
    }
    // Prefer the LAST fenced block (most recent decision).
    for fragment in fenced.iter().rev() {
        if let Ok(object) = serde_json::from_str::<Value>(fragment) {
            let keys: HashSet<String> = object
                .as_object()
                .map(|map| map.keys().cloned().collect())
                .unwrap_or_default();
            let matched = if tag == "state" {
                keys.contains("new_states")
            } else {
                keys.contains("answer") || keys.contains("new_state")
            };
            if object.is_object() && matched {
                return Some(fragment.trim().to_string());
            }
        }
    }
    None
}

/// `apply_patch`: ONLY existing ids; mutable fields are `candidate` /
/// `candidate_strength` / `discovered_clues`. `id` is immutable — patches may
/// not add variables. Returns `None` when nothing changed.
pub fn apply_patch(base: &SlotState, branch_patches: &[Value]) -> Option<SlotState> {
    let mut new_vars: Vec<Variable> = base.state.clone();
    let mut changed = false;
    for patch in branch_patches {
        let Some(object) = patch.as_object() else {
            return None;
        };
        let Some(id) = object.get("id").and_then(Value::as_i64) else {
            return None;
        };
        let Some(variable) = new_vars.iter_mut().find(|variable| variable.id == id) else {
            continue;
        };
        if let Some(candidate) = object.get("candidate") {
            variable.candidate = match candidate {
                Value::String(text) if text.is_empty() => None,
                Value::String(text) => Some(text.clone()),
                Value::Null => None,
                Value::Bool(flag) => {
                    if *flag {
                        Some("True".to_string())
                    } else {
                        None
                    }
                }
                Value::Number(number) => {
                    if number.as_f64().map(|value| value != 0.0).unwrap_or(true) {
                        Some(number.to_string())
                    } else {
                        None
                    }
                }
                other => Some(other.to_string()),
            };
            changed = true;
        }
        if let Some(strength) = object.get("candidate_strength") {
            if !strength.is_null() {
                let number = strength.as_f64().or_else(|| {
                    strength
                        .as_str()
                        .and_then(|text| text.trim().parse::<f64>().ok())
                });
                if let Some(number) = number {
                    variable.candidate_strength = Some(number.clamp(0.0, 1.0));
                    changed = true;
                }
            }
        }
        if let Some(clues) = object.get("discovered_clues").and_then(Value::as_array) {
            let tail: Vec<&Value> = clues
                .iter()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            for clue in tail {
                let text = match clue {
                    Value::String(value) => value.clone(),
                    other => other.to_string(),
                };
                variable
                    .discovered_clues
                    .push(text.chars().take(160).collect());
            }
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    let mut patched = SlotState::new(new_vars, base.depth + 1);
    patched.retrieved_evidence_ids = base.retrieved_evidence_ids.clone();
    Some(patched)
}

/// `_parse_terminal`: parse the two DeepSearch terminal output blocks.
/// Returns `(new_states, found_answer, terminal_type, terminal_payload)` —
/// exactly one of the first two may be set.
pub fn parse_terminal(
    content: &str,
    parent: &SlotState,
) -> (
    Vec<SlotState>,
    Option<String>,
    Option<String>,
    Option<Value>,
) {
    if content.contains("<state>") {
        let block = extract_tag(content, "state").unwrap_or_else(|| "{}".to_string());
        let data = extract_json(&block).unwrap_or_else(|| serde_json::json!({}));
        let mut raw_branches: Vec<Value> = data
            .get("new_states")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if !raw_branches.is_empty()
            && raw_branches
                .iter()
                .all(|branch| branch.is_object() && branch.get("state").is_none())
        {
            raw_branches = raw_branches
                .into_iter()
                .map(|branch| serde_json::json!({"state": [branch]}))
                .collect();
        }
        let mut branches: Vec<SlotState> = Vec::new();
        for branch in raw_branches {
            let patches = branch
                .get("state")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(state) = apply_patch(parent, &patches) {
                branches.push(state);
            }
        }
        return (branches, None, Some("state".to_string()), Some(data));
    }
    if content.contains("<answer>") {
        let block = extract_tag(content, "answer").unwrap_or_else(|| "{}".to_string());
        let data = extract_json(&block).unwrap_or_else(|| serde_json::json!({}));
        let answer = data
            .get("answer")
            .map(|value| match value {
                Value::String(text) => text.trim().to_string(),
                other => other.to_string().trim().trim_matches('"').to_string(),
            })
            .filter(|text| !text.is_empty());
        let mut final_state = parent.clone();
        if let Some(list) = data.get("new_state").and_then(Value::as_array) {
            if let Some(patched) = apply_patch(parent, list) {
                final_state = patched;
            }
        }
        return (
            vec![final_state],
            answer,
            Some("answer".to_string()),
            Some(data),
        );
    }
    (Vec::new(), None, None, None)
}

/// One normalized provider tool call.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
    /// Unknown names are kept (never dropped) so the protocol stays paired.
    pub unknown: bool,
}

/// `_parse_tool_calls`: normalize provider-native tool calls to
/// `ToolCall`s; synthesize stable `call_N` ids when the provider omitted one.
pub fn parse_tool_calls(raw_calls: &[Value]) -> Vec<ToolCall> {
    let mut calls: Vec<ToolCall> = Vec::new();
    for (index, call) in raw_calls.iter().enumerate() {
        let function = call.get("function");
        let name = function
            .and_then(|f| f.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let raw_args = function.and_then(|f| f.get("arguments"));
        let args: Value = match raw_args {
            Some(Value::String(text)) if !text.trim().is_empty() => {
                serde_json::from_str::<Value>(text)
                    .ok()
                    .filter(Value::is_object)
                    .unwrap_or_else(|| serde_json::json!({}))
            }
            Some(value) if value.is_object() => value.clone(),
            _ => serde_json::json!({}),
        };
        let id = call
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| format!("call_{index}"));
        let unknown = !TOOL_MAP_NAMES.contains(&name.as_str());
        calls.push(ToolCall {
            id,
            name,
            args,
            unknown,
        });
    }
    calls
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    use serde_json::json;

    fn state_with(vars: Vec<Variable>) -> SlotState {
        SlotState::new(vars, 0)
    }

    #[test]
    fn tag_and_json_extraction() {
        let text = "blah <state>{\"new_states\": []}</state> tail";
        assert_eq!(
            extract_tag(text, "state"),
            Some("{\"new_states\": []}".to_string())
        );
        // Fenced fallback for models that ignore the XML protocol.
        let fenced = "here you go\n```json\n{\"answer\": \"42\"}\n```";
        assert_eq!(
            extract_tag(fenced, "answer"),
            Some("{\"answer\": \"42\"}".to_string())
        );
        assert_eq!(extract_tag("no protocol here", "state"), None);
        // extract_json picks the first parseable object.
        assert_eq!(
            extract_json("noise {\"a\": 1} trailing"),
            Some(json!({"a": 1}))
        );
        assert_eq!(extract_json("no objects"), None);

        let mut var = Variable {
            id: 1,
            r#type: "person".to_string(),
            ..Default::default()
        };
        var.candidate = Some("Ada".to_string());
        assert_eq!(var.candidate.as_deref(), Some("Ada"));
    }

    #[test]
    fn apply_patch_only_mutates_existing_ids() {
        let base = state_with(vec![Variable {
            id: 1,
            r#type: "person".to_string(),
            ..Default::default()
        }]);
        // Unknown id -> no change -> None.
        assert!(apply_patch(&base, &[json!({"id": 9, "candidate": "X"})]).is_none());
        // Missing id -> structural error -> None.
        assert!(apply_patch(&base, &[json!({"candidate": "X"})]).is_none());
        // Valid patch on an existing id.
        let patched = apply_patch(
            &base,
            &[json!({"id": 1, "candidate": "Ada", "candidate_strength": 1.7, "discovered_clues": ["a", "b"]})],
        )
        .expect("patch applied");
        assert_eq!(patched.depth, 1);
        assert_eq!(patched.state[0].candidate.as_deref(), Some("Ada"));
        assert_eq!(
            patched.state[0].candidate_strength,
            Some(1.0),
            "strength clamps to 1.0"
        );
        assert_eq!(
            patched.state[0].discovered_clues,
            vec!["a".to_string(), "b".to_string()]
        );
        // Patch that changes nothing -> None.
        assert!(apply_patch(&base, &[json!({"id": 1})]).is_none());
    }

    #[test]
    fn terminal_parsing_paths() {
        let base = state_with(vec![Variable {
            id: 1,
            r#type: "person".to_string(),
            ..Default::default()
        }]);
        // <state> with bare-dict branches gets wrapped and patched.
        let (branches, answer, kind, _payload) = parse_terminal(
            "<state>{\"new_states\": [{\"id\": 1, \"candidate\": \"Ada\"}]}</state>",
            &base,
        );
        assert_eq!(kind.as_deref(), Some("state"));
        assert!(answer.is_none());
        assert_eq!(branches.len(), 1);
        assert_eq!(branches[0].state[0].candidate.as_deref(), Some("Ada"));

        // <answer> may patch the parent state too.
        let (states, answer, kind, payload) = parse_terminal(
            "<answer>{\"answer\": \" 42 \", \"new_state\": [{\"id\": 1, \"candidate\": \"Ada\"}]}</answer>",
            &base,
        );
        assert_eq!(kind.as_deref(), Some("answer"));
        assert_eq!(answer.as_deref(), Some("42"));
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].state[0].candidate.as_deref(), Some("Ada"));
        assert_eq!(payload.unwrap()["answer"], json!(" 42 "));

        // Empty answer -> None, no branches.
        let (states, answer, kind, _) =
            parse_terminal("<answer>{\"answer\": \"  \"}</answer>", &base);
        assert!(answer.is_none());
        assert_eq!(kind.as_deref(), Some("answer"));
        assert_eq!(states.len(), 1, "final_state mirrors the parent");

        // Neither block -> nothing.
        let (states, answer, kind, _) = parse_terminal("just prose", &base);
        assert!(states.is_empty() && answer.is_none() && kind.is_none());
    }

    #[test]
    fn tool_calls_normalize_and_mark_unknown() {
        let raw = vec![
            json!({"id": "c1", "function": {"name": "retrieve", "arguments": "{\"query\": [\"a\"]}"}}),
            json!({"function": {"name": "state", "arguments": "not json"}}),
            json!({"id": "c3", "function": {"name": "search_chunks", "arguments": {"query": "b"}}}),
        ];
        let calls = parse_tool_calls(&raw);
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].id, "c1");
        assert_eq!(calls[0].args["query"][0], json!("a"));
        assert!(!calls[0].unknown);
        assert_eq!(calls[1].id, "call_1", "missing ids are synthesized");
        assert!(calls[1].unknown, "XML protocol tags are unknown tools");
        assert_eq!(calls[1].args, json!({}), "bad JSON falls back to {{}}");
        assert_eq!(
            calls[2].args["query"],
            json!("b"),
            "object args pass through"
        );
    }

    #[test]
    fn tool_surface_gates() {
        let mut high = std::collections::BTreeSet::new();
        for name in ["retrieve", "search_chunks", "navigate_tree", "web_search"] {
            high.insert(name.to_string());
        }
        let mut ultra = high.clone();
        ultra.insert("graph_explore".to_string());
        let none: HashSet<String> = HashSet::new();
        assert_eq!(
            active_tool_specs(&high, true, &none),
            vec!["retrieve", "search_chunks", "navigate_tree", "web_search"]
        );
        assert_eq!(
            active_tool_specs(&high, false, &none),
            vec!["retrieve", "search_chunks", "navigate_tree"],
            "no provider hides web_search"
        );
        assert!(active_tool_specs(&ultra, true, &none).contains(&"graph_explore".to_string()));
        let disabled: HashSet<String> = ["navigate_tree".to_string()].into_iter().collect();
        assert!(!active_tool_specs(&high, true, &disabled).contains(&"navigate_tree".to_string()));

        assert_eq!(reason_status("no_structure"), OUTCOME_EMPTY);
        assert_eq!(reason_status("no_doc"), OUTCOME_MISS);
        assert_eq!(reason_status("bad_args"), OUTCOME_ERROR);
        assert_eq!(reason_status("infra"), OUTCOME_ERROR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn near_dup_detects_paraphrases() {
        let seen = vec!["Culdcept original creator designer OmiyaSoft".to_string()];
        assert!(is_near_dup(
            "Culdcept creator designer OmiyaSoft original",
            &seen
        ));
        assert!(!is_near_dup("weather in Tokyo tomorrow", &seen));
        assert!(!is_near_dup("", &seen));
        assert!(!is_near_dup("short", &seen));
        assert!(!is_near_dup("culdcept", &[]));
    }

    #[test]
    fn variable_and_state_render() {
        let mut variable = Variable {
            id: 1,
            r#type: "person".to_string(),
            question_clues: vec!["the founder".to_string()],
            ..Default::default()
        };
        assert!(!variable.filled());
        assert_eq!(variable.brief(), "[1] person: EMPTY");
        variable.candidate = Some("Ada".to_string());
        variable.candidate_strength = Some(0.75);
        assert!(variable.filled());
        assert_eq!(variable.brief(), "[1] person: Ada (0.75)");

        let mut empty = Variable {
            id: 2,
            r#type: "date".to_string(),
            candidate: Some(String::new()),
            ..Default::default()
        };
        assert!(!empty.filled(), "empty candidate is unfilled");
        empty.discovered_clues = vec!["a".to_string(), "b".to_string()];

        let state = SlotState::new(vec![variable.clone(), empty.clone()], 2);
        assert!(
            state.id.starts_with("002_"),
            "depth-prefixed id: {}",
            state.id
        );
        assert_eq!(state.unresolved().len(), 1);
        assert_eq!(state.by_id(2).map(|v| v.id), Some(2));
        assert_eq!(state.brief(), "d2(+.)");
        let rendered = state.render_slots();
        assert!(rendered.contains("- id=1 type=person"));
        assert!(rendered.contains("CANDIDATE: Ada (strength=0.75)"));
        assert!(rendered.contains("discovered_clues: a; b"));
    }

    #[test]
    fn tool_outcome_constructors() {
        let outcome = ToolOutcome::ok(vec![json!({"chunk_id": "c1"})], vec!["c1".to_string()]);
        assert_eq!(outcome.status, OUTCOME_OK);
        assert_eq!(outcome.reason, "");
        assert_eq!(outcome.evidence_ids, vec!["c1".to_string()]);
        let empty = ToolOutcome::empty("no_structure");
        assert_eq!(empty.status, OUTCOME_EMPTY);
        assert_eq!(empty.reason, "no_structure");
        assert_eq!(ToolOutcome::miss().status, OUTCOME_MISS);
    }
}
