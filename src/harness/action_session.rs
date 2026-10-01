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
    if include_doc_id && let Some(object) = entry.as_object_mut() {
        object.insert(
            "doc_id".to_string(),
            Value::String(chunk_utils::doc_id(chunk)),
        );
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
        if let Ok(value) = serde_json::from_str::<Value>(&text[index..])
            && value.is_object()
        {
            return Some(value);
        }
        if let Some(end) = balanced_object_end(&text[index..])
            && let Ok(value) = serde_json::from_str::<Value>(&text[index..index + end])
            && value.is_object()
        {
            return Some(value);
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
    if let (Some(start), Some(end)) = (text.rfind(&open), text.rfind(&close))
        && end > start
    {
        return Some(text[start + open.len()..end].trim().to_string());
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
        if let Some(strength) = object.get("candidate_strength")
            && !strength.is_null()
        {
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
        if let Some(list) = data.get("new_state").and_then(Value::as_array)
            && let Some(patched) = apply_patch(parent, list)
        {
            final_state = patched;
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

// ── Tool spec constants (part 3b-1; `_TOOL_MAP` schemas verbatim) ───────────

/// `_RETRIEVE_TOOL_SPEC` description.
pub const RETRIEVE_TOOL_DESCRIPTION: &str = "WHEN TO CALL: Use when you know or suspect exact surface terms or keywords in the corpus (names, titles, codes, phrases). Best as the first recall pass; send 1-3 queries covering different facets.DO NOT CALL: When you already hold a doc_id and need to read it (use list_chunks); when the answer shares no surface words with any query (use search_chunks); for counting or enumerating a whole document.ARGUMENTS: query — array of 1-3 strings (natural-language queries). Note: doc_scope exists inside the executor but is NOT a declared parameter; do not pass it.OUTPUT: Short exact-term-matched snippets, each carrying its doc_id and chunk id. Status ok means new evidence entered the pool; redundant means everything was already there.IF IT FAILS: miss (empty payload) means this query matched nothing — rephrase or switch to search_chunks; do not conclude the corpus lacks the fact. redundant means stop re-searching and emit a state patch.";

/// `_LIST_CHUNKS_TOOL_SPEC` description.
pub const LIST_CHUNKS_TOOL_DESCRIPTION: &str = "WHEN TO CALL: You need the FULL text of one document (enumeration, counts, arithmetic over many passages) and you already have its doc_id from a prior tool result.DO NOT CALL: When you only need a single passage (use search_chunks or retrieve first); when you have no doc_id yet (locate it via navigate_tree or search_chunks first).ARGUMENTS: doc_id — string, the document id seen in a retrieve / search_chunks / navigate result. ONLY doc_id is accepted; there is no chunk_ids argument, and the tool returns the whole document (capped at 30 chunks).OUTPUT: All chunks of the document in reading order. ok = new evidence; redundant = already in pool.IF IT FAILS: An unknown or blank doc_id yields an empty result (query-level miss, not a dataset fact) — pick a different doc_id or locate one first. Do not treat this as a reason to disable the tool.";

/// `_SEARCH_CHUNKS_TOOL_SPEC` description.
pub const SEARCH_CHUNKS_TOOL_DESCRIPTION: &str = "WHEN TO CALL: Primary semantic recall. Use when exact retrieve returns nothing useful, when the corpus is large and you are unsure which document holds the answer, or when the answer passage shares no surface words with your query. Send 1-2 queries.DO NOT CALL: When you already have a doc_id and want to read that document (use list_chunks); when a single exact passage would be found faster by grep-style retrieve.ARGUMENTS: query — array of 1-2 strings. Compiled-structure expansion is automatic and a no-op on datasets without compiled structure, so no extra argument is needed.OUTPUT: Relevance-ranked snippet chunks, possibly with structural neighbours (parent/child headings, sibling pages) appended. ok = new evidence; redundant = already seen.IF IT FAILS: miss means this query matched nothing — change the angle or fall back to retrieve or navigate_tree. Re-issuing a near-duplicate query is skipped as redundant, so vary the query instead of paraphrasing it.";

/// `_WEB_SEARCH_TOOL_SPEC` description.
pub const WEB_SEARCH_TOOL_DESCRIPTION: &str = "WHEN TO CALL: The needed fact is world knowledge, a recent event, or newer than the corpus (a current event, a person's alive-now status, a fresh statistic). This tool only appears when a web provider is configured.DO NOT CALL: When the fact plausibly lives in the fixed corpus — prefer retrieve or search_chunks first. For corpus-only questions this tool is unavailable.ARGUMENTS: query — array of 1-2 strings.OUTPUT: Web results shaped like corpus chunks, merged into the same evidence pool.IF IT FAILS: error (no provider) — it will not appear at all this session; if it does appear and fails, switch to corpus tools permanently and do not retry it.";

/// `_NAVIGATE_TREE_TOOL_SPEC` description.
pub const NAVIGATE_TREE_TOOL_DESCRIPTION: &str = "WHEN TO CALL: The question names a topic, entity, or alias but you do NOT know which document discusses it, especially on a large corpus. Routes by topic or cluster similarity over the compiled navigation tree.DO NOT CALL: When you already hold a doc_id (go straight to navigate_structure); when the answer is likely a single exact passage (use retrieve or search_chunks).ARGUMENTS: query — string, the topic / entity / alias whose document(s) to locate. Note: keywords is read by the executor but is NOT a declared parameter; do not pass it.OUTPUT: Candidate doc_ids plus a first-chunk summary of each; these become your known-docs set for the next step.IF IT FAILS: empty (no_structure) means the dataset has no compiled navigation tree — immediately switch to search_chunks. A second such empty disables this tool for the rest of the session, so do not retry it.";

/// `_NAVIGATE_STRUCTURE_TOOL_SPEC` description.
pub const NAVIGATE_STRUCTURE_TOOL_DESCRIPTION: &str = "WHEN TO CALL: You know the doc_id and need to PINPOINT where the answer lives inside that one document, without reading every chunk. The in-document counterpart of navigate_tree.DO NOT CALL: When you have no doc_id yet; when the document has no compiled structure (use list_chunks to read the full document).ARGUMENTS: doc_id — string, required. query — string, what to locate within the document. kind — enum catalog / mindmap / graph, default catalog (compiled-structure kind).OUTPUT: The structure outline annotated with matching chunk_ids, reading-order aware. ok = useful hits; poor (chunk_ptrs = 0) means it drilled to nothing usable.IF IT FAILS: empty (no_structure) — try another doc_id or kind, or fall back to list_chunks / search_chunks. poor — read the full document via list_chunks(doc_id). A second empty disables the tool for the session.";

/// `_CALCULATE_TOOL_SPEC` description.
pub const CALCULATE_TOOL_DESCRIPTION: &str = "WHEN TO CALL: The question asks you to DERIVE a number by combining facts you found (sum / difference / percentage / ratio / sort / compare / length / age / price / area / growth). NEVER do arithmetic mentally.DO NOT CALL: When the answer IS one of the stated numbers (no combination needed) — answer directly. When a needed number is still missing — retrieve it first; do not estimate.ARGUMENTS: question — string, the user question verbatim. facts — array of strings, the numbers or facts found in evidence, verbatim (keep the original language; pass them exactly as written).OUTPUT: an object with expression and result — report the computed result verbatim.IF IT FAILS: poor (no numeric answer derivable) — retrieve more numbers, or answer directly if the answer is already stated. Never fabricate a computation.";

/// `_GRAPH_EXPLORE_TOOL_SPEC` description.
pub const GRAPH_EXPLORE_TOOL_DESCRIPTION: &str = "EXPLORE the compiled KNOWLEDGE GRAPH (entities + relations) for a RELATIONAL/multi-hop answer. Different from navigate_*: instead of locating a document or passage, it seeds entities for the query, hops along their RELATIONS, and returns either a direct answer or the source passages behind the relevant entities/relations. Use when the answer requires connecting several entities through their relations (e.g. who-was-related-to-whom, cause-effect chains, membership/ownership) and you already have a starting entity from a search result, a navigation outline, or a list_chunks reading. If the dataset has NO compiled knowledge graph, it returns empty — fall back to search_chunks / navigate_structure.";

/// `_TOOL_MAP`: the OpenAI function schema for one tool name (`None` when the
/// name is not a registered tool).
pub fn tool_spec(name: &str) -> Option<Value> {
    let (description, parameters) = match name {
        "retrieve" => (
            RETRIEVE_TOOL_DESCRIPTION,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 3}
                },
                "required": ["query"]
            }),
        ),
        "list_chunks" => (
            LIST_CHUNKS_TOOL_DESCRIPTION,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string", "description": "document id seen in a retrieve snippet"}
                },
                "required": ["doc_id"]
            }),
        ),
        "search_chunks" => (
            SEARCH_CHUNKS_TOOL_DESCRIPTION,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 2}
                },
                "required": ["query"]
            }),
        ),
        "web_search" => (
            WEB_SEARCH_TOOL_DESCRIPTION,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 2}
                },
                "required": ["query"]
            }),
        ),
        "navigate_tree" => (
            NAVIGATE_TREE_TOOL_DESCRIPTION,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "topic / entity / alias whose document(s) to locate"}
                },
                "required": ["query"]
            }),
        ),
        "navigate_structure" => (
            NAVIGATE_STRUCTURE_TOOL_DESCRIPTION,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "doc_id": {"type": "string", "description": "document id seen from a prior tool result"},
                    "query": {"type": "string", "description": "what to locate within the document"},
                    "kind": {"type": "string", "enum": ["catalog", "mindmap", "graph"], "description": "compiled structure kind, default catalog"}
                },
                "required": ["doc_id"]
            }),
        ),
        "calculate" => (
            CALCULATE_TOOL_DESCRIPTION,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string", "description": "the user's question, verbatim"},
                    "facts": {"type": "array", "items": {"type": "string"}, "description": "numbers/facts found in the evidence, verbatim"}
                },
                "required": ["question", "facts"]
            }),
        ),
        "graph_explore" => (
            GRAPH_EXPLORE_TOOL_DESCRIPTION,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "the relational question / starting entity"},
                    "doc_scope": {"type": "array", "items": {"type": "string"}, "description": "optional doc_ids to restrict the graph to (from prior navigation/list_chunks)"}
                },
                "required": ["query"]
            }),
        ),
        _ => return None,
    };
    Some(serde_json::json!({
        "type": "function",
        "function": {"name": name, "description": description, "parameters": parameters}
    }))
}

