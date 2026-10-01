//! LangGraph agentic-search graph — RAGFlow v0.27.2
//! `rag/advanced_rag/agentic_rag_graph.py`.
//!
//! The five-phase Agentic RAG sequence (planner fan-out → parallel research →
//! sufficient-context review → gap-pursuit rewrite loop → synthesis) lives in
//! ONE compiled graph for medium/high/ultra; `low` keeps the lightweight
//! direct-search graph.
//!
//! Port mapping: LangGraph's reducer state becomes [`AgenticState`]; the graph
//! builders become explicit node functions driven by a loop; LLM / search /
//! KB services arrive through injected contracts (same pattern as the
//! action-session port). The think-stream splitter is a feed-based state
//! machine because Rust has no async generators.

use std::collections::HashSet;
use std::time::Instant;

use regex::Regex;
use serde_json::Value;

use crate::harness::action_session::SlotState;
use crate::harness::chunk_utils;
use crate::harness::orchestrator::direct::Kbinfos;
use crate::harness::tools::search::query_to_terms;

// ── Global research budget & per-call timeouts (source-level switches) ──

/// Whole-graph wall-clock ceiling per question.
pub const TOTAL_BUDGET_S: f64 = 180.0;
/// Need at least this much left to start a new round.
pub const MIN_ROUND_HEADROOM_S: f64 = 50.0;
/// Slot research pass wall-clock.
pub const PASS_TIMEOUT_S: f64 = 120.0;
/// Programmatic fan-out fetch.
pub const PREFETCH_TIMEOUT_S: f64 = 90.0;
/// Fallback draft synthesis.
pub const DRAFT_TIMEOUT_S: f64 = 60.0;
/// Sufficient-context review call.
pub const SCA_TIMEOUT_S: f64 = 60.0;
/// Gap → query rewrite call.
pub const REWRITE_TIMEOUT_S: f64 = 45.0;
/// Chunks shown to the Sufficient Context Agent per review.
pub const SCA_VIEW_CAP: usize = 60;

const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";

fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `_snip`: whitespace-collapsed one-line rendering with a truncation suffix.
pub fn snip(value: &Value, limit: usize) -> String {
    let text = value_to_text(value);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = collapsed.chars().collect();
    if chars.len() > limit {
        let head: String = chars[..limit].iter().collect();
        format!("{head}...(+{} chars)", chars.len() - limit)
    } else {
        collapsed
    }
}

/// `_safe_list`: coerce a possibly-poisoned graph-state field into a plain
/// list. Rust has no coroutine objects, so the poisoning branch is reserved
/// for genuine type surprises (logged by callers via the returned emptiness).
pub fn safe_list(value: &Value) -> Vec<Value> {
    match value {
        Value::Null => Vec::new(),
        Value::Array(items) => items.clone(),
        _ => Vec::new(),
    }
}

/// `_is_poisoned`: upstream checks for coroutine objects; always false here.
pub fn is_poisoned(_value: &Value) -> bool {
    false
}

/// `_select_sca_view`: rank the stored pool down to the SCA review view.
/// Returns `(view, identity)` — `identity` is a stable hash of the selected
/// chunk ids (upstream uses Python's per-process string hash; the port uses a
/// deterministic SipHash so the unproductive-round detector is reproducible).
pub fn select_sca_view(
    chunks: &[Value],
    focus_terms: &[String],
    cap: Option<usize>,
) -> (Vec<Value>, String) {
    let capped = cap.unwrap_or(SCA_VIEW_CAP);
    let mut terms: Vec<String> = Vec::new();
    for term in focus_terms {
        let term = term.to_lowercase();
        if term.chars().count() >= 3 && !terms.iter().any(|existing| existing == &term) {
            terms.push(term);
        }
    }
    let score = |index: usize, chunk: &Value| -> f64 {
        let text = ["content", "content_with_weight", "title", "question_toks"]
            .iter()
            .map(|key| chunk.get(*key).map(value_to_text).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        let coverage = terms
            .iter()
            .filter(|term| text.contains(term.as_str()))
            .count();
        let coverage_ratio = if terms.is_empty() {
            0.5
        } else {
            coverage as f64 / terms.len() as f64
        };
        let relevance = chunk
            .get("similarity")
            .and_then(Value::as_f64)
            .filter(|value| *value != 0.0)
            .or_else(|| chunk.get("score").and_then(Value::as_f64))
            .unwrap_or(0.0);
        let freshness = ((index as f64) / 20.0).min(0.2);
        relevance * 0.45 + coverage_ratio.min(1.0) * 0.45 + freshness
    };
    let mut ranked: Vec<(usize, f64)> = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| (index, score(index, chunk)))
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let view: Vec<Value> = ranked
        .iter()
        .take(capped)
        .map(|(index, _)| chunks[*index].clone())
        .collect();
    let mut ids: Vec<String> = view
        .iter()
        .map(|chunk| chunk_utils::chunk_id(chunk))
        .filter(|id| !id.is_empty())
        .collect();
    ids.sort();
    let identity = ids.join("|");
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    identity.hash(&mut hasher);
    (view, hasher.finish().to_string())
}

