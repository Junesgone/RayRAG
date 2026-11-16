//! LLM-call instrumentation for the agentic RAG harness — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/stats.py`.
//!
//! Every phase of the agentic pipeline (route / planner / orchestrator /
//! agent / sufficiency / grounded / finalize, …) drives the LLM through the
//! chat bundle. This module records, per phase, the number of LLM calls, the
//! token usage reported by the provider, and the phase wall-clock time —
//! unlike summed LLM latency, which is meaningless when calls run in parallel.
//! Upstream keeps the active stats in a `ContextVar`; RayRAG hands the shared
//! [`StatsHandle`] to the tasks that make up one `rag` call.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde_json::Value;

/// Canonical pipeline order for the per-phase usage table.
pub const PHASE_ORDER: [&str; 10] = [
    "formalize",
    "route",
    "planner",
    "decompose",
    "direct",
    "orchestrator",
    "claim_research",
    "sufficiency",
    "grounded",
    "finalize",
];

/// Per-phase LLM call, wall-clock & token counters for one agentic `rag` run.
#[derive(Debug, Default)]
pub struct LLMUsageStats {
    calls: HashMap<String, usize>,
    failed: HashMap<String, usize>,
    phase_time_ms: HashMap<String, f64>,
    prompt_tokens: HashMap<String, i64>,
    completion_tokens: HashMap<String, i64>,
    total_tokens: HashMap<String, i64>,
    rounds: HashMap<String, usize>,
    round_times: HashMap<String, Vec<f64>>,
    round_phase_times_ms: HashMap<String, Vec<f64>>,
    round_claim_counts: HashMap<String, Vec<i64>>,
    current_round: usize,
    phase_active_counts: HashMap<String, usize>,
    phase_starts: HashMap<String, Instant>,
    round_phase_active_counts: HashMap<(String, usize), usize>,
    round_phase_starts: HashMap<(String, usize), Instant>,
    round_starts: HashMap<String, Instant>,
}

impl LLMUsageStats {
    /// Index (1-based) of the orchestrator round currently executing, 0 outside.
    pub fn current_round(&self) -> usize {
        self.current_round
    }

    /// `note_start` (the log row order is fixed by [`PHASE_ORDER`]).
    pub fn note_start(&self, _phase_name: &str) {}

    /// `record_call`.
    pub fn record_call(&mut self, phase_name: &str) {
        *self.calls.entry(phase_name.to_string()).or_default() += 1;
    }

    /// `record_failed`.
    pub fn record_failed(&mut self, phase_name: &str) {
        *self.failed.entry(phase_name.to_string()).or_default() += 1;
    }

    fn accumulate_phase_time(&mut self, phase_name: &str, elapsed_ms: f64, entry_round: usize) {
        *self
            .phase_time_ms
            .entry(phase_name.to_string())
            .or_default() += elapsed_ms;
        if entry_round > 0 {
            let times = self
                .round_phase_times_ms
                .entry(phase_name.to_string())
                .or_default();
            while times.len() < entry_round {
                times.push(0.0);
            }
            times[entry_round - 1] += elapsed_ms;
        }
        let rounds = *self.rounds.get(phase_name).unwrap_or(&0);
        let round_times = self.round_times.entry(phase_name.to_string()).or_default();
        if rounds > round_times.len() {
            let settled: f64 = round_times.iter().sum();
            let total = *self.phase_time_ms.get(phase_name).unwrap_or(&0.0);
            round_times.push((total - settled).max(0.0));
            self.round_starts.remove(phase_name);
        }
        if phase_name == "orchestrator" {
            self.current_round = 0;
        }
    }

    /// `note_phase_enter` (shadowing: nested re-entries of the same name do not
    /// restart the clock).
    pub fn note_phase_enter(&mut self, phase_name: &str, entry_round: usize) {
        let now = Instant::now();
        let count = self
            .phase_active_counts
            .entry(phase_name.to_string())
            .or_default();
        *count += 1;
        if *count == 1 {
            self.phase_starts.insert(phase_name.to_string(), now);
        }
        if entry_round > 0 {
            let key = (phase_name.to_string(), entry_round);
            let round_count = self
                .round_phase_active_counts
                .entry(key.clone())
                .or_default();
            *round_count += 1;
            if *round_count == 1 {
                self.round_phase_starts.insert(key, now);
            }
        }
    }

    /// `note_phase_exit`.
    pub fn note_phase_exit(&mut self, phase_name: &str, entry_round: usize) {
        let now = Instant::now();
        if let Some(count) = self.phase_active_counts.get_mut(phase_name)
            && *count > 0
        {
            *count -= 1;
            if *count == 0 {
                let start = self.phase_starts.remove(phase_name).unwrap_or(now);
                self.phase_active_counts.remove(phase_name);
                let elapsed = (now - start).as_secs_f64() * 1000.0;
                self.accumulate_phase_time(phase_name, elapsed, 0);
            }
        }
        if entry_round > 0 {
            let key = (phase_name.to_string(), entry_round);
            if let Some(count) = self.round_phase_active_counts.get_mut(&key)
                && *count > 0
            {
                *count -= 1;
                if *count == 0 {
                    let start = self.round_phase_starts.remove(&key).unwrap_or(now);
                    self.round_phase_active_counts.remove(&key);
                    let times = self
                        .round_phase_times_ms
                        .entry(phase_name.to_string())
                        .or_default();
                    while times.len() < entry_round {
                        times.push(0.0);
                    }
                    times[entry_round - 1] += (now - start).as_secs_f64() * 1000.0;
                }
            }
        }
    }

