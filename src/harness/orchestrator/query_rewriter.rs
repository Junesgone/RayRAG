//! Query Rewriter — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/orchestrator/query_rewriter.py`.
//!
//! Phase 4 role of the orchestrator: turn a Sufficient Context Agent forward
//! gap (what is missing + a search hint) into a concrete, retrievable query.
//! Unlike reusing the gap text verbatim, this names the missing entity +
//! relation (and anchors on already-resolved bridge values), so the next
//! search hits the gap instead of re-searching the same angle.

use serde_json::{Value, json};

use crate::harness::HarnessChat;
use crate::harness::stats::StatsHandle;

/// `load_prompt("sca_query_rewrite")` — the upstream template verbatim.
pub const REWRITE_PROMPT: &str = r#"You are a Query Rewriter for a multi-hop RAG system. Your job is to turn the Sufficient Context Agent's missing-pieces feedback into targeted, retrievable search queries.

Original user question:
{{ question }}

Already-resolved bridge values (facts confirmed from earlier searches — use these to anchor the new query instead of re-deriving them):
{{ bridge_values }}

Research history and evidence at hand (from previous retrieval rounds):
{{ research_context }}

Missing pieces identified by the Sufficient Context Agent (each has "what" = what the answer still needs, and "hint" = a suggested search hint):
{{ gaps }}

Rewrite each missing piece into a concrete search query that the retriever can hit directly. Rules:
1. The query must name the specific missing entity + the relation/property needed (e.g. instead of "the patient's allergies", write "allergic reactions adverse events discharge John Doe").
2. MULTI-HOP: when the question is multi-hop and the missing piece is the NEXT hop's value, ANCHOR the query to the already-resolved bridge value. E.g. if the bridge value is "M*A*S*H and Cheers are the two most-watched finales" and the gap is "their runtimes", the query must be "MASH finale run time minutes" / "Cheers finale run time minutes" — not "finale run times" alone, which would lose the resolved anchor.
3. If the "what" names a specific entity (person / work / place / year) whose property is missing, anchor the query to that entity + the missing property (e.g. "MASH finale run time minutes", "Brian Bergstein employer company").
4. If a disambiguation is needed, add the distinguishing qualifier ONLY when the evidence actually suggests it (e.g. a disambiguation "the heritage 341 London Broncos player" when the claim is about that specific player). CRITICAL: NEVER introduce an entity/relation/attribute that the evidence and the missing piece do NOT suggest, and NEVER re-infer a category from the question alone (e.g. do NOT write "Ron Hutchinson football player" unless the evidence actually identifies him as a footballer — he may be a hockey player; the query must stay faithful to what the missing piece says).
5. Keep each query standalone and searchable — do NOT use pronouns ("he", "it", "this") — repeat the key entity explicitly.
6. ONE QUERY PER MISSING PIECE. Do not emit multiple near-duplicate queries for the same gap ("What teams"/"For each team"/"Complete enumeration" of the same thing are duplicates — keep exactly one, the most concretely anchored). The number of output queries should equal the number of distinct, genuinely-different missing pieces.
7. Drop any missing piece that cannot be turned into a searchable query.
8. DIVERSITY: consult the "Research history and evidence at hand" section. Do NOT output any query that paraphrases an already-tried one (listed there WITH its outcome — a previous search that yielded nothing new means that angle is dead). Aim each new query at aspects the current evidence does NOT yet cover, possibly combining the bridge values with different entity/relation combinations.

Output format (JSON):
```json
{
  "queries": [
    {"query": "concrete search query 1"},
    {"query": "concrete search query 2"}
  ]
}
```

Return a strict JSON object with no commentary before or after.
"#;

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Render `REWRITE_PROMPT` with the call's question, gaps, bridge values and
/// research context.
pub fn render_rewrite_prompt(
    question: &str,
    gaps: &[(String, String)],
    bridge_values: &[Value],
    research_context: &str,
) -> String {
    let gaps_text = gaps
        .iter()
        .map(|(what, hint)| format!("- what: {what}; hint: {hint}"))
        .collect::<Vec<_>>()
        .join("\n");
    let bridge_text = bridge_values
        .iter()
        .filter(|value| !value_text(value).trim().is_empty())
        .map(|value| format!("- {}", value_text(value)))
        .collect::<Vec<_>>()
        .join("\n");
    REWRITE_PROMPT
        .replace("{{ question }}", question)
        .replace("{{ bridge_values }}", &bridge_text)
        .replace("{{ research_context }}", research_context)
        .replace("{{ gaps }}", &gaps_text)
}

