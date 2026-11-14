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
///
/// Three upstream helpers are Python host plumbing with no Rust counterpart,
/// because these executors take their backends explicitly and hold no
/// request-scoped globals: `_inject_nav_tools_ref` (navigation module
/// `_tools_ref`), `_kb_ids` (`_get_kb_ids(tools)`; the search backend owns its
/// kb scope) and `_seed_evidence` (`tools.kbinfos`; the pool is the explicit
/// `Kbinfos` argument).
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
        if let Some(text) = id.as_str()
            && !text.is_empty()
        {
            ids.push(text.to_string());
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

/// In-session ladder resume (`_tool_node`'s ladder block): after an executed
/// call, if the session still rests on a nav rung whose `next` verdict for
/// THIS call's status is non-empty, the remaining ladder runs in code so one
/// weak step cascades through the cheaper, wider rungs instead of costing a
/// model turn per fallback.
pub struct LadderResume<'a> {
    /// `_direction`: this slot's question — needed to re-run retrieval later.
    pub direction: &'a str,
    /// `_routed_docs`: navigate_tree's routed doc_ids (scope of later rungs).
    pub routed_docs: &'a [String],
    /// `deadline_left`, for the ladder budget.
    pub deadline_left: Option<f64>,
    /// `_nav_rule_id`: the rung control currently rests on (`""` = finished).
    pub pending_rule: &'a mut String,
    /// `_nav_tool_surface(tools)`: the session's callable tool names.
    pub available: &'a HashSet<String>,
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
    ladder: Option<&mut LadderResume<'_>>,
) -> ToolNodeResult {
    let mut result = ToolNodeResult::default();
    result.evidence_ids = prior_evidence_ids.to_vec();
    let mut ladder = ladder;
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
        let call_status = outcome.status.clone();
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
        // Continue the ladder in code when the session's rung came back weak:
        // keep going on the rule's own verdict so one weak step can cascade
        // through the remaining (cheaper, wider) rungs.
        if let Some(ladder) = ladder.as_deref_mut() {
            let resting = ladder.pending_rule.clone();
            if !resting.is_empty()
                && let Some(rule) = nav_rule_by_id(&resting)
            {
                let next_rule = rule.next_rule(&call_status);
                if !next_rule.is_empty() {
                    let mut ctx = NavContext {
                        direction: ladder.direction.to_string(),
                        known_docs: ladder.routed_docs.to_vec(),
                        ..NavContext::default()
                    };
                    let mut ladder_messages: Vec<Value> = Vec::new();
                    let ladder_budget = (ladder.deadline_left.unwrap_or(ACTION_TIMEOUT_S)
                        * NAV_PREFIX_BUDGET_RATIO)
                        .max(5.0);
                    let stepped_to = run_nav_chain(
                        runtime,
                        kbinfos,
                        &mut ctx,
                        next_rule,
                        ladder_budget,
                        ladder.available,
                        &mut ladder_messages,
                        &mut result.evidence_ids,
                        &mut result.outcomes,
                        "ladder",
                        MAX_TOOL_RESPONSE_CHARS * 4,
                    )
                    .await;
                    *ladder.pending_rule = stepped_to;
                    result.tool_messages.extend(ladder_messages);
                }
            }
        }
    }
    result.disabled = disabled.clone();
    result
}

// ── Session node loop (upstream `_run_action_node` / `_finalize_node` /
// `_route` / `_route_after_tool` / `_build_session_graph`) ──────────────────
//
// The LangGraph wiring is replaced by [`run_session_loop`], an explicit loop
// with the same edges: `run_action → {tool | finalize | END | run_action}` and
// `tool → {run_action | finalize}`.

/// Per-mode action-session turn budget. Upstream `resolve_mode(...).action_max_turns`;
/// `harness/config.py` sets 4 for ultra / high / medium at v0.27.2.
pub const ACTION_MAX_TURNS: usize = 4;

/// `_action_max_turns` (the caller resolves the provider's mode).
pub fn action_max_turns(_mode: &str) -> usize {
    ACTION_MAX_TURNS
}

/// Upstream's "neither a call nor a terminal block" user nudge (verbatim).
pub const NUDGE_TEXT: &str = "Call the retrieve tool, or output a <state> patch, or emit <answer>.";

/// Upstream's tool-budget salvage prompt (verbatim).
pub const BUDGET_PROMPT: &str = "TOOL BUDGET EXHAUSTED. Based ONLY on the passages retrieved above, output now — no prose outside the block:\n<state>{\"new_states\": [{\"state\": [{\"id\": <slot_id>, \"candidate\": \"<value>\", \"candidate_strength\": <0..1>, \"discovered_clues\": [\"...\"]}]}]}</state>\nIf NOTHING was learned use: <state>{\"new_states\": []}</state>";

/// One raw completion: content plus the model's native tool calls (OpenAI
/// shape, passed through verbatim from the provider).
#[derive(Clone, Debug, Default)]
pub struct LlmReply {
    pub content: String,
    pub tool_calls: Vec<Value>,
}

/// `_acompletion` / `_llm_once_with_tools`: the provider-agnostic LLM contract.
/// The implementation owns its transport; `timeout_s` is the caller's
/// wall-clock bound, and a timeout or transport error must yield `None`.
#[async_trait]
pub trait ActionLlmBackend: Send + Sync {
    /// One native-tool completion with THIS mode's tool surface (temperature 0.3).
    async fn complete_with_tools(
        &self,
        messages: &[Value],
        tool_schemas: &[Value],
        timeout_s: f64,
    ) -> Option<LlmReply>;

    /// The tools-free salvage call used by the finalize node (temperature 0.3).
    async fn complete_plain(&self, messages: &[Value], timeout_s: f64) -> Option<LlmReply>;
}

/// `_SessionState` (node-loop subset): everything the graph edges touch, plus
/// the per-session tool-loop bookkeeping adopted from [`tool_node_policy`].
pub struct SessionState {
    pub messages: Vec<Value>,
    pub parent_state: SlotState,
    pub mode_tools: std::collections::BTreeSet<String>,
    pub has_web: bool,
    pub mode: String,
    pub pending_calls: Vec<ToolCall>,
    pub done: bool,
    pub tool_cache: std::collections::HashMap<String, ToolOutcome>,
    pub new_states: Vec<SlotState>,
    pub found_answer: Option<String>,
    pub terminal_type: Option<String>,
    pub terminal_payload: Option<Value>,
    pub retrieved_evidence_ids: Vec<String>,
    pub attempts: usize,
    pub deadline_left: Option<f64>,
    pub strikes: std::collections::HashMap<String, usize>,
    pub disabled: HashSet<String>,
    pub outcomes: Vec<Value>,
    pub skipped_dup: usize,
    pub direction: String,
    pub routed_docs: Vec<String>, // _routed_docs: navigate_tree routed doc ids
    pub nav_rule_id: String,
    pub seen_queries: Vec<String>,
}

impl SessionState {
    /// A fresh session over `parent_state`.
    pub fn new(
        parent_state: SlotState,
        mode: &str,
        mode_tools: std::collections::BTreeSet<String>,
        has_web: bool,
        direction: &str,
    ) -> Self {
        Self {
            messages: Vec::new(),
            parent_state,
            mode_tools,
            has_web,
            mode: mode.to_string(),
            pending_calls: Vec::new(),
            done: false,
            tool_cache: std::collections::HashMap::new(),
            new_states: Vec::new(),
            found_answer: None,
            terminal_type: None,
            terminal_payload: None,
            retrieved_evidence_ids: Vec::new(),
            attempts: 0,
            deadline_left: None,
            strikes: std::collections::HashMap::new(),
            disabled: HashSet::new(),
            outcomes: Vec::new(),
            skipped_dup: 0,
            direction: direction.to_string(),
            routed_docs: Vec::new(),
            nav_rule_id: String::new(),
            seen_queries: Vec::new(),
        }
    }

    /// `_action_max_turns` for this session's mode.
    pub fn max_turns(&self) -> usize {
        action_max_turns(&self.mode)
    }
}

/// `_run_action_node`: one native-tool LLM turn; route on tool_calls /
/// terminal / nudge. A timeout or provider error converges the session empty.
pub async fn run_action_node(llm: &dyn ActionLlmBackend, state: &mut SessionState) {
    let wall = state
        .deadline_left
        .unwrap_or(ACTION_TIMEOUT_S)
        .min(ACTION_TIMEOUT_S)
        .max(15.0);
    state.attempts += 1;
    let specs = active_tool_specs(&state.mode_tools, state.has_web, &state.disabled);
    let schemas = active_tool_schemas(&specs);
    let reply = match llm
        .complete_with_tools(&state.messages, &schemas, wall)
        .await
    {
        Some(reply) => reply,
        None => {
            state.done = true;
            return;
        }
    };
    let calls = parse_tool_calls(&reply.tool_calls);
    if !calls.is_empty() {
        // Pass the model's native tool_calls through VERBATIM (normalized ids
        // and args) so the protocol pairs them with the tool responses.
        let tool_calls: Vec<Value> = calls
            .iter()
            .map(|call| {
                serde_json::json!({
                    "id": call.id,
                    "type": "function",
                    "function": {
                        "name": call.name,
                        "arguments": serde_json::to_string(&call.args)
                            .unwrap_or_else(|_| "{}".to_string()),
                    }
                })
            })
            .collect();
        state.messages.push(serde_json::json!({
            "role": "assistant",
            "content": reply.content,
            "tool_calls": tool_calls,
        }));
        state.pending_calls = calls;
        return;
    }
    let (new_states, found_answer, terminal_type, terminal_payload) =
        parse_terminal(&reply.content, &state.parent_state);
    if found_answer.is_some() || !new_states.is_empty() {
        state.new_states = new_states;
        state.found_answer = found_answer;
        state.terminal_type = terminal_type;
        state.terminal_payload = terminal_payload;
        state.pending_calls.clear();
        state.done = true;
        return;
    }
    // Neither a call nor a terminal block: nudge once per turn.
    state
        .messages
        .push(serde_json::json!({"role": "assistant", "content": reply.content}));
    state
        .messages
        .push(serde_json::json!({"role": "user", "content": NUDGE_TEXT}));
    state.pending_calls.clear();
}

/// `_strip_unpaired_tool_calls`: drop `assistant.tool_calls` that never got a
/// matching tool response so the provider history stays well-formed.
pub fn strip_unpaired_tool_calls(messages: &[Value]) -> Vec<Value> {
    let mut responded: HashSet<String> = HashSet::new();
    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("tool")
            && let Some(id) = message.get("tool_call_id").and_then(Value::as_str)
        {
            responded.insert(id.to_string());
        }
    }
    let mut cleaned: Vec<Value> = Vec::with_capacity(messages.len());
    for message in messages {
        if message.get("role").and_then(Value::as_str) == Some("assistant")
            && let Some(calls) = message.get("tool_calls").and_then(Value::as_array)
            && !calls.is_empty()
        {
            let paired = calls.iter().all(|call| {
                call.get("id")
                    .and_then(Value::as_str)
                    .map(|id| responded.contains(id))
                    .unwrap_or(false)
            });
            if !paired {
                let mut clone = message.clone();
                if let Some(object) = clone.as_object_mut() {
                    object.remove("tool_calls");
                }
                cleaned.push(clone);
                continue;
            }
        }
        cleaned.push(message.clone());
    }
    cleaned
}

