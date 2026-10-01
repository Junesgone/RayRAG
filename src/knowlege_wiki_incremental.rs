//! Wiki incremental compilation — RAGFlow v0.27.2
//! `rag/advanced_rag/knowlege_compile/wiki_incremental.py`.
//!
//! Dual-mode incremental wiki compilation: entity mode (1 concept = 1 page)
//! and topic mode (LLM-grouped pages), both sharing MAP + REDUCE + FINALIZE.
//!
//! Port progress: part 1 — module constants, refine task runner, small
//! helpers (page-id derivation, query text, think/JSON parsing, one-shot
//! chat), re-synthesis gate, verbatim chunk loading/enrichment and the
//! REFINE-failure marker CRUD.
//!
//! Established divergences:
//! - Sequential task execution (upstream `asyncio.as_completed` fan-out).
//! - Batch `{"id": [ids]}` conditions do not translate to the local store's
//!   containment semantics, so chunk fetches use per-id `get()` calls.
//! - Rune budgets are counted in `char`s.
use crate::doc_store::{DocRow, DocStore, SearchQuery};
use crate::harness::{HarnessChat, form_message, message_fit_in};
use crate::structure_compile::{knowledge_compile_gen_conf, stable_row_id};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// `WIKI_REFINE_PROGRESS_UPDATES`.
pub const WIKI_REFINE_PROGRESS_UPDATES: usize = 20;

// ----- constants -----------------------------------------------------------

/// `WIKI_PAGE_COMPILE_KWD`.
pub const WIKI_PAGE_COMPILE_KWD: &str = "wiki_page";
/// `WIKI_PLAN_GROUP_COMPILE_KWD`.
pub const WIKI_PLAN_GROUP_COMPILE_KWD: &str = "wiki_plan_group";
/// `WIKI_DOC_PAGE_SOURCE_COMPILE_KWD`.
pub const WIKI_DOC_PAGE_SOURCE_COMPILE_KWD: &str = "wiki_doc_page_source";
/// `WIKI_CANONICAL_ENTITY_COMPILE_KWD`.
pub const WIKI_CANONICAL_ENTITY_COMPILE_KWD: &str = "wiki_canonical_entity";
/// `WIKI_REFINE_FAILURE_COMPILE_KWD`.
pub const WIKI_REFINE_FAILURE_COMPILE_KWD: &str = "wiki_refine_failure";

/// `ENTITY_MERGE_THRESHOLD`.
pub const ENTITY_MERGE_THRESHOLD: f64 = 0.90;
/// `ENTITY_AMBIGUOUS_LOW`.
pub const ENTITY_AMBIGUOUS_LOW: f64 = 0.75;
/// `ENTITY_PAIRWISE_BLOCK_SIZE`.
pub const ENTITY_PAIRWISE_BLOCK_SIZE: usize = 1024;

/// `ENTITY_MATCH_KNN_CONCURRENT`.
pub const ENTITY_MATCH_KNN_CONCURRENT: usize = 20;
/// `CANONICAL_PERSIST_CONCURRENT`.
pub const CANONICAL_PERSIST_CONCURRENT: usize = 20;
/// `PAGE_ROUTER_KNN_CONCURRENT`.
pub const PAGE_ROUTER_KNN_CONCURRENT: usize = 20;
/// `WIKI_GROUP_LLM_MAX_CONCURRENT`.
pub const WIKI_GROUP_LLM_MAX_CONCURRENT: usize = 8;
/// `WIKI_GROUP_LLM_CANDIDATE_SIZE`.
pub const WIKI_GROUP_LLM_CANDIDATE_SIZE: usize = 24;
/// `WIKI_ROUTE_LLM_BATCH_SIZE`.
pub const WIKI_ROUTE_LLM_BATCH_SIZE: usize = 12;

/// `WIKI_TOPIC_FALLBACK`.
pub const WIKI_TOPIC_FALLBACK: &str = "General";
/// `WIKI_PAGE_TOPIC_CANDIDATE_LIMIT`.
pub const WIKI_PAGE_TOPIC_CANDIDATE_LIMIT: usize = 50;

/// `PAGE_ROUTER_MAYBE_THRESHOLD`.
pub const PAGE_ROUTER_MAYBE_THRESHOLD: f64 = 0.50;
/// `PAGE_ROUTER_TOP_K`.
pub const PAGE_ROUTER_TOP_K: usize = 5;
/// `PAGE_ROUTER_MAX_CANDIDATES`.
pub const PAGE_ROUTER_MAX_CANDIDATES: usize = 12;
/// `PAGE_CLUSTER_MIN_PAGES`.
pub const PAGE_CLUSTER_MIN_PAGES: usize = 8;
/// `PAGE_CLUSTER_MAX_PAGES`.
pub const PAGE_CLUSTER_MAX_PAGES: usize = 60;
/// `PAGE_CLUSTER_ITEMS_PER_PAGE`.
pub const PAGE_CLUSTER_ITEMS_PER_PAGE: usize = 3;
/// `PAGE_CLUSTER_HARD_MAX_SIZE`.
pub const PAGE_CLUSTER_HARD_MAX_SIZE: usize = 8;
/// `PAGE_CLUSTER_MAX_ITERATIONS`.
pub const PAGE_CLUSTER_MAX_ITERATIONS: usize = 20;
/// `PAGE_CLUSTER_CONVERGENCE_EPSILON`.
pub const PAGE_CLUSTER_CONVERGENCE_EPSILON: f64 = 1e-4;

/// `RE_SYNTHESIS_MIN_SOURCES`.
pub const RE_SYNTHESIS_MIN_SOURCES: usize = 5;
/// `RE_SYNTHESIS_GROWTH_RATIO`.
pub const RE_SYNTHESIS_GROWTH_RATIO: f64 = 1.5;
/// `RE_SYNTHESIS_MIN_CLAIMS`.
pub const RE_SYNTHESIS_MIN_CLAIMS: usize = 15;
/// `RE_SYNTHESIS_MIN_VERSIONS`.
pub const RE_SYNTHESIS_MIN_VERSIONS: i64 = 3;

/// `WIKI_SOURCE_BUDGET_CHARS`.
pub const WIKI_SOURCE_BUDGET_CHARS: usize = 32_768;
/// `WIKI_SOURCE_BUDGET_RUNES`.
pub const WIKI_SOURCE_BUDGET_RUNES: usize = 12_000;

// ----- helpers -------------------------------------------------------------