/// `_view_terms`: terms describing what the SCA should look FOR this round.
pub fn view_terms(state: &AgenticState) -> Vec<String> {
    let mut terms = query_to_terms(&state.question);
    for query in safe_list(&Value::Array(state.current_queries.clone())) {
        if let Some(text) = query.as_str() {
            terms.extend(query_to_terms(text));
        }
    }
    let mut out: Vec<String> = Vec::new();
    for term in terms {
        if !out.iter().any(|existing| existing == &term) {
            out.push(term);
        }
    }
    out
}

/// `_remaining_s`: seconds left in the question's global research budget
/// (infinite when unset).
pub fn remaining_s(state: &AgenticState) -> f64 {
    match state.deadline {
        Some(deadline) => deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64(),
        None => f64::INFINITY,
    }
}

/// `_bounded`: await `future` under a wall-clock bound; `None` on expiry.
pub async fn bounded<F, T>(future: F, timeout_s: f64) -> Option<T>
where
    F: std::future::Future<Output = T>,
{
    tokio::time::timeout(std::time::Duration::from_secs_f64(timeout_s), future)
        .await
        .ok()
}

/// `_partial_tag_tail`: length of the longest suffix of `text` that is a
/// prefix of `tag`.
pub fn partial_tag_tail(text: &str, tag: &str) -> usize {
    let max = text
        .chars()
        .count()
        .min(tag.chars().count().saturating_sub(1));
    for k in (1..=max).rev() {
        let suffix: String = text.chars().skip(text.chars().count() - k).collect();
        let prefix: String = tag.chars().take(k).collect();
        if suffix == prefix {
            return k;
        }
    }
    0
}

/// `"think"` vs `"answer"` stream piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkKind {
    Think,
    Answer,
}

/// `_split_think_stream` (feed-based): split model deltas into think / answer
/// text. Besides ordinary `<think>...</think>` streams, some providers emit the
/// opening tag only on the first reasoning delta and append `</think>` to every
/// subsequent delta, so an unmatched closing tag still marks the text before it
/// as reasoning.
#[derive(Debug, Default)]
pub struct ThinkSplitter {
    buf: String,
    in_think: bool,
}

impl ThinkSplitter {
    /// Feed one model delta; returns the ready `(kind, text)` pieces.
    pub fn push(&mut self, token: &str) -> Vec<(ThinkKind, String)> {
        let mut out: Vec<(ThinkKind, String)> = Vec::new();
        self.buf.push_str(token);
        loop {
            if self.buf.is_empty() {
                break;
            }
            if self.in_think {
                if let Some(index) = self.buf.find(THINK_CLOSE) {
                    if index > 0 {
                        out.push((ThinkKind::Think, self.buf[..index].to_string()));
                    }
                    self.buf = self.buf[index + THINK_CLOSE.len()..].to_string();
                    self.in_think = false;
                    continue;
                }
                let hold = partial_tag_tail(&self.buf, THINK_CLOSE);
                let split = self.buf.len() - hold;
                let safe = self.buf[..split].to_string();
                if !safe.is_empty() {
                    out.push((ThinkKind::Think, safe));
                }
                self.buf = self.buf[split..].to_string();
                break;
            }
            let open_index = self.buf.find(THINK_OPEN);
            let close_index = self.buf.find(THINK_CLOSE);
            if let Some(close) = close_index {
                let precedes = match open_index {
                    Some(open) => close < open,
                    None => true,
                };
                if precedes {
                    if close > 0 {
                        out.push((ThinkKind::Think, self.buf[..close].to_string()));
                    }
                    self.buf = self.buf[close + THINK_CLOSE.len()..].to_string();
                    continue;
                }
            }
            if let Some(open) = open_index {
                if open > 0 {
                    out.push((ThinkKind::Answer, self.buf[..open].to_string()));
                }
                self.buf = self.buf[open + THINK_OPEN.len()..].to_string();
                self.in_think = true;
                continue;
            }
            let hold = partial_tag_tail(&self.buf, THINK_OPEN)
                .max(partial_tag_tail(&self.buf, THINK_CLOSE));
            let split = self.buf.len() - hold;
            let safe = self.buf[..split].to_string();
            if !safe.is_empty() {
                out.push((ThinkKind::Answer, safe));
            }
            self.buf = self.buf[split..].to_string();
            break;
        }
        out
    }