/// `_TOOL_MAP` spec list for a set of visible tool names (`_active_tool_specs`
/// payload): schemas in registry order.
pub fn active_tool_schemas(names: &[String]) -> Vec<Value> {
    TOOL_MAP_NAMES
        .iter()
        .filter(|name| names.iter().any(|visible| visible == *name))
        .filter_map(|name| tool_spec(name))
        .collect()
}

#[cfg(test)]
mod spec_tests {
    use super::*;

    #[test]
    fn all_eight_specs_build_with_verbatim_names() {
        let names: Vec<String> = TOOL_MAP_NAMES.iter().map(|n| (*n).to_string()).collect();
        let specs = active_tool_schemas(&names);
        assert_eq!(specs.len(), 8);
        for (index, spec) in specs.iter().enumerate() {
            assert_eq!(spec["type"], serde_json::json!("function"));
            assert_eq!(
                spec["function"]["name"],
                serde_json::json!(TOOL_MAP_NAMES[index])
            );
            let description = spec["function"]["description"].as_str().unwrap();
            assert!(
                description.chars().count() > 40,
                "spec {} carries a description",
                TOOL_MAP_NAMES[index]
            );
            assert_eq!(
                spec["function"]["parameters"]["type"],
                serde_json::json!("object")
            );
        }
        // Subset filtering keeps registry order.
        let subset = active_tool_schemas(&["list_chunks".to_string(), "retrieve".to_string()]);
        assert_eq!(subset.len(), 2);
        assert_eq!(subset[0]["function"]["name"], serde_json::json!("retrieve"));
        assert_eq!(
            subset[1]["function"]["name"],
            serde_json::json!("list_chunks")
        );
        assert!(tool_spec("unknown").is_none());
    }
}