/// `_wiki_run_refine_tasks`: sequential port emitting ~20 progress updates.
pub async fn wiki_run_refine_tasks(
    tasks: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
    progress: Option<&(dyn Fn(String) + Send + Sync)>,
) {
    let total = tasks.len();
    if total == 0 {
        return;
    }
    let report_every =
        1.max((total + WIKI_REFINE_PROGRESS_UPDATES - 1) / WIKI_REFINE_PROGRESS_UPDATES);
    for (index, task) in tasks.into_iter().enumerate() {
        task.await;
        let done = index + 1;
        if done % report_every == 0 || done == total {
            if let Some(progress) = progress {
                progress(format!("{done}/{total} pages completed."));
            }
        }
    }
}

/// `_wiki_log_stats`: machine-readable single-line compilation statistics.
pub fn wiki_log_stats(stage: &str, event: &str, fields: &[(&str, Value)]) {
    let mut map: BTreeMap<String, Value> = BTreeMap::new();
    map.insert("stage".to_string(), Value::String(stage.to_string()));
    map.insert("event".to_string(), Value::String(event.to_string()));
    for (key, value) in fields {
        map.insert((*key).to_string(), value.clone());
    }
    let payload = Value::Object(map.into_iter().collect());
    tracing::info!("wiki stats {}", payload);
}

fn page_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^a-zA-Z0-9\u{4e00}-\u{9fff}]+").expect("page id regex"))
}

/// `_wiki_derive_page_id`: URL-safe page identifier from a name.
pub fn derive_page_id(term: &str, prefix: &str) -> String {
    let replaced = page_id_re().replace_all(term, "-");
    let slug = replaced.trim_matches('-').to_lowercase();
    format!("{prefix}/{slug}")
}

fn truthy_str(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(s) => {
            if s.is_empty() {
                None
            } else {
                Some(s.clone())
            }
        }
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::Array(a) => {
            if a.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
        Value::Object(m) => {
            if m.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
    }
}

/// `_entity_to_query_text`.
pub fn entity_to_query_text(entity: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    let primary = ["entity_name", "name", "term"]
        .iter()
        .find_map(|key| entity.get(*key).and_then(Value::as_str))
        .unwrap_or("");
    parts.push(primary.to_string());
    if let Some(aliases) = entity.get("aliases") {
        let alias_values: Vec<Value> = match aliases {
            Value::String(_) => vec![aliases.clone()],
            Value::Array(items) => items.clone(),
            _ => Vec::new(),
        };
        for alias in alias_values.iter().take(5) {
            if let Some(text) = truthy_str(alias) {
                parts.push(text);
            }
        }
    }
    let description = ["definition_excerpt", "description", "statement"]
        .iter()
        .find_map(|key| entity.get(*key))
        .and_then(truthy_str);
    if let Some(description) = description {
        parts.push(description);
    }
    if let Some(claims) = entity.get("claims").and_then(Value::as_array) {
        for claim in claims.iter().take(3) {
            let statement = ["statement", "text"]
                .iter()
                .find_map(|key| claim.get(*key))
                .and_then(truthy_str);
            if let Some(statement) = statement {
                parts.push(statement);
            }
        }
    }
    parts.join(" ")
}

/// `_strip_think`: drop a leading `</think>` marker remnant.
pub fn strip_think_prefix(text: &str) -> String {
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix("</think>") {
        rest.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// `_wiki_parse_json_array`: extract one JSON array from an LLM response.
pub fn parse_json_array(text: &str) -> Option<Vec<Value>> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end < start {
        return None;
    }
    let slice = text.get(start..=end)?;
    match serde_json::from_str::<Value>(slice) {
        Ok(Value::Array(items)) => Some(items),
        _ => None,
    }
}

/// `_chat_mdl_ask`: one-shot chat returning think-stripped text; a response
/// line starting with `**ERROR**` is raised as an error (upstream raises).
pub async fn chat_mdl_ask(
    chat: &dyn HarnessChat,
    system_prompt: &str,
    user_prompt: &str,
    temperature: f64,
) -> Result<String, String> {
    let messages = form_message(system_prompt, user_prompt);
    let (_, messages) = message_fit_in(messages, chat.max_length());
    let gen_conf = knowledge_compile_gen_conf(
        &chat.model_name(),
        Some(&Map::from_iter([(
            "temperature".to_string(),
            json!(temperature),
        )])),
    );
    let request_conf = Value::Object(gen_conf);
    let system = messages
        .first()
        .and_then(|message| message.get("content"))
        .and_then(Value::as_str)
        .unwrap_or(system_prompt);
    let history: Vec<Value> = messages.iter().skip(1).cloned().collect();
    let raw = chat.chat(system, &history, &request_conf).await?;
    let response = strip_think_prefix(&raw);
    if response
        .lines()
        .any(|line| line.trim_start().starts_with("**ERROR**"))
    {
        return Err(format!("Wiki LLM call failed: {response}"));
    }
    Ok(response)
}

fn json_int(value: &Value, default: i64) -> i64 {
    match value {
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64))
            .unwrap_or(default),
        Value::String(text) => text.trim().parse::<i64>().unwrap_or(default),
        _ => default,
    }
}

/// `_wiki_should_re_synthesize`: four-condition re-synthesis trigger.
pub fn should_re_synthesize(
    page: &Value,
    new_source_doc_ids: &BTreeSet<String>,
    next_version: i64,
) -> bool {
    let existing_sources: BTreeSet<String> = page
        .get("source_doc_ids")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let total_sources = existing_sources.union(new_source_doc_ids).count();
    let claim_count = page
        .get("claims")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let last_synth_ver = page
        .get("synthesis_version_int")
        .map(|value| json_int(value, 1))
        .unwrap_or(1);
    let versions_since = next_version - last_synth_ver;

    total_sources >= RE_SYNTHESIS_MIN_SOURCES
        && claim_count >= RE_SYNTHESIS_MIN_CLAIMS
        && versions_since >= RE_SYNTHESIS_MIN_VERSIONS
        && (total_sources as f64) >= (existing_sources.len() as f64) * RE_SYNTHESIS_GROWTH_RATIO
}

fn inc_search_page(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    fields: &[String],
    condition: &Map<String, Value>,
    offset: usize,
    limit: usize,
) -> crate::Result<Vec<Value>> {
    let query = SearchQuery {
        select_fields: fields.to_vec(),
        condition: condition.clone(),
        match_expressions: Vec::new(),
        offset,
        limit,
        index_names: vec![crate::harness::knowlege_dataset_nav::index_name(tenant_id)],
        dataset_ids: vec![kb_id.to_string()],
        ..Default::default()
    };
    let response = store.search(&query)?;
    Ok(store
        .get_fields(&response, fields)
        .into_values()
        .map(Value::Object)
        .collect())
}