    /// Flush the tail: any remaining text is delivered with residual think tags
    /// stripped.
    pub fn finish(&mut self) -> Vec<(ThinkKind, String)> {
        if self.buf.is_empty() {
            return Vec::new();
        }
        let kind = if self.in_think {
            ThinkKind::Think
        } else {
            ThinkKind::Answer
        };
        let re = Regex::new(r"</?think>").expect("think tag regex");
        let text = re.replace_all(&self.buf, "").to_string();
        self.buf.clear();
        vec![(kind, text)]
    }
}

/// Reducer state of the agentic graph (LangGraph `AgenticState`).
pub struct AgenticState {
    // Conversation input.
    pub messages: Vec<Value>,
    pub question: String,
    pub keywords: String,
    // Evolving research state.
    pub plan: Value,
    pub current_queries: Vec<Value>,
    pub slot_table: Option<SlotState>,
    pub slot_draft: String,
    pub collected_answer: String,
    pub unresolved_slots: Vec<Value>,
    pub research_feedback: Vec<Value>,
    pub kbinfos: Kbinfos,
    pub draft: String,
    pub rag_answer: String,
    pub partial_answer: bool,
    pub abstain: bool,
    pub empty_result: bool,
    pub verdict: Value,
    pub sca: Value,
    // Budgets & counters.
    pub max_loops: usize,
    pub deadline: Option<Instant>,
    pub search_rounds: usize,
    pub sca_view_id: String,
    pub attempted: Vec<Value>,
    pub fills_found: bool,
    pub no_progress: bool,
}