// ── Tool loop (part 3b-2a): executors, dispatch, policy ─────────────────────

use crate::harness::tools::navigation::NavResult;

/// The compiled-structure / calculator backends the tool branches drive.
#[async_trait]
pub trait ActionToolBackend: Send + Sync {
    /// `navigate_tree`.
    async fn navigate_tree(&self, query: &str) -> NavResult;
    /// `navigate_structure`.
    async fn navigate_structure(&self, doc_id: &str, query: &str, kind: &str) -> NavResult;
    /// `compute_from_facts` (None = nothing derivable).
    async fn calculate(&self, question: &str, facts: &[String]) -> Option<Value>;
    /// `graph_explore` → `{answer, chunks}` (`answer` may be empty).
    async fn graph_explore(&self, query: &str, doc_scope: &[String]) -> Value;
}

/// Everything one action-session turn needs from the host.
pub struct ActionRuntime<'a> {
    pub search: &'a dyn ActionSearchBackend,
    pub tools: &'a dyn ActionToolBackend,
}

fn nav_metrics(res: &NavResult) -> serde_json::Map<String, Value> {
    let mut metrics = serde_json::Map::new();
    metrics.insert("hits".to_string(), serde_json::json!(res.doc_ids.len()));
    metrics.insert("entities".to_string(), serde_json::json!(res.entities));
    metrics.insert("chunk_ptrs".to_string(), serde_json::json!(res.chunk_ptrs));
    metrics.insert("top_score".to_string(), serde_json::json!(res.top_score));
    metrics.insert(
        "chunk_paths".to_string(),
        serde_json::json!(res.chunk_paths),
    );
    metrics.insert(
        "routed_docs".to_string(),
        serde_json::json!(
            res.routed_docs
                .iter()
                .map(|(doc, summary)| serde_json::json!([doc, summary]))
                .collect::<Vec<_>>()
        ),
    );
    metrics
}

