//! Unified Sufficient Context Agent — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/orchestrator/sufficient_context.py`.
//!
//! ONE review pass that simultaneously examines (1) each claim's intermediate
//! draft, (2) a bounded evidence anchor for the snippets that claim cited, and
//! (3) what is still missing (forward gaps + structured sub-query coverage).
//! Returns a unified verdict; adapter helpers [`to_boost`] / [`to_grounded`]
//! feed the existing decision ladder without changing its contracts.

use serde_json::{Value, json};

use crate::harness::HarnessChat;
use crate::harness::orchestrator::direct::Kbinfos;
use crate::harness::stats::StatsHandle;

/// Total cap for the rendered claims context (drafts + evidence anchors).
pub const SCA_CLAIMS_CONTEXT_MAX: usize = 48000;
/// Max chars of each cited snippet's evidence anchor (tables keep full text).
pub const SCA_EVIDENCE_ANCHOR_CHARS: usize = 300;
/// How many unresolved facts the boost feedback names.
pub const FEEDBACK_MAX: usize = 4;

/// `SCA_REVIEW` (`load_prompt("sca_select")`) — the upstream template verbatim.
pub const SCA_SELECT: &str = r##"You are Google-style "Sufficient Context Agent" — a quality-control inspector for a multi-hop RAG retrieval loop. In ONE pass you perform a unified three-part review of (1) the retrieved snippets, (2) each claim's intermediate draft, and (3) what is still missing — then you decide whether the context is sufficient to answer the original question.

## Input

Question:
{{ question }}

OVERALL INTERMEDIATE DRAFT — a problem-level candidate answer assembled from the claims' reports (each claim already answered its sub-question as concretely as the evidence allows). Review THIS as the thing that would become the final answer:
{{ overall_draft }}

Per-claim reports (the intermediate drafts that produced the overall draft above):
{{ claims_context }}

## Part A — Sufficient Context (global)

Determine whether the retrieved content is sufficient to answer the user's question, using the Sufficient Context paper's autorater criterion:

Sufficient Context = 1 IF the CONTEXT is sufficient to INFER the answer to the question, and 0 IF the CONTEXT cannot be used to INFER the answer to the question. "A diligent reader could craft a definitive answer using only the supplied text." The answer does NOT need to be proven correct or match a ground truth — only that a reasonable answer can be constructed from the context. Multi-hop reasoning is allowed, but leaps of faith are not; ambiguities must be resolved inside the snippet bundle.

The CONTEXT here is the OVERALL INTERMEDIATE DRAFT (the problem-level candidate answer assembled from the claims) plus the per-claim reports. Judge whether THIS draft lets a diligent reader construct the answer to the ORIGINAL question end-to-end — including any cross-claim synthesis (e.g. two claims that must be COMBINED into one derived answer, or an enumeration the claims only partially cover). If the draft answers the question (even partially), mark sufficient; if the question cannot be answered from the draft at all, mark insufficient and say exactly what is missing (Part C).

MANDATORY three-stage reasoning (this is what makes the judgement reliable — follow it strictly):
1. FIRST, write down the STEP-BY-STEP SUB-QUESTIONS you would need to answer in order to arrive at the label. Make sure to include questions about any ASSUMPTIONS implicit in the QUESTION, and include questions about any MATHEMATICAL CALCULATIONS or ARITHMETIC that would be required.
2. THEN, answer each of those sub-questions step by step, working through any required mathematical calculations or arithmetic explicitly.
3. FINALLY, use these answers to evaluate the criterion and decide `is_sufficient`. If the sub-question answers let you construct a definitive answer from the context, it is sufficient — even if the answer is not perfectly complete. Only when the sub-questions cannot be answered from the context (so NO plausible answer can be inferred) is it insufficient, and then you must fill `missing_information` with exactly what is missing (Part C).

Computed answers: when a sub-question requires arithmetic (e.g. difference, count, "how many years"), WORK THE CALCULATION yourself from the values in the context and only mark sufficient if it resolves to a definite result with the RIGHT operands. If it cannot be completed, mark insufficient and list what is missing.