/// Parse the rewriter reply into `[{"query": …}]` (deduped, order-preserving).
pub fn parse_rewrite_queries(raw: &str) -> Vec<Value> {
    let think = regex::Regex::new("(?s)^.*</think>").unwrap();
    let fences = regex::Regex::new(r"```(?:json)?\s*|\s*```").unwrap();
    let cleaned = fences
        .replace_all(&think.replace(raw, ""), "")
        .trim()
        .to_string();
    let result: Value = serde_json::from_str(&cleaned).unwrap_or_else(|_| json!({}));
    let mut out: Vec<Value> = Vec::new();
    if let Some(queries) = result.get("queries").and_then(Value::as_array) {
        for query in queries {
            let text = if query.is_object() {
                ["query", "question"]
                    .iter()
                    .find_map(|key| query.get(*key).and_then(Value::as_str))
                    .unwrap_or("")
                    .trim()
                    .to_string()
            } else {
                value_text(query).trim().to_string()
            };
            if !text.is_empty()
                && !out.iter().any(|existing| {
                    existing.get("query").and_then(Value::as_str) == Some(text.as_str())
                })
            {
                out.push(json!({"query": text}));
            }
        }
    }
    out
}

/// `rewrite_gap_to_query`: rewrite forward gaps into targeted search queries.
/// Returns empty when the rewrite is unavailable / fails (the caller falls
/// back to the original gap text).
pub async fn rewrite_gap_to_query(
    chat: Option<&dyn HarnessChat>,
    question: &str,
    gaps: &[(String, String)],
    bridge_values: &[Value],
    research_context: &str,
    stats: &StatsHandle,
) -> Vec<Value> {
    let _phase = stats.enter_phase("rewrite");
    let Some(chat) = chat else {
        return Vec::new();
    };
    if gaps.is_empty() {
        return Vec::new();
    }
    let rendered = render_rewrite_prompt(question, gaps, bridge_values, research_context);
    // `gen_json(rendered, "Output:\n", chat_mdl)`: the rendered prompt is the
    // system message and the tail is the user message.
    let history = vec![json!({"role": "user", "content": "Output:\n"})];
    let raw = match chat.chat(&rendered, &history, &json!({})).await {
        Ok(raw) => raw,
        Err(_) => return Vec::new(),
    };
    parse_rewrite_queries(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct MockChat {
        reply: Result<String, String>,
    }

    #[async_trait]
    impl HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            self.reply.clone()
        }

        fn max_length(&self) -> usize {
            4096
        }
    }

    #[test]
    fn rendering_mirrors_upstream_layout() {
        let rendered = render_rewrite_prompt(
            "Who ran the longest?",
            &[
                ("finale runtimes".to_string(), "MASH Cheers".to_string()),
                ("".to_string(), "".to_string()),
            ],
            &[json!("M*A*S*H"), json!(""), json!(42)],
            "round 1: tried X",
        );
        assert!(rendered.contains("Original user question:\nWho ran the longest?"));
        assert!(rendered.contains("- M*A*S*H"));
        assert!(rendered.contains("- 42"));
        assert!(!rendered.contains("}} after"));
        assert!(rendered.contains("- what: finale runtimes; hint: MASH Cheers\n- what: ; hint: "));
        assert!(rendered.contains("round 1: tried X"));
        assert!(!rendered.contains("{{ question }}"));
    }

    #[test]
    fn parsing_handles_objects_strings_and_dedup() {
        let raw = "<think>x</think>```json\n{\"queries\": [{\"query\": \"alpha final\"}, {\"question\": \"beta run\"}, \"alpha final\", {\"query\": \"  \"}]}\n```";
        let queries = parse_rewrite_queries(raw);
        assert_eq!(queries.len(), 2);
        assert_eq!(queries[0]["query"], json!("alpha final"));
        assert_eq!(queries[1]["query"], json!("beta run"));
        assert!(parse_rewrite_queries("not json").is_empty());
    }

    #[tokio::test]
    async fn rewrite_flow_paths() {
        let stats = StatsHandle::new();
        let gaps = vec![("what".to_string(), "hint".to_string())];

        // No chat model → empty.
        let out = rewrite_gap_to_query(None, "q", &gaps, &[], "", &stats).await;
        assert!(out.is_empty());
        // No gaps → empty.
        let chat = MockChat {
            reply: Ok("{\"queries\": [{\"query\": \"x\"}]}".to_string()),
        };
        let out = rewrite_gap_to_query(Some(&chat), "q", &[], &[], "", &stats).await;
        assert!(out.is_empty());
        // Success path.
        let out = rewrite_gap_to_query(Some(&chat), "q", &gaps, &[], "", &stats).await;
        assert_eq!(out, vec![json!({"query": "x"})]);
        // Chat failure → empty.
        let failing = MockChat {
            reply: Err("boom".to_string()),
        };
        let out = rewrite_gap_to_query(Some(&failing), "q", &gaps, &[], "", &stats).await;
        assert!(out.is_empty());
    }
}