/// `_exec_navigate_tree`: route documents through the compiled nav tree.
pub async fn exec_navigate_tree(tools: &dyn ActionToolBackend, query: &str) -> ToolOutcome {
    let res = tools.navigate_tree(query).await;
    let metrics = nav_metrics(&res);
    if !res.empty_reason.is_empty() {
        return ToolOutcome {
            payload: vec![serde_json::json!({
                "kind": "navigate_tree",
                "note": "No compiled navigation tree reachable for this query. Switch to search_chunks / retrieve; a second such empty disables this tool for the session."
            })],
            evidence_ids: Vec::new(),
            status: reason_status(&res.empty_reason).to_string(),
            reason: res.empty_reason,
            metrics,
        };
    }
    ToolOutcome {
        payload: vec![serde_json::json!({
            "kind": "navigate_tree",
            "doc_ids": res.doc_ids,
            "content": res.text,
        })],
        evidence_ids: res.doc_ids.clone(),
        status: OUTCOME_OK.to_string(),
        reason: String::new(),
        metrics,
    }
}

/// `_exec_navigate_structure`: compiled outline of one document.
pub async fn exec_navigate_structure(
    tools: &dyn ActionToolBackend,
    doc_id: &str,
    query: &str,
    kind: &str,
) -> ToolOutcome {
    let res = tools.navigate_structure(doc_id, query, kind).await;
    let metrics = nav_metrics(&res);
    if !res.empty_reason.is_empty() {
        return ToolOutcome {
            payload: vec![serde_json::json!({
                "kind": "navigate_structure",
                "doc_id": doc_id,
                "note": format!("No compiled structure of kind={kind:?} reachable for this document. Try another doc_id or kind, or use search_chunks / retrieve / list_chunks."),
            })],
            evidence_ids: Vec::new(),
            status: reason_status(&res.empty_reason).to_string(),
            reason: res.empty_reason,
            metrics,
        };
    }
    // Reached a structure but drilled to nothing usable: weak, not an error.
    let status = if res.chunk_ptrs == 0 {
        OUTCOME_POOR
    } else {
        OUTCOME_OK
    };
    let text: String = res.text.chars().take(8000).collect();
    let mut evidence_ids = res.doc_ids.clone();
    if evidence_ids.is_empty() && !doc_id.is_empty() {
        evidence_ids.push(doc_id.to_string());
    }
    ToolOutcome {
        payload: vec![serde_json::json!({
            "kind": "navigate_structure",
            "doc_id": doc_id,
            "content": text,
        })],
        evidence_ids,
        status: status.to_string(),
        reason: String::new(),
        metrics,
    }
}

/// `_exec_calculate`: arithmetic terminal via `compute_from_facts`.
pub async fn exec_calculate(
    tools: &dyn ActionToolBackend,
    question: &str,
    facts: &[String],
) -> ToolOutcome {
    let result = tools.calculate(question, facts).await;
    match result {
        None => ToolOutcome {
            payload: vec![serde_json::json!({
                "kind": "calculate",
                "expression": Value::Null,
                "note": "no numeric answer derivable from given facts; answer directly or retrieve more numbers."
            })],
            evidence_ids: Vec::new(),
            status: OUTCOME_POOR.to_string(),
            reason: "no_doc".to_string(),
            metrics: serde_json::Map::new(),
        },
        Some(value) => ToolOutcome {
            payload: vec![serde_json::json!({
                "kind": "calculate",
                "expression": value.get("expression").cloned().unwrap_or(Value::Null),
                "result": value.get("value").cloned().unwrap_or(Value::Null),
            })],
            evidence_ids: Vec::new(),
            status: OUTCOME_OK.to_string(),
            reason: String::new(),
            metrics: serde_json::Map::new(),
        },
    }
}