impl Default for AgenticState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            question: String::new(),
            keywords: String::new(),
            plan: serde_json::json!({}),
            current_queries: Vec::new(),
            slot_table: None,
            slot_draft: String::new(),
            collected_answer: String::new(),
            unresolved_slots: Vec::new(),
            research_feedback: Vec::new(),
            kbinfos: Kbinfos::default(),
            draft: String::new(),
            rag_answer: String::new(),
            partial_answer: false,
            abstain: false,
            empty_result: false,
            verdict: serde_json::json!({}),
            sca: serde_json::json!({}),
            max_loops: 3,
            deadline: None,
            search_rounds: 0,
            sca_view_id: String::new(),
            attempted: Vec::new(),
            fills_found: false,
            no_progress: false,
        }
    }
}
#[cfg(test)]
mod graph_core_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snip_collapses_and_truncates() {
        assert_eq!(snip(&json!("a   b\nc"), 240), "a b c");
        let long = "x".repeat(300);
        let out = snip(&json!(long), 240);
        assert!(out.ends_with("...(+60 chars)"));
        assert_eq!(snip(&json!({"k": 1}), 240), "{\"k\":1}");
        assert_eq!(snip(&Value::Null, 240), "");
    }

    #[test]
    fn safe_list_matrix() {
        assert!(safe_list(&Value::Null).is_empty());
        assert_eq!(safe_list(&json!([1, 2])).len(), 2);
        assert!(safe_list(&json!(5)).is_empty(), "scalars degrade to empty");
        assert!(!is_poisoned(&json!("x")));
    }

    #[test]
    fn sca_view_ranks_relevance_coverage_and_freshness() {
        let chunks = vec![
            json!({"chunk_id": "a", "content": "unrelated", "similarity": 0.1}),
            json!({"chunk_id": "b", "content": "paris population", "similarity": 0.2}),
            json!({"chunk_id": "c", "content": "paris", "similarity": 0.9}),
        ];
        let (view, _identity) = select_sca_view(&chunks, &["paris".to_string()], None);
        assert_eq!(view[0]["chunk_id"], "c", "relevance dominates");
        assert_eq!(view[1]["chunk_id"], "b", "coverage beats none");
        assert_eq!(view[2]["chunk_id"], "a");
        let (capped, _) = select_sca_view(&chunks, &[], Some(1));
        assert_eq!(capped.len(), 1);
    }

    #[test]
    fn sca_view_identity_changes_with_membership() {
        let first = vec![json!({"chunk_id": "a"}), json!({"chunk_id": "b"})];
        let second = vec![json!({"chunk_id": "a"}), json!({"chunk_id": "c"})];
        let (_, identity_a) = select_sca_view(&first, &[], None);
        let (_, identity_b) = select_sca_view(&second, &[], None);
        let (_, identity_a2) = select_sca_view(&first, &[], None);
        assert_ne!(identity_a, identity_b);
        assert_eq!(identity_a, identity_a2, "stable within a process");
    }

    #[test]
    fn view_terms_dedupes_question_and_queries() {
        let mut state = AgenticState::default();
        state.question = "paris population".to_string();
        state.current_queries = vec![json!("paris growth"), json!(7)];
        let terms = view_terms(&state);
        assert!(terms.contains(&"paris".to_string()));
        assert!(terms.contains(&"population".to_string()));
        assert!(terms.contains(&"growth".to_string()));
        assert_eq!(
            terms.iter().filter(|term| *term == "paris").count(),
            1,
            "deduped"
        );
    }

    #[tokio::test]
    async fn bounded_returns_none_on_timeout() {
        let slow = async {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            7
        };
        assert_eq!(bounded(slow, 0.05).await, None);
        let fast = async { 7 };
        assert_eq!(bounded(fast, 1.0).await, Some(7));
    }

    #[test]
    fn partial_tag_tail_matches_suffixes() {
        assert_eq!(partial_tag_tail("abc<th", "<think>"), 3);
        assert_eq!(partial_tag_tail("abc", "<think>"), 0);
        assert_eq!(partial_tag_tail("</thi", "</think>"), 5);
        assert_eq!(
            partial_tag_tail("<think>", "<think>"),
            0,
            "a full tag is not a tail of itself (the leading char drops)"
        );
    }

    #[test]
    fn think_splitter_handles_plain_blocks() {
        let mut splitter = ThinkSplitter::default();
        let mut pieces = splitter.push("hello <think>reason</think> world");
        pieces.extend(splitter.finish());
        let kinds: Vec<ThinkKind> = pieces.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(
            kinds,
            vec![ThinkKind::Answer, ThinkKind::Think, ThinkKind::Answer]
        );
        assert_eq!(pieces[0].1, "hello ");
        assert_eq!(pieces[1].1, "reason");
        assert_eq!(pieces[2].1, " world");
    }

    #[test]
    fn think_splitter_handles_unmatched_close_and_holds() {
        let mut splitter = ThinkSplitter::default();
        let mut pieces = splitter.push("oops</think>tail");
        assert_eq!(pieces[0], (ThinkKind::Think, "oops".to_string()));
        assert_eq!(pieces[1], (ThinkKind::Answer, "tail".to_string()));

        let mut holding = ThinkSplitter::default();
        assert!(holding.push("<thi").is_empty(), "partial tag held back");
        let pieces = holding.push("nk>x</think>");
        assert_eq!(pieces[0], (ThinkKind::Think, "x".to_string()));

        let mut answer_hold = ThinkSplitter::default();
        let first = answer_hold.push("ab<thi");
        assert_eq!(first[0], (ThinkKind::Answer, "ab".to_string()));
        let pieces = answer_hold.push("nk>y</think>");
        assert_eq!(pieces[0], (ThinkKind::Think, "y".to_string()));
    }

    #[test]
    fn think_splitter_finish_strips_residual_tags() {
        let mut splitter = ThinkSplitter::default();
        let first = splitter.push("tail<thi");
        assert_eq!(first[0], (ThinkKind::Answer, "tail".to_string()));
        let flushed = splitter.finish();
        assert_eq!(flushed[0].0, ThinkKind::Answer);
        assert_eq!(flushed[0].1, "<thi");
        assert!(splitter.finish().is_empty());
    }
}