The STEP-BY-STEP SUB-QUESTIONS from stage 1 MUST ALSO be emitted in a structured `sub_queries` array (one entry per sub-question), because they are the precise "what is missing" signal the next search round consumes. Decomposition rules (mutually-exclusive + complete coverage — every sub-question is an atomic fact needed to answer the ORIGINAL question, sub-questions do not overlap, and together they fully cover the question):
- MULTI-STEP / multi-hop: emit ONE sub-query per logical hop, and the LAST hop is the entity/role the question actually asks for. E.g. Q="Who is the president of the team whose name was inspired by the Boston Braves?" -> [sub-query "Which team's name was inspired by the Boston Braves?", sub-query "Who is that team's president?"].
- MULTI-ASPECT / enumeration+aggregate: emit ONE sub-query for the enumeration (values of EVERY member) and ONE for the aggregate (the combination over those members). E.g. Q="average left-field distance of every stadium" -> [sub-query "left-field distance of every stadium", sub-query "average of those left-field distances"].
- ARITHMETIC / temporal: the arithmetic itself is a sub-query, and every OPERAND must have a value. If an operand is absent, that sub-query is `satisfied: false` and `missing_fact` names the absent operand specifically. E.g. Q="how many days between the two deaths?" -> [sub-query "McLean death date" (satisfied), sub-query "Meyer death date" (satisfied:false, missing_fact:"Eugene Meyer's exact death date", search_hint:"Eugene Meyer Washington Post died date"), sub-query "days between them"].

Each entry in `sub_queries` is `{"sub_query": "...", "satisfied": true/false}`; when `satisfied` is false it MUST additionally carry `missing_fact` (the concrete absent fact) and `search_hint` (a searchable query anchoring entity+attribute+time). `satisfied: false` marks exactly WHAT is missing and WHERE to search next.

## Part B — Claim-draft groundedness (per claim)

For each claim's report (the intermediate draft), determine whether it is grounded and self-consistent. Each report was generated from that claim's cited evidence by the search agent, so the main risk is NOT absent-evidence hallucination but (a) prior-knowledge padding sneaking past the "saw it in evidence" rule, (b) internal contradictions between a claim's own numbers/entities, and (c) over-claiming a derived value.

A report is GROUNDED only if it reads as a faithful, internally consistent, evidence-backed finding. Flag as UNGROUNDED anything that:
- asserts a specific number/entity/relation but contradicts another part of the same report (internal conflict);
- over-claims a computed/derived result beyond what the underlying values support;
- reads like prior-knowledge padding rather than an evidence-backed finding.

For each claim, classify the report as GROUNDED or UNGROUNDED. List each ungrounded assertion with a one-line reason.

Derived/computed assertions — APPLY ONLY IF the question asks for a DERIVED result AND the report states that computed result. Then VERIFY THE CALCULATION YOURSELF from the values in the evidence: every operand must have an explicit value in the evidence AND be the RIGHT entity; recompute and check the report's stated result matches. If any operand's figure is absent, or the operands are the wrong entities, or the recomputed result disagrees, the computed assertion is UNGROUNDED (name the missing/wrong operand or the recomputed vs stated value).

## Part C — Missing pieces (forward gap)

`missing_information` = what the answer STILL needs but the evidence does NOT contain, with a searchable hint. This is the targeted re-search signal (Google Query Rewriter input). It is distinct from `ungrounded_assertions`:
- `ungrounded_assertions` = the draft asserted something the evidence does NOT support -> drop/correct it.
- `missing_information` = the answer REQUIRES a fact/entity/value the evidence lacks -> search for it.

Populate `missing_information` when, even if every draft is grounded, the evidence does NOT cover a part the question asks for. Examples:
- the question asks for a specific property (age / year / employer / distance / count) and the evidence has the entity but not that property;
- a disambiguation is needed (evidence has a same-named but wrong entity);
- the evidence covers only part of an enumeration (some list members present, others absent).