    /// `record_usage`.
    pub fn record_usage(&mut self, phase_name: &str, usage: Option<&Value>) {
        let Some(usage) = usage else {
            return;
        };
        let get = |key: &str| -> i64 {
            usage
                .get(key)
                .and_then(|value| value.as_i64().or_else(|| value.as_f64().map(|f| f as i64)))
                .unwrap_or(0)
        };
        *self
            .prompt_tokens
            .entry(phase_name.to_string())
            .or_default() += get("prompt_tokens");
        *self
            .completion_tokens
            .entry(phase_name.to_string())
            .or_default() += get("completion_tokens");
        *self.total_tokens.entry(phase_name.to_string()).or_default() += get("total_tokens");
    }

    /// `record_round`: count one iteration of a looping phase.
    pub fn record_round(&mut self, phase_name: &str) {
        let rounds = self.rounds.entry(phase_name.to_string()).or_default();
        *rounds += 1;
        self.current_round = *rounds;
        let now = Instant::now();
        if let Some(previous) = self.round_starts.remove(phase_name) {
            let elapsed = (now - previous).as_secs_f64() * 1000.0;
            self.round_times
                .entry(phase_name.to_string())
                .or_default()
                .push(elapsed);
        }
        self.round_starts.insert(phase_name.to_string(), now);
    }

    /// `record_round_claims`.
    pub fn record_round_claims(&mut self, phase_name: &str, count: i64) {
        if self.current_round == 0 {
            return;
        }
        let counts = self
            .round_claim_counts
            .entry(phase_name.to_string())
            .or_default();
        while counts.len() < self.current_round {
            counts.push(0);
        }
        counts[self.current_round - 1] += count;
    }

    /// `snapshot`: rows in canonical pipeline order, unknown phases appended
    /// alphabetically.
    pub fn snapshot(&self) -> Vec<(String, Value)> {
        let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
        known.extend(self.calls.keys().cloned());
        known.extend(self.failed.keys().cloned());
        known.extend(self.total_tokens.keys().cloned());
        known.extend(self.phase_time_ms.keys().cloned());
        known.extend(self.rounds.keys().cloned());

        let mut phases: Vec<String> = PHASE_ORDER
            .iter()
            .filter(|phase| known.contains(**phase))
            .map(|phase| (*phase).to_string())
            .collect();
        let mut rest: Vec<String> = known
            .iter()
            .filter(|phase| !phases.contains(phase))
            .cloned()
            .collect();
        rest.sort();
        phases.extend(rest);

        phases
            .into_iter()
            .map(|phase| {
                let per_round = self.round_phase_times_ms.get(&phase).cloned().unwrap_or_default();
                let (rounds, round_times) = if !per_round.is_empty() {
                    (per_round.len(), per_round.clone())
                } else {
                    (
                        *self.rounds.get(&phase).unwrap_or(&0),
                        self.round_times.get(&phase).cloned().unwrap_or_default(),
                    )
                };
                let row = serde_json::json!({
                    "calls": self.calls.get(&phase).copied().unwrap_or(0),
                    "failed": self.failed.get(&phase).copied().unwrap_or(0),
                    "phase_time_ms": self.phase_time_ms.get(&phase).copied().unwrap_or(0.0),
                    "prompt_tokens": self.prompt_tokens.get(&phase).copied().unwrap_or(0),
                    "completion_tokens": self.completion_tokens.get(&phase).copied().unwrap_or(0),
                    "total_tokens": self.total_tokens.get(&phase).copied().unwrap_or(0),
                    "rounds": rounds,
                    "round_times": round_times,
                    "round_claim_counts": self.round_claim_counts.get(&phase).cloned().unwrap_or_default(),
                });
                (phase, row)
            })
            .collect()
    }