/// `_wiki_load_chunk_texts`: verbatim chunk text by id (per-id `get()`),
/// capped by the rune budget (chars here).
pub fn wiki_load_chunk_texts(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    chunk_ids: &[String],
) -> Map<String, Value> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut unique: Vec<String> = Vec::new();
    for cid in chunk_ids {
        if cid.is_empty() || seen.contains(cid) {
            continue;
        }
        seen.insert(cid.clone());
        unique.push(cid.clone());
    }
    if unique.is_empty() {
        return Map::new();
    }
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let mut out: Map<String, Value> = Map::new();
    for cid in &unique {
        let row = store.get(cid, &index, &[kb_id.to_string()]).ok().flatten();
        if let Some(row) = row {
            let content = row
                .get("content_with_weight")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !content.is_empty() {
                out.insert(cid.clone(), Value::String(content.to_string()));
            }
        }
    }
    let total_chars: usize = out
        .values()
        .filter_map(Value::as_str)
        .map(|text| text.chars().count())
        .sum();
    if total_chars > WIKI_SOURCE_BUDGET_RUNES {
        let mut trimmed: Map<String, Value> = Map::new();
        let mut budget = 0usize;
        for (cid, content) in out.iter() {
            budget += content
                .as_str()
                .map(|text| text.chars().count())
                .unwrap_or(0);
            if budget > WIKI_SOURCE_BUDGET_RUNES {
                break;
            }
            trimmed.insert(cid.clone(), content.clone());
        }
        out = trimmed;
    }
    out
}

/// `_wiki_enrich_source_chunks`: replace condensed claim text with the
/// verbatim chunk body when available (order-preserving, id-deduped).
pub fn wiki_enrich_source_chunks(
    source_chunks: &[Value],
    chunk_texts: &Map<String, Value>,
) -> Vec<Value> {
    let mut enriched: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for sc in source_chunks {
        let cid = ["id", "chunk_id"]
            .iter()
            .find_map(|key| sc.get(*key))
            .and_then(|value| match value {
                Value::String(text) if !text.is_empty() => Some(text.clone()),
                Value::String(_) => None,
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            });
        let Some(cid) = cid else { continue };
        if !seen.insert(cid.clone()) {
            continue;
        }
        let verbatim = chunk_texts
            .get(&cid)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty());
        let text = match verbatim {
            Some(text) => Value::String(text.to_string()),
            None => ["text", "content_with_weight"]
                .iter()
                .find_map(|key| sc.get(*key).cloned())
                .unwrap_or(Value::String(String::new())),
        };
        enriched.push(json!({
            "id": cid,
            "text": text,
            "_verbatim": verbatim.is_some(),
        }));
    }
    enriched
}

/// `_wiki_has_any_pages`.
pub fn wiki_has_any_pages(store: &dyn DocStore, tenant_id: &str, kb_id: &str) -> bool {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    // index_exist 在本地后端仅在显式 create_idx 后为真；以直查为准。
    let fields: Vec<String> = vec!["slug_kwd".to_string()];
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    match inc_search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1) {
        Ok(rows) => !rows.is_empty(),
        Err(err) => {
            tracing::warn!(error = %err, kb = kb_id, "wiki: _wiki_has_any_pages search failed");
            false
        }
    }
}

/// `_wiki_load_refine_failures`: persisted failures keyed by page id.
pub fn wiki_load_refine_failures(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
) -> Map<String, Value> {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let fields: Vec<String> = ["slug_kwd", "content_with_weight"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_REFINE_FAILURE_COMPILE_KWD.to_string()),
    );
    let rows = match inc_search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1000) {
        Ok(rows) => rows,
        Err(err) => {
            tracing::warn!(error = %err, kb = kb_id, "wiki: failed to load REFINE failures");
            return Map::new();
        }
    };
    let mut failures: Map<String, Value> = Map::new();
    for row in &rows {
        let page_id = row
            .get("slug_kwd")
            .map(|value| match value {
                Value::Array(items) => items.first().cloned().unwrap_or(Value::Null),
                other => other.clone(),
            })
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        let page_id = page_id.trim().to_string();
        if page_id.is_empty() {
            continue;
        }
        let payload = row
            .get("content_with_weight")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        failures.insert(page_id, payload);
    }
    failures
}

/// `_wiki_record_refine_failure`: persist one failed page for retry.
pub fn wiki_record_refine_failure(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    page_id: &str,
    entity_names: &[String],
    error: &str,
) {
    let page_id = page_id.trim();
    if page_id.is_empty() {
        return;
    }
    let names: Vec<Value> = entity_names
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(|name| Value::String(name.to_string()))
        .collect();
    let error_text: String = error.chars().take(1000).collect();
    let payload = json!({
        "page_id": page_id,
        "entity_names": names,
        "error": error_text,
    });
    let mut row = Map::new();
    row.insert(
        "id".to_string(),
        Value::String(stable_row_id(&[
            WIKI_REFINE_FAILURE_COMPILE_KWD.to_string(),
            kb_id.to_string(),
            page_id.to_string(),
        ])),
    );
    row.insert("doc_id".to_string(), Value::String(kb_id.to_string()));
    row.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_REFINE_FAILURE_COMPILE_KWD.to_string()),
    );
    row.insert("slug_kwd".to_string(), Value::String(page_id.to_string()));
    row.insert(
        "content_with_weight".to_string(),
        Value::String(payload.to_string()),
    );
    row.insert("available_int".to_string(), json!(0));
    let mut delete_condition = Map::new();
    delete_condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_REFINE_FAILURE_COMPILE_KWD.to_string()),
    );
    delete_condition.insert("slug_kwd".to_string(), Value::String(page_id.to_string()));
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    if let Err(err) = store.delete(&delete_condition, &index, kb_id) {
        tracing::debug!(error = %err, "wiki: prior REFINE failure delete failed");
    }
    if let Err(err) = store.insert(&[row], &index, kb_id) {
        tracing::error!(error = %err, page = page_id, "wiki: failed to persist REFINE failure");
    }
}

/// `_wiki_clear_refine_failure`: remove the retry marker.
pub fn wiki_clear_refine_failure(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    page_id: &str,
) {
    let page_id = page_id.trim();
    if page_id.is_empty() {
        return;
    }
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_REFINE_FAILURE_COMPILE_KWD.to_string()),
    );
    condition.insert("slug_kwd".to_string(), Value::String(page_id.to_string()));
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    if let Err(err) = store.delete(&condition, &index, kb_id) {
        tracing::warn!(error = %err, page = page_id, "wiki: failed to clear REFINE failure");
    }
}