/// `_finalize_node`: the tool budget is spent — ONE last call WITHOUT tools
/// demanding the terminal JSON, then a deterministic loose-clue harvest.
pub async fn finalize_node(llm: &dyn ActionLlmBackend, state: &mut SessionState) {
    let parent = state.parent_state.clone();
    let wall = state.deadline_left.unwrap_or(150.0).min(150.0).max(15.0);
    let mut finalize_messages = strip_unpaired_tool_calls(&state.messages);
    finalize_messages.push(serde_json::json!({"role": "user", "content": BUDGET_PROMPT}));
    if let Some(reply) = llm.complete_plain(&finalize_messages, wall).await {
        let (new_states, found_answer, terminal_type, terminal_payload) =
            parse_terminal(&reply.content, &parent);
        state.new_states = new_states;
        state.found_answer = found_answer;
        state.terminal_type = terminal_type;
        state.terminal_payload = terminal_payload;
    }
    // Loose-clue harvest (deterministic, zero-LLM): even when every JSON
    // protocol attempt failed, the last narration often carries facts worth
    // keeping as breadcrumbs.
    if state.new_states.is_empty() && state.found_answer.is_none() {
        let mut loose_clues: Vec<String> = Vec::new();
        for message in state.messages.iter().rev() {
            if message.get("role").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let text = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if text.is_empty() {
                continue;
            }
            if text.chars().count() >= 24 {
                let narrative: String = text.chars().take(220).collect();
                loose_clues = vec![format!("narrative: {narrative}")];
            }
            break;
        }
        if !loose_clues.is_empty() {
            let unresolved = parent.unresolved();
            let target_id = unresolved
                .first()
                .map(|variable| variable.id)
                .or_else(|| parent.state.first().map(|variable| variable.id));
            if let Some(target_id) = target_id
                && let Some(patched) = apply_patch(
                    &parent,
                    &[serde_json::json!({"id": target_id, "discovered_clues": loose_clues})],
                )
            {
                state.new_states = vec![patched];
            }
        }
    }
    state.pending_calls.clear();
    state.done = true;
}

/// One edge decision of the session graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDecision {
    End,
    Tool,
    Finalize,
    RunAction,
}

/// `_route`: after a run_action step.
pub fn route(state: &SessionState) -> RouteDecision {
    if state.done {
        return RouteDecision::End;
    }
    // Run pending tool_calls FIRST, even at the turn budget: leaving an
    // assistant.tool_calls message without its tool response makes the
    // provider reject the next call. The tool node clears the pending list,
    // then route_after_tool re-checks the budget.
    if !state.pending_calls.is_empty() {
        return RouteDecision::Tool;
    }
    if state.attempts >= state.max_turns() {
        return RouteDecision::Finalize;
    }
    // No-progress convergence: >=2 near-duplicate retrievals skipped means
    // further turns are unlikely to surface new evidence.
    if state.skipped_dup >= 2 {
        return RouteDecision::Finalize;
    }
    RouteDecision::RunAction
}

/// `_route_after_tool`: after executing a tool batch (the budget is checked
/// AFTER the tool responses are appended, so a pending call always receives
/// its matching response before the session converges).
pub fn route_after_tool(state: &SessionState) -> RouteDecision {
    if state.attempts >= state.max_turns() {
        RouteDecision::Finalize
    } else {
        RouteDecision::RunAction
    }
}

/// Explicit equivalent of `_build_session_graph().compile()`: the same edges
/// with the same guards, running until a terminal state or the finalize node.
pub async fn run_session_loop(
    runtime: &ActionRuntime<'_>,
    llm: &dyn ActionLlmBackend,
    kbinfos: &mut Kbinfos,
    state: &mut SessionState,
) {
    loop {
        run_action_node(llm, state).await;
        match route(state) {
            RouteDecision::End => break,
            RouteDecision::RunAction => continue,
            RouteDecision::Finalize => {
                finalize_node(llm, state).await;
                break;
            }
            RouteDecision::Tool => {
                let pending = std::mem::take(&mut state.pending_calls);
                let available: HashSet<String> =
                    active_tool_specs(&state.mode_tools, state.has_web, &state.disabled)
                        .into_iter()
                        .collect();
                let mut ladder = LadderResume {
                    direction: &state.direction,
                    routed_docs: &state.routed_docs,
                    deadline_left: state.deadline_left,
                    pending_rule: &mut state.nav_rule_id,
                    available: &available,
                };
                let result = tool_node_policy(
                    runtime,
                    kbinfos,
                    &pending,
                    &mut state.seen_queries,
                    &mut state.tool_cache,
                    &mut state.strikes,
                    &mut state.disabled,
                    &state.retrieved_evidence_ids,
                    Some(&mut ladder),
                )
                .await;
                state.messages.extend(result.tool_messages);
                state.retrieved_evidence_ids = result.evidence_ids;
                state.skipped_dup += result.skipped_dup;
                state.outcomes.extend(result.outcomes);
                if route_after_tool(state) == RouteDecision::Finalize {
                    finalize_node(llm, state).await;
                    break;
                }
            }
        }
    }
}

// ── Deterministic navigation prefix (upstream `_NavRule` / `_NavContext` /
// `_run_drill_merge` / `_NAV_RULES` / `_emit_nav_pair` / `_run_nav_chain` /
// `run_nav_prefix` / `_extract_relevant_evidence`) ──────────────────────────

/// The navigation ladder is armed at v0.27.2 (`_NAV_RULES_ENABLED = True`).
pub const NAV_RULES_ENABLED: bool = true;
/// Share of the session deadline the prefix may spend; the ReAct loop keeps the rest.
pub const NAV_PREFIX_BUDGET_RATIO: f64 = 0.35;
/// Per-call ceiling so one slow navigation cannot swallow the whole prefix.
pub const NAV_PREFIX_CALL_TIMEOUT_S: f64 = 25.0;
/// Cap on the nav-hint text fed back into retrieval as BM25 keywords.
pub const NAV_HINT_CHARS: usize = 600;
/// The first ladder rung.
pub const NAV_START_RULE: &str = "locate";

/// `_NavRule.mode`: AUTO steps run in code; LLM steps hand control back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMode {
    Auto,
    Llm,
}

/// One step of the navigation chain.
#[derive(Debug, Clone)]
pub struct NavRule {
    pub id: &'static str,
    pub tool: &'static str,
    pub mode: NavMode,
    /// `status -> next rule id`; a missing key (or `""`) ends the chain.
    pub next: &'static [(&'static str, &'static str)],
}

impl NavRule {
    /// ``next.get(status, "")``.
    pub fn next_rule(&self, status: &str) -> &'static str {
        self.next
            .iter()
            .find(|(key, _)| *key == status)
            .map(|(_, value)| *value)
            .unwrap_or("")
    }
}

/// The per-slot strategy ladder (upstream `_NAV_RULES`).
///
/// ``locate`` navigate_tree → route to top-n docs; ``drill`` retrieve+merge →
/// whole-corpus retrieve (nav summaries as a soft BM25 hint) + structure paths
/// merged on chunk_id + routed-doc chunks re-ranked to the top; ``global``
/// retrieve → the safety net when drill itself comes back empty.
pub const NAV_RULES: [NavRule; 3] = [
    NavRule {
        id: "locate",
        tool: "navigate_tree",
        mode: NavMode::Auto,
        next: &[
            (OUTCOME_OK, "drill"),
            (OUTCOME_MISS, "global"),
            (OUTCOME_EMPTY, "global"),
            (OUTCOME_POOR, "global"),
            (OUTCOME_ERROR, "global"),
        ],
    },
    NavRule {
        id: "drill",
        tool: "navigate_structure",
        mode: NavMode::Auto,
        // drill returns OK with the merged, re-ranked evidence; only an empty
        // whole-corpus result (MISS) or an infra failure falls through to global.
        next: &[
            (OUTCOME_OK, ""),
            (OUTCOME_MISS, "global"),
            (OUTCOME_EMPTY, "global"),
            (OUTCOME_ERROR, "global"),
        ],
    },
    NavRule {
        id: "global",
        tool: "retrieve",
        mode: NavMode::Auto,
        next: &[],
    },
];

/// `_nav_rule_by_id`.
pub fn nav_rule_by_id(id: &str) -> Option<&'static NavRule> {
    NAV_RULES.iter().find(|rule| rule.id == id)
}

/// The `args` lambdas of the three rules (`"query": direction`, list form for
/// `retrieve`) — also used for display when the drill step composes internally.
pub fn nav_rule_args(rule_id: &str, ctx: &NavContext) -> Value {
    if rule_id == "global" {
        serde_json::json!({"query": [ctx.direction]})
    } else {
        serde_json::json!({"query": ctx.direction})
    }
}

/// Mutable state threaded through the navigation chain.
#[derive(Debug, Clone, Default)]
pub struct NavContext {
    pub direction: String,
    /// The routed scope (doc_ids, for validate-against checks).
    pub known_docs: Vec<String>,
    /// Each routed doc's OVERALL SUMMARY — the hint that feeds retrieval as a
    /// soft boost instead of a hard filter ("nav is a hint, not a constraint").
    pub routed_docs: Vec<(String, String)>,
    /// The joined summaries used as retrieval keywords.
    pub nav_hint: String,
}