Each `missing_information` item MUST carry a concrete `search_hint`: a searchable query (keywords / qualifiers / the specific entity+relation) that would retrieve the missing fact. Leave empty when the evidence is complete.

## Output format (JSON) — ONE object, no commentary before/after

```json
{
  "is_sufficient": true,
  "confidence": 0.0,
  "required_entities": ["Entity 1"],
  "contradictions": ["conflicting figures if any"],
  "reasoning": "step-by-step sufficiency judgment",
  "sub_queries": [
    {
      "sub_query": "Which team's name was inspired by the Boston Braves?",
      "satisfied": true
    },
    {
      "sub_query": "Who is that team's president?",
      "satisfied": false,
      "missing_fact": "the team's current president",
      "search_hint": "Washington Commanders president 2024"
    }
  ],
  "claims": [
    {
      "claim_id": "c1",
      "grounded": true,
      "ungrounded_assertions": [],
      "missing_information": [
        {"what": "the specific missing fact the answer needs", "search_hint": "searchable query to retrieve it"}
      ]
    }
  ]
}
```

## Requirements

0. **The TOP-LEVEL output is ONE single JSON object.** `claims` is an array FIELD inside that object — never wrap the whole output in an array, and never output just the `claims` array. The output must start with `{` and end with `}`.
1. `is_sufficient` true iff a plausible answer can be inferred from the context (Part A). Use the paper's SUFFICIENT-CONTEXT criterion: sufficient means a plausible answer can be constructed from the context — it does NOT need to be provably correct or complete. **A missing detail that a rough/partial answer can still accommodate does NOT make the context insufficient** — if the context lets you infer ANY plausible answer, mark sufficient (the presentation layer decides whether to caveat). Only when the context cannot support ANY plausible answer is it insufficient, and then your job is to say exactly what is missing (Part C).
2. `confidence` (0-1): how confident you are in the sufficiency decision. 0.9-1.0 if clearly sufficient/fails; 0.5-0.7 if partial/ambiguous; below 0.5 if you cannot tell.
3. `contradictions`: list internally conflicting figures/statements that make a single answer ambiguous. Empty array when none.
4. `claims` is MANDATORY and must NEVER be empty when there is at least one claim draft. Include EVERY claim_id exactly once. For each claim you must output `grounded`, `ungrounded_assertions`, AND `missing_information`. Do NOT omit any claim or any field.
5. `missing_information` is the MOST IMPORTANT field when `is_sufficient` is false. It is what drives the next search. When the context is insufficient, you MUST list, per claim, the concrete facts/entities/relations that are absent but required — each with a searchable `search_hint`. An empty `missing_information` is only acceptable when the claim is fully covered. **Never output an empty `claims` array while also saying `is_sufficient: false`** — that is a contradiction and aborts the whole search. If you judge the context insufficient, your primary job is to say exactly WHAT is missing and WHERE to search next.
6. `sub_queries` is MANDATORY (may be an empty array only when there is nothing to decompose). Emit the full step-by-step sub-query set from Part A stage 1, each marked `satisfied`. A `satisfied: false` entry MUST carry `missing_fact` + `search_hint` and is the precise "what to search next" signal — the Query Rewriter turns exactly those into the next round's queries. Keep sub-queries mutually-exclusive and fully-covering (no overlap, no gap beyond the unsatisfied ones).
7. `ungrounded_assertions` empty when grounded; otherwise list each with a one-line `reason`.
8. `reasoning` should be concise and clear.
"##;

/// One claim's review input: id, intermediate draft, cited evidence (indices).
#[derive(Debug, Clone, Default)]
pub struct ScClaim {
    pub claim_id: String,
    pub draft: String,
    pub evidence_ids: Vec<i64>,
}

/// `_is_table_text`: HTML table markup or >=3 pipe rows.
pub fn is_table_text(text: &str) -> bool {
    let lowered = text.to_lowercase();
    if lowered.contains("<table") || lowered.contains("<tr") {
        return true;
    }
    text.lines()
        .filter(|line| line.matches('|').count() >= 2)
        .count()
        >= 3
}