#[cfg(test)]
mod wiki_incremental_part1_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::HarnessChat;
    use std::sync::Mutex;

    struct FakeChat {
        reply: String,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl HarnessChat for FakeChat {
        async fn chat(
            &self,
            _system: &str,
            history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            let user = history
                .last()
                .and_then(|message| message.get("content"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.calls.lock().unwrap().push(user);
            Ok(self.reply.clone())
        }

        fn max_length(&self) -> usize {
            100_000
        }
    }

    #[test]
    fn derive_page_id_slugifies_and_keeps_cjk() {
        assert_eq!(
            derive_page_id("Smartphone Industry", "concept"),
            "concept/smartphone-industry"
        );
        assert_eq!(
            derive_page_id("  Unstable  Name!! ", "entity"),
            "entity/unstable-name"
        );
        assert_eq!(
            derive_page_id("智能手机 产业", "concept"),
            "concept/智能手机-产业"
        );
    }

    #[test]
    fn entity_query_text_aggregates_variants() {
        let entity = json!({
            "entity_name": "Alpha",
            "aliases": ["A", "Alpha Inc", "", 7, "E1", "E2", "E3"],
            "definition_excerpt": "def",
            "claims": [{"statement": "s1"}, {"text": "s2"}, {"nope": 1}, {"statement": "s4"}]
        });
        let text = entity_to_query_text(&entity);
        assert!(text.starts_with("Alpha A Alpha Inc 7 E1"));
        assert!(text.contains("def"));
        assert!(text.contains("s1 s2"));
        assert!(!text.contains("s4"));
        let empty = json!({});
        assert_eq!(entity_to_query_text(&empty), "");
    }

    #[test]
    fn strip_think_and_parse_array() {
        assert_eq!(strip_think_prefix("</think> answer"), "answer");
        assert_eq!(strip_think_prefix("plain"), "plain");
        assert_eq!(strip_think_prefix("a</think>b"), "a</think>b");
        let parsed = parse_json_array("intro [1, 2, 3] outro").expect("array");
        assert_eq!(parsed.len(), 3);
        assert!(parse_json_array("no array").is_none());
        assert!(parse_json_array("[1,2").is_none());
    }

    #[tokio::test]
    async fn chat_mdl_ask_flags_error_lines() {
        let ok_chat = FakeChat {
            reply: "fine answer".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        assert_eq!(
            chat_mdl_ask(&ok_chat, "sys", "user", 0.0)
                .await
                .expect("ok"),
            "fine answer"
        );
        let err_chat = FakeChat {
            reply: "  **ERROR** boom".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        assert!(chat_mdl_ask(&err_chat, "sys", "user", 0.0).await.is_err());
    }

    #[test]
    fn should_re_synthesize_conditions() {
        let page = json!({
            "source_doc_ids": ["d1", "d2"],
            "claims": vec![json!({"statement": "s"}); 15],
            "synthesis_version_int": 1
        });
        let mut new_sources = BTreeSet::new();
        new_sources.insert("d3".to_string());
        new_sources.insert("d4".to_string());
        new_sources.insert("d5".to_string());
        assert!(should_re_synthesize(&page, &new_sources, 4));
        assert!(!should_re_synthesize(&page, &new_sources, 3));
        let small = json!({
            "source_doc_ids": [],
            "claims": [{"statement": "s"}],
            "synthesis_version_int": 1
        });
        assert!(!should_re_synthesize(&small, &new_sources, 10));
    }

    #[test]
    fn chunk_texts_budget_and_enrichment() {
        let store = MemoryDocStore::new();
        let big_a = "a".repeat(5000);
        let big_b = "b".repeat(5000);
        let big_c = "c".repeat(5000);
        let rows: Vec<DocRow> = vec![
            json!({"id": "c1", "content_with_weight": big_a})
                .as_object()
                .cloned()
                .unwrap(),
            json!({"id": "c2", "content_with_weight": big_b})
                .as_object()
                .cloned()
                .unwrap(),
            json!({"id": "c3", "content_with_weight": big_c})
                .as_object()
                .cloned()
                .unwrap(),
        ];
        store
            .insert(
                &rows,
                &crate::harness::knowlege_dataset_nav::index_name("t1"),
                "kb1",
            )
            .unwrap();
        let ids = vec![
            "c1".to_string(),
            "c1".to_string(),
            "c2".to_string(),
            "c3".to_string(),
        ];
        let texts = wiki_load_chunk_texts(&store, "t1", "kb1", &ids);
        assert_eq!(texts.len(), 2);
        assert!(texts.contains_key("c1") && texts.contains_key("c2"));

        let source_chunks = vec![
            json!({"id": "c1", "text": "claim text"}),
            json!({"id": "c1", "text": "dup"}),
            json!({"chunk_id": "c9", "content_with_weight": "fallback"}),
        ];
        let enriched = wiki_enrich_source_chunks(&source_chunks, &texts);
        assert_eq!(enriched.len(), 2);
        assert_eq!(enriched[0]["id"], json!("c1"));
        assert_eq!(enriched[0]["_verbatim"], json!(true));
        assert_eq!(enriched[1]["text"], json!("fallback"));
        assert_eq!(enriched[1]["_verbatim"], json!(false));
    }

    #[test]
    fn refine_failure_marker_roundtrip() {
        let store = MemoryDocStore::new();
        assert!(!wiki_has_any_pages(&store, "t1", "kb1"));
        wiki_record_refine_failure(
            &store,
            "t1",
            "kb1",
            "concept/alpha",
            &["Alpha".to_string(), "  ".to_string()],
            "boom",
        );
        let failures = wiki_load_refine_failures(&store, "t1", "kb1");
        assert_eq!(failures.len(), 1);
        assert_eq!(failures["concept/alpha"]["error"], json!("boom"));
        assert_eq!(failures["concept/alpha"]["entity_names"], json!(["Alpha"]));
        wiki_clear_refine_failure(&store, "t1", "kb1", "concept/alpha");
        assert!(wiki_load_refine_failures(&store, "t1", "kb1").is_empty());
        wiki_record_refine_failure(&store, "t1", "kb1", "", &[], "x");
        assert!(wiki_load_refine_failures(&store, "t1", "kb1").is_empty());
    }
}

// ---------------------------------------------------------------------------
// Part 2 — canonical entity index CRUD (`_load_canonical_entities` ..
// `_normalize_key`).
//
// Adaptation note: the `index_exist` pre-checks are dropped (local backends
// only report an index after an explicit `create_idx`); KNN similarity is
// computed client-side (`extra_options` min-score is not translated), which
// matches the upstream `_score >= threshold` post-check. Entity embeddings
// are dual-written to `q_<dim>_vec` and `embedding` so local scoring works.
// ---------------------------------------------------------------------------

/// `_load_canonical_entities`: `{entity_name: row}`.
pub fn load_canonical_entities(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
) -> Map<String, Value> {
    let fields: Vec<String> = [
        "entity_kwd",
        "entity_type_kwd",
        "aliases",
        "source_doc_ids",
        "source_chunk_ids",
        "mention_count_int",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_CANONICAL_ENTITY_COMPILE_KWD.to_string()),
    );
    let mut results: Map<String, Value> = Map::new();
    let mut offset = 0usize;
    let page_size = 1000usize;
    loop {
        let rows = match inc_search_page(
            store, tenant_id, kb_id, &fields, &condition, offset, page_size,
        ) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, kb = kb_id, "wiki: failed to load canonical entities");
                return results;
            }
        };
        if rows.is_empty() {
            break;
        }
        let page_len = rows.len();
        for mut row in rows {
            let name = match row.get("entity_kwd") {
                Some(Value::Array(items)) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<&str>>()
                    .join(" "),
                Some(Value::String(text)) => text.clone(),
                _ => String::new(),
            };
            let name = name.trim().to_string();
            if name.is_empty() {
                continue;
            }
            if let Some(obj) = row.as_object_mut() {
                for field in ["aliases", "source_doc_ids", "source_chunk_ids"] {
                    if let Some(Value::String(text)) = obj.get(field) {
                        let parsed = if text.is_empty() {
                            Value::Array(Vec::new())
                        } else {
                            serde_json::from_str(text).unwrap_or(Value::Array(Vec::new()))
                        };
                        obj.insert(field.to_string(), parsed);
                    }
                }
                let mention = obj
                    .get("mention_count_int")
                    .map(|value| match value {
                        Value::String(text) if text.chars().all(|c| c.is_ascii_digit()) => {
                            Value::Number(text.parse::<i64>().unwrap_or(0).into())
                        }
                        Value::String(_) => json!(0),
                        other => other.clone(),
                    })
                    .unwrap_or(json!(0));
                obj.insert("mention_count_int".to_string(), mention);
                let entity_type = obj
                    .get("entity_type_kwd")
                    .map(|value| match value {
                        Value::Array(items) => items
                            .first()
                            .cloned()
                            .unwrap_or(Value::String(String::new())),
                        other => other.clone(),
                    })
                    .map(|value| {
                        let text = value.as_str().map(str::trim).unwrap_or("").to_string();
                        if text.is_empty() {
                            "entity".to_string()
                        } else {
                            text
                        }
                    })
                    .unwrap_or_else(|| "entity".to_string());
                obj.insert("entity_type_kwd".to_string(), Value::String(entity_type));
            }
            results.insert(name, row);
        }
        if page_len < page_size {
            break;
        }
        offset += page_size;
    }
    results
}