fn parse_routed_docs(value: Option<&Value>) -> Vec<(String, String)> {
    value
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let pair = row.as_array()?;
                    Some((
                        pair.first()?.as_str()?.to_string(),
                        pair.get(1)?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `_run_drill_merge`: corpus retrieve for REAL chunks + the root->chunk
/// structure paths from navigate_structure, MERGED on chunk_id so every
/// retrieved chunk carries its hierarchy context.
///
/// **Nav is a hint, not a constraint.** Retrieval runs over the WHOLE corpus
/// (no doc_scope filter), the routed docs' summaries ride along as BM25
/// `keywords` (soft boost), and the returned chunks are re-ranked so routed-doc
/// chunks float to the top while non-routed chunks stay below.
pub async fn run_drill_merge(
    runtime: &ActionRuntime<'_>,
    kbinfos: &mut Kbinfos,
    ctx: &NavContext,
    available: &HashSet<String>,
    budget_s: f64,
) -> ToolOutcome {
    // The host enforces the per-call ceiling `min(NAV_PREFIX_CALL_TIMEOUT_S, budget_s)`.
    let _ = budget_s;
    let mut merged: Vec<Value> = Vec::new();
    let mut evidence_ids: Vec<String> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    // 1. Skeleton A: whole-corpus retrieval, softly boosted by the nav hints.
    let ret_oc = exec_retrieve(
        runtime.search,
        kbinfos,
        std::slice::from_ref(&ctx.direction),
        None,
        &ctx.nav_hint,
    )
    .await;
    for cid in &ret_oc.evidence_ids {
        if seen_ids.insert(cid.clone()) {
            evidence_ids.push(cid.clone());
        }
    }
    let routed_ids: HashSet<String> = ctx.known_docs.iter().cloned().collect();

    // 2. Supplement B: root->chunk paths from EVERY routed doc's structure.
    let mut paths: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    if available.contains("navigate_structure") && !ctx.known_docs.is_empty() {
        for doc_id in &ctx.known_docs {
            let args = serde_json::json!({
                "doc_id": doc_id,
                "query": ctx.direction,
                "kind": "catalog",
            });
            let s_oc = execute_tool(
                runtime,
                kbinfos,
                &HashSet::new(),
                "navigate_structure",
                &args,
            )
            .await;
            for d in &s_oc.evidence_ids {
                if seen_ids.insert(d.clone()) {
                    evidence_ids.push(d.clone());
                }
            }
            if let Some(map) = s_oc.metrics.get("chunk_paths").and_then(Value::as_object) {
                for (cid, path) in map {
                    if !cid.is_empty() && !paths.contains_key(cid) {
                        paths.insert(
                            cid.clone(),
                            path.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| path.to_string()),
                        );
                    }
                }
            }
        }
    }

    // 3. Merge: skeleton A is the base; attach structure_path where ids align.
    //    Re-rank so routed-doc chunks float to the top WITHOUT deleting the rest.
    for passage in &ret_oc.payload {
        if !passage.is_object() {
            merged.push(serde_json::json!({
                "content": passage
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| passage.to_string()),
            }));
            continue;
        }
        let mut entry = passage.clone();
        let cid = entry.get("id").and_then(Value::as_str).map(str::to_string);
        if let Some(cid) = cid
            && let Some(path) = paths.get(&cid)
        {
            entry["structure_path"] = serde_json::json!(path);
        }
        let doc = entry
            .get("doc_id")
            .and_then(Value::as_str)
            .or_else(|| entry.get("document_id").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        entry["nav_rank"] = serde_json::json!(if routed_ids.contains(&doc) { 0 } else { 1 });
        merged.push(entry);
    }
    merged.sort_by_key(|entry| entry.get("nav_rank").and_then(Value::as_i64).unwrap_or(1));

    if merged.is_empty() {
        let mut metrics = serde_json::Map::new();
        metrics.insert("hits".to_string(), serde_json::json!(0));
        return ToolOutcome {
            payload: Vec::new(),
            evidence_ids,
            status: OUTCOME_MISS.to_string(),
            reason: "no_doc".to_string(),
            metrics,
        };
    }
    let hits = merged.len();
    let mut metrics = serde_json::Map::new();
    metrics.insert("hits".to_string(), serde_json::json!(hits));
    metrics.insert("routed".to_string(), serde_json::json!(routed_ids.len()));
    ToolOutcome {
        payload: merged,
        evidence_ids,
        status: OUTCOME_OK.to_string(),
        reason: String::new(),
        metrics,
    }
}

/// `_emit_nav_pair`: append one completed assistant/tool exchange. The pair
/// MUST be well formed — every tool_call needs its tool response. `max_chars`
/// caps the serialized payload (0 = uncapped); the truncation keeps at least
/// 800 characters. Returns the characters actually added.
pub fn emit_nav_pair(
    messages: &mut Vec<Value>,
    call_id: &str,
    tool: &str,
    args: &Value,
    oc: &ToolOutcome,
    max_chars: usize,
) -> usize {
    let mut payload = serde_json::json!({"passages": oc.payload}).to_string();
    if max_chars > 0 && payload.chars().count() > max_chars {
        let keep = max_chars.max(800);
        payload = payload.chars().take(keep).collect();
    }
    let added = payload.chars().count();
    messages.push(serde_json::json!({
        "role": "assistant",
        "content": "",
        "tool_calls": [{
            "id": call_id,
            "type": "function",
            "function": {"name": tool, "arguments": args.to_string()},
        }],
    }));
    messages.push(serde_json::json!({
        "role": "tool",
        "tool_call_id": call_id,
        "content": payload,
    }));
    added
}

/// Run `NAV_RULES` from `start_id`, stopping at the first LLM step. AUTO steps
/// are executed here; an LLM step is returned without being run. Returns the
/// id of the rule control now rests on (`""` when finished or abandoned).
#[allow(clippy::too_many_arguments)]
pub async fn run_nav_chain(
    runtime: &ActionRuntime<'_>,
    kbinfos: &mut Kbinfos,
    ctx: &mut NavContext,
    start_id: &str,
    budget_s: f64,
    available: &HashSet<String>,
    messages: &mut Vec<Value>,
    evidence_ids: &mut Vec<String>,
    outcomes: &mut Vec<Value>,
    id_prefix: &str,
    max_chars: usize,
) -> String {
    let started = std::time::Instant::now();
    let mut spent = 0usize;
    let mut rule_id = start_id.to_string();
    while !rule_id.is_empty() {
        let Some(rule) = nav_rule_by_id(&rule_id) else {
            break;
        };
        if !available.contains(rule.tool) {
            break;
        }
        if rule.mode == NavMode::Llm {
            return rule_id; // hand control back: the model must decide
        }
        let remaining = budget_s - started.elapsed().as_secs_f64();
        if remaining <= 1.0 {
            break;
        }
        let args = nav_rule_args(rule.id, ctx);
        let oc = if rule.id == "drill" {
            // A composed step owns its own tool calls; it only needs the budget.
            run_drill_merge(runtime, kbinfos, ctx, available, remaining.max(5.0)).await
        } else {
            execute_tool(runtime, kbinfos, &HashSet::new(), rule.tool, &args).await
        };
        outcomes.push(serde_json::json!({
            "name": rule.tool,
            "status": oc.status.clone(),
            "reason": oc.reason.clone(),
            "metrics": oc.metrics.clone(),
        }));
        if rule.tool == "navigate_tree" && oc.status == OUTCOME_OK {
            // Routed docs become the validated scope for every later step.
            if let Some(doc_ids) = oc
                .payload
                .first()
                .and_then(|passage| passage.get("doc_ids"))
                .and_then(Value::as_array)
            {
                for doc in doc_ids {
                    if let Some(doc) = doc.as_str()
                        && !doc.is_empty()
                        && !ctx.known_docs.iter().any(|known| known == doc)
                    {
                        ctx.known_docs.push(doc.to_string());
                    }
                }
            }
            // Keep the routed docs WITH their summaries: the hint used to boost
            // (not filter) retrieval for the drilled docs.
            ctx.routed_docs = parse_routed_docs(oc.metrics.get("routed_docs"));
            let hints: Vec<String> = ctx
                .routed_docs
                .iter()
                .filter_map(|(_, summary)| {
                    if summary.is_empty() {
                        None
                    } else {
                        Some(summary.clone())
                    }
                })
                .collect();
            if !hints.is_empty() {
                ctx.nav_hint = hints.join(" ").chars().take(NAV_HINT_CHARS).collect();
            }
        }
        evidence_ids.extend(oc.evidence_ids.iter().cloned());
        spent += emit_nav_pair(
            messages,
            &format!("{id_prefix}_{}", rule.id),
            rule.tool,
            &args,
            &oc,
            max_chars,
        );
        rule_id = rule.next_rule(&oc.status).to_string();
    }
    let _ = spent;
    rule_id
}

/// `run_nav_prefix`: run the navigation ladder up to the first model-driven
/// step. Returns ``(messages, evidence_ids, outcomes, pending_rule_id)`` — the
/// messages are completed assistant/tool pairs ready to seed the session
/// history. Returns empty results when navigation is unavailable — the ladder
/// is an optimisation, never a precondition for the session to run.
///
/// `available` is the caller's `_nav_tool_surface` (the `active_tool_specs`
/// name set with the mode/web/disabled gates already applied).
pub async fn run_nav_prefix(
    runtime: &ActionRuntime<'_>,
    kbinfos: &mut Kbinfos,
    direction: &str,
    deadline_left: Option<f64>,
    available: &HashSet<String>,
    ctx: Option<&mut NavContext>,
) -> (Vec<Value>, Vec<String>, Vec<Value>, String) {
    if available.is_empty() || !available.contains("navigate_tree") {
        return (Vec::new(), Vec::new(), Vec::new(), String::new());
    }
    let mut owned;
    let ctx: &mut NavContext = match ctx {
        Some(ctx) => ctx,
        None => {
            owned = NavContext {
                direction: direction.to_string(),
                ..NavContext::default()
            };
            &mut owned
        }
    };
    let mut messages: Vec<Value> = Vec::new();
    let mut evidence_ids: Vec<String> = Vec::new();
    let mut outcomes: Vec<Value> = Vec::new();
    let budget = (deadline_left.unwrap_or(ACTION_TIMEOUT_S) * NAV_PREFIX_BUDGET_RATIO).max(5.0);
    let pending = run_nav_chain(
        runtime,
        kbinfos,
        ctx,
        NAV_START_RULE,
        budget,
        available,
        &mut messages,
        &mut evidence_ids,
        &mut outcomes,
        "nav",
        0,
    )
    .await;
    (messages, evidence_ids, outcomes, pending)
}

/// `_extract_relevant_evidence`: chunks of the pool relevant to `direction`
/// (per-token substring hits, stable ranking), for seed-user injection.
pub fn extract_relevant_evidence(kbinfos: &Kbinfos, direction: &str, max_chunks: usize) -> String {
    let chunks = &kbinfos.chunks;
    if chunks.is_empty() {
        return String::new();
    }
    let token_re = Regex::new(r"[a-zA-Z0-9\u{4e00}-\u{9fff}]{2,}").expect("token regex");
    let lowered = direction.to_lowercase();
    let dir_tokens: HashSet<String> = token_re
        .find_iter(&lowered)
        .map(|m| m.as_str().to_string())
        .collect();
    let mut ranked: Vec<&Value> = if dir_tokens.is_empty() {
        let start = chunks.len().saturating_sub(max_chunks);
        chunks[start..].iter().collect()
    } else {
        let mut scored: Vec<(usize, &Value)> = chunks
            .iter()
            .map(|chunk| {
                let text = chunk_utils::chunk_text(chunk).to_lowercase();
                let rel = dir_tokens
                    .iter()
                    .filter(|token| text.contains(token.as_str()))
                    .count();
                (rel, chunk)
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, chunk)| chunk).collect()
    };
    ranked.truncate(max_chunks);
    let lines: Vec<String> = ranked
        .iter()
        .map(|chunk| {
            let cid = chunk_utils::chunk_id(chunk);
            let text: String = chunk_utils::chunk_text(chunk)
                .chars()
                .take(300)
                .collect::<String>()
                .replace('\n', " ");
            format!("[{cid}] {text}")
        })
        .collect();
    lines.join("\n")
}

/// Verbatim rag/prompts/action_run.md (as load_prompt returns it: stripped).
pub const ACTION_RUN_PROMPT: &str = r#"You are a deep research assistant working INSIDE a bounded search tree.

Input: the user message contains ONE research `Direction`, plus the current
`State` (slot table with immutable ids and mutable candidate fields).

Execute this single direction. Reply with EXACTLY ONE of the three below.

The three differ in HOW they are delivered — read this carefully, because only
the first one is a tool call:

1) TOOL CALL MODE — a real tool call: call `retrieve` with 1-3 corpus queries.
   Results arrive in the next turn.

2) STATE PATCH MODE — NOT a tool call. Write this XML as plain TEXT in your
   reply body (do not call any tool named "state"):