/// `_exec_graph_explore`: relational knowledge-graph exploration.
pub async fn exec_graph_explore(
    tools: &dyn ActionToolBackend,
    query: &str,
    doc_scope: &[String],
) -> ToolOutcome {
    let res = tools.graph_explore(query, doc_scope).await;
    let answer = res
        .get("answer")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let chunks = res
        .get("chunks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !answer.is_empty() {
        return ToolOutcome {
            payload: vec![serde_json::json!({"kind": "graph_explore", "answer": answer})],
            evidence_ids: Vec::new(),
            status: OUTCOME_OK.to_string(),
            reason: String::new(),
            metrics: [("hits".to_string(), serde_json::json!(0))]
                .into_iter()
                .collect(),
        };
    }
    if chunks.is_empty() {
        return ToolOutcome {
            payload: vec![serde_json::json!({
                "kind": "graph_explore",
                "note": "This dataset has NO compiled knowledge graph (or none in the given scope). graph_explore is unavailable; use search_chunks / navigate_structure / retrieve instead."
            })],
            evidence_ids: Vec::new(),
            status: OUTCOME_EMPTY.to_string(),
            reason: "no_structure".to_string(),
            metrics: serde_json::Map::new(),
        };
    }
    let mut snippet: Vec<Value> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    for chunk in chunks.iter().take(6) {
        let id = chunk.get("id").cloned().unwrap_or(Value::Null);
        let content: String = chunk
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(1500)
            .collect();
        snippet.push(serde_json::json!({"id": id, "content": content}));
        if let Some(text) = id.as_str() {
            if !text.is_empty() {
                ids.push(text.to_string());
            }
        }
    }
    ToolOutcome {
        payload: vec![serde_json::json!({"kind": "graph_explore", "chunks": snippet})],
        evidence_ids: ids.clone(),
        status: OUTCOME_OK.to_string(),
        reason: String::new(),
        metrics: [("hits".to_string(), serde_json::json!(ids.len()))]
            .into_iter()
            .collect(),
    }
}

/// `_disable_tool`: mark a compile-only tool unavailable for the rest of the
/// session (the session state owns the set in Rust).
pub fn disable_tool(disabled: &mut HashSet<String>, name: &str) {
    if !TOOL_MAP_NAMES.contains(&name) {
        return;
    }
    disabled.insert(name.to_string());
}

/// `execute_tool`: dispatch ONE native tool call by name.
pub async fn execute_tool(
    runtime: &ActionRuntime<'_>,
    kbinfos: &mut Kbinfos,
    disabled: &HashSet<String>,
    name: &str,
    args: &Value,
) -> ToolOutcome {
    // Short-circuit a tool already proven unavailable this session.
    if disabled.contains(name) {
        return ToolOutcome {
            payload: vec![serde_json::json!({
                "kind": name,
                "note": format!("{name} is unavailable in this dataset (no compiled structure of its kind). Use search_chunks / retrieve / list_chunks instead.")
            })],
            evidence_ids: Vec::new(),
            status: OUTCOME_EMPTY.to_string(),
            reason: "no_structure".to_string(),
            metrics: serde_json::Map::new(),
        };
    }
    match name {
        "retrieve" => {
            let scope: Vec<String> = args
                .get("doc_scope")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            exec_retrieve(
                runtime.search,
                kbinfos,
                &arg_query_list(args, 3),
                if scope.is_empty() { None } else { Some(&scope) },
                "",
            )
            .await
        }
        "search_chunks" => {
            exec_search_chunks(runtime.search, kbinfos, &arg_query_list(args, 2), true).await
        }
        "list_chunks" => {
            let doc_id = args
                .get("doc_id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            exec_list_chunks(runtime.search, kbinfos, &doc_id).await
        }
        "navigate_tree" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            exec_navigate_tree(runtime.tools, query).await
        }
        "navigate_structure" => {
            let doc_id = args.get("doc_id").and_then(Value::as_str).unwrap_or("");
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let kind = args
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("catalog");
            exec_navigate_structure(runtime.tools, doc_id, query, kind).await
        }
        "calculate" => {
            let question = args.get("question").and_then(Value::as_str).unwrap_or("");
            let facts: Vec<String> = args
                .get("facts")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .map(|item| match item {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        })
                        .filter(|text| !text.trim().is_empty())
                        .collect()
                })
                .unwrap_or_default();
            exec_calculate(runtime.tools, question, &facts).await
        }
        "graph_explore" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let scope: Vec<String> = args
                .get("doc_scope")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            exec_graph_explore(runtime.tools, query, &scope).await
        }
        "web_search" => exec_web_search(runtime.search, kbinfos, &arg_query_list(args, 2)).await,
        _ => ToolOutcome {
            payload: Vec::new(),
            evidence_ids: Vec::new(),
            status: OUTCOME_ERROR.to_string(),
            reason: "bad_args".to_string(),
            metrics: serde_json::Map::new(),
        },
    }
}