/// `_build_canonical_entity_doc`: canonical row for insert or update.
#[allow(clippy::too_many_arguments)]
pub fn build_canonical_entity_doc(
    kb_id: &str,
    entity_name: &str,
    entity_type: &str,
    aliases: &[String],
    source_doc_ids: &[String],
    claim_count: i64,
    embedding: Option<&[f32]>,
    source_chunk_ids: Option<&[String]>,
) -> Value {
    let dim = embedding.map(|vector| vector.len()).unwrap_or(768);
    let alias_set: BTreeSet<String> = aliases.iter().cloned().collect();
    let doc_set: BTreeSet<String> = source_doc_ids.iter().cloned().collect();
    let chunk_set: BTreeSet<String> = source_chunk_ids.unwrap_or(&[]).iter().cloned().collect();
    let mut doc = Map::new();
    doc.insert(
        "id".to_string(),
        Value::String(stable_row_id(&[
            WIKI_CANONICAL_ENTITY_COMPILE_KWD.to_string(),
            kb_id.to_string(),
            entity_name.to_string(),
        ])),
    );
    doc.insert(
        "entity_kwd".to_string(),
        Value::String(entity_name.to_string()),
    );
    doc.insert(
        "entity_type_kwd".to_string(),
        Value::String(entity_type.to_string()),
    );
    doc.insert(
        "aliases".to_string(),
        Value::String(Value::Array(alias_set.into_iter().map(Value::String).collect()).to_string()),
    );
    doc.insert(
        "source_doc_ids".to_string(),
        Value::Array(doc_set.into_iter().map(Value::String).collect()),
    );
    doc.insert(
        "source_chunk_ids".to_string(),
        Value::Array(chunk_set.into_iter().map(Value::String).collect()),
    );
    doc.insert("mention_count_int".to_string(), json!(claim_count));
    doc.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_CANONICAL_ENTITY_COMPILE_KWD.to_string()),
    );
    doc.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
    if let Some(embedding) = embedding {
        doc.insert(format!("q_{dim}_vec"), json!(embedding));
        doc.insert("embedding".to_string(), json!(embedding));
    }
    Value::Object(doc)
}

fn canonical_condition(entity_name: &str) -> Map<String, Value> {
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_CANONICAL_ENTITY_COMPILE_KWD.to_string()),
    );
    condition.insert(
        "entity_kwd".to_string(),
        Value::String(entity_name.to_string()),
    );
    condition
}

/// `_save_canonical_entity`: insert or update by existence query.
#[allow(clippy::too_many_arguments)]
pub fn save_canonical_entity(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    entity_name: &str,
    entity_type: &str,
    aliases: &[String],
    source_doc_ids: &[String],
    claim_count: i64,
    embedding: Option<&[f32]>,
    source_chunk_ids: Option<&[String]>,
) {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let doc = build_canonical_entity_doc(
        kb_id,
        entity_name,
        entity_type,
        aliases,
        source_doc_ids,
        claim_count,
        embedding,
        source_chunk_ids,
    );
    let condition = canonical_condition(entity_name);
    let fields: Vec<String> = vec!["entity_kwd".to_string()];
    let exists = match inc_search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1) {
        Ok(rows) => !rows.is_empty(),
        Err(_) => false,
    };
    if exists {
        let mut update_value = doc.as_object().cloned().unwrap_or_default();
        update_value.remove("id");
        if let Err(err) = store.update(&condition, &update_value, &index, kb_id) {
            tracing::warn!(error = %err, entity = entity_name, "wiki: canonical update failed");
        }
    } else if let Some(row) = doc.as_object().cloned() {
        if let Err(err) = store.insert(&[row], &index, kb_id) {
            tracing::warn!(error = %err, entity = entity_name, "wiki: canonical insert failed");
        }
    }
}