<state>
{"new_states": [
  {"state": [{"id": <int>, "candidate": "<value>", "candidate_strength": <0..1>, "discovered_clues": ["..."]}, ...]},
  ...more branches allowed...
]}
</state>
Rules: patch ONLY existing ids; include ONLY changed variables; every change must trace to retrieved evidence; candidate_strength semantics: proven >0.9, strong 0.7-0.9, tentative 0.4-0.7, weak <0.4. An EMPTY branch list (`"new_states": []`) signals no progress — emit it rather than calling tools forever.

3) FINAL ANSWER MODE — NOT a tool call either. Write this XML as plain TEXT in
   your reply body (do not call any tool named "answer"). Use it only when ALL
   slots can be filled consistently:
<answer>
{"answer": "<final answer text>", "new_state": [{"id": ..., "candidate": ..., "candidate_strength": ...}]}
</answer>

CRITICAL RULES
- Think before choosing a mode, but output exactly ONE mode per response.
- Strength >0.7 on the answer slot means you MUST emit final answer instead of another state patch.
- ALWAYS end this action with a state patch: a patch with your updates, or `<state>{"new_states": []}</state>` if you found nothing new.
- **Do NOT keep calling tools once the direction is reasonably exhausted.** If further searches return repetitive, irrelevant, or empty results, immediately return a state patch (with updates or empty). Extra redundant searches waste the session — stop after 1-2 useful tool calls per direction unless a NEW fact is actually emerging.
- ACTION COMPLETION IS MANDATORY: when you have what you need (or hit a dead end), output the state patch now. Do not ask to continue searching.
- Unverifiable candidates must be eliminated (set candidate null) with a clue documenting why.
- Partial verification is OK: record a candidate at tentative strength (0.4-0.7) if you can't fully verify it yet, and move on.

# TOOL PLAYBOOK

You get tools only in medium / high (7 tools: `retrieve`, `search_chunks`, `list_chunks`, `navigate_tree`, `navigate_structure`, `calculate`, `web_search`) and ultra (those 7 + `graph_explore`). Low mode has NO tool loop — answer with plain retrieval. Every native tool call still REQUIRES the decision envelope from CRITICAL RULES (it is a mandatory tool parameter, not optional).

## 1. Combination chains (call in this order)

- **You already hold a `doc_id`** → `navigate_structure(doc_id, query)` to find the right passage, then `list_chunks(doc_id)` to read it. Do NOT call `navigate_tree` first.
- **No `doc_id` yet, and the corpus is large** → `navigate_tree(query)` to route to candidate documents, take a `doc_id`, then `navigate_structure(doc_id, query)` → `list_chunks(doc_id)`.
- **Exact term / short answer** → `retrieve(query[1-3])` first; if snippets are insufficient, `search_chunks(query[1-2])` (semantic, may find passages with NO shared surface words); if you need the full document, `list_chunks(doc_id)`.
- **You must DERIVE a number** → first collect every needed number with any of the above, then `calculate(question, facts)` with the facts verbatim, and report the computed result as-is. If the answer is already one of the stated numbers, answer directly.
- **Relational multi-hop (ultra only)** → get a start entity from `search_chunks` / `navigate_structure`, then `graph_explore(query, doc_scope)`.

## 2. Convergence rules (hard, enforced by the runtime — follow them to avoid wasted turns)

- Make at most **1-2 useful tool calls per direction**, then emit a state patch. Do not keep searching once the direction is reasonably exhausted.
- Re-submitting the SAME intent with a paraphrase is intercepted as a near-duplicate and SKIPPED (you get a nudge, not new results). Change the angle or patch what you have.
- If a compile-only tool (`navigate_tree` / `navigate_structure` / `graph_explore`) returns "no compiled structure", switch to `search_chunks` / `retrieve` / `list_chunks` **immediately**. A second such result disables that tool for the REST of the session — do not retry it.
- `web_search` only appears when a web provider is configured; if it does, use it ONLY for world knowledge / time-sensitive facts that plausibly live outside the fixed corpus.
- `list_chunks` accepts ONLY `doc_id` (no `chunk_ids` argument) — you cannot ask it to read specific chunks; it returns the whole document (capped).

## 3. What a tool result means → your next action

Each tool returns a status. Act on it:

| Status | Meaning | Your next action |
| --- | --- | --- |
| `ok` | New evidence entered the shared pool | Fill slots / move to the next direction |
| `miss` | This query matched nothing, but the tool itself is valid | Rephrase or switch tools — do NOT conclude the dataset lacks it |
| `empty` (`no_structure`) | Dataset-level: no such compiled structure exists | Switch to `search_chunks` / `retrieve` / `list_chunks` now; retrying disables the tool |
| `poor` | Output returned but too weak to use | Add evidence with another tool |
| `redundant` | Every hit was already in your evidence | Stop re-searching; emit a `<state>` patch with what you have |
| `error` | Infrastructure / provider failure | Switch tools; do not retry the same call |"#;

/// Verbatim rag/prompts/action_initialize_state.md.
pub const ACTION_INITIALIZE_STATE_PROMPT: &str = r#"You are a research strategist. Decompose the user's question into a table of FACT SLOTS that must be filled to answer it.

The question may span MULTIPLE DATASETS and/or the open WEB. The available data sources are listed in the user message:

- "Dataset '<name>'" — slots whose facts live in that corpus. If more than one dataset is listed, create at least one slot PER DATASET (its facts may need corpus-specific phrasing); cross-referencing between datasets is encouraged when the question spans them.
- "Web" — when listed, the open web is a source: create a dedicated slot (type "web") for facts that are current-world knowledge, recent events, or simply not covered by the listed datasets.

Output ONLY a JSON object:
{
  "slots": [
    {"id": 0, "type": "<entity|person|date|duration|count|number|place|web|dataset>", "clues": ["<what identifies this slot from the question>", "..."], "source": "<dataset name or 'web'>"},
    ...
  ],
  "first_queries": ["<concrete searchable query for the first retrieval round>", ...]
}

Rules:
- 2-6 slots; each slot ONE fact (a name, a date, a count...), never a clause.
- Order slots so the FIRST one holds the top-level requested fact; the later ones are its dependencies.
- Cover EVERY listed source: one or more slots per dataset, plus a "web" slot when Web is available and the question touches world knowledge or recent events.
- clues must be self-contained phrases usable as retrieval hints.
- 1-4 first_queries: direct keyword-style searches against the listed sources.
- No prose outside JSON."#;

// ── Session entry + slot-table builder (upstream run_action_session / _init_chat / _init_retry_timeout / initialize_state) ──

/// `_init_retry_timeout`: budget for the slot-table decomposition retry.
///
/// - floor: the first attempt's budget (never shrink below what already failed),
/// - ceiling: 2x the first attempt, capped at a generous 90s absolute bound,
/// - deadline: leave at least 5s of the round budget for the rest of the session.
pub fn init_retry_timeout(first_tmo: f64, deadline_left: Option<f64>) -> f64 {
    let ceiling = (2.0 * first_tmo).min(90.0);
    match deadline_left {
        None => ceiling,
        Some(left) if left <= 0.0 => ceiling,
        Some(left) => first_tmo.max(ceiling.min(left - 5.0)),
    }
}

/// `_init_chat`: ONE bounded LLM turn for the slot-table decomposition
/// ({system, user} messages; a timeout or error yields "").
async fn init_chat(llm: &dyn ActionLlmBackend, system: &str, user: &str, timeout_s: f64) -> String {
    let messages = vec![
        serde_json::json!({"role": "system", "content": system}),
        serde_json::json!({"role": "user", "content": user}),
    ];
    llm.complete_plain(&messages, timeout_s)
        .await
        .map(|reply| reply.content)
        .unwrap_or_default()
}