/// `_clamp` (defaults to 1.0 on unparseable input, like upstream).
pub fn clamp01(value: &Value) -> f64 {
    match value {
        Value::Number(number) => number.as_f64().map(|f| f.clamp(0.0, 1.0)).unwrap_or(1.0),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map(|f| f.clamp(0.0, 1.0))
            .unwrap_or(1.0),
        _ => 1.0,
    }
}

/// `_coerce_dict`: tolerate model format drift (object / array-of-objects /
/// JSON string).
pub fn coerce_dict(result: &Value) -> Option<Value> {
    match result {
        Value::Object(_) => Some(result.clone()),
        Value::Array(items) => items.iter().find(|item| item.is_object()).cloned(),
        Value::String(text) => {
            let parsed: Value = serde_json::from_str(text).ok()?;
            coerce_dict(&parsed)
        }
        _ => None,
    }
}

/// `_render_reports`: `Claim <id>: <draft>` lines.
pub fn render_reports(reports: &[(String, String)]) -> String {
    let lines: Vec<String> = reports
        .iter()
        .filter(|(_, draft)| !draft.is_empty())
        .map(|(claim_id, draft)| format!("Claim {claim_id}: {draft}"))
        .collect();
    if lines.is_empty() {
        return "(no claim drafts)".to_string();
    }
    lines.join("\n")
}

/// `_bounded_excerpt`: a bounded evidence window around a draft term; tables
/// return their full content.
pub fn bounded_excerpt(text: &str, hints: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if is_table_text(trimmed) {
        return trimmed.to_string();
    }
    let max_chars = max_chars.max(80);
    let token_re = regex::Regex::new(r"[A-Za-z0-9_\u{4E00}-\u{9FFF}]{3,}").unwrap();
    let lowered = trimmed.to_lowercase();
    let mut start_char: Option<usize> = None;
    let chars: Vec<char> = trimmed.chars().collect();
    for found in token_re.find_iter(hints) {
        let needle = found.as_str().to_lowercase();
        if let Some(position) = lowered.find(&needle) {
            start_char = Some(trimmed[..position.min(trimmed.len())].chars().count());
            break;
        }
    }
    let char_len = chars.len();
    let Some(start_char) = start_char else {
        if char_len <= max_chars {
            return trimmed.to_string();
        }
        let tail = max_chars / 2;
        let head: String = chars[..max_chars - tail].iter().collect();
        let back: String = chars[char_len - tail..].iter().collect();
        return format!("{head} … {back}");
    };
    let half = max_chars / 2;
    let mut left = start_char.saturating_sub(half);
    let right = (left + max_chars).min(char_len);
    left = right.saturating_sub(max_chars);
    let prefix = if left > 0 { "…" } else { "" };
    let suffix = if right < char_len { "…" } else { "" };
    let window: String = chars[left..right].iter().collect();
    format!("{prefix}{window}{suffix}")
}