/// `_update_canonical_entity`: update without an existence query.
#[allow(clippy::too_many_arguments)]
pub fn update_canonical_entity(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    entity_name: &str,
    entity_type: &str,
    aliases: &[String],
    source_doc_ids: &[String],
    claim_count: i64,
    source_chunk_ids: Option<&[String]>,
) {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let doc = build_canonical_entity_doc(
        kb_id,
        entity_name,
        entity_type,
        aliases,
        source_doc_ids,
        claim_count,
        None,
        source_chunk_ids,
    );
    let mut update_value = doc.as_object().cloned().unwrap_or_default();
    update_value.remove("id");
    let condition = canonical_condition(entity_name);
    if let Err(err) = store.update(&condition, &update_value, &index, kb_id) {
        tracing::warn!(error = %err, entity = entity_name, "wiki: canonical update failed");
    }
}

/// `_delete_canonical_entity`.
pub fn delete_canonical_entity(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    entity_name: &str,
) {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let condition = canonical_condition(entity_name);
    if let Err(err) = store.delete(&condition, &index, kb_id) {
        tracing::warn!(error = %err, entity = entity_name, "wiki: canonical delete failed");
    }
}

fn row_vector(row: &Value, vec_field: &str) -> Vec<f32> {
    row.get(vec_field)
        .or_else(|| row.get("embedding"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_f64().map(|number| number as f32))
                .collect()
        })
        .unwrap_or_default()
}

/// `_knn_search_canonical`: `(entity_name, score)` or None.
pub fn knn_search_canonical(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    embedding: &[f32],
    threshold: f64,
) -> Option<(String, f64)> {
    if embedding.is_empty() {
        return None;
    }
    let dim = embedding.len();
    let vec_field = format!("q_{dim}_vec");
    let fields: Vec<String> = ["entity_kwd", "embedding", vec_field.as_str()]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_CANONICAL_ENTITY_COMPILE_KWD.to_string()),
    );
    let query = SearchQuery {
        select_fields: fields.clone(),
        condition,
        match_expressions: vec![crate::doc_store::MatchExpr::dense(
            &vec_field,
            embedding.to_vec(),
            "cosine",
            1,
        )],
        offset: 0,
        limit: 1,
        index_names: vec![crate::harness::knowlege_dataset_nav::index_name(tenant_id)],
        dataset_ids: vec![kb_id.to_string()],
        ..Default::default()
    };
    let response = match store.search(&query) {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, "wiki: canonical KNN failed");
            return None;
        }
    };
    for (_, row) in store.get_fields(&response, &fields) {
        let name = match row.get("entity_kwd") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .filter(|text| !text.is_empty())
                .collect::<Vec<&str>>()
                .join(" "),
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            continue;
        }
        let row_value = Value::Object(row);
        let stored = row_vector(&row_value, &vec_field);
        let score = crate::merge::cosine_similarity(embedding, &stored) as f64;
        if score >= threshold {
            return Some((name, score));
        }
        return None;
    }
    None
}

/// `_normalize_key`: lowercase, keep word chars + whitespace, strip.
pub fn normalize_key(name: &str) -> String {
    let lowered = name.to_lowercase();
    let filtered: String = lowered
        .chars()
        .filter(|ch| ch.is_alphanumeric() || *ch == '_' || ch.is_whitespace())
        .collect();
    filtered.trim().to_string()
}

#[cfg(test)]
mod wiki_incremental_part2_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;

    #[test]
    fn canonical_doc_shape() {
        let doc = build_canonical_entity_doc(
            "kb1",
            "Alpha",
            "concept",
            &["A".to_string(), "Alpha".to_string(), "A".to_string()],
            &["d2".to_string(), "d1".to_string(), "d1".to_string()],
            5,
            Some(&[1.0, 0.0]),
            Some(&["c2".to_string(), "c1".to_string()]),
        );
        assert_eq!(doc["entity_kwd"], json!("Alpha"));
        assert_eq!(doc["entity_type_kwd"], json!("concept"));
        let aliases: Value = serde_json::from_str(doc["aliases"].as_str().unwrap()).unwrap();
        assert_eq!(aliases, json!(["A", "Alpha"]));
        assert_eq!(doc["source_doc_ids"], json!(["d1", "d2"]));
        assert_eq!(doc["source_chunk_ids"], json!(["c1", "c2"]));
        assert_eq!(doc["mention_count_int"], json!(5));
        assert_eq!(doc["q_2_vec"], json!([1.0, 0.0]));
        assert_eq!(doc["embedding"], json!([1.0, 0.0]));
        assert_eq!(doc["id"].as_str().unwrap().len(), 16);
        let no_vec = build_canonical_entity_doc("kb1", "B", "entity", &[], &[], 0, None, None);
        assert!(no_vec.get("q_2_vec").is_none());
        assert_eq!(no_vec["mention_count_int"], json!(0));
    }

    #[test]
    fn canonical_crud_roundtrip() {
        let store = MemoryDocStore::new();
        save_canonical_entity(
            &store,
            "t1",
            "kb1",
            "Alpha",
            "concept",
            &["A".to_string()],
            &["d1".to_string()],
            3,
            Some(&[1.0, 0.0]),
            Some(&["c1".to_string()]),
        );
        let loaded = load_canonical_entities(&store, "t1", "kb1");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["Alpha"]["aliases"], json!(["A"]));
        assert_eq!(loaded["Alpha"]["mention_count_int"], json!(3));
        assert_eq!(loaded["Alpha"]["entity_type_kwd"], json!("concept"));
        assert_eq!(loaded["Alpha"]["source_chunk_ids"], json!(["c1"]));

        save_canonical_entity(
            &store,
            "t1",
            "kb1",
            "Alpha",
            "concept",
            &["A".to_string(), "B".to_string()],
            &["d1".to_string(), "d2".to_string()],
            7,
            None,
            None,
        );
        let loaded = load_canonical_entities(&store, "t1", "kb1");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["Alpha"]["mention_count_int"], json!(7));
        assert_eq!(loaded["Alpha"]["source_doc_ids"], json!(["d1", "d2"]));

        update_canonical_entity(
            &store,
            "t1",
            "kb1",
            "Alpha",
            "concept",
            &["A".to_string()],
            &["d9".to_string()],
            9,
            None,
        );
        let loaded = load_canonical_entities(&store, "t1", "kb1");
        assert_eq!(loaded["Alpha"]["mention_count_int"], json!(9));

        delete_canonical_entity(&store, "t1", "kb1", "Alpha");
        assert!(load_canonical_entities(&store, "t1", "kb1").is_empty());
    }

    #[test]
    fn canonical_knn_threshold() {
        let store = MemoryDocStore::new();
        save_canonical_entity(
            &store,
            "t1",
            "kb1",
            "Alpha",
            "entity",
            &[],
            &[],
            1,
            Some(&[1.0, 0.0]),
            None,
        );
        save_canonical_entity(
            &store,
            "t1",
            "kb1",
            "Beta",
            "entity",
            &[],
            &[],
            1,
            Some(&[0.0, 1.0]),
            None,
        );
        let (name, score) =
            knn_search_canonical(&store, "t1", "kb1", &[0.9, 0.1], 0.5).expect("knn");
        assert_eq!(name, "Alpha");
        assert!(score > 0.99);
        let (name2, score2) =
            knn_search_canonical(&store, "t1", "kb1", &[0.0, 1.0], 0.5).expect("knn2");
        assert_eq!(name2, "Beta");
        assert!((score2 - 1.0).abs() < 1e-6);
        assert!(knn_search_canonical(&store, "t1", "kb1", &[0.0, 1.0], 1.01).is_none());
        assert!(knn_search_canonical(&store, "t1", "kb1", &[], 0.0).is_none());
    }

    #[test]
    fn normalize_key_shapes() {
        assert_eq!(normalize_key("Hello, World!"), "hello world");
        assert_eq!(normalize_key("  Mixed_Case-42  "), "mixed_case42");
        assert_eq!(normalize_key("智能手机 产业"), "智能手机 产业");
        assert_eq!(normalize_key("a.b/c"), "abc");
    }
}