    /// `log`: render the usage table (header + canonical rows; orchestrator
    /// rounds are appended as `phase#N` rows).
    pub fn log_lines(&self) -> Vec<String> {
        let rows = self.snapshot();
        if rows.is_empty() {
            return vec!["[Agentic RAG] LLM usage by phase: (cached / no LLM calls)".to_string()];
        }
        let mut lines = vec![
            "[Agentic RAG] LLM usage by phase:".to_string(),
            format!(
                "  {:<16} {:>7} {:>10} {:>12} {:>10} {:>10}",
                "phase", "llm_calls", "prompt_tok", "output_tok", "total_tok", "time(s)"
            ),
        ];
        for (phase, row) in rows {
            let rounds = row.get("rounds").and_then(Value::as_u64).unwrap_or(0);
            let suffix = if rounds > 0 {
                format!("#{rounds}")
            } else {
                String::new()
            };
            lines.push(format!(
                "  {:<16} {:>7} {:>10} {:>12} {:>10} {:>10.2}",
                format!("{phase}{suffix}"),
                row.get("calls").and_then(Value::as_u64).unwrap_or(0),
                row.get("prompt_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                row.get("completion_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                row.get("total_tokens").and_then(Value::as_i64).unwrap_or(0),
                row.get("phase_time_ms")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0)
                    / 1000.0,
            ));
        }
        lines
    }
}

/// Shared handle for one `rag` call's stats.
#[derive(Clone, Default)]
pub struct StatsHandle(pub Arc<Mutex<LLMUsageStats>>);

impl StatsHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter a phase; the returned scope exits the phase on drop.
    pub fn enter_phase(&self, name: &str) -> PhaseScope {
        let mut stats = self.0.lock().unwrap();
        let entry_round = stats.current_round();
        stats.note_phase_enter(name, entry_round);
        drop(stats);
        PhaseScope {
            handle: self.clone(),
            name: name.to_string(),
            entry_round,
        }
    }

    pub fn record_call(&self, phase: &str) {
        self.0.lock().unwrap().record_call(phase);
    }

    pub fn record_failed(&self, phase: &str) {
        self.0.lock().unwrap().record_failed(phase);
    }

    pub fn record_usage(&self, phase: &str, usage: Option<&Value>) {
        self.0.lock().unwrap().record_usage(phase, usage);
    }

    pub fn record_round(&self, phase: &str) {
        self.0.lock().unwrap().record_round(phase);
    }

    pub fn record_round_claims(&self, phase: &str, count: i64) {
        self.0.lock().unwrap().record_round_claims(phase, count);
    }

    pub fn snapshot(&self) -> Vec<(String, Value)> {
        self.0.lock().unwrap().snapshot()
    }

    pub fn log_lines(&self) -> Vec<String> {
        self.0.lock().unwrap().log_lines()
    }
}

/// RAII phase scope (`with phase(name):` / `@in_phase(name)`).
pub struct PhaseScope {
    handle: StatsHandle,
    name: String,
    entry_round: usize,
}

impl Drop for PhaseScope {
    fn drop(&mut self) {
        let mut stats = self.handle.0.lock().unwrap();
        stats.note_phase_exit(&self.name, self.entry_round);
    }
}

/// `phase(name)` helper for a plain closure.
pub fn with_phase<T>(handle: &StatsHandle, name: &str, f: impl FnOnce() -> T) -> T {
    let _scope = handle.enter_phase(name);
    f()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counters_and_tokens_accumulate() {
        let mut stats = LLMUsageStats::default();
        stats.record_call("route");
        stats.record_call("route");
        stats.record_failed("route");
        stats.record_usage(
            "route",
            Some(&json!({"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14})),
        );
        stats.record_usage("route", None);
        let rows = stats.snapshot();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "route");
        assert_eq!(rows[0].1["calls"], json!(2));
        assert_eq!(rows[0].1["failed"], json!(1));
        assert_eq!(rows[0].1["total_tokens"], json!(14));
    }

    #[test]
    fn snapshot_uses_canonical_order_then_sorted_fallback() {
        let mut stats = LLMUsageStats::default();
        stats.record_call("finalize");
        stats.record_call("custom_b");
        stats.record_call("route");
        stats.record_call("custom_a");
        let phases: Vec<String> = stats
            .snapshot()
            .into_iter()
            .map(|(phase, _)| phase)
            .collect();
        assert_eq!(phases, vec!["route", "finalize", "custom_a", "custom_b"]);
        assert_eq!(stats.log_lines()[0], "[Agentic RAG] LLM usage by phase:");
    }

    #[test]
    fn round_bookkeeping_and_phase_scopes() {
        let handle = StatsHandle::new();
        {
            let _outer = handle.enter_phase("orchestrator");
            {
                let _inner = handle.enter_phase("orchestrator");
            }
        }
        handle.record_round("orchestrator");
        assert_eq!(handle.0.lock().unwrap().current_round(), 1);
        handle.record_round_claims("orchestrator", 3);
        handle.record_round("orchestrator");
        assert_eq!(handle.0.lock().unwrap().current_round(), 2);
        handle.record_round_claims("orchestrator", 2);
        let rows = handle.snapshot();
        let row = rows
            .iter()
            .find(|(phase, _)| phase == "orchestrator")
            .unwrap();
        assert_eq!(row.1["rounds"], json!(2));
        assert_eq!(row.1["round_claim_counts"], json!([3, 2]));
        assert!(row.1["phase_time_ms"].as_f64().unwrap() >= 0.0);
        // with_phase returns the closure value.
        let value = with_phase(&handle, "route", || 7);
        assert_eq!(value, 7);
    }
}