/// `_render_claim_context`: per-claim draft + bounded evidence anchors.
pub fn render_claim_context(claims: &[ScClaim], kbinfos: Option<&Kbinfos>) -> String {
    if claims.is_empty() {
        return "(no claim drafts)".to_string();
    }
    let empty: Vec<Value> = Vec::new();
    let chunks: &Vec<Value> = kbinfos.map(|k| &k.chunks).unwrap_or(&empty);
    let mut blocks: Vec<String> = Vec::new();
    let mut used = 0usize;
    for claim in claims {
        if claim.draft.is_empty() {
            continue;
        }
        let mut block = format!("Claim {} (draft):\n{}", claim.claim_id, claim.draft);
        used += block.chars().count() + 2;
        if !claim.evidence_ids.is_empty() {
            let mut anchors: Vec<String> = Vec::new();
            for evidence_id in claim.evidence_ids.iter().take(20) {
                let Some(chunk) = chunks.get(*evidence_id as usize) else {
                    continue;
                };
                let text = chunk
                    .get("content_with_weight")
                    .or_else(|| chunk.get("content"))
                    .or_else(|| chunk.get("chunk"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if text.is_empty() {
                    continue;
                }
                let excerpt = bounded_excerpt(&text, &claim.draft, SCA_EVIDENCE_ANCHOR_CHARS);
                if !excerpt.is_empty() {
                    anchors.push(excerpt);
                }
                if anchors.len() >= 3 {
                    break;
                }
            }
            if !anchors.is_empty() {
                let anchor_text = anchors.join(" | ");
                block.push_str("\n  Evidence: ");
                block.push_str(&anchor_text);
                used += anchor_text.chars().count() + 4;
            }
        }
        blocks.push(block);
        if used >= SCA_CLAIMS_CONTEXT_MAX {
            break;
        }
    }
    if blocks.is_empty() {
        return "(no claim drafts)".to_string();
    }
    blocks.join("\n\n")
}

/// `_render_overall_draft`: `[Claim <id>] <draft>` blocks, capped.
pub fn render_overall_draft(claims: &[ScClaim]) -> String {
    let parts: Vec<String> = claims
        .iter()
        .filter(|claim| !claim.draft.trim().is_empty())
        .map(|claim| format!("[Claim {}] {}", claim.claim_id, claim.draft.trim()))
        .collect();
    if parts.is_empty() {
        return "(no overall draft)".to_string();
    }
    let draft = parts.join("\n");
    draft.chars().take(SCA_CLAIMS_CONTEXT_MAX).collect()
}

/// Render the SCA prompt for a review round.
pub fn render_sca_prompt(question: &str, claims_context: &str, overall_draft: &str) -> String {
    SCA_SELECT
        .replace("{{ question }}", question)
        .replace("{{ overall_draft }}", overall_draft)
        .replace("{{ claims_context }}", claims_context)
}

/// `sufficient_context_agent`: unified three-part review. Returns the verdict
/// object, or `{}` when unavailable (no chat model, no drafts, or failure).
pub async fn sufficient_context_agent(
    chat: Option<&dyn HarnessChat>,
    question: &str,
    claims: &[ScClaim],
    kbinfos: Option<&Kbinfos>,
    stats: &StatsHandle,
) -> Value {
    let _phase = stats.enter_phase("sca");
    if claims.is_empty() {
        return json!({});
    }
    let Some(chat) = chat else {
        return json!({});
    };
    let claims_context = render_claim_context(claims, kbinfos);
    if claims_context == "(no claim drafts)" {
        return json!({});
    }
    let overall_draft = render_overall_draft(claims);
    let prompt_text = render_sca_prompt(question, &claims_context, &overall_draft);
    let history = vec![json!({"role": "user", "content": "Output:\n"})];
    let raw = match chat.chat(&prompt_text, &history, &json!({})).await {
        Ok(raw) => raw,
        Err(_) => return json!({}),
    };
    let think = regex::Regex::new("(?s)^.*</think>").unwrap();
    let fences = regex::Regex::new(r"```(?:json)?\s*|\s*```").unwrap();
    let cleaned = fences
        .replace_all(&think.replace(&raw, ""), "")
        .trim()
        .to_string();
    let parsed: Value = serde_json::from_str(&cleaned).unwrap_or(Value::Null);
    let Some(result) = coerce_dict(&parsed) else {
        return json!({});
    };

    let mut claims_out = serde_json::Map::new();
    if let Some(items) = result.get("claims").and_then(Value::as_array) {
        for item in items {
            let claim_id = item
                .get("claim_id")
                .map(|value| match value {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default();
            if claim_id.is_empty() {
                continue;
            }
            let mut ungrounded: Vec<String> = Vec::new();
            if let Some(list) = item.get("ungrounded_assertions").and_then(Value::as_array) {
                for entry in list {
                    if entry.is_object() {
                        let text = entry
                            .get("assertion")
                            .or_else(|| entry.get("reason"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if !text.is_empty() {
                            ungrounded.push(text);
                        }
                    } else if !entry.is_null() {
                        ungrounded.push(match entry {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        });
                    }
                }
            }
            let mut missing_info: Vec<Value> = Vec::new();
            if let Some(list) = item.get("missing_information").and_then(Value::as_array) {
                for entry in list {
                    if entry.is_object() {
                        let what = entry
                            .get("what")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        let hint = entry
                            .get("search_hint")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if !what.is_empty() || !hint.is_empty() {
                            missing_info.push(json!({"what": what, "search_hint": hint}));
                        }
                    } else if !entry.is_null() {
                        missing_info.push(json!({
                            "what": match entry {
                                Value::String(text) => text.trim().to_string(),
                                other => other.to_string(),
                            },
                            "search_hint": "",
                        }));
                    }
                }
            }
            claims_out.insert(
                claim_id,
                json!({
                    "grounded": item.get("grounded").and_then(Value::as_bool).unwrap_or(false),
                    "ungrounded": ungrounded,
                    "missing_information": missing_info,
                }),
            );
        }
    }

    let mut sub_queries: Vec<Value> = Vec::new();
    if let Some(items) = result.get("sub_queries").and_then(Value::as_array) {
        for item in items {
            if !item.is_object() {
                continue;
            }
            let sub_query = item
                .get("sub_query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if sub_query.is_empty() {
                continue;
            }
            let satisfied = item
                .get("satisfied")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if satisfied {
                sub_queries.push(json!({"sub_query": sub_query, "satisfied": true}));
            } else {
                sub_queries.push(json!({
                    "sub_query": sub_query,
                    "satisfied": false,
                    "missing_fact": item.get("missing_fact").and_then(Value::as_str).unwrap_or("").trim(),
                    "search_hint": item.get("search_hint").and_then(Value::as_str).unwrap_or("").trim(),
                }));
            }
        }
    }

    let is_sufficient = result
        .get("is_sufficient")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Failsafe: insufficient with an EMPTY claims array would otherwise leave the
    // orchestrator with nothing to re-search; harvest top-level missing pieces
    // (or fall back to the un-verified drafts as coarse gaps).
    if !is_sufficient && claims_out.is_empty() {
        let mut top_missing: Vec<Value> = Vec::new();
        if let Some(list) = result.get("missing_information").and_then(Value::as_array) {
            for entry in list {
                if entry.is_object() {
                    let what = entry
                        .get("what")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let hint = entry
                        .get("search_hint")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !what.is_empty() || !hint.is_empty() {
                        top_missing.push(json!({"what": what, "search_hint": hint}));
                    }
                } else if !entry.is_null() {
                    top_missing.push(json!({
                        "what": match entry {
                            Value::String(text) => text.trim().to_string(),
                            other => other.to_string(),
                        },
                        "search_hint": "",
                    }));
                }
            }
        }
        if !top_missing.is_empty() {
            claims_out.insert(
                "_global".to_string(),
                json!({"grounded": false, "ungrounded": [], "missing_information": top_missing}),
            );
        } else {
            let mut draft_gaps: Vec<Value> = Vec::new();
            for claim in claims {
                let draft = claim.draft.trim();
                if !draft.is_empty() {
                    draft_gaps.push(json!({"what": draft, "search_hint": draft}));
                }
            }
            if !draft_gaps.is_empty() {
                claims_out.insert(
                    "_global".to_string(),
                    json!({"grounded": false, "ungrounded": [], "missing_information": draft_gaps}),
                );
            }
        }
    }

    let contradictions: Vec<String> = result
        .get("contradictions")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|entry| match entry {
                    Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .filter(|text| !text.trim().is_empty())
                .collect()
        })
        .unwrap_or_default();

    json!({
        "is_sufficient": is_sufficient,
        "confidence": clamp01(result.get("confidence").unwrap_or(&Value::Null)),
        "contradictions": contradictions,
        "reasoning": result.get("reasoning").and_then(Value::as_str).unwrap_or("").trim(),
        "sub_queries": sub_queries,
        "claims": Value::Object(claims_out),
    })
}

/// `to_boost`: adapt the unified output into the decision-ladder `boost` dict.
pub fn to_boost(sca: &Value, fallback_followups: &[Value]) -> Value {
    let mut missing: Vec<String> = Vec::new();
    if let Some(claims) = sca.get("claims").and_then(Value::as_object) {
        for group in claims.values() {
            if let Some(items) = group.get("missing_information").and_then(Value::as_array) {
                for item in items {
                    let what = item
                        .get("what")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !what.is_empty() && !missing.contains(&what) {
                        missing.push(what);
                    }
                }
            }
        }
    }
    let contradictions: Vec<Value> = sca
        .get("contradictions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let feedback = if missing.is_empty() {
        String::new()
    } else {
        format!(
            "missing: {}",
            missing
                .iter()
                .take(FEEDBACK_MAX)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        )
    };
    json!({
        "is_sufficient": sca.get("is_sufficient").and_then(Value::as_bool).unwrap_or(false),
        "confidence": clamp01(sca.get("confidence").unwrap_or(&Value::Null)),
        "missing": missing,
        "contradictions": contradictions,
        "followups": fallback_followups,
        "feedback": feedback,
        "_sub_queries": sca.get("sub_queries").cloned().unwrap_or(json!([])),
    })
}

/// `to_grounded`: adapt the unified output into the `grounded` dict.
pub fn to_grounded(sca: &Value) -> Value {
    sca.get("claims").cloned().unwrap_or(json!({}))
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
            8192
        }
    }

    fn claim(id: &str, draft: &str, evidence: &[i64]) -> ScClaim {
        ScClaim {
            claim_id: id.to_string(),
            draft: draft.to_string(),
            evidence_ids: evidence.to_vec(),
        }
    }

    #[test]
    fn rendering_mirrors_upstream() {
        let reports = vec![
            ("c1".to_string(), "alpha draft".to_string()),
            ("c2".to_string(), String::new()),
        ];
        assert_eq!(render_reports(&reports), "Claim c1: alpha draft");
        assert_eq!(render_reports(&[]), "(no claim drafts)");

        let claims = vec![claim("c1", "alpha", &[0]), claim("c2", "beta", &[9, 1])];
        let kbinfos = Kbinfos {
            chunks: vec![
                json!({"content": "Alpha evidence first line here."}),
                json!({"content": "Beta table likely"}),
            ],
            doc_aggs: vec![],
            pre_summary: None,
        };
        let context = render_claim_context(&claims, Some(&kbinfos));
        assert!(context.contains("Claim c1 (draft):\nalpha"));
        assert!(context.contains("Evidence: Alpha evidence first line here."));
        // Evidence id 9 is out of range and skipped; id 1 resolves.
        assert!(context.contains("Beta table likely"));

        let overall = render_overall_draft(&claims);
        assert_eq!(overall, "[Claim c1] alpha\n[Claim c2] beta");
    }

    #[test]
    fn bounded_excerpt_tables_and_windows() {
        assert!(is_table_text("| a | b |\n| - | - |\n| 1 | 2 |"));
        assert!(!is_table_text("plain text"));
        let table = "| a | b |\n| - | - |\n| 1 | 2 |";
        assert_eq!(bounded_excerpt(table, "nothing", 5), table);
        let long = format!("{}needle{}", "x".repeat(400), "y".repeat(400));
        let excerpt = bounded_excerpt(&long, "needle", 300);
        assert!(excerpt.contains("needle"));
        assert!(excerpt.chars().count() <= 302);
        assert!(excerpt.starts_with('…'));
        assert!(excerpt.ends_with('…'));
    }

    #[test]
    fn coercion_and_clamp() {
        assert_eq!(coerce_dict(&json!({"a": 1})), Some(json!({"a": 1})));
        assert_eq!(
            coerce_dict(&json!([{"a": 1}, {"b": 2}])),
            Some(json!({"a": 1}))
        );
        assert_eq!(
            coerce_dict(&json!("{\"ok\": true}")),
            Some(json!({"ok": true}))
        );
        assert_eq!(coerce_dict(&json!(42)), None);
        assert_eq!(clamp01(&json!(1.7)), 1.0);
        assert_eq!(clamp01(&json!(-3)), 0.0);
        assert_eq!(clamp01(&json!("0.4")), 0.4);
        assert_eq!(clamp01(&json!(null)), 1.0);
    }

    #[tokio::test]
    async fn unified_review_and_failsafes() {
        let stats = StatsHandle::new();
        let claims = vec![claim("c1", "alpha draft", &[])];
        let reply = serde_json::json!({
            "is_sufficient": false,
            "confidence": 0.4,
            "contradictions": ["conflict"],
            "reasoning": "reason",
            "sub_queries": [
                {"sub_query": "hop one", "satisfied": true},
                {"sub_query": "hop two", "satisfied": false, "missing_fact": "fact", "search_hint": "hint"}
            ],
            "claims": [
                {"claim_id": "c1", "grounded": true, "ungrounded_assertions": [{"assertion": "bad", "reason": "r"}], "missing_information": [{"what": "w", "search_hint": "h"}]}
            ]
        })
        .to_string();
        let chat = MockChat { reply: Ok(reply) };
        let verdict = sufficient_context_agent(Some(&chat), "q", &claims, None, &stats).await;
        assert_eq!(verdict["is_sufficient"], json!(false));
        assert_eq!(verdict["confidence"], json!(0.4));
        assert_eq!(verdict["claims"]["c1"]["grounded"], json!(true));
        assert_eq!(verdict["sub_queries"][1]["satisfied"], json!(false));

        // Insufficient with empty claims: top-level missing pieces become _global.
        let reply = json!({"is_sufficient": false, "missing_information": [{"what": "w2", "search_hint": "h2"}], "claims": []}).to_string();
        let chat = MockChat { reply: Ok(reply) };
        let verdict = sufficient_context_agent(Some(&chat), "q", &claims, None, &stats).await;
        assert_eq!(verdict["claims"]["_global"]["grounded"], json!(false));
        assert_eq!(
            verdict["claims"]["_global"]["missing_information"][0]["what"],
            json!("w2")
        );

        // Insufficient with no structured gap at all: derive coarse gaps.
        let reply = json!({"is_sufficient": false, "claims": []}).to_string();
        let chat = MockChat { reply: Ok(reply) };
        let verdict = sufficient_context_agent(Some(&chat), "q", &claims, None, &stats).await;
        assert_eq!(
            verdict["claims"]["_global"]["missing_information"][0]["what"],
            json!("alpha draft")
        );

        // No chat / chat failure / no drafts -> {}.
        assert_eq!(
            sufficient_context_agent(None, "q", &claims, None, &stats).await,
            json!({})
        );
        let failing = MockChat {
            reply: Err("boom".to_string()),
        };
        assert_eq!(
            sufficient_context_agent(Some(&failing), "q", &claims, None, &stats).await,
            json!({})
        );
        assert_eq!(
            sufficient_context_agent(Some(&chat), "q", &[], None, &stats).await,
            json!({})
        );
    }

    #[tokio::test]
    async fn adapters_mirror_upstream() {
        let sca = json!({
            "is_sufficient": false,
            "confidence": 0.3,
            "contradictions": ["c"],
            "sub_queries": [{"sub_query": "s", "satisfied": false}],
            "claims": {
                "c1": {"grounded": true, "ungrounded": [], "missing_information": [{"what": "w1", "search_hint": ""}]},
                "c2": {"grounded": false, "ungrounded": ["u"], "missing_information": [{"what": "w2", "search_hint": ""}]}
            }
        });
        let boost = to_boost(&sca, &[json!({"query": "f"})]);
        assert_eq!(boost["is_sufficient"], json!(false));
        assert_eq!(boost["missing"], json!(["w1", "w2"]));
        assert_eq!(boost["feedback"], json!("missing: w1; w2"));
        assert_eq!(boost["_sub_queries"][0]["sub_query"], json!("s"));
        let grounded = to_grounded(&sca);
        assert_eq!(grounded["c2"]["grounded"], json!(false));
        assert_eq!(grounded["c2"]["ungrounded"], json!(["u"]));
    }
}