// ---------------------------------------------------------------------------
// Part 3a — lightweight raw-entity extraction (`_as_str_list` ..
// `_extract_raw_entities`).
//
// Adaptation note: internal sets become BTreeSets and are emitted as sorted
// lists (upstream Python sets are unordered); string entities/concepts/
// claims are parsed as JSON when possible.
// ---------------------------------------------------------------------------

/// `_as_str_list`: coerce a stored field (JSON string / list / None) to strings.
pub fn as_str_list(raw: Option<&Value>) -> Vec<String> {
    match raw {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(text)) => {
            if text.is_empty() {
                Vec::new()
            } else {
                match serde_json::from_str::<Value>(text) {
                    Ok(parsed) => as_str_list(Some(&parsed)),
                    Err(_) => vec![text.clone()],
                }
            }
        }
        Some(Value::Array(items)) => items
            .iter()
            .filter(|item| !item.is_null())
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| item.to_string())
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `_as_int`: coerce numeric doc-store fields (some backends return strings).
pub fn as_int(raw: Option<&Value>, default: i64) -> i64 {
    match raw {
        Some(Value::Number(number)) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64))
            .unwrap_or(default),
        Some(Value::String(text)) => text.trim().parse::<i64>().unwrap_or(default),
        Some(Value::Bool(flag)) => {
            if *flag {
                1
            } else {
                0
            }
        }
        _ => default,
    }
}

/// `_wiki_claim_chunk_ids`: chunk id(s) a MAP item is attributed to.
pub fn wiki_claim_chunk_ids(claim: &Value) -> Vec<String> {
    let Some(obj) = claim.as_object() else {
        return Vec::new();
    };
    let ids = obj.get("chunk_ids");
    if let Some(Value::String(text)) = ids {
        if !text.is_empty() {
            return vec![text.clone()];
        }
        return Vec::new();
    }
    if let Some(Value::Array(items)) = ids {
        return items
            .iter()
            .filter_map(|item| item.as_str())
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .collect();
    }
    match obj.get("source_chunk_id").and_then(Value::as_str) {
        Some(text) if !text.is_empty() => vec![text.to_string()],
        _ => Vec::new(),
    }
}