/// Result of one `tool_node` pass.
#[derive(Debug, Default)]
pub struct ToolNodeResult {
    pub tool_messages: Vec<Value>,
    pub evidence_ids: Vec<String>,
    pub skipped_dup: usize,
    pub disabled: HashSet<String>,
    pub outcomes: Vec<Value>,
}

/// `_tool_node` policy loop: execute pending tool calls with the near-duplicate
/// guard, unknown-tool correction, same-session cache, empty-strike disabling
/// and explicit redundant notices.
#[allow(clippy::too_many_arguments)]
pub async fn tool_node_policy(
    runtime: &ActionRuntime<'_>,
    kbinfos: &mut Kbinfos,
    pending: &[ToolCall],
    seen_queries: &mut Vec<String>,
    tool_cache: &mut std::collections::HashMap<String, ToolOutcome>,
    strikes: &mut std::collections::HashMap<String, usize>,
    disabled: &mut HashSet<String>,
    prior_evidence_ids: &[String],
) -> ToolNodeResult {
    let mut result = ToolNodeResult::default();
    result.evidence_ids = prior_evidence_ids.to_vec();
    for call in pending {
        let query = call
            .args
            .get("query")
            .map(|value| match value {
                Value::String(text) => text.trim().to_string(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        // Near-duplicate retrieval suppression.
        if RETRIEVAL_TOOLS.contains(&call.name.as_str())
            && !query.is_empty()
            && is_near_dup(&query, seen_queries)
        {
            result.skipped_dup += 1;
            result.tool_messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": serde_json::json!({
                    "passages": [{
                        "kind": call.name,
                        "note": "This query is a near-duplicate of an earlier retrieval and was skipped to avoid redundant searching. Patch the slot with what you have, or issue a genuinely NEW retrieval angle."
                    }]
                }).to_string(),
            }));
            continue;
        }
        // Unknown tool name: never execute; answer with a correction.
        if call.unknown {
            let hint = format!(
                "'{}' is not a tool. State patches and final answers are plain TEXT in your reply body, wrapped in <state>...</state> or <answer>...</answer> XML tags — do not emit them as tool calls. Available tools: {}.",
                call.name,
                TOOL_MAP_NAMES.join(", ")
            );
            result.tool_messages.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": serde_json::json!({"passages": [{"kind": "error", "note": hint}]}).to_string(),
            }));
            continue;
        }
        // Same-session tool cache: avoid re-executing the same call.
        let cache_key = format!(
            "{}:{}",
            call.name,
            serde_json::to_string(&call.args).unwrap_or_default()
        );
        let outcome = match tool_cache.get(&cache_key) {
            Some(cached) => cached.clone(),
            None => {
                let outcome =
                    execute_tool(runtime, kbinfos, disabled, &call.name, &call.args).await;
                tool_cache.insert(cache_key, outcome.clone());
                outcome
            }
        };
        if !query.is_empty() {
            seen_queries.push(query);
        }
        result
            .evidence_ids
            .extend(outcome.evidence_ids.iter().cloned());
        let mut chunks = outcome.payload.clone();
        // Policy: act on WHAT happened.
        if outcome.status == OUTCOME_OK {
            strikes.remove(&call.name);
        } else if outcome.status == OUTCOME_EMPTY && outcome.reason == "no_structure" {
            let count = strikes.get(&call.name).copied().unwrap_or(0) + 1;
            strikes.insert(call.name.clone(), count);
            if count >= EMPTY_STRIKES {
                disable_tool(disabled, &call.name);
            }
        } else if outcome.status == OUTCOME_REDUNDANT {
            chunks.push(serde_json::json!({
                "kind": call.name,
                "note": "All hits from this call were ALREADY in the shared evidence pool; re-searching the same ground adds nothing. Patch the slot or issue a genuinely new angle."
            }));
        }
        result.outcomes.push(serde_json::json!({
            "name": call.name,
            "status": outcome.status,
            "reason": outcome.reason,
            "metrics": outcome.metrics,
        }));
        result.tool_messages.push(serde_json::json!({
            "role": "tool",
            "tool_call_id": call.id,
            "content": serde_json::json!({"passages": chunks}).to_string(),
        }));
    }
    result.disabled = disabled.clone();
    result
}