/// `initialize_state`: decompose the question into a slot table + first queries.
pub async fn initialize_state(
    llm: &dyn ActionLlmBackend,
    question: &str,
    fanout_hint: &[String],
    deadline_left: Option<f64>,
) -> (SlotState, Vec<String>) {
    let system = ACTION_INITIALIZE_STATE_PROMPT;
    let mut user = format!("Question: {question}");
    if !fanout_hint.is_empty() {
        user.push_str("\n\nCandidate aspects already identified:\n");
        user.push_str(
            &fanout_hint
                .iter()
                .map(|hint| format!("- {hint}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    let tmo = INIT_TIMEOUT_S.min(deadline_left.unwrap_or(INIT_TIMEOUT_S));
    let mut raw = init_chat(llm, system, &user, tmo).await;
    let mut data = extract_json(&raw)
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let empty = data
        .as_object()
        .map(|object| object.is_empty())
        .unwrap_or(true);
    if empty {
        // one quick retry with a longer budget (slow models time out at 45s).
        let retry_tmo = init_retry_timeout(tmo, deadline_left);
        raw = init_chat(llm, system, &user, retry_tmo).await;
        data = extract_json(&raw)
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
    }
    let mut slots: Vec<Variable> = Vec::new();
    if let Some(items) = data.get("slots").and_then(Value::as_array) {
        for (index, slot) in items.iter().enumerate() {
            if !slot.is_object() {
                continue;
            }
            let id = slot
                .get("id")
                .and_then(|value| {
                    value
                        .as_i64()
                        .or_else(|| value.as_f64().map(|number| number as i64))
                })
                .unwrap_or(index as i64);
            let slot_type = slot
                .get("type")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .unwrap_or("entity")
                .to_string();
            let clues: Vec<String> = slot
                .get("clues")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .take(4)
                        .map(|clue| {
                            clue.as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| clue.to_string())
                        })
                        .collect()
                })
                .unwrap_or_default();
            slots.push(Variable {
                id,
                r#type: slot_type,
                question_clues: clues,
                ..Variable::default()
            });
        }
    }
    let mut first_queries: Vec<String> = data
        .get("first_queries")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .take(3)
                .map(|query| {
                    query
                        .as_str()
                        .map(|text| text.trim().to_string())
                        .unwrap_or_else(|| query.to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    if slots.is_empty() {
        // Decomposition failed (timeout/parse): build the table from planner
        // fanouts so the first round still targets DISTINCT aspects.
        let hint_slots: Vec<Variable> = fanout_hint
            .iter()
            .take(4)
            .enumerate()
            .map(|(index, hint)| Variable {
                id: index as i64,
                r#type: "aspect".to_string(),
                question_clues: vec![hint.chars().take(120).collect()],
                ..Variable::default()
            })
            .collect();
        if !hint_slots.is_empty() {
            slots = hint_slots;
            if first_queries.is_empty() {
                first_queries = fanout_hint.iter().take(3).cloned().collect();
            }
        } else {
            slots = vec![Variable {
                id: 0,
                r#type: "answer".to_string(),
                question_clues: vec![question.to_string()],
                ..Variable::default()
            }];
            if first_queries.is_empty() {
                first_queries = vec![question.to_string()];
            }
        }
    } else if first_queries.is_empty() {
        first_queries = vec![question.to_string()];
    }
    (SlotState::new(slots, 0), first_queries)
}

/// `run_action_session`: bounded graph-edge session pursuing ONE direction.
#[allow(clippy::too_many_arguments)]
pub async fn run_action_session(
    runtime: &ActionRuntime<'_>,
    llm: Option<&dyn ActionLlmBackend>,
    kbinfos: &mut Kbinfos,
    direction: &str,
    parent_state: &SlotState,
    mode_tools: &std::collections::BTreeSet<String>,
    mode: &str,
    has_web: bool,
    disabled: &HashSet<String>,
    deadline_left: Option<f64>,
    base_summary: &str,
    shared_tool_cache: Option<std::collections::HashMap<String, ToolOutcome>>,
    shared_search_queries: Option<Vec<String>>,
) -> SessionResult {
    let Some(llm) = llm else {
        // Upstream: no usable model resolved -> the session reports nothing.
        return SessionResult::default();
    };
    let mut seed_user = format!(
        "Direction: {direction}\n\nState:\n{}",
        parent_state.render_slots()
    );
    let existing = extract_relevant_evidence(kbinfos, direction, 4);
    if !existing.is_empty() {
        seed_user.push_str(
            "\n\nALREADY RETRIEVED (do NOT re-retrieve these — use them to fill slots or identify gaps):\n",
        );
        seed_user.push_str(&existing);
    }
    if !base_summary.is_empty() {
        seed_user.push_str(&format!("\n\nPrior round summary:\n{base_summary}"));
    }
    let budget_left = deadline_left.unwrap_or(ACTION_TIMEOUT_S);
    let prefix_started = std::time::Instant::now();
    let mut nav_ctx = NavContext {
        direction: direction.to_string(),
        ..NavContext::default()
    };
    // Walk the navigation ladder up to its first model-driven rung; the ladder
    // is guaranteed in code, so the model cannot skip navigate_tree. Skipped
    // entirely when NAV_RULES_ENABLED is off, which also leaves nav_rule_id
    // empty so the in-session ladder never arms either.
    let (prefix_msgs, prefix_ids, prefix_outcomes, pending_rule) = if NAV_RULES_ENABLED {
        let available: HashSet<String> = active_tool_specs(mode_tools, has_web, disabled)
            .into_iter()
            .collect();
        run_nav_prefix(
            runtime,
            kbinfos,
            direction,
            Some(budget_left),
            &available,
            Some(&mut nav_ctx),
        )
        .await
    } else {
        (Vec::new(), Vec::new(), Vec::new(), String::new())
    };
    let spent = prefix_started.elapsed().as_secs_f64();

    let mut state = SessionState::new(
        parent_state.clone(),
        mode,
        mode_tools.clone(),
        has_web,
        direction,
    );
    state
        .messages
        .push(serde_json::json!({"role": "system", "content": ACTION_RUN_PROMPT}));
    state
        .messages
        .push(serde_json::json!({"role": "user", "content": seed_user}));
    state.messages.extend(prefix_msgs);
    state.retrieved_evidence_ids = prefix_ids;
    state.deadline_left = Some((budget_left - spent).max(10.0));
    state.tool_cache = shared_tool_cache.unwrap_or_default();
    state.seen_queries = shared_search_queries.unwrap_or_default();
    state.outcomes = prefix_outcomes;
    state.routed_docs = nav_ctx.known_docs.clone();
    state.nav_rule_id = pending_rule;
    state.disabled = disabled.clone();
    run_session_loop(runtime, llm, kbinfos, &mut state).await;
    SessionResult {
        messages: state.messages,
        new_states: state.new_states,
        found_answer: state.found_answer,
        retrieved_evidence_ids: state.retrieved_evidence_ids,
        terminal_type: state.terminal_type,
        terminal_payload: state.terminal_payload,
    }
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
            None,
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
            None,
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
            None,
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
            None,
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
#[cfg(test)]
mod session_loop_tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

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
            Ok(Vec::new())
        }
        async fn hybrid_search(&self, _q: &str, _t: usize, _c: bool) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        async fn list_chunks(&self, _d: &str) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        fn has_web(&self) -> bool {
            false
        }
        async fn web_search(&self, _q: &str) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
    }

    struct MockTools;

    #[async_trait]
    impl ActionToolBackend for MockTools {
        async fn navigate_tree(&self, _q: &str) -> NavResult {
            NavResult::default()
        }
        async fn navigate_structure(&self, _d: &str, _q: &str, _k: &str) -> NavResult {
            NavResult::default()
        }
        async fn calculate(&self, _q: &str, _facts: &[String]) -> Option<Value> {
            Some(json!({"expression": "1+1", "result": "2"}))
        }
        async fn graph_explore(&self, _q: &str, _s: &[String]) -> Value {
            json!({"answer": "", "chunks": []})
        }
    }

    struct MockLlm {
        with_tools: Mutex<Vec<Option<LlmReply>>>,
        plain: Mutex<Vec<Option<LlmReply>>>,
        with_tools_calls: Mutex<Vec<(Vec<Value>, Vec<Value>, f64)>>,
        plain_calls: Mutex<Vec<(Vec<Value>, f64)>>,
    }

    impl MockLlm {
        fn new(with_tools: Vec<Option<LlmReply>>, plain: Vec<Option<LlmReply>>) -> Self {
            Self {
                with_tools: Mutex::new(with_tools),
                plain: Mutex::new(plain),
                with_tools_calls: Mutex::new(Vec::new()),
                plain_calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ActionLlmBackend for MockLlm {
        async fn complete_with_tools(
            &self,
            messages: &[Value],
            tool_schemas: &[Value],
            timeout_s: f64,
        ) -> Option<LlmReply> {
            self.with_tools_calls.lock().unwrap().push((
                messages.to_vec(),
                tool_schemas.to_vec(),
                timeout_s,
            ));
            let mut queue = self.with_tools.lock().unwrap();
            if queue.is_empty() {
                None
            } else {
                queue.remove(0)
            }
        }
        async fn complete_plain(&self, messages: &[Value], timeout_s: f64) -> Option<LlmReply> {
            self.plain_calls
                .lock()
                .unwrap()
                .push((messages.to_vec(), timeout_s));
            let mut queue = self.plain.lock().unwrap();
            if queue.is_empty() {
                None
            } else {
                queue.remove(0)
            }
        }
    }

    fn content_reply(content: &str) -> LlmReply {
        LlmReply {
            content: content.to_string(),
            tool_calls: Vec::new(),
        }
    }

    fn call_reply(id: &str, name: &str, args: &Value) -> LlmReply {
        LlmReply {
            content: "working on it".to_string(),
            tool_calls: vec![json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": args.to_string()},
            })],
        }
    }

    fn session_fixture() -> SessionState {
        let mut session = SessionState::new(
            SlotState::new(
                vec![
                    Variable {
                        id: 0,
                        r#type: "found".to_string(),
                        candidate: Some("x".to_string()),
                        ..Variable::default()
                    },
                    Variable {
                        id: 1,
                        r#type: "missing".to_string(),
                        ..Variable::default()
                    },
                ],
                0,
            ),
            "ultra",
            BTreeSet::from([
                "retrieve".to_string(),
                "search_chunks".to_string(),
                "calculate".to_string(),
            ]),
            false,
            "who?",
        );
        session
            .messages
            .push(json!({"role": "user", "content": "who?"}));
        session
    }

    #[tokio::test]
    async fn run_action_node_terminal_converges() {
        let llm = MockLlm::new(
            vec![Some(content_reply(
                "<answer>{\"answer\": \"forty-two\"}</answer>",
            ))],
            vec![],
        );
        let mut state = session_fixture();
        run_action_node(&llm, &mut state).await;
        assert!(state.done);
        assert_eq!(state.found_answer.as_deref(), Some("forty-two"));
        assert_eq!(state.attempts, 1);
        assert_eq!(
            state.messages.len(),
            1,
            "terminal turns append no assistant message"
        );
        let calls = llm.with_tools_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].2, ACTION_TIMEOUT_S);
        assert!(!calls[0].1.is_empty(), "tool schemas must be advertised");
    }

    #[tokio::test]
    async fn run_action_node_timeout_converges_empty() {
        let llm = MockLlm::new(vec![None], vec![]);
        let mut state = session_fixture();
        run_action_node(&llm, &mut state).await;
        assert!(state.done);
        assert!(state.found_answer.is_none());
        assert!(state.new_states.is_empty());
        assert_eq!(state.attempts, 1);
        assert_eq!(
            state.messages.len(),
            1,
            "no message is appended on a timeout"
        );
    }

    #[tokio::test]
    async fn run_action_node_tool_call_message_shape() {
        let args = json!({"query": ["alpha"]});
        let llm = MockLlm::new(vec![Some(call_reply("c1", "search_chunks", &args))], vec![]);
        let mut state = session_fixture();
        run_action_node(&llm, &mut state).await;
        assert!(!state.done);
        assert_eq!(state.pending_calls.len(), 1);
        assert_eq!(state.pending_calls[0].id, "c1");
        assert_eq!(state.pending_calls[0].name, "search_chunks");
        assert!(!state.pending_calls[0].unknown);
        let message = &state.messages[1];
        assert_eq!(message["role"], "assistant");
        assert_eq!(message["content"], "working on it");
        let call = &message["tool_calls"][0];
        assert_eq!(call["id"], "c1");
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["name"], "search_chunks");
        let arguments: Value =
            serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(arguments, args);
    }

    #[tokio::test]
    async fn run_action_node_nudges_without_terminal() {
        let llm = MockLlm::new(vec![Some(content_reply("just thinking"))], vec![]);
        let mut state = session_fixture();
        run_action_node(&llm, &mut state).await;
        assert!(!state.done);
        assert!(state.pending_calls.is_empty());
        assert_eq!(state.messages.len(), 3);
        assert_eq!(state.messages[1]["role"], "assistant");
        assert_eq!(state.messages[2]["role"], "user");
        assert_eq!(state.messages[2]["content"], NUDGE_TEXT);
    }

    #[test]
    fn strip_unpaired_tool_calls_matrix() {
        let paired = json!({
            "role": "assistant",
            "content": "paired",
            "tool_calls": [{"id": "c1"}],
        });
        let unpaired = json!({
            "role": "assistant",
            "content": "unpaired",
            "tool_calls": [{"id": "c2"}],
        });
        let messages = vec![
            json!({"role": "user", "content": "q"}),
            paired.clone(),
            json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}),
            unpaired.clone(),
        ];
        let cleaned = strip_unpaired_tool_calls(&messages);
        assert_eq!(cleaned.len(), 4);
        assert!(cleaned[1].get("tool_calls").is_some(), "paired stays");
        assert!(cleaned[3].get("tool_calls").is_none(), "unpaired drops");
        assert_eq!(cleaned[3]["content"], "unpaired", "content is kept");
    }

    #[tokio::test]
    async fn finalize_prompts_strips_and_harvests() {
        let llm = MockLlm::new(vec![], vec![Some(content_reply("no JSON at all"))]);
        let mut state = session_fixture();
        state.messages.push(json!({
            "role": "assistant",
            "content": "working on it",
            "tool_calls": [{"id": "c9", "type": "function",
                "function": {"name": "calculate", "arguments": "{}"}}],
        }));
        state.messages.push(json!({
            "role": "assistant",
            "content": "the corpus narration carries a promising clue indeed",
        }));
        finalize_node(&llm, &mut state).await;
        assert!(state.done);
        assert_eq!(
            state.new_states.len(),
            1,
            "loose-clue harvest patches a slot"
        );
        let slot = state.new_states[0]
            .state
            .iter()
            .find(|variable| variable.id == 1)
            .unwrap();
        assert!(slot.discovered_clues[0].starts_with("narrative: "));
        let plain_calls = llm.plain_calls.lock().unwrap();
        assert_eq!(plain_calls.len(), 1);
        assert_eq!(plain_calls[0].1, 150.0);
        let history = &plain_calls[0].0;
        let last = history.last().unwrap();
        assert_eq!(last["role"], "user");
        assert_eq!(last["content"], BUDGET_PROMPT);
        let stripped = history
            .iter()
            .find(|message| message["content"] == "working on it")
            .unwrap();
        assert!(stripped.get("tool_calls").is_none());
    }

    #[test]
    fn route_matrix() {
        let mut state = session_fixture();
        assert_eq!(route(&state), RouteDecision::RunAction);
        state.done = true;
        assert_eq!(route(&state), RouteDecision::End);
        state.done = false;
        state.pending_calls.push(ToolCall {
            id: "c1".to_string(),
            name: "retrieve".to_string(),
            args: json!({}),
            unknown: false,
        });
        state.attempts = state.max_turns();
        assert_eq!(
            route(&state),
            RouteDecision::Tool,
            "pending beats the budget"
        );
        state.pending_calls.clear();
        assert_eq!(route(&state), RouteDecision::Finalize);
        state.attempts = 1;
        state.skipped_dup = 2;
        assert_eq!(route(&state), RouteDecision::Finalize);
        state.skipped_dup = 0;
        assert_eq!(route_after_tool(&state), RouteDecision::RunAction);
        state.attempts = state.max_turns();
        assert_eq!(route_after_tool(&state), RouteDecision::Finalize);
    }

    #[tokio::test]
    async fn run_session_loop_tool_then_answer() {
        let args = json!({"question": "1+1", "facts": ["1", "1"]});
        let llm = MockLlm::new(
            vec![
                Some(call_reply("c1", "calculate", &args)),
                Some(content_reply("<answer>{\"answer\": \"2\"}</answer>")),
            ],
            vec![],
        );
        let search = MockSearch;
        let tools = MockTools;
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let mut state = session_fixture();
        run_session_loop(&runtime, &llm, &mut kbinfos, &mut state).await;
        assert!(state.done);
        assert_eq!(state.found_answer.as_deref(), Some("2"));
        assert_eq!(state.attempts, 2);
        assert_eq!(state.messages.len(), 3);
        assert_eq!(state.messages[2]["role"], "tool");
        assert_eq!(state.messages[2]["tool_call_id"], "c1");
        assert_eq!(state.outcomes.len(), 1);
    }

    #[tokio::test]
    async fn run_session_loop_budget_finalizes() {
        let nudge = "thinking hard about the question without a terminal block";
        let llm = MockLlm::new(
            vec![
                Some(content_reply(nudge)),
                Some(content_reply(nudge)),
                Some(content_reply(nudge)),
                Some(content_reply(nudge)),
            ],
            vec![Some(content_reply("<state>{\"new_states\": []}</state>"))],
        );
        let search = MockSearch;
        let tools = MockTools;
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let mut state = session_fixture();
        run_session_loop(&runtime, &llm, &mut kbinfos, &mut state).await;
        assert!(state.done);
        assert_eq!(state.attempts, ACTION_MAX_TURNS);
        assert_eq!(llm.with_tools_calls.lock().unwrap().len(), 4);
        assert_eq!(llm.plain_calls.lock().unwrap().len(), 1);
        assert_eq!(
            state.new_states.len(),
            1,
            "finalize harvest keeps breadcrumbs"
        );
    }
}
#[cfg(test)]
mod nav_prefix_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn chunk(id: &str, content: &str, doc_id: &str) -> Value {
        json!({"chunk_id": id, "content_with_weight": content, "doc_id": doc_id})
    }

    struct NavMockSearch {
        grep: Vec<Value>,
        keywords: Mutex<Vec<Option<String>>>,
    }

    #[async_trait]
    impl ActionSearchBackend for NavMockSearch {
        async fn grep_search(
            &self,
            _q: &str,
            _t: usize,
            _s: Option<&[String]>,
            keywords: Option<&str>,
        ) -> Result<Vec<Value>, String> {
            self.keywords
                .lock()
                .unwrap()
                .push(keywords.map(str::to_string));
            Ok(self.grep.clone())
        }
        async fn hybrid_search(&self, _q: &str, _t: usize, _c: bool) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        async fn list_chunks(&self, _d: &str) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        fn has_web(&self) -> bool {
            false
        }
        async fn web_search(&self, _q: &str) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
    }

    struct NavMockTools {
        tree: NavResult,
        structure: NavResult,
    }

    #[async_trait]
    impl ActionToolBackend for NavMockTools {
        async fn navigate_tree(&self, _q: &str) -> NavResult {
            self.tree.clone()
        }
        async fn navigate_structure(&self, _d: &str, _q: &str, _k: &str) -> NavResult {
            self.structure.clone()
        }
        async fn calculate(&self, _q: &str, _f: &[String]) -> Option<Value> {
            None
        }
        async fn graph_explore(&self, _q: &str, _s: &[String]) -> Value {
            json!({"answer": "", "chunks": []})
        }
    }

    fn ctx_fixture() -> NavContext {
        NavContext {
            direction: "alpha topic".to_string(),
            known_docs: vec!["d1".to_string()],
            routed_docs: vec![("d1".to_string(), "routed summary".to_string())],
            nav_hint: "routed summary".to_string(),
        }
    }

    #[test]
    fn nav_rules_table_matches_upstream() {
        assert!(NAV_RULES_ENABLED);
        assert_eq!(NAV_START_RULE, "locate");
        assert_eq!(NAV_RULES.len(), 3);
        assert_eq!(NAV_RULES[0].id, "locate");
        assert_eq!(NAV_RULES[0].tool, "navigate_tree");
        assert_eq!(NAV_RULES[1].id, "drill");
        assert_eq!(NAV_RULES[1].tool, "navigate_structure");
        assert_eq!(NAV_RULES[2].id, "global");
        assert_eq!(NAV_RULES[2].tool, "retrieve");
        assert!(NAV_RULES.iter().all(|rule| rule.mode == NavMode::Auto));
        assert_eq!(NAV_RULES[0].next_rule(OUTCOME_OK), "drill");
        assert_eq!(NAV_RULES[0].next_rule(OUTCOME_MISS), "global");
        assert_eq!(NAV_RULES[0].next_rule(OUTCOME_POOR), "global");
        assert_eq!(NAV_RULES[1].next_rule(OUTCOME_OK), "");
        assert_eq!(NAV_RULES[1].next_rule(OUTCOME_EMPTY), "global");
        assert_eq!(NAV_RULES[2].next_rule(OUTCOME_OK), "");
        assert_eq!(
            NAV_RULES[0].next_rule(OUTCOME_REDUNDANT),
            "",
            "unmapped statuses end the chain"
        );
    }

    #[tokio::test]
    async fn drill_merge_soft_boosts_and_reranks() {
        let search = NavMockSearch {
            grep: vec![
                chunk("cB", "beta text", "d9"),
                chunk("cA", "alpha text", "d1"),
            ],
            keywords: Mutex::new(Vec::new()),
        };
        let tools = NavMockTools {
            tree: NavResult::default(),
            structure: NavResult {
                chunk_ptrs: 1,
                chunk_paths: HashMap::from([("cA".to_string(), "Root > A".to_string())]),
                ..NavResult::default()
            },
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let ctx = ctx_fixture();
        let available: HashSet<String> = ["retrieve".to_string(), "navigate_structure".to_string()]
            .into_iter()
            .collect();
        let oc = run_drill_merge(&runtime, &mut kbinfos, &ctx, &available, 10.0).await;
        assert_eq!(oc.status, OUTCOME_OK);
        assert_eq!(oc.payload.len(), 2);
        assert_eq!(oc.payload[0]["id"], "cA", "routed chunk floats to the top");
        assert_eq!(oc.payload[0]["nav_rank"], 0);
        assert_eq!(oc.payload[0]["structure_path"], "Root > A");
        assert_eq!(oc.payload[1]["id"], "cB", "non-routed chunk stays below");
        assert_eq!(oc.payload[1]["nav_rank"], 1);
        assert!(oc.payload[1].get("structure_path").is_none());
        assert!(oc.evidence_ids.contains(&"cA".to_string()));
        assert!(oc.evidence_ids.contains(&"d1".to_string()));
        let keywords = search.keywords.lock().unwrap();
        assert_eq!(
            keywords[0].as_deref(),
            Some("routed summary"),
            "nav hint is a soft BM25 boost"
        );
    }

    #[tokio::test]
    async fn nav_chain_locates_then_drills_to_completion() {
        let search = NavMockSearch {
            grep: vec![chunk("cA", "alpha text", "d1")],
            keywords: Mutex::new(Vec::new()),
        };
        let tools = NavMockTools {
            tree: NavResult {
                doc_ids: vec!["d1".to_string()],
                routed_docs: vec![("d1".to_string(), "routed summary".to_string())],
                ..NavResult::default()
            },
            structure: NavResult {
                chunk_ptrs: 1,
                chunk_paths: HashMap::from([("cA".to_string(), "Root > A".to_string())]),
                ..NavResult::default()
            },
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let mut ctx = NavContext {
            direction: "alpha topic".to_string(),
            ..NavContext::default()
        };
        let available: HashSet<String> = [
            "navigate_tree".to_string(),
            "retrieve".to_string(),
            "navigate_structure".to_string(),
        ]
        .into_iter()
        .collect();
        let mut messages: Vec<Value> = Vec::new();
        let mut evidence_ids: Vec<String> = Vec::new();
        let mut outcomes: Vec<Value> = Vec::new();
        let pending = run_nav_chain(
            &runtime,
            &mut kbinfos,
            &mut ctx,
            NAV_START_RULE,
            30.0,
            &available,
            &mut messages,
            &mut evidence_ids,
            &mut outcomes,
            "nav",
            0,
        )
        .await;
        assert_eq!(pending, "", "drill OK ends the ladder");
        assert_eq!(ctx.known_docs, vec!["d1".to_string()]);
        assert_eq!(
            ctx.routed_docs,
            vec![("d1".to_string(), "routed summary".to_string())]
        );
        assert_eq!(ctx.nav_hint, "routed summary");
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0]["name"], "navigate_tree");
        assert_eq!(outcomes[1]["name"], "navigate_structure");
        assert_eq!(messages.len(), 4, "two completed assistant/tool pairs");
        assert_eq!(messages[0]["tool_calls"][0]["id"], "nav_locate");
        assert_eq!(messages[2]["tool_calls"][0]["id"], "nav_drill");
        assert!(evidence_ids.contains(&"cA".to_string()));
        assert!(evidence_ids.contains(&"d1".to_string()));
    }

    #[tokio::test]
    async fn nav_prefix_requires_navigate_tree() {
        let search = NavMockSearch {
            grep: Vec::new(),
            keywords: Mutex::new(Vec::new()),
        };
        let tools = NavMockTools {
            tree: NavResult::default(),
            structure: NavResult::default(),
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let available: HashSet<String> = ["retrieve".to_string()].into_iter().collect();
        let (messages, evidence_ids, outcomes, pending) =
            run_nav_prefix(&runtime, &mut kbinfos, "q", Some(30.0), &available, None).await;
        assert!(messages.is_empty() && evidence_ids.is_empty() && outcomes.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn emit_nav_pair_caps_payload() {
        let mut messages: Vec<Value> = Vec::new();
        let oc = ToolOutcome::ok(
            vec![json!({"id": "c1", "content": "x"})],
            vec!["c1".to_string()],
        );
        let added = emit_nav_pair(
            &mut messages,
            "nav_global",
            "retrieve",
            &json!({"query": ["q"]}),
            &oc,
            0,
        );
        assert!(added > 0);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[1]["tool_call_id"], "nav_global");
        let parsed: Value = serde_json::from_str(messages[1]["content"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["passages"][0]["id"], "c1");

        let big = "y".repeat(2000);
        let oc_big = ToolOutcome::ok(
            vec![json!({"id": "c2", "content": big})],
            vec!["c2".to_string()],
        );
        let mut capped: Vec<Value> = Vec::new();
        let added = emit_nav_pair(
            &mut capped,
            "nav_global",
            "retrieve",
            &json!({}),
            &oc_big,
            100,
        );
        assert_eq!(added, 800, "max(max_chars, 800) keeps at least 800 chars");
        assert_eq!(capped[1]["content"].as_str().unwrap().chars().count(), 800);
    }

    #[test]
    fn extract_relevant_evidence_ranks_by_tokens() {
        let mut kbinfos = Kbinfos::default();
        kbinfos.chunks = vec![
            chunk("c1", "alpha", "d1"),
            chunk("c2", "gamma", "d1"),
            chunk("c3", "alpha beta gamma", "d1"),
        ];
        let text = extract_relevant_evidence(&kbinfos, "alpha gamma", 4);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("[c3] "), "two token hits rank first");
        assert!(lines[1].starts_with("[c1] "));
        assert!(lines[2].starts_with("[c2] "));
        let none = extract_relevant_evidence(&kbinfos, "", 2);
        let lines: Vec<&str> = none.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].starts_with("[c2] "),
            "no tokens -> the last chunks"
        );
        assert!(lines[1].starts_with("[c3] "));
        assert!(extract_relevant_evidence(&Kbinfos::default(), "alpha", 4).is_empty());
    }
}
#[cfg(test)]
mod session_entry_tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    struct EntryMockSearch {
        grep: Vec<Value>,
    }

    #[async_trait]
    impl ActionSearchBackend for EntryMockSearch {
        async fn grep_search(
            &self,
            _q: &str,
            _t: usize,
            _s: Option<&[String]>,
            _k: Option<&str>,
        ) -> Result<Vec<Value>, String> {
            Ok(self.grep.clone())
        }
        async fn hybrid_search(&self, _q: &str, _t: usize, _c: bool) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        async fn list_chunks(&self, _d: &str) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        fn has_web(&self) -> bool {
            false
        }
        async fn web_search(&self, _q: &str) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
    }

    struct EntryMockTools {
        tree: NavResult,
        structure: NavResult,
    }

    #[async_trait]
    impl ActionToolBackend for EntryMockTools {
        async fn navigate_tree(&self, _q: &str) -> NavResult {
            self.tree.clone()
        }
        async fn navigate_structure(&self, _d: &str, _q: &str, _k: &str) -> NavResult {
            self.structure.clone()
        }
        async fn calculate(&self, _q: &str, _f: &[String]) -> Option<Value> {
            Some(json!({"expression": "1+1", "result": "2"}))
        }
        async fn graph_explore(&self, _q: &str, _s: &[String]) -> Value {
            json!({"answer": "", "chunks": []})
        }
    }

    struct EntryMockLlm {
        with_tools: Mutex<Vec<Option<LlmReply>>>,
        plain: Mutex<Vec<Option<LlmReply>>>,
        with_tools_calls: Mutex<Vec<(Vec<Value>, f64)>>,
        plain_calls: Mutex<Vec<(Vec<Value>, f64)>>,
    }

    impl EntryMockLlm {
        fn new(with_tools: Vec<Option<LlmReply>>, plain: Vec<Option<LlmReply>>) -> Self {
            Self {
                with_tools: Mutex::new(with_tools),
                plain: Mutex::new(plain),
                with_tools_calls: Mutex::new(Vec::new()),
                plain_calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ActionLlmBackend for EntryMockLlm {
        async fn complete_with_tools(
            &self,
            messages: &[Value],
            _schemas: &[Value],
            timeout_s: f64,
        ) -> Option<LlmReply> {
            self.with_tools_calls
                .lock()
                .unwrap()
                .push((messages.to_vec(), timeout_s));
            let mut queue = self.with_tools.lock().unwrap();
            if queue.is_empty() {
                None
            } else {
                queue.remove(0)
            }
        }
        async fn complete_plain(&self, messages: &[Value], timeout_s: f64) -> Option<LlmReply> {
            self.plain_calls
                .lock()
                .unwrap()
                .push((messages.to_vec(), timeout_s));
            let mut queue = self.plain.lock().unwrap();
            if queue.is_empty() {
                None
            } else {
                queue.remove(0)
            }
        }
    }

    fn content_reply(content: &str) -> LlmReply {
        LlmReply {
            content: content.to_string(),
            tool_calls: Vec::new(),
        }
    }

    fn call_reply(id: &str, name: &str, args: &Value) -> LlmReply {
        LlmReply {
            content: String::new(),
            tool_calls: vec![json!({
                "id": id,
                "type": "function",
                "function": {"name": name, "arguments": args.to_string()},
            })],
        }
    }

    fn fixture_chunk(id: &str, content: &str, doc_id: &str) -> Value {
        json!({"chunk_id": id, "content_with_weight": content, "doc_id": doc_id})
    }

    fn parent_fixture() -> SlotState {
        SlotState::new(
            vec![Variable {
                id: 0,
                r#type: "answer".to_string(),
                question_clues: vec!["who?".to_string()],
                ..Variable::default()
            }],
            0,
        )
    }

    #[test]
    fn init_retry_timeout_matrix() {
        assert_eq!(init_retry_timeout(45.0, None), 90.0);
        assert_eq!(init_retry_timeout(45.0, Some(100.0)), 90.0);
        assert_eq!(init_retry_timeout(45.0, Some(60.0)), 55.0);
        assert_eq!(
            init_retry_timeout(45.0, Some(40.0)),
            45.0,
            "never below the first bound"
        );
        assert_eq!(init_retry_timeout(45.0, Some(3.0)), 45.0);
        assert_eq!(init_retry_timeout(45.0, Some(0.0)), 90.0);
    }

    #[tokio::test]
    async fn initialize_state_parses_llm_json() {
        let fenced = "```json\n{\"slots\": [{\"id\": 0, \"type\": \"person\", \"clues\": [\"c1\", \"c2\", \"c3\", \"c4\", \"c5\"], \"source\": \"ds\"}], \"first_queries\": [\"q1\", \"q2\", \"q3\", \"q4\"]}\n```";
        let llm = EntryMockLlm::new(vec![], vec![Some(content_reply(fenced))]);
        let (root, queries) = initialize_state(&llm, "who?", &[], None).await;
        assert_eq!(root.state.len(), 1);
        assert_eq!(root.state[0].r#type, "person");
        assert_eq!(
            root.state[0].question_clues.len(),
            4,
            "clues are capped at 4"
        );
        assert_eq!(queries.len(), 3, "first_queries are capped at 3");
        assert_eq!(queries[0], "q1");
        let calls = llm.plain_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "a parsed table needs no retry");
        assert_eq!(calls[0].1, INIT_TIMEOUT_S);
        assert_eq!(calls[0].0[0]["role"], "system");
        assert!(
            calls[0].0[1]["content"]
                .as_str()
                .unwrap()
                .starts_with("Question: who?")
        );
    }

    #[tokio::test]
    async fn initialize_state_falls_back_to_fanouts() {
        let llm = EntryMockLlm::new(vec![], vec![None, None]);
        let hints = vec!["aspect one".to_string(), "aspect two".to_string()];
        let (root, queries) = initialize_state(&llm, "who?", &hints, None).await;
        assert_eq!(root.state.len(), 2);
        assert!(root.state.iter().all(|slot| slot.r#type == "aspect"));
        assert_eq!(root.state[0].question_clues, vec!["aspect one".to_string()]);
        assert_eq!(queries, hints);
        let calls = llm.plain_calls.lock().unwrap();
        assert_eq!(calls.len(), 2, "one retry when the parse fails");
        assert_eq!(calls[1].1, 90.0, "the retry gets the doubled budget");
    }

    #[tokio::test]
    async fn initialize_state_answer_slot_fallback() {
        let llm = EntryMockLlm::new(vec![], vec![None, None]);
        let (root, queries) = initialize_state(&llm, "who exactly?", &[], None).await;
        assert_eq!(root.state.len(), 1);
        assert_eq!(root.state[0].r#type, "answer");
        assert_eq!(
            root.state[0].question_clues,
            vec!["who exactly?".to_string()]
        );
        assert_eq!(queries, vec!["who exactly?".to_string()]);
    }

    #[tokio::test]
    async fn run_action_session_without_model_returns_empty() {
        let search = EntryMockSearch { grep: Vec::new() };
        let tools = EntryMockTools {
            tree: NavResult::default(),
            structure: NavResult::default(),
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let mode_tools: BTreeSet<String> = ["retrieve".to_string()].into_iter().collect();
        let result = run_action_session(
            &runtime,
            None,
            &mut kbinfos,
            "who?",
            &parent_fixture(),
            &mode_tools,
            "high",
            false,
            &HashSet::new(),
            Some(60.0),
            "",
            None,
            None,
        )
        .await;
        assert!(result.messages.is_empty());
        assert!(result.found_answer.is_none());
        assert!(result.new_states.is_empty());
    }

    #[tokio::test]
    async fn run_action_session_end_to_end_terminal() {
        let search = EntryMockSearch { grep: Vec::new() };
        let tools = EntryMockTools {
            tree: NavResult::default(),
            structure: NavResult::default(),
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let llm = EntryMockLlm::new(
            vec![
                Some(call_reply(
                    "c1",
                    "calculate",
                    &json!({"question": "1+1", "facts": ["1", "1"]}),
                )),
                Some(content_reply("<answer>{\"answer\": \"2\"}</answer>")),
            ],
            vec![],
        );
        let mut kbinfos = Kbinfos::default();
        let mode_tools: BTreeSet<String> = ["calculate".to_string()].into_iter().collect();
        let result = run_action_session(
            &runtime,
            Some(&llm),
            &mut kbinfos,
            "how much?",
            &parent_fixture(),
            &mode_tools,
            "high",
            false,
            &HashSet::new(),
            Some(60.0),
            "",
            None,
            None,
        )
        .await;
        assert_eq!(result.found_answer.as_deref(), Some("2"));
        assert_eq!(result.messages.len(), 4);
        assert_eq!(result.messages[0]["role"], "system");
        assert_eq!(result.messages[0]["content"], ACTION_RUN_PROMPT);
        assert!(
            result.messages[1]["content"]
                .as_str()
                .unwrap()
                .contains("Direction: how much?")
        );
        assert_eq!(result.messages[2]["role"], "assistant");
        assert_eq!(result.messages[3]["role"], "tool");
        let calls = llm.with_tools_calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1 >= 15.0 && calls[0].1 <= ACTION_TIMEOUT_S);
    }

    #[tokio::test]
    async fn run_action_session_seeds_nav_prefix() {
        let search = EntryMockSearch {
            grep: vec![fixture_chunk("cA", "alpha text", "d1")],
        };
        let tools = EntryMockTools {
            tree: NavResult {
                doc_ids: vec!["d1".to_string()],
                routed_docs: vec![("d1".to_string(), "routed summary".to_string())],
                ..NavResult::default()
            },
            structure: NavResult {
                chunk_ptrs: 1,
                chunk_paths: std::collections::HashMap::from([(
                    "cA".to_string(),
                    "Root > A".to_string(),
                )]),
                ..NavResult::default()
            },
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let llm = EntryMockLlm::new(
            vec![Some(content_reply(
                "<answer>{\"answer\": \"done\"}</answer>",
            ))],
            vec![],
        );
        let mut kbinfos = Kbinfos::default();
        let mode_tools: BTreeSet<String> = [
            "navigate_tree".to_string(),
            "navigate_structure".to_string(),
            "retrieve".to_string(),
        ]
        .into_iter()
        .collect();
        let result = run_action_session(
            &runtime,
            Some(&llm),
            &mut kbinfos,
            "alpha topic",
            &parent_fixture(),
            &mode_tools,
            "high",
            false,
            &HashSet::new(),
            Some(60.0),
            "",
            None,
            None,
        )
        .await;
        assert_eq!(result.found_answer.as_deref(), Some("done"));
        assert_eq!(result.messages.len(), 6, "system + seed + two nav pairs");
        assert_eq!(result.messages[2]["tool_calls"][0]["id"], "nav_locate");
        assert_eq!(result.messages[4]["tool_calls"][0]["id"], "nav_drill");
        assert!(result.retrieved_evidence_ids.contains(&"cA".to_string()));
        assert!(result.retrieved_evidence_ids.contains(&"d1".to_string()));
        let calls = llm.with_tools_calls.lock().unwrap();
        assert!(
            calls[0].0[1]["content"]
                .as_str()
                .unwrap()
                .contains("Direction: alpha topic")
        );
    }

    #[test]
    fn prompt_consts_are_verbatim_and_trimmed() {
        assert!(ACTION_RUN_PROMPT.starts_with("You are a deep research assistant"));
        assert!(ACTION_RUN_PROMPT.contains("# TOOL PLAYBOOK"));
        assert!(!ACTION_RUN_PROMPT.ends_with(char::is_whitespace));
        assert!(ACTION_INITIALIZE_STATE_PROMPT.starts_with("You are a research strategist."));
        assert!(ACTION_INITIALIZE_STATE_PROMPT.contains("first_queries"));
        assert!(!ACTION_INITIALIZE_STATE_PROMPT.ends_with(char::is_whitespace));
    }
}
#[cfg(test)]
mod ladder_resume_tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashMap;

    fn chunk(id: &str, content: &str, doc_id: &str) -> Value {
        json!({"chunk_id": id, "content_with_weight": content, "doc_id": doc_id})
    }

    struct LadderMockSearch {
        grep: Vec<Value>,
    }

    #[async_trait]
    impl ActionSearchBackend for LadderMockSearch {
        async fn grep_search(
            &self,
            _q: &str,
            _t: usize,
            _s: Option<&[String]>,
            _k: Option<&str>,
        ) -> Result<Vec<Value>, String> {
            Ok(self.grep.clone())
        }
        async fn hybrid_search(&self, _q: &str, _t: usize, _c: bool) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        async fn list_chunks(&self, _d: &str) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
        fn has_web(&self) -> bool {
            false
        }
        async fn web_search(&self, _q: &str) -> Result<Vec<Value>, String> {
            Ok(Vec::new())
        }
    }

    struct LadderMockTools {
        tree: NavResult,
        structure: NavResult,
    }

    #[async_trait]
    impl ActionToolBackend for LadderMockTools {
        async fn navigate_tree(&self, _q: &str) -> NavResult {
            self.tree.clone()
        }
        async fn navigate_structure(&self, _d: &str, _q: &str, _k: &str) -> NavResult {
            self.structure.clone()
        }
        async fn calculate(&self, _q: &str, _f: &[String]) -> Option<Value> {
            None
        }
        async fn graph_explore(&self, _q: &str, _s: &[String]) -> Value {
            json!({"answer": "", "chunks": []})
        }
    }

    fn available_fixture() -> HashSet<String> {
        ["navigate_tree", "retrieve", "navigate_structure"]
            .iter()
            .map(|name| name.to_string())
            .collect()
    }

    #[tokio::test]
    async fn ladder_resumes_after_weak_rung() {
        let search = LadderMockSearch {
            grep: vec![chunk("cA", "alpha text", "d1")],
        };
        let tools = LadderMockTools {
            tree: NavResult {
                doc_ids: vec!["d1".to_string()],
                routed_docs: vec![("d1".to_string(), "routed summary".to_string())],
                ..NavResult::default()
            },
            structure: NavResult {
                chunk_ptrs: 1,
                chunk_paths: HashMap::from([("cA".to_string(), "Root > A".to_string())]),
                ..NavResult::default()
            },
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let pending = vec![ToolCall {
            id: "c1".to_string(),
            name: "navigate_tree".to_string(),
            args: json!({"query": "alpha topic"}),
            unknown: false,
        }];
        let mut seen_queries = Vec::new();
        let mut cache = HashMap::new();
        let mut strikes = HashMap::new();
        let mut disabled = HashSet::new();
        let available = available_fixture();
        let mut pending_rule = "locate".to_string();
        let mut ladder = LadderResume {
            direction: "alpha topic",
            routed_docs: &[],
            deadline_left: Some(60.0),
            pending_rule: &mut pending_rule,
            available: &available,
        };
        let result = tool_node_policy(
            &runtime,
            &mut kbinfos,
            &pending,
            &mut seen_queries,
            &mut cache,
            &mut strikes,
            &mut disabled,
            &[],
            Some(&mut ladder),
        )
        .await;
        assert_eq!(pending_rule, "", "drill OK ends the ladder");
        assert_eq!(result.tool_messages.len(), 3, "call + one ladder pair");
        assert_eq!(
            result.tool_messages[1]["tool_calls"][0]["id"],
            "ladder_drill"
        );
        assert_eq!(
            result.tool_messages[1]["tool_calls"][0]["function"]["name"],
            "navigate_structure"
        );
        assert_eq!(result.tool_messages[2]["role"], "tool");
        assert_eq!(result.outcomes.len(), 2, "call outcome + ladder outcome");
        assert_eq!(result.outcomes[1]["name"], "navigate_structure");
        assert!(result.evidence_ids.contains(&"cA".to_string()));
    }

    #[tokio::test]
    async fn ladder_skipped_when_rung_finished() {
        let search = LadderMockSearch { grep: Vec::new() };
        let tools = LadderMockTools {
            tree: NavResult {
                doc_ids: vec!["d1".to_string()],
                ..NavResult::default()
            },
            structure: NavResult::default(),
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let pending = vec![ToolCall {
            id: "c1".to_string(),
            name: "navigate_tree".to_string(),
            args: json!({"query": "alpha topic"}),
            unknown: false,
        }];
        let mut seen_queries = Vec::new();
        let mut cache = HashMap::new();
        let mut strikes = HashMap::new();
        let mut disabled = HashSet::new();
        let available = available_fixture();
        let mut pending_rule = "drill".to_string();
        let mut ladder = LadderResume {
            direction: "alpha topic",
            routed_docs: &[],
            deadline_left: Some(60.0),
            pending_rule: &mut pending_rule,
            available: &available,
        };
        let result = tool_node_policy(
            &runtime,
            &mut kbinfos,
            &pending,
            &mut seen_queries,
            &mut cache,
            &mut strikes,
            &mut disabled,
            &[],
            Some(&mut ladder),
        )
        .await;
        assert_eq!(
            pending_rule, "drill",
            "an OK verdict on drill ends the chain"
        );
        assert_eq!(result.tool_messages.len(), 1, "no ladder pair is appended");
        assert_eq!(result.outcomes.len(), 1);
    }

    #[tokio::test]
    async fn ladder_absent_by_default() {
        let search = LadderMockSearch { grep: Vec::new() };
        let tools = LadderMockTools {
            tree: NavResult::default(),
            structure: NavResult::default(),
        };
        let runtime = ActionRuntime {
            search: &search,
            tools: &tools,
        };
        let mut kbinfos = Kbinfos::default();
        let pending = vec![ToolCall {
            id: "c1".to_string(),
            name: "navigate_tree".to_string(),
            args: json!({"query": "alpha topic"}),
            unknown: false,
        }];
        let mut seen_queries = Vec::new();
        let mut cache = HashMap::new();
        let mut strikes = HashMap::new();
        let mut disabled = HashSet::new();
        let result = tool_node_policy(
            &runtime,
            &mut kbinfos,
            &pending,
            &mut seen_queries,
            &mut cache,
            &mut strikes,
            &mut disabled,
            &[],
            None,
        )
        .await;
        assert_eq!(result.tool_messages.len(), 1);
        assert_eq!(result.outcomes.len(), 1);
    }
}