/// `_wiki_dedupe_claims`.
pub fn wiki_dedupe_claims(claims: &[Value]) -> Vec<Value> {
    let mut result: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<(String, String, Vec<String>)> = BTreeSet::new();
    for claim in claims {
        let Some(obj) = claim.as_object() else {
            continue;
        };
        let statement = obj
            .get("statement")
            .or_else(|| obj.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let doc = obj
            .get("source_doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut chunks = wiki_claim_chunk_ids(claim);
        chunks.sort();
        let key = (statement, doc, chunks);
        if seen.insert(key) {
            result.push(claim.clone());
        }
    }
    result
}

fn parse_json_object(value: &Value) -> Option<Value> {
    match value {
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object),
        Value::Object(_) => Some(value.clone()),
        _ => None,
    }
}

fn set_union_field(
    entry: &mut Map<String, Value>,
    field: &str,
    values: impl IntoIterator<Item = String>,
) {
    let mut set: BTreeSet<String> = entry
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    for value in values {
        if !value.is_empty() {
            set.insert(value);
        }
    }
    entry.insert(
        field.to_string(),
        Value::Array(set.into_iter().map(Value::String).collect()),
    );
}

/// `_extract_raw_entities`: (lightweight entities, claim index).
pub fn extract_raw_entities(map_results: &[Value]) -> (Vec<Value>, Map<String, Value>) {
    let mut raw: Map<String, Value> = Map::new();
    let mut claim_index: Map<String, Value> = Map::new();

    let ensure_entry =
        |raw: &mut Map<String, Value>, name: &str, entity_type: &str, aliases: Vec<String>| {
            if !raw.contains_key(name) {
                let mut entry = Map::new();
                entry.insert("name".to_string(), Value::String(name.to_string()));
                entry.insert("type".to_string(), Value::String(entity_type.to_string()));
                entry.insert(
                    "aliases".to_string(),
                    Value::Array(aliases.into_iter().map(Value::String).collect()),
                );
                entry.insert("claim_count".to_string(), json!(0));
                entry.insert("source_doc_ids".to_string(), Value::Array(Vec::new()));
                entry.insert("source_chunk_ids".to_string(), Value::Array(Vec::new()));
                raw.insert(name.to_string(), Value::Object(entry));
            }
        };

    for mr in map_results {
        let doc_id = mr
            .get("doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if let Some(entities) = mr.get("entities").and_then(Value::as_array) {
            for ent in entities {
                let Some(ent) = parse_json_object(ent) else {
                    continue;
                };
                let name = ent
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                let entity_type = ent
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("entity")
                    .to_string();
                let aliases: Vec<String> = ent
                    .get("aliases")
                    .and_then(Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                ensure_entry(&mut raw, &name, &entity_type, aliases);
                if let Some(entry) = raw.get_mut(&name).and_then(Value::as_object_mut) {
                    set_union_field(entry, "source_doc_ids", vec![doc_id.clone()]);
                    set_union_field(entry, "source_chunk_ids", wiki_claim_chunk_ids(&ent));
                }
            }
        }

        if let Some(concepts) = mr.get("concepts").and_then(Value::as_array) {
            for concept in concepts {
                let Some(concept) = parse_json_object(concept) else {
                    continue;
                };
                let term = concept
                    .get("term")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if term.is_empty() {
                    continue;
                }
                ensure_entry(&mut raw, &term, "concept", vec![term.clone()]);
                if let Some(entry) = raw.get_mut(&term).and_then(Value::as_object_mut) {
                    set_union_field(entry, "source_doc_ids", vec![doc_id.clone()]);
                    set_union_field(entry, "source_chunk_ids", wiki_claim_chunk_ids(&concept));
                }
            }
        }

        if let Some(claims) = mr.get("claims").and_then(Value::as_array) {
            for claim in claims {
                let Some(claim) = parse_json_object(claim) else {
                    continue;
                };
                let subject = ["entity_name", "subject", "term"]
                    .iter()
                    .find_map(|key| claim.get(*key).and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string();
                if subject.is_empty() {
                    continue;
                }
                if let Some(entry) = raw.get_mut(&subject).and_then(Value::as_object_mut) {
                    let count = entry
                        .get("claim_count")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    entry.insert("claim_count".to_string(), json!(count + 1));
                    set_union_field(entry, "source_chunk_ids", wiki_claim_chunk_ids(&claim));
                    let bucket = claim_index
                        .entry(subject.clone())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(items) = bucket.as_array_mut() {
                        items.push(claim);
                    }
                }
            }
        }

        if let Some(relations) = mr.get("relations").and_then(Value::as_array) {
            for relation in relations {
                let Some(relation) = parse_json_object(relation) else {
                    continue;
                };
                let relation_chunks = wiki_claim_chunk_ids(&relation);
                for endpoint in [
                    relation.get("from").and_then(Value::as_str),
                    relation.get("to").and_then(Value::as_str),
                ]
                .into_iter()
                .flatten()
                {
                    if let Some(entry) = raw.get_mut(endpoint).and_then(Value::as_object_mut) {
                        set_union_field(entry, "source_chunk_ids", relation_chunks.clone());
                    }
                }
            }
        }
    }

    let result: Vec<Value> = raw
        .into_values()
        .map(|mut entry| {
            if let Some(obj) = entry.as_object_mut() {
                for field in ["source_doc_ids", "source_chunk_ids"] {
                    if let Some(Value::Array(items)) = obj.get(field) {
                        let mut sorted: Vec<String> = items
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect();
                        sorted.sort();
                        obj.insert(
                            field.to_string(),
                            Value::Array(sorted.into_iter().map(Value::String).collect()),
                        );
                    }
                }
            }
            entry
        })
        .collect();
    (result, claim_index)
}

#[cfg(test)]
mod wiki_incremental_part3_tests {
    use super::*;

    #[test]
    fn as_str_list_and_as_int_shapes() {
        assert_eq!(
            as_str_list(Some(&json!(["a", null, 2]))),
            vec!["a".to_string(), "2".to_string()]
        );
        assert_eq!(as_str_list(Some(&json!("a"))), vec!["a".to_string()]);
        assert_eq!(
            as_str_list(Some(&json!("[\"x\",\"y\"]"))),
            vec!["x".to_string(), "y".to_string()]
        );
        assert_eq!(
            as_str_list(Some(&json!("not json"))),
            vec!["not json".to_string()]
        );
        assert!(as_str_list(None).is_empty());
        assert_eq!(as_int(Some(&json!(5)), 0), 5);
        assert_eq!(as_int(Some(&json!(5.9)), 0), 5);
        assert_eq!(as_int(Some(&json!("7")), 0), 7);
        assert_eq!(as_int(Some(&json!(true)), 0), 1);
        assert_eq!(as_int(Some(&json!("x")), 9), 9);
    }

    #[test]
    fn claim_chunk_ids_and_dedupe() {
        assert_eq!(
            wiki_claim_chunk_ids(&json!({"chunk_ids": ["c1", "c2"]})),
            vec!["c1".to_string(), "c2".to_string()]
        );
        assert_eq!(
            wiki_claim_chunk_ids(&json!({"chunk_ids": "c3"})),
            vec!["c3".to_string()]
        );
        assert_eq!(
            wiki_claim_chunk_ids(&json!({"source_chunk_id": "c9"})),
            vec!["c9".to_string()]
        );
        assert!(wiki_claim_chunk_ids(&json!({"chunk_ids": [""]})).is_empty());
        let claims = vec![
            json!({"statement": "s", "source_doc_id": "d1", "chunk_ids": ["c2", "c1"]}),
            json!({"statement": "s", "source_doc_id": "d1", "chunk_ids": ["c1", "c2"]}),
            json!({"statement": "s", "source_doc_id": "d2", "chunk_ids": ["c1"]}),
            json!({"text": "t", "chunk_ids": ["c1"]}),
        ];
        assert_eq!(wiki_dedupe_claims(&claims).len(), 3);
    }

    #[test]
    fn extract_raw_entities_aggregates_metadata() {
        let map_results = vec![
            json!({
                "doc_id": "d1",
                "entities": [{"name": "Alpha", "type": "org", "aliases": ["A"], "chunk_ids": ["c1"]}],
                "concepts": [{"term": "Beta", "chunk_ids": ["c2"]}],
                "claims": [{"subject": "Alpha", "statement": "s1", "chunk_ids": ["c3"]}, {"subject": "Unknown", "statement": "drop"}],
                "relations": [{"from": "Alpha", "to": "Beta", "type": "uses", "chunk_ids": ["c4"]}]
            }),
            json!({
                "doc_id": "d2",
                "entities": [{"name": "Alpha", "chunk_ids": ["c5"]}],
                "concepts": [],
                "claims": [{"subject": "Alpha", "statement": "s1", "chunk_ids": ["c3"]}],
                "relations": []
            }),
        ];
        let (entities, claim_index) = extract_raw_entities(&map_results);
        assert_eq!(entities.len(), 2);
        let alpha = entities
            .iter()
            .find(|e| e["name"] == json!("Alpha"))
            .expect("alpha");
        assert_eq!(alpha["type"], json!("org"));
        assert_eq!(alpha["claim_count"], json!(2));
        assert_eq!(alpha["source_doc_ids"], json!(["d1", "d2"]));
        assert_eq!(alpha["source_chunk_ids"], json!(["c1", "c3", "c4", "c5"]));
        assert_eq!(alpha["aliases"], json!(["A"]));
        let beta = entities
            .iter()
            .find(|e| e["name"] == json!("Beta"))
            .expect("beta");
        assert_eq!(beta["type"], json!("concept"));
        assert_eq!(beta["aliases"], json!(["Beta"]));
        assert_eq!(beta["source_chunk_ids"], json!(["c2", "c4"]));
        assert_eq!(claim_index["Alpha"].as_array().unwrap().len(), 2);
        assert!(claim_index.get("Unknown").is_none());
    }
}