#[cfg(test)]
mod loop_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockSearch;
    #[async_trait]
    impl ActionSearchBackend for MockSearch {
        async fn grep_search(
            &self,
            _q: &str,
            _t: usize,
            _s: Option<&[String]>,
            _k: Option<&str>,
        ) -> Result<Vec<Value>, String> {
            Ok(vec![])
        }
        async fn hybrid_search(&self, _q: &str, _t: usize, _c: bool) -> Result<Vec<Value>, String> {
            Ok(vec![])
        }
        async fn list_chunks(&self, _d: &str) -> Result<Vec<Value>, String> {
            Ok(vec![])
        }
        fn has_web(&self) -> bool {
            false
        }
        async fn web_search(&self, _q: &str) -> Result<Vec<Value>, String> {
            Ok(vec![])
        }
    }

    struct MockTools {
        nav_result: std::sync::Mutex<NavResult>,
        calc: bool,
        graph: Value,
    }

    #[async_trait]
    impl ActionToolBackend for MockTools {
        async fn navigate_tree(&self, _q: &str) -> NavResult {
            self.nav_result.lock().unwrap().clone()
        }
        async fn navigate_structure(&self, _d: &str, _q: &str, _k: &str) -> NavResult {
            self.nav_result.lock().unwrap().clone()
        }
        async fn calculate(&self, _q: &str, _f: &[String]) -> Option<Value> {
            if self.calc {
                Some(json!({"expression": "1+1", "value": "2"}))
            } else {
                None
            }
        }
        async fn graph_explore(&self, _q: &str, _s: &[String]) -> Value {
            self.graph.clone()
        }
    }

    fn runtime<'a>(search: &'a MockSearch, tools: &'a MockTools) -> ActionRuntime<'a> {
        ActionRuntime { search, tools }
    }

    #[tokio::test]
    async fn dispatch_and_disable_paths() {
        let search = MockSearch;
        let tools = MockTools {
            nav_result: std::sync::Mutex::new(NavResult {
                empty_reason: "no_structure".to_string(),
                ..NavResult::default()
            }),
            calc: true,
            graph: json!({"answer": "", "chunks": []}),
        };
        let rt = runtime(&search, &tools);
        let mut kbinfos = Kbinfos::default();
        let disabled: HashSet<String> = HashSet::new();

        let outcome = execute_tool(
            &rt,
            &mut kbinfos,
            &disabled,
            "navigate_tree",
            &json!({"query": "q"}),
        )
        .await;
        assert_eq!(outcome.status, OUTCOME_EMPTY);
        assert_eq!(outcome.reason, "no_structure");

        let outcome = execute_tool(
            &rt,
            &mut kbinfos,
            &disabled,
            "calculate",
            &json!({"question": "q", "facts": ["1", "1"]}),
        )
        .await;
        assert_eq!(outcome.status, OUTCOME_OK);
        assert_eq!(outcome.payload[0]["result"], json!("2"));

        let outcome = execute_tool(
            &rt,
            &mut kbinfos,
            &disabled,
            "graph_explore",
            &json!({"query": "q"}),
        )
        .await;
        assert_eq!(outcome.status, OUTCOME_EMPTY);
        assert_eq!(outcome.reason, "no_structure");

        let outcome = execute_tool(&rt, &mut kbinfos, &disabled, "bogus", &json!({})).await;
        assert_eq!(outcome.status, OUTCOME_ERROR);
        assert_eq!(outcome.reason, "bad_args");

        // Disabled tools short-circuit with a note.
        let mut blocked: HashSet<String> = HashSet::new();
        blocked.insert("graph_explore".to_string());
        let outcome = execute_tool(
            &rt,
            &mut kbinfos,
            &blocked,
            "graph_explore",
            &json!({"query": "q"}),
        )
        .await;
        assert_eq!(outcome.status, OUTCOME_EMPTY);
        assert!(
            outcome.payload[0]["note"]
                .as_str()
                .unwrap()
                .contains("unavailable")
        );

        // calculate without a derivable answer is poor.
        let tools = MockTools {
            calc: false,
            ..tools
        };
        let rt = runtime(&search, &tools);
        let outcome = execute_tool(
            &rt,
            &mut kbinfos,
            &disabled,
            "calculate",
            &json!({"question": "q", "facts": ["x"]}),
        )
        .await;
        assert_eq!(outcome.status, OUTCOME_POOR);
    }

    #[tokio::test]
    async fn tool_node_policy_covers_guards() {
        let search = MockSearch;
        let tools = MockTools {
            nav_result: std::sync::Mutex::new(NavResult {
                empty_reason: "no_structure".to_string(),
                ..NavResult::default()
            }),
            calc: true,
            graph: json!({"answer": "", "chunks": []}),
        };
        let rt = runtime(&search, &tools);
        let mut kbinfos = Kbinfos::default();
        let mut seen: Vec<String> = vec!["alpha creator OmiyaSoft".to_string()];
        let mut cache = std::collections::HashMap::new();
        let mut strikes = std::collections::HashMap::new();
        let mut disabled: HashSet<String> = HashSet::new();
        let pending = vec![
            // Near-duplicate of an earlier retrieval -> skipped.
            ToolCall {
                id: "c1".into(),
                name: "search_chunks".into(),
                args: json!({"query": "OmiyaSoft alpha creator"}),
                unknown: false,
            },
            // Unknown tool -> correction.
            ToolCall {
                id: "c2".into(),
                name: "state".into(),
                args: json!({}),
                unknown: true,
            },
            // A working tool clears strikes.
            ToolCall {
                id: "c3".into(),
                name: "navigate_tree".into(),
                args: json!({"query": "q"}),
                unknown: false,
            },
        ];
        // First pass: two no_structure empties disable navigate_tree.
        let first = tool_node_policy(
            &rt,
            &mut kbinfos,
            &pending[2..],
            &mut seen,
            &mut cache,
            &mut strikes,
            &mut disabled,
            &[],
        )
        .await;
        assert_eq!(first.skipped_dup, 0);
        assert!(first.disabled.is_empty(), "one strike is not enough");
        let second = tool_node_policy(
            &rt,
            &mut kbinfos,
            &pending[2..],
            &mut seen,
            &mut cache,
            &mut strikes,
            &mut disabled,
            &[],
        )
        .await;
        assert!(
            second.disabled.contains("navigate_tree"),
            "second strike disables"
        );

        let mut strikes2 = std::collections::HashMap::new();
        strikes2.insert("navigate_tree".to_string(), 1);
        let mut disabled2: HashSet<String> = HashSet::new();
        let ok_nav = MockTools {
            nav_result: std::sync::Mutex::new(NavResult {
                doc_ids: vec!["d1".to_string()],
                ..NavResult::default()
            }),
            calc: true,
            graph: json!({"answer": "", "chunks": []}),
        };
        let rt2 = runtime(&search, &ok_nav);
        cache.clear();
        let third = tool_node_policy(
            &rt2,
            &mut kbinfos,
            &pending[2..],
            &mut seen,
            &mut cache,
            &mut strikes2,
            &mut disabled2,
            &[],
        )
        .await;
        assert!(
            strikes2.get("navigate_tree").is_none(),
            "a real hit clears the strike record"
        );
        assert_eq!(third.evidence_ids, vec!["d1".to_string()]);

        // Near-dup skip + unknown correction in one pass.
        let mixed = tool_node_policy(
            &rt2,
            &mut kbinfos,
            &pending[..2],
            &mut seen,
            &mut cache,
            &mut strikes2,
            &mut disabled2,
            &[],
        )
        .await;
        assert_eq!(mixed.skipped_dup, 1);
        assert!(
            mixed.tool_messages[0]["content"]
                .as_str()
                .unwrap()
                .contains("near-duplicate")
        );
        assert!(
            mixed.tool_messages[1]["content"]
                .as_str()
                .unwrap()
                .contains("is not a tool")
        );
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
