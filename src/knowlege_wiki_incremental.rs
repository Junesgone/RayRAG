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
use crate::embed::Embedder;
use crate::harness::knowlege_wiki::{load_active_map_state, load_map_extracts_for_state};
use crate::harness::{HarnessChat, form_message, message_fit_in};
use crate::structure_compile::{knowledge_compile_gen_conf, stable_row_id, tokenize_for_search};
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

/// Paged row query against the tenant index. Shared with the skill compiler so both read the
/// index the same way.
pub(crate) fn inc_search_page(
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

// ---------------------------------------------------------------------------
// Part 3b — entity matching (`_wiki_match_entities`, `_wiki_confirm_batch`,
// `_search_existing_pages`).
//
// Adaptation note: the numpy blockwise matrix becomes a plain per-type Rust
// double loop (same 0.75/0.90 thresholds and union-find semantics); embedding
// or KNN failures skip that step with a warning (upstream raises); all
// concurrency is sequential.
// ---------------------------------------------------------------------------

fn confirm_array_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)\[.*?\]").expect("confirm array regex"))
}

/// `_wiki_confirm_batch`: batch LLM confirm of same-entity pairs.
pub async fn wiki_confirm_batch(
    chat: &dyn HarnessChat,
    candidates: &[(String, String)],
) -> Vec<(String, String)> {
    if candidates.is_empty() {
        return Vec::new();
    }
    let batch_size = 50usize;
    let mut confirmed: Vec<(String, String)> = Vec::new();
    for batch in candidates.chunks(batch_size) {
        let mut prompt_lines: Vec<String> = Vec::new();
        for (idx, (a, b)) in batch.iter().enumerate() {
            prompt_lines.push(format!("{}. \"{a}\" vs \"{b}\"", idx + 1));
        }
        let prompt = format!(
            "You are a KB dedup assistant. For each pair, determine if they refer to the SAME real-world entity.\nRespond with a JSON array of booleans in the same order:\n  [true, false, true, ...]\nwhere true = SAME entity, false = DIFFERENT.\n\n{}",
            prompt_lines.join("\n")
        );
        let response = match chat_mdl_ask(chat, "You are a KB dedup assistant.", &prompt, 0.0).await
        {
            Ok(response) => response,
            Err(err) => {
                tracing::warn!(error = %err, "wiki: LLM confirm batch failed");
                continue;
            }
        };
        let response = response.trim().to_string();
        let Some(captured) = confirm_array_re().find(&response) else {
            continue;
        };
        let Ok(Value::Array(booleans)) = serde_json::from_str::<Value>(captured.as_str()) else {
            continue;
        };
        for (idx, is_same) in booleans.iter().enumerate() {
            if json_truthy_int(is_same) && idx < batch.len() {
                confirmed.push(batch[idx].clone());
            }
        }
    }
    confirmed
}

fn json_truthy_int(value: &Value) -> bool {
    crate::harness::knowlege_wiki::json_truthy(value)
}

fn entry_field_i64(entry: &Value, field: &str) -> i64 {
    entry
        .get(field)
        .map(|value| as_int(Some(value), 0))
        .unwrap_or(0)
}

fn entry_field_names(entry: &Value, field: &str) -> Vec<String> {
    entry
        .get(field)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn set_field_union(entry: &mut Value, field: &str, values: impl IntoIterator<Item = String>) {
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
    if let Some(obj) = entry.as_object_mut() {
        obj.insert(
            field.to_string(),
            Value::Array(set.into_iter().map(Value::String).collect()),
        );
    }
}

/// `_wiki_match_entities`: raw entities → canonical entities.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_match_entities(
    store: &dyn DocStore,
    embd: Option<&dyn Embedder>,
    chat: Option<&dyn HarnessChat>,
    tenant_id: &str,
    kb_id: &str,
    raw_entities: &[Value],
    existing_canonical: &Map<String, Value>,
    incremental: bool,
    progress: Option<&(dyn Fn(String) + Send + Sync)>,
) -> (Map<String, Value>, Map<String, Value>) {
    let report = |message: &str| {
        tracing::info!(message = message, "wiki entity matching");
        if let Some(progress) = progress {
            progress(format!("Entity Matching: {message}"));
        }
    };

    // Step 1 — exact match against the canonical index.
    report(&format!(
        "exact matching {} raw entries against {} canonical entries ...",
        raw_entities.len(),
        existing_canonical.len()
    ));
    let mut exact_flat: BTreeMap<String, String> = BTreeMap::new();
    for (cname, centry) in existing_canonical {
        let Some(aliases) = centry.get("aliases").and_then(Value::as_array) else {
            continue;
        };
        let mut names: Vec<String> = vec![cname.clone()];
        names.extend(aliases.iter().filter_map(Value::as_str).map(str::to_string));
        for alias in names {
            exact_flat.insert(normalize_key(&alias), cname.clone());
        }
    }
    let mut name_resolution: Map<String, Value> = Map::new();
    let mut llm_merge_pairs: Vec<Value> = Vec::new();
    let mut unmatched: Vec<Value> = Vec::new();
    for entry in raw_entities {
        let raw_name = entry.get("name").and_then(Value::as_str).unwrap_or("");
        let norm = normalize_key(raw_name);
        if let Some(canonical) = exact_flat.get(&norm) {
            name_resolution.insert(raw_name.to_string(), Value::String(canonical.clone()));
        } else {
            unmatched.push(entry.clone());
        }
    }
    report(&format!(
        "exact matched {}; {} entries still need semantic matching.",
        name_resolution.len(),
        unmatched.len()
    ));

    // Step 2 — KNN match for unmatched entities.
    if !unmatched.is_empty() && embd.is_some() && !existing_canonical.is_empty() {
        let embd = embd.unwrap();
        let texts: Vec<String> = unmatched.iter().map(entity_to_query_text).collect();
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        report(&format!("KNN unmatched entities {} ...", text_refs.len()));
        match embd.embed(&text_refs).await {
            Ok(embeddings) if embeddings.len() == unmatched.len() => {
                let mut still_unmatched: Vec<Value> = Vec::new();
                let mut maybe_pairs: Vec<(Value, String)> = Vec::new();
                for (entry, vec) in unmatched.iter().zip(embeddings.iter()) {
                    let knn =
                        knn_search_canonical(store, tenant_id, kb_id, vec, ENTITY_AMBIGUOUS_LOW);
                    let (cname, score) = match knn {
                        Some((name, score)) => (Some(name), score),
                        None => (None, 0.0),
                    };
                    let entry_name = entry
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Some(cname) = cname {
                        if score >= ENTITY_MERGE_THRESHOLD {
                            name_resolution.insert(entry_name, Value::String(cname));
                        } else if score >= ENTITY_AMBIGUOUS_LOW {
                            let entry_type = entry
                                .get("type")
                                .and_then(Value::as_str)
                                .unwrap_or("entity");
                            if entry_type == "concept" {
                                name_resolution.insert(entry_name, Value::String(cname));
                            } else {
                                maybe_pairs.push((entry.clone(), cname));
                            }
                        } else {
                            still_unmatched.push(entry.clone());
                        }
                    } else {
                        still_unmatched.push(entry.clone());
                    }
                }

                if !maybe_pairs.is_empty() {
                    if let Some(chat) = chat {
                        let candidates: Vec<(String, String)> = maybe_pairs
                            .iter()
                            .map(|(entry, cname)| {
                                (
                                    entry
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    cname.clone(),
                                )
                            })
                            .collect();
                        let confirmed = wiki_confirm_batch(chat, &candidates).await;
                        let mut confirmed_set: BTreeSet<String> = BTreeSet::new();
                        for (raw_name, cname) in confirmed {
                            name_resolution.insert(raw_name.clone(), Value::String(cname.clone()));
                            confirmed_set.insert(raw_name.clone());
                            llm_merge_pairs.push(json!({
                                "from": raw_name,
                                "into": cname,
                                "scope": "existing_canonical",
                            }));
                        }
                        for (entry, _cname) in maybe_pairs {
                            let raw_name = entry
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if !confirmed_set.contains(&raw_name) {
                                still_unmatched.push(entry);
                            }
                        }
                        unmatched = still_unmatched;
                    } else {
                        for (entry, _cname) in maybe_pairs {
                            still_unmatched.push(entry);
                        }
                        unmatched = still_unmatched;
                    }
                } else {
                    unmatched = still_unmatched;
                }
                report("KNN unmatched entities done.");
            }
            Ok(_) => {
                tracing::warn!("wiki: KNN embedding count mismatch; skipping semantic match");
            }
            Err(err) => {
                tracing::warn!(error = %err, "wiki: KNN embedding failed; skipping semantic match");
            }
        }
    }

    // Step 3 — intra-build pairwise (first build only).
    if !incremental && unmatched.len() > 1 && embd.is_some() {
        let embd = embd.unwrap();
        let texts: Vec<String> = unmatched.iter().map(entity_to_query_text).collect();
        let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let embeddings = match embd.embed(&text_refs).await {
            Ok(embeddings) if embeddings.len() == unmatched.len() => embeddings,
            Ok(_) => {
                tracing::warn!("wiki: pairwise embedding count mismatch; skipping semantic merge");
                Vec::new()
            }
            Err(err) => {
                tracing::warn!(error = %err, "wiki: pairwise embedding failed; skipping semantic merge");
                Vec::new()
            }
        };
        if !embeddings.is_empty() {
            let dim = embeddings[0].len();
            let shapes_ok = dim > 0 && embeddings.iter().all(|vector| vector.len() == dim);
            if !shapes_ok {
                tracing::warn!("wiki: invalid embedding matrix shape; skipping semantic merge");
            } else {
                let normalized: Vec<Vec<f32>> = embeddings
                    .iter()
                    .map(|vector| {
                        let norm = (vector
                            .iter()
                            .map(|x| (*x as f64) * (*x as f64))
                            .sum::<f64>())
                        .sqrt();
                        if norm > 0.0 {
                            vector.iter().map(|x| (*x as f64 / norm) as f32).collect()
                        } else {
                            vector.clone()
                        }
                    })
                    .collect();
                let n = unmatched.len();
                let mut merged_into: BTreeMap<usize, usize> = BTreeMap::new();
                let root = |merged_into: &BTreeMap<usize, usize>, mut index: usize| -> usize {
                    while let Some(next) = merged_into.get(&index) {
                        index = *next;
                    }
                    index
                };
                let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
                for (idx, entry) in unmatched.iter().enumerate() {
                    let entry_type = entry
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("entity")
                        .to_string();
                    groups.entry(entry_type).or_default().push(idx);
                }
                let mut auto_pairs: Vec<(usize, usize)> = Vec::new();
                let mut ambiguous_pairs: Vec<(usize, usize)> = Vec::new();
                for indices in groups.values() {
                    for (left_pos, left) in indices.iter().enumerate() {
                        for right in indices.iter().skip(left_pos + 1) {
                            let score = crate::merge::cosine_similarity(
                                &normalized[*left],
                                &normalized[*right],
                            ) as f64;
                            if score >= ENTITY_MERGE_THRESHOLD {
                                auto_pairs.push((*left, *right));
                            } else if score >= ENTITY_AMBIGUOUS_LOW {
                                ambiguous_pairs.push((*left, *right));
                            }
                        }
                    }
                }
                for (left, right) in auto_pairs {
                    let rl = root(&merged_into, left);
                    let rr = root(&merged_into, right);
                    if rl == rr {
                        continue;
                    }
                    let left_count = entry_field_i64(&unmatched[rl], "claim_count");
                    let right_count = entry_field_i64(&unmatched[rr], "claim_count");
                    if left_count >= right_count {
                        merged_into.insert(rr, rl);
                    } else {
                        merged_into.insert(rl, rr);
                    }
                }
                let still_ambiguous: Vec<(usize, usize)> = ambiguous_pairs
                    .into_iter()
                    .filter(|(left, right)| root(&merged_into, *left) != root(&merged_into, *right))
                    .collect();
                if !still_ambiguous.is_empty() {
                    if let Some(chat) = chat {
                        let candidates: Vec<(String, String)> = still_ambiguous
                            .iter()
                            .map(|(left, right)| {
                                (
                                    unmatched[*left]
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    unmatched[*right]
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                )
                            })
                            .collect();
                        let confirmed = wiki_confirm_batch(chat, &candidates).await;
                        let confirmed_map: BTreeSet<(String, String)> = confirmed
                            .into_iter()
                            .map(|(a, b)| if a <= b { (a, b) } else { (b, a) })
                            .collect();
                        for (left, right) in &still_ambiguous {
                            let left_name = unmatched[*left]
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let right_name = unmatched[*right]
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            let key = if left_name <= right_name {
                                (left_name.clone(), right_name.clone())
                            } else {
                                (right_name.clone(), left_name.clone())
                            };
                            if !confirmed_map.contains(&key) {
                                continue;
                            }
                            let rl = root(&merged_into, *left);
                            let rr = root(&merged_into, *right);
                            if rl == rr {
                                continue;
                            }
                            let left_count = entry_field_i64(&unmatched[rl], "claim_count");
                            let right_count = entry_field_i64(&unmatched[rr], "claim_count");
                            if left_count >= right_count {
                                merged_into.insert(rr, rl);
                                llm_merge_pairs.push(json!({
                                    "from": unmatched[rr].get("name").cloned().unwrap_or(Value::Null),
                                    "into": unmatched[rl].get("name").cloned().unwrap_or(Value::Null),
                                    "scope": "intra_build",
                                }));
                            } else {
                                merged_into.insert(rl, rr);
                                llm_merge_pairs.push(json!({
                                    "from": unmatched[rl].get("name").cloned().unwrap_or(Value::Null),
                                    "into": unmatched[rr].get("name").cloned().unwrap_or(Value::Null),
                                    "scope": "intra_build",
                                }));
                            }
                        }
                    }
                }

                let mut merged_indices: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
                for index in 0..n {
                    let pi = root(&merged_into, index);
                    merged_indices.entry(pi).or_default().push(index);
                }
                let mut new_unmatched: Vec<Value> = Vec::new();
                for (_, indices) in merged_indices {
                    if indices.len() > 1 {
                        let mut master = unmatched[indices[0]].clone();
                        for index in &indices[1..] {
                            let slave = &unmatched[*index];
                            let master_count = entry_field_i64(&master, "claim_count")
                                + entry_field_i64(slave, "claim_count");
                            if let Some(obj) = master.as_object_mut() {
                                obj.insert("claim_count".to_string(), json!(master_count));
                            }
                            let mut aliases: BTreeSet<String> =
                                entry_field_names(&master, "aliases").into_iter().collect();
                            aliases.extend(entry_field_names(slave, "aliases"));
                            if let Some(slave_name) = slave.get("name").and_then(Value::as_str) {
                                aliases.insert(slave_name.to_string());
                            }
                            if let Some(obj) = master.as_object_mut() {
                                obj.insert(
                                    "aliases".to_string(),
                                    Value::Array(aliases.into_iter().map(Value::String).collect()),
                                );
                            }
                            set_field_union(
                                &mut master,
                                "source_doc_ids",
                                entry_field_names(slave, "source_doc_ids"),
                            );
                            set_field_union(
                                &mut master,
                                "source_chunk_ids",
                                entry_field_names(slave, "source_chunk_ids"),
                            );
                            if let (Some(slave_name), Some(master_name)) = (
                                slave.get("name").and_then(Value::as_str),
                                master.get("name").and_then(Value::as_str),
                            ) {
                                name_resolution.insert(
                                    slave_name.to_string(),
                                    Value::String(master_name.to_string()),
                                );
                            }
                        }
                        new_unmatched.push(master);
                    } else {
                        new_unmatched.push(unmatched[indices[0]].clone());
                    }
                }
                unmatched = new_unmatched;
            }
        }
    }

    // Step 4 — build the canonical map.
    let mut canonical_map: Map<String, Value> = Map::new();
    for entry in &unmatched {
        let cname = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        canonical_map.insert(cname.clone(), entry.clone());
        name_resolution
            .entry(cname.clone())
            .or_insert(Value::String(cname));
    }
    for (_, cname_value) in name_resolution.iter() {
        let Some(cname) = cname_value.as_str() else {
            continue;
        };
        if canonical_map.contains_key(cname) {
            continue;
        }
        if let Some(existing) = existing_canonical.get(cname) {
            let merged = json!({
                "name": cname,
                "type": existing.get("entity_type_kwd").and_then(Value::as_str).unwrap_or("entity"),
                "aliases": existing.get("aliases").cloned().unwrap_or(Value::Array(Vec::new())),
                "claim_count": existing.get("mention_count_int").map(|value| as_int(Some(value), 0)).unwrap_or(0),
                "source_doc_ids": existing.get("source_doc_ids").cloned().unwrap_or(Value::Array(Vec::new())),
                "source_chunk_ids": existing.get("source_chunk_ids").cloned().unwrap_or(Value::Array(Vec::new())),
            });
            canonical_map.insert(cname.to_string(), merged);
        }
    }

    // Aggregate lightweight metadata across all raw entities.
    for entry in raw_entities {
        let raw_name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let cname = name_resolution
            .get(&raw_name)
            .and_then(Value::as_str)
            .unwrap_or(raw_name.as_str())
            .to_string();
        let Some(cm) = canonical_map.get_mut(&cname) else {
            continue;
        };
        let count = entry_field_i64(cm, "claim_count") + entry_field_i64(entry, "claim_count");
        if let Some(obj) = cm.as_object_mut() {
            obj.insert("claim_count".to_string(), json!(count));
        }
        set_field_union(
            cm,
            "source_doc_ids",
            entry_field_names(entry, "source_doc_ids"),
        );
        set_field_union(
            cm,
            "source_chunk_ids",
            entry_field_names(entry, "source_chunk_ids"),
        );
        let mut aliases: BTreeSet<String> = entry_field_names(cm, "aliases").into_iter().collect();
        for alias in entry_field_names(entry, "aliases") {
            if !alias.is_empty() {
                aliases.insert(alias);
            }
        }
        if raw_name != cname {
            aliases.insert(raw_name);
        }
        aliases.remove(&cname);
        if let Some(obj) = cm.as_object_mut() {
            obj.insert(
                "aliases".to_string(),
                Value::Array(aliases.into_iter().map(Value::String).collect()),
            );
        }
    }

    let incremental_value = json!(incremental);
    for merge in &llm_merge_pairs {
        wiki_log_stats(
            "MATCH",
            "llm_merge",
            &[
                ("kb_id", json!(kb_id)),
                ("incremental", incremental_value.clone()),
                ("from", merge.get("from").cloned().unwrap_or(Value::Null)),
                ("into", merge.get("into").cloned().unwrap_or(Value::Null)),
                ("scope", merge.get("scope").cloned().unwrap_or(Value::Null)),
            ],
        );
    }
    wiki_log_stats(
        "MATCH",
        "llm_merge_summary",
        &[
            ("kb_id", json!(kb_id)),
            ("incremental", incremental_value),
            ("before", json!(raw_entities.len())),
            ("after", json!(canonical_map.len())),
            ("llm_merge_count", json!(llm_merge_pairs.len())),
        ],
    );

    (canonical_map, name_resolution)
}

/// `_search_existing_pages`: all `wiki_page` rows keyed by storage row id.
pub fn search_existing_pages(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    select_fields: &[String],
) -> Map<String, Value> {
    let mut results: Map<String, Value> = Map::new();
    let mut offset = 0usize;
    let page_size = 1000usize;
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    loop {
        let query = SearchQuery {
            select_fields: select_fields.to_vec(),
            condition: condition.clone(),
            match_expressions: Vec::new(),
            offset,
            limit: page_size,
            index_names: vec![crate::harness::knowlege_dataset_nav::index_name(tenant_id)],
            dataset_ids: vec![kb_id.to_string()],
            ..Default::default()
        };
        let response = match store.search(&query) {
            Ok(response) => response,
            Err(err) => {
                tracing::warn!(error = %err, kb = kb_id, "wiki: failed to load existing pages");
                return results;
            }
        };
        let rows = store.get_fields(&response, select_fields);
        let page_len = rows.len();
        if page_len == 0 {
            break;
        }
        for (row_id, mut row) in rows {
            row.insert("id".to_string(), Value::String(row_id));
            let row_value = Value::Object(row);
            let slug = row_scalar_string(&row_value, "slug_kwd").trim().to_string();
            if !slug.is_empty() {
                results.insert(slug, row_value);
            }
        }
        if page_len < page_size {
            break;
        }
        offset += page_size;
    }
    results
}

#[cfg(test)]
mod wiki_incremental_part4_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::HarnessChat;
    use std::collections::HashMap;
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

    struct MapEmb {
        map: HashMap<String, Vec<f32>>,
    }

    #[async_trait::async_trait]
    impl Embedder for MapEmb {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|text| self.map.get(*text).cloned().unwrap_or_default())
                .collect())
        }
    }

    fn map_emb(pairs: &[(&str, [f32; 2])]) -> MapEmb {
        MapEmb {
            map: pairs
                .iter()
                .map(|(text, vec)| (text.to_string(), vec.to_vec()))
                .collect(),
        }
    }

    #[tokio::test]
    async fn confirm_batch_parses_booleans() {
        let chat = FakeChat {
            reply: "Here you go: [true, false]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let candidates = vec![
            ("Alpha".to_string(), "A Corp".to_string()),
            ("Beta".to_string(), "B Corp".to_string()),
        ];
        let confirmed = wiki_confirm_batch(&chat, &candidates).await;
        assert_eq!(confirmed, vec![("Alpha".to_string(), "A Corp".to_string())]);
        let bad = FakeChat {
            reply: "no json".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        assert!(wiki_confirm_batch(&bad, &candidates).await.is_empty());
    }

    #[tokio::test]
    async fn match_entities_exact_and_intra_merge() {
        let store = MemoryDocStore::new();
        let mut existing: Map<String, Value> = Map::new();
        existing.insert(
            "Alpha".to_string(),
            json!({"aliases": ["A"], "entity_type_kwd": "org", "mention_count_int": 2, "source_doc_ids": ["d9"], "source_chunk_ids": ["c9"]}),
        );
        let raw_exact = vec![
            json!({"name": "A", "type": "org", "aliases": [], "claim_count": 1, "source_doc_ids": ["d1"], "source_chunk_ids": ["c1"]}),
        ];
        let (canonical, resolution) = wiki_match_entities(
            &store, None, None, "t1", "kb1", &raw_exact, &existing, false, None,
        )
        .await;
        assert_eq!(resolution["A"], json!("Alpha"));
        assert_eq!(canonical["Alpha"]["claim_count"], json!(3));

        let empty_existing: Map<String, Value> = Map::new();
        let raw_pair = vec![
            json!({"name": "Alpha2", "type": "org", "aliases": [], "claim_count": 5, "source_doc_ids": ["d2"], "source_chunk_ids": ["c2"]}),
            json!({"name": "AlphaTwo", "type": "org", "aliases": ["AT"], "claim_count": 1, "source_doc_ids": ["d3"], "source_chunk_ids": ["c3"]}),
        ];
        let embd = map_emb(&[("Alpha2", [1.0, 0.0]), ("AlphaTwo AT", [0.999, 0.001])]);
        let (canonical, resolution) = wiki_match_entities(
            &store,
            Some(&embd),
            None,
            "t1",
            "kb1",
            &raw_pair,
            &empty_existing,
            false,
            None,
        )
        .await;
        assert_eq!(resolution["AlphaTwo"], json!("Alpha2"));
        let entry = canonical.get("Alpha2").expect("alpha2");
        // Step 3 folded 5+1 into the master; Step 4 folds the raw counts
        // again (upstream double-count semantics preserved).
        assert_eq!(entry["claim_count"], json!(12));
        assert_eq!(entry["source_doc_ids"], json!(["d2", "d3"]));
        let aliases: Vec<String> = entry["aliases"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        assert!(aliases.contains(&"AT".to_string()));
        assert!(aliases.contains(&"AlphaTwo".to_string()));
        assert!(!canonical.contains_key("AlphaTwo"));
    }

    #[tokio::test]
    async fn match_entities_knn_existing_merge() {
        let store = MemoryDocStore::new();
        let doc = build_canonical_entity_doc(
            "kb1",
            "Alpha",
            "entity",
            &[],
            &[],
            1,
            Some(&[1.0, 0.0]),
            None,
        );
        store
            .insert(
                &[doc.as_object().cloned().unwrap()],
                &crate::harness::knowlege_dataset_nav::index_name("t1"),
                "kb1",
            )
            .unwrap();
        let existing = load_canonical_entities(&store, "t1", "kb1");
        let raw = vec![
            json!({"name": "Gamma", "type": "entity", "aliases": [], "claim_count": 1, "source_doc_ids": ["d1"], "source_chunk_ids": ["c1"]}),
        ];
        let embd = map_emb(&[("Gamma", [0.995, 0.005])]);
        let (canonical, resolution) = wiki_match_entities(
            &store,
            Some(&embd),
            None,
            "t1",
            "kb1",
            &raw,
            &existing,
            true,
            None,
        )
        .await;
        assert_eq!(resolution["Gamma"], json!("Alpha"));
        assert!(canonical.contains_key("Alpha"));
        assert_eq!(canonical["Alpha"]["claim_count"], json!(2));
    }

    #[test]
    fn existing_pages_keyed_by_slug() {
        let store = MemoryDocStore::new();
        let rows: Vec<DocRow> = vec![
            json!({"id": "p1", "compile_kwd": "wiki_page", "slug_kwd": "entity/a"})
                .as_object()
                .cloned()
                .unwrap(),
            json!({"id": "p2", "compile_kwd": "wiki_page", "slug_kwd": "entity/b"})
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
        let fields: Vec<String> = ["id", "slug_kwd"].iter().map(|f| f.to_string()).collect();
        let pages = search_existing_pages(&store, "t1", "kb1", &fields);
        assert_eq!(pages.len(), 2);
        assert_eq!(pages["entity/a"]["id"], json!("p1"));
        assert_eq!(pages["entity/b"]["slug_kwd"], json!("entity/b"));
    }
}

// ---------------------------------------------------------------------------
// Part 5 — graph page loading and link machinery (`_load_map_relations` ..
// `_wiki_resolve_dead_slug`).
//
// Adaptation note: character-index semantics are preserved for link scans
// (byte→char conversion); `name_slug` iteration is ordered (BTreeMap).
// ---------------------------------------------------------------------------

/// `_load_map_relations`: extracted (from, to, type) relations from MAP rows.
pub fn load_map_relations(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    excluded_doc_ids: Option<&BTreeSet<String>>,
    chunk_state: Option<&Map<String, Value>>,
) -> Vec<Value> {
    let state: Map<String, Value> = match chunk_state {
        Some(state) => state.clone(),
        None => load_active_map_state(store, tenant_id, kb_id).unwrap_or_default(),
    };
    let extracts = load_map_extracts_for_state(store, tenant_id, kb_id, &state, None);
    let mut relations: Vec<Value> = Vec::new();
    for extract in &extracts {
        if let Some(excluded) = excluded_doc_ids {
            if let Some(doc_id) = extract.get("doc_id").and_then(Value::as_str) {
                if excluded.contains(doc_id) {
                    continue;
                }
            }
        }
        if let Some(items) = extract.get("relations").and_then(Value::as_array) {
            for relation in items {
                let Some(source) = relation.get("from").and_then(Value::as_str) else {
                    continue;
                };
                let Some(target) = relation.get("to").and_then(Value::as_str) else {
                    continue;
                };
                let relation_type = relation
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("related");
                relations.push(json!({
                    "from": source,
                    "to": target,
                    "type": relation_type,
                }));
            }
        }
    }
    relations
}

fn row_scalar_string(row: &Value, field: &str) -> String {
    match row.get(field) {
        Some(Value::Array(items)) => items
            .first()
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// `_wiki_load_pages_for_graph`: compiled pages in the canvas-graph shape.
pub fn load_pages_for_graph(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    excluded_doc_ids: Option<&BTreeSet<String>>,
    chunk_state: Option<&Map<String, Value>>,
) -> Vec<Value> {
    let select_fields: Vec<String> = [
        "slug_kwd",
        "title_kwd",
        "page_type_kwd",
        "summary_with_weight",
        "md_with_weight",
        "entity_names_kwd",
        "outlinks_kwd",
        "source_chunk_ids",
        "source_doc_ids",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    let mut pages: Vec<Value> = Vec::new();
    let mut offset = 0usize;
    let page_size = 1000usize;
    loop {
        let rows = match inc_search_page(
            store,
            tenant_id,
            kb_id,
            &select_fields,
            &condition,
            offset,
            page_size,
        ) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, kb = kb_id, "wiki: failed to load pages for graph");
                return pages;
            }
        };
        if rows.is_empty() {
            break;
        }
        let page_len = rows.len();
        for row in &rows {
            let slug = row_scalar_string(row, "slug_kwd").trim().to_string();
            if slug.is_empty() {
                continue;
            }
            let mut outlinks = extract_outlinks_from_content(
                row.get("md_with_weight")
                    .and_then(Value::as_str)
                    .unwrap_or(""),
                kb_id,
            );
            if outlinks.is_empty() {
                outlinks = as_str_list(row.get("outlinks_kwd"));
            }
            let title = {
                let value = row_scalar_string(row, "title_kwd");
                if value.is_empty() {
                    slug.clone()
                } else {
                    value
                }
            };
            let page_type = {
                let value = row_scalar_string(row, "page_type_kwd");
                if value.is_empty() {
                    "concept".to_string()
                } else {
                    value
                }
            };
            pages.push(json!({
                "slug": slug,
                "title": title,
                "summary": row.get("summary_with_weight").and_then(Value::as_str).unwrap_or(""),
                "page_type": page_type,
                "entity_names": as_str_list(row.get("entity_names_kwd")),
                "outlinks": outlinks,
                "source_chunk_ids": as_str_list(row.get("source_chunk_ids")),
                "source_doc_ids": as_str_list(row.get("source_doc_ids")),
            }));
        }
        if page_len < page_size {
            break;
        }
        offset += page_size;
    }

    // Fallback edges from grounded MAP relations.
    if !pages.is_empty() {
        let mut name_to_slug: BTreeMap<String, String> = BTreeMap::new();
        for page in &pages {
            let slugs = page.get("slug").and_then(Value::as_str).unwrap_or("");
            let mut names: Vec<String> =
                vec![slugs.rsplit('/').next().unwrap_or(slugs).to_string()];
            if let Some(title) = page.get("title").and_then(Value::as_str) {
                names.push(title.to_string());
            }
            if let Some(entity_names) = page.get("entity_names").and_then(Value::as_array) {
                names.extend(
                    entity_names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string),
                );
            }
            for name in names {
                let name = name.trim();
                if !name.is_empty() {
                    name_to_slug
                        .entry(name.to_string())
                        .or_insert_with(|| slugs.to_string());
                }
            }
        }
        let map_relations =
            load_map_relations(store, tenant_id, kb_id, excluded_doc_ids, chunk_state);
        let mut slug_index: BTreeMap<String, usize> = BTreeMap::new();
        for (idx, page) in pages.iter().enumerate() {
            if let Some(slug) = page.get("slug").and_then(Value::as_str) {
                slug_index.insert(slug.to_string(), idx);
            }
        }
        for relation in &map_relations {
            let source_name = relation
                .get("from")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let target_name = relation
                .get("to")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let Some(source) = name_to_slug.get(source_name) else {
                continue;
            };
            let Some(target) = name_to_slug.get(target_name) else {
                continue;
            };
            if source == target {
                continue;
            }
            let Some(idx) = slug_index.get(source) else {
                continue;
            };
            if let Some(obj) = pages[*idx].as_object_mut() {
                let outlinks = obj
                    .entry("outlinks".to_string())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Some(items) = outlinks.as_array_mut() {
                    if !items
                        .iter()
                        .any(|item| item.as_str() == Some(target.as_str()))
                    {
                        items.push(Value::String(target.clone()));
                    }
                }
            }
        }
    }
    pages
}

fn wikilink_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\]]+)\]\]").expect("wikilink re"))
}

/// `_wiki_extract_outlinks_from_content`: unique internal link targets.
pub fn extract_outlinks_from_content(content: &str, kb_id: &str) -> Vec<String> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut outlinks: Vec<String> = Vec::new();
    for caps in wikilink_re().captures_iter(content) {
        let link = caps
            .get(1)
            .map(|m| m.as_str().split('|').next().unwrap_or("").trim())
            .unwrap_or("");
        if !link.is_empty() && seen.insert(link.to_string()) {
            outlinks.push(link.to_string());
        }
    }
    if !kb_id.is_empty() {
        let pattern = format!(r"\]\(artifact/{}/([^)]+)\)", regex::escape(kb_id));
        if let Ok(re) = Regex::new(&pattern) {
            for caps in re.captures_iter(content) {
                let slug = caps
                    .get(1)
                    .map(|m| m.as_str().split('|').next().unwrap_or("").trim())
                    .unwrap_or("");
                if !slug.is_empty() && seen.insert(slug.to_string()) {
                    outlinks.push(slug.to_string());
                }
            }
        }
    }
    outlinks
}

fn protected_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\[\[[^\]\n]+\]\]|\[[^\]\n]*\]\([^)\n]+\)").expect("protected link re")
    })
}

fn byte_to_char_index(content: &str, byte_index: usize) -> usize {
    content[..byte_index].chars().count()
}

/// `_wiki_find_unlinked_mention`: first occurrence outside link markup
/// (character index, -1 when absent).
pub fn find_unlinked_mention(content: &str, name: &str) -> i64 {
    if content.is_empty() || name.is_empty() {
        return -1;
    }
    let mut spans: Vec<(usize, usize)> = protected_link_re()
        .find_iter(content)
        .map(|m| {
            (
                byte_to_char_index(content, m.start()),
                byte_to_char_index(content, m.end()),
            )
        })
        .collect();
    if let Some(raw_open_byte) = content.rfind("[[") {
        let after_open = &content[raw_open_byte + 2..];
        if after_open.find("]]").is_none() {
            let open_char = byte_to_char_index(content, raw_open_byte);
            spans.push((open_char, content.chars().count()));
        }
    }
    let chars: Vec<char> = content.chars().collect();
    let name_chars: Vec<char> = name.chars().collect();
    let name_len = name_chars.len();
    let mut start = 0usize;
    while start + name_len <= chars.len() {
        let idx = match (start..=chars.len() - name_len)
            .find(|pos| chars[*pos..*pos + name_len] == name_chars[..])
        {
            Some(pos) => pos,
            None => return -1,
        };
        let end = idx + name_len;
        let overlaps = spans
            .iter()
            .any(|(protect_start, protect_end)| idx < *protect_end && end > *protect_start);
        if !overlaps {
            return idx as i64;
        }
        start = idx + 1;
    }
    -1
}

fn wiki_pipe_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\[\]|]+?)\|([^\[\]]+?)\]\]").expect("pipe link re"))
}

fn wiki_simple_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[\[([^\[\]|]+?)\]\]").expect("simple link re"))
}

/// `_wiki_render_links`: `[[slug]]` / `[[slug|text]]` → navigable markdown.
pub fn render_links(content: &str, kb_id: &str, valid_slugs: &BTreeSet<String>) -> String {
    if content.is_empty() {
        return content.to_string();
    }
    let kb = kb_id.to_string();
    let rendered = wiki_pipe_link_re().replace_all(content, |caps: &regex::Captures| {
        let slug = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let text = caps.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        if !valid_slugs.contains(slug) {
            return text.to_string();
        }
        format!("[{text}](artifact/{kb}/{slug})")
    });
    let rendered = wiki_simple_link_re().replace_all(&rendered, |caps: &regex::Captures| {
        let slug = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if !valid_slugs.contains(slug) {
            return slug.to_string();
        }
        let label = if slug.contains('/') {
            slug.rsplit('/').next().unwrap_or(slug)
        } else {
            slug
        };
        format!("[{label}](artifact/{kb}/{slug})")
    });
    rendered.to_string()
}

fn norm_dead_slug(text: &str) -> String {
    let lowered = text.trim().to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut prev_dash = false;
    for ch in lowered.chars() {
        if ch == '-' || ch == '_' {
            if !prev_dash {
                out.push('-');
            }
            prev_dash = true;
        } else {
            out.push(ch);
            prev_dash = false;
        }
    }
    out
}

fn bigrams(text: &str) -> BTreeSet<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut set = BTreeSet::new();
    if chars.len() >= 2 {
        for pair in chars.windows(2) {
            set.insert(pair.iter().collect());
        }
    }
    set
}

/// `_wiki_resolve_dead_slug`: fuzzy resolution of a dead wikilink.
pub fn resolve_dead_slug(
    link: &str,
    valid_ids: &BTreeSet<String>,
    name_slug: &BTreeMap<String, String>,
) -> Option<String> {
    if link.is_empty() {
        return None;
    }
    let plain = if link.contains('/') {
        link.rsplit('/').next().unwrap_or(link)
    } else {
        link
    };
    let link_norm = norm_dead_slug(link);
    let plain_norm = norm_dead_slug(plain);

    if valid_ids.contains(link) {
        return Some(link.to_string());
    }
    if valid_ids.contains(&link_norm) {
        return Some(link_norm);
    }
    if let Some(value) = name_slug.get(plain) {
        return Some(value.clone());
    }
    for (key, value) in name_slug {
        if norm_dead_slug(key) == plain_norm {
            return Some(value.clone());
        }
    }

    let plain_tokens: BTreeSet<String> = plain_norm.split('-').map(str::to_string).collect();
    let plain_bigrams = bigrams(&plain_norm);
    let mut best: Option<(f64, String)> = None;
    for (candidate_name, candidate_slug) in name_slug {
        let candidate_norm = norm_dead_slug(candidate_name);
        let candidate_tokens: BTreeSet<String> =
            candidate_norm.split('-').map(str::to_string).collect();
        if plain_tokens.is_disjoint(&candidate_tokens) {
            continue;
        }
        let candidate_bigrams = bigrams(&candidate_norm);
        let union: BTreeSet<&String> = plain_bigrams.union(&candidate_bigrams).collect();
        if union.is_empty() {
            continue;
        }
        let intersection = plain_bigrams.intersection(&candidate_bigrams).count();
        let score = intersection as f64 / union.len() as f64;
        if best
            .as_ref()
            .map(|(best_score, _)| score > *best_score)
            .unwrap_or(true)
        {
            best = Some((score, candidate_slug.clone()));
        }
    }
    match best {
        Some((score, slug)) if score >= 0.5 => Some(slug),
        _ => None,
    }
}

#[cfg(test)]
mod wiki_incremental_part5_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::knowlege_wiki::{build_resume_doc, commit_active_map_state};

    #[test]
    fn outlink_extraction_forms() {
        let content = "See [[entity/alpha]] and [[concept/beta|Beta]] and [x](artifact/kb1/entity/gamma) and [[entity/alpha]] again";
        let links = extract_outlinks_from_content(content, "kb1");
        assert_eq!(
            links,
            vec![
                "entity/alpha".to_string(),
                "concept/beta".to_string(),
                "entity/gamma".to_string()
            ]
        );
        assert!(extract_outlinks_from_content("", "kb1").is_empty());
    }

    #[test]
    fn unlinked_mention_skips_links() {
        let content = "Intro [[entity/alpha]] then Alpha here and [[unclosed Alpha";
        assert_eq!(find_unlinked_mention(content, "Alpha"), 28);
        assert_eq!(find_unlinked_mention("only [[Alpha]] here", "Alpha"), -1);
        assert_eq!(find_unlinked_mention("nothing", "Zeta"), -1);
        assert_eq!(find_unlinked_mention("", "Zeta"), -1);
    }

    #[test]
    fn render_links_gates_and_formats() {
        let valid: BTreeSet<String> = ["entity/alpha".to_string()].into_iter().collect();
        let rendered = render_links(
            "[[entity/alpha]] [[entity/ghost]] [[entity/alpha|A]]",
            "kb1",
            &valid,
        );
        assert!(rendered.contains("[alpha](artifact/kb1/entity/alpha)"));
        assert!(rendered.contains("entity/ghost"));
        assert!(rendered.contains("[A](artifact/kb1/entity/alpha)"));
    }

    #[test]
    fn dead_slug_resolution_layers() {
        let valid: BTreeSet<String> = ["entity/smart-phone".to_string()].into_iter().collect();
        let mut name_slug: BTreeMap<String, String> = BTreeMap::new();
        name_slug.insert(
            "Smartphone Industry".to_string(),
            "concept/smartphone-industry".to_string(),
        );
        name_slug.insert("smart_phone".to_string(), "entity/smart-phone".to_string());
        assert_eq!(
            resolve_dead_slug("entity/smart-phone", &valid, &name_slug),
            Some("entity/smart-phone".to_string())
        );
        assert_eq!(
            resolve_dead_slug("entity/smart_phone", &valid, &name_slug),
            Some("entity/smart-phone".to_string())
        );
        assert_eq!(
            resolve_dead_slug("Smartphone Industry", &valid, &name_slug),
            Some("concept/smartphone-industry".to_string())
        );
        assert_eq!(resolve_dead_slug("unrelated", &valid, &name_slug), None);
    }

    #[test]
    fn pages_for_graph_content_and_relation_edges() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        // MAP rows with a grounded relation Alpha -> Beta.
        let extract = json!({
            "entities": [],
            "concepts": [],
            "claims": [],
            "relations": [{"from": "Alpha", "to": "Beta", "type": "uses"}],
            "topics": []
        });
        let row = build_resume_doc("c1", "d1", &extract, "h1");
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
        let mut state = Map::new();
        state.insert("c1".to_string(), json!({"doc_id": "d1", "hash": "h1"}));
        commit_active_map_state(&store, "t1", "kb1", &state).unwrap();
        // wiki_page rows: alpha has a content link, beta has none.
        let pages: Vec<DocRow> = vec![
            json!({"id": "p1", "compile_kwd": "wiki_page", "slug_kwd": "entity/alpha", "title_kwd": "Alpha", "page_type_kwd": "entity", "md_with_weight": "see [[concept/beta]]", "entity_names_kwd": ["Alpha"]})
                .as_object().cloned().unwrap(),
            json!({"id": "p2", "compile_kwd": "wiki_page", "slug_kwd": "concept/beta", "title_kwd": "Beta", "page_type_kwd": "concept", "md_with_weight": "no links here", "entity_names_kwd": ["Beta"]})
                .as_object().cloned().unwrap(),
        ];
        store.insert(&pages, &index, "kb1").unwrap();
        let graph = load_pages_for_graph(&store, "t1", "kb1", None, None);
        assert_eq!(graph.len(), 2);
        let alpha = graph
            .iter()
            .find(|p| p["slug"] == json!("entity/alpha"))
            .unwrap();
        assert_eq!(alpha["outlinks"], json!(["concept/beta"]));
        assert_eq!(alpha["title"], json!("Alpha"));
        assert_eq!(alpha["page_type"], json!("entity"));
        // Relation fallback only adds an edge when content produced none for
        // that page; beta already has no link -> it gains the... wait, the
        // relation source is Alpha (has content link) so no extra edge on beta.
        let beta = graph
            .iter()
            .find(|p| p["slug"] == json!("concept/beta"))
            .unwrap();
        assert!(beta["outlinks"].as_array().unwrap().is_empty());
        // Excluding the relation doc removes fallback edges entirely.
        let mut excluded: BTreeSet<String> = BTreeSet::new();
        excluded.insert("d1".to_string());
        let graph2 = load_pages_for_graph(&store, "t1", "kb1", Some(&excluded), None);
        let alpha2 = graph2
            .iter()
            .find(|p| p["slug"] == json!("entity/alpha"))
            .unwrap();
        assert_eq!(alpha2["outlinks"], json!(["concept/beta"]));
    }

    #[test]
    fn pages_for_graph_relation_fallback_adds_missing_edge() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let extract = json!({
            "entities": [], "concepts": [], "claims": [],
            "relations": [{"from": "Beta", "to": "Alpha", "type": "used_by"}],
            "topics": []
        });
        let row = build_resume_doc("c1", "d1", &extract, "h1");
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
        let mut state = Map::new();
        state.insert("c1".to_string(), json!({"doc_id": "d1", "hash": "h1"}));
        commit_active_map_state(&store, "t1", "kb1", &state).unwrap();
        let pages: Vec<DocRow> = vec![
            json!({"id": "p1", "compile_kwd": "wiki_page", "slug_kwd": "entity/alpha", "title_kwd": "Alpha", "page_type_kwd": "entity", "md_with_weight": "", "entity_names_kwd": ["Alpha"]})
                .as_object().cloned().unwrap(),
            json!({"id": "p2", "compile_kwd": "wiki_page", "slug_kwd": "concept/beta", "title_kwd": "Beta", "page_type_kwd": "concept", "md_with_weight": "", "entity_names_kwd": ["Beta"]})
                .as_object().cloned().unwrap(),
        ];
        store.insert(&pages, &index, "kb1").unwrap();
        let graph = load_pages_for_graph(&store, "t1", "kb1", None, None);
        let beta = graph
            .iter()
            .find(|p| p["slug"] == json!("concept/beta"))
            .unwrap();
        assert_eq!(beta["outlinks"], json!(["entity/alpha"]));
    }
}

// ---------------------------------------------------------------------------
// Part 6 — topic aggregation and ranking (`_wiki_topics_for_docs` ..
// `_wiki_decide_concept_pages`).
//
// Adaptation note: numpy normalization/dot products become plain Rust; the
// no-vector paths mirror upstream (topics without vectors are skipped when
// ranking).
// ---------------------------------------------------------------------------

fn fallback_topic_key() -> String {
    normalize_key(WIKI_TOPIC_FALLBACK)
}

/// `_wiki_topics_for_docs`: deduped topics for a set of documents.
pub fn wiki_topics_for_docs(
    doc_ids: &[String],
    doc_topics: Option<&Map<String, Value>>,
    topic_pool: Option<&Map<String, Value>>,
) -> Vec<String> {
    let mut topics: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let fallback_key = fallback_topic_key();
    for doc_id in doc_ids {
        if let Some(values) = doc_topics
            .and_then(|map| map.get(doc_id))
            .and_then(Value::as_array)
        {
            for topic in values {
                let Some(topic) = topic.as_str() else {
                    continue;
                };
                let topic = topic.trim();
                let key = normalize_key(topic);
                if topic.is_empty() || key == fallback_key || seen.contains(&key) {
                    continue;
                }
                seen.insert(key);
                topics.push(topic.to_string());
            }
        }
    }
    if let Some(pool) = topic_pool {
        for value in pool.values() {
            let Some(topic) = value.as_str() else {
                continue;
            };
            let key = normalize_key(topic);
            if !topic.is_empty() && !seen.contains(&key) {
                seen.insert(key);
                topics.push(topic.to_string());
            }
        }
    }
    topics
}

/// `_wiki_prepare_topic_embeddings`: topic → embedding vector.
pub async fn wiki_prepare_topic_embeddings(
    embd: Option<&dyn Embedder>,
    doc_topics: &Map<String, Value>,
    extra_topics: Option<&[String]>,
) -> Map<String, Value> {
    let fallback_key = fallback_topic_key();
    let mut topics: BTreeSet<String> = BTreeSet::new();
    for values in doc_topics.values() {
        if let Some(items) = values.as_array() {
            for topic in items {
                if let Some(topic) = topic.as_str() {
                    let trimmed = topic.trim();
                    if !trimmed.is_empty() && normalize_key(trimmed) != fallback_key {
                        topics.insert(trimmed.to_string());
                    }
                }
            }
        }
    }
    if let Some(extra) = extra_topics {
        for topic in extra {
            let trimmed = topic.trim();
            if !trimmed.is_empty() && normalize_key(trimmed) != fallback_key {
                topics.insert(trimmed.to_string());
            }
        }
    }
    let mut sorted: Vec<String> = topics.into_iter().collect();
    sorted.sort_by_key(|value| (value.to_lowercase(), value.clone()));
    let Some(embd) = embd else {
        return Map::new();
    };
    if sorted.is_empty() {
        return Map::new();
    }
    let refs: Vec<&str> = sorted.iter().map(String::as_str).collect();
    let embeddings = match embd.embed(&refs).await {
        Ok(embeddings) if embeddings.len() == sorted.len() => embeddings,
        _ => return Map::new(),
    };
    let mut out: Map<String, Value> = Map::new();
    for (topic, vector) in sorted.into_iter().zip(embeddings.into_iter()) {
        out.insert(topic, json!(vector));
    }
    out
}

/// `_wiki_topic_query_text`.
pub fn wiki_topic_query_text(
    page_title: &str,
    claims: Option<&[Value]>,
    source_chunks: Option<&[Value]>,
    existing_page: Option<&Value>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !page_title.is_empty() {
        parts.push(format!("title={page_title}"));
    }
    if let Some(page) = existing_page {
        let summary = page
            .get("summary_with_weight")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !summary.is_empty() {
            parts.push(format!("summary={summary}"));
        }
    }
    let evidence: Vec<String> = claims
        .unwrap_or(&[])
        .iter()
        .take(8)
        .filter_map(|claim| {
            ["statement", "text"]
                .iter()
                .find_map(|key| claim.get(*key))
                .and_then(truthy_str)
        })
        .collect();
    if !evidence.is_empty() {
        parts.push(format!("evidence={}", evidence.join(" | ")));
    }
    let chunk_text: Vec<String> = source_chunks
        .unwrap_or(&[])
        .iter()
        .take(4)
        .filter_map(|chunk| {
            ["text", "content_with_weight"]
                .iter()
                .find_map(|key| chunk.get(*key))
                .and_then(truthy_str)
                .map(|text| truncate_chars_p6(&text, 500))
        })
        .collect();
    if !chunk_text.is_empty() {
        parts.push(format!("source={}", chunk_text.join(" | ")));
    }
    parts.join("; ")
}

fn truncate_chars_p6(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn vector_of_value(value: Option<&Value>) -> Vec<f32> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_f64().map(|number| number as f32))
                .collect()
        })
        .unwrap_or_default()
}

/// `_wiki_rank_topic_candidates`: embedding recall + ordered TOP-50.
pub async fn wiki_rank_topic_candidates(
    embd: Option<&dyn Embedder>,
    page_title: &str,
    claims: Option<&[Value]>,
    source_chunks: Option<&[Value]>,
    existing_page: Option<&Value>,
    topic_candidates: Option<&[String]>,
    topic_embeddings: Option<&mut Map<String, Value>>,
) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for topic in topic_candidates.unwrap_or(&[]) {
        let topic = topic.trim();
        let key = normalize_key(topic);
        if !topic.is_empty() && !seen.contains(&key) {
            seen.insert(key);
            candidates.push(topic.to_string());
        }
    }
    if candidates.len() <= 1 || embd.is_none() {
        return candidates
            .into_iter()
            .take(WIKI_PAGE_TOPIC_CANDIDATE_LIMIT)
            .collect();
    }
    let embd = embd.unwrap();
    let query_text = wiki_topic_query_text(page_title, claims, source_chunks, existing_page);
    let query_vector = match embd.embed(&[query_text.as_str()]).await {
        Ok(vectors) if !vectors.is_empty() => vectors.into_iter().next().unwrap_or_default(),
        _ => {
            return candidates
                .into_iter()
                .take(WIKI_PAGE_TOPIC_CANDIDATE_LIMIT)
                .collect();
        }
    };
    let query_norm = (query_vector
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>())
    .sqrt();
    if query_norm <= 0.0 {
        return candidates
            .into_iter()
            .take(WIKI_PAGE_TOPIC_CANDIDATE_LIMIT)
            .collect();
    }
    let query_normalized: Vec<f32> = query_vector
        .iter()
        .map(|x| (*x as f64 / query_norm) as f32)
        .collect();

    let mut local: Map<String, Value> = topic_embeddings
        .as_ref()
        .map(|map| (**map).clone())
        .unwrap_or_default();
    let missing: Vec<String> = candidates
        .iter()
        .filter(|topic| !local.contains_key(*topic))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let refs: Vec<&str> = missing.iter().map(String::as_str).collect();
        match embd.embed(&refs).await {
            Ok(vectors) if vectors.len() == missing.len() => {
                let pairs: Vec<(String, Value)> = missing
                    .iter()
                    .cloned()
                    .zip(vectors.into_iter().map(|vector| json!(vector)))
                    .collect();
                for (topic, vector) in &pairs {
                    local.insert(topic.clone(), vector.clone());
                }
                if let Some(caller) = topic_embeddings {
                    for (topic, vector) in pairs {
                        caller.insert(topic, vector);
                    }
                }
            }
            _ => {}
        }
    }

    let mut ranked: Vec<(f64, String)> = Vec::new();
    for topic in &candidates {
        let vector = vector_of_value(local.get(topic));
        if vector.is_empty() {
            continue;
        }
        let norm = (vector
            .iter()
            .map(|x| (*x as f64) * (*x as f64))
            .sum::<f64>())
        .sqrt();
        let score = if norm > 0.0 {
            query_normalized
                .iter()
                .zip(vector.iter())
                .map(|(q, v)| (*q as f64) * (*v as f64 / norm))
                .sum::<f64>()
        } else {
            -1.0
        };
        ranked.push((score, topic.clone()));
    }
    ranked.sort_by(|(left_score, left_topic), (right_score, right_topic)| {
        right_score
            .partial_cmp(left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left_topic.cmp(right_topic))
    });
    ranked
        .into_iter()
        .take(WIKI_PAGE_TOPIC_CANDIDATE_LIMIT)
        .map(|(_, topic)| topic)
        .collect()
}

/// `_wiki_decide_concept_pages`: every concept becomes its own page.
pub fn wiki_decide_concept_pages(all_concepts: &[Value]) -> Vec<Value> {
    let mut pages: Vec<Value> = Vec::new();
    for concept in all_concepts {
        let term = concept.get("term").and_then(Value::as_str).unwrap_or("");
        let claims: Vec<Value> = concept
            .get("claims")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut source_docs: BTreeSet<String> = BTreeSet::new();
        for claim in &claims {
            if let Some(doc_id) = claim.get("source_doc_id").and_then(Value::as_str) {
                if !doc_id.is_empty() {
                    source_docs.insert(doc_id.to_string());
                }
            }
        }
        pages.push(json!({
            "page_id": derive_page_id(term, "concept"),
            "page_title": term,
            "concept": concept,
            "claims": claims,
            "source_doc_ids": source_docs.into_iter().collect::<Vec<String>>(),
        }));
    }
    pages
}

#[cfg(test)]
mod wiki_incremental_part6_tests {
    use super::*;
    use crate::embed::Embedder;

    struct MapEmb {
        map: BTreeMap<String, Vec<f32>>,
    }

    #[async_trait::async_trait]
    impl Embedder for MapEmb {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts
                .iter()
                .map(|text| self.map.get(*text).cloned().unwrap_or_default())
                .collect())
        }
    }

    #[test]
    fn topics_for_docs_dedup_and_pool() {
        let mut doc_topics: Map<String, Value> = Map::new();
        doc_topics.insert(
            "d1".to_string(),
            json!(["Alpha", " alpha ", "General", 5, ""]),
        );
        doc_topics.insert("d2".to_string(), json!(["Beta", "Alpha"]));
        let mut pool: Map<String, Value> = Map::new();
        pool.insert("t1".to_string(), json!("Gamma"));
        pool.insert("t2".to_string(), json!("alpha"));
        let topics = wiki_topics_for_docs(
            &["d1".to_string(), "d2".to_string()],
            Some(&doc_topics),
            Some(&pool),
        );
        assert_eq!(
            topics,
            vec!["Alpha".to_string(), "Beta".to_string(), "Gamma".to_string()]
        );
    }

    #[tokio::test]
    async fn prepare_topic_embeddings_filters_and_sorts() {
        let mut doc_topics: Map<String, Value> = Map::new();
        doc_topics.insert("d1".to_string(), json!(["Beta", "alpha", "General"]));
        doc_topics.insert("d2".to_string(), json!(["Gamma", " alpha "]));
        let embd = MapEmb {
            map: [
                ("alpha".to_string(), vec![1.0, 0.0]),
                ("Beta".to_string(), vec![0.0, 1.0]),
                ("Gamma".to_string(), vec![0.5, 0.5]),
                ("Delta".to_string(), vec![0.2, 0.8]),
            ]
            .into_iter()
            .collect(),
        };
        let extra = vec!["Delta".to_string()];
        let embeddings =
            wiki_prepare_topic_embeddings(Some(&embd), &doc_topics, Some(&extra)).await;
        assert_eq!(embeddings.len(), 4);
        assert_eq!(embeddings["alpha"], json!([1.0, 0.0]));
        assert!(!embeddings.contains_key("General"));
        let delta = embeddings["Delta"].as_array().unwrap();
        assert!((delta[0].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert!((delta[1].as_f64().unwrap() - 0.8).abs() < 1e-6);
        assert!(
            wiki_prepare_topic_embeddings(None, &doc_topics, None)
                .await
                .is_empty()
        );
    }

    #[test]
    fn topic_query_text_composition() {
        let claims = vec![
            json!({"statement": "s1"}),
            json!({"text": "s2"}),
            json!({"nope": 1}),
        ];
        let chunks = vec![json!({"text": "chunk text"}), json!({"nope": 1})];
        let existing = json!({"summary_with_weight": "sum"});
        let text = wiki_topic_query_text("Alpha", Some(&claims), Some(&chunks), Some(&existing));
        assert_eq!(
            text,
            "title=Alpha; summary=sum; evidence=s1 | s2; source=chunk text"
        );
        assert_eq!(wiki_topic_query_text("", None, None, None), "");
    }

    #[tokio::test]
    async fn rank_topic_candidates_orders_by_similarity() {
        let embd = MapEmb {
            map: [
                ("title=Alpha".to_string(), vec![1.0, 0.0]),
                ("alpha".to_string(), vec![1.0, 0.0]),
                ("beta".to_string(), vec![0.0, 1.0]),
                ("gamma".to_string(), vec![0.7, 0.7]),
            ]
            .into_iter()
            .collect(),
        };
        let candidates = vec![
            "beta".to_string(),
            "alpha".to_string(),
            "alpha".to_string(),
            "gamma".to_string(),
        ];
        let mut embeddings: Map<String, Value> = Map::new();
        embeddings.insert("alpha".to_string(), json!([1.0, 0.0]));
        let ranked = wiki_rank_topic_candidates(
            Some(&embd),
            "Alpha",
            None,
            None,
            None,
            Some(&candidates),
            Some(&mut embeddings),
        )
        .await;
        assert_eq!(
            ranked,
            vec!["alpha".to_string(), "gamma".to_string(), "beta".to_string()]
        );
        assert!(embeddings.contains_key("beta"));
        assert!(embeddings.contains_key("gamma"));
        let single = wiki_rank_topic_candidates(
            Some(&embd),
            "Alpha",
            None,
            None,
            None,
            Some(&["solo".to_string()]),
            None,
        )
        .await;
        assert_eq!(single, vec!["solo".to_string()]);
    }

    #[test]
    fn decide_concept_pages_shape() {
        let concepts = vec![json!({
            "term": "Smartphone Industry",
            "claims": [
                {"statement": "s", "source_doc_id": "d1"},
                {"statement": "s2", "source_doc_id": "d1"},
                {"statement": "s3", "source_doc_id": "d2"}
            ]
        })];
        let pages = wiki_decide_concept_pages(&concepts);
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0]["page_id"], json!("concept/smartphone-industry"));
        assert_eq!(pages[0]["page_title"], json!("Smartphone Industry"));
        assert_eq!(pages[0]["source_doc_ids"], json!(["d1", "d2"]));
    }
}

// ---------------------------------------------------------------------------
// Part 7 — per-entity REDUCE and doc-page-source tracking
// (`_wiki_reduce_entity` .. `_wiki_delete_doc_page_source`).
//
// Adaptation note: the pure reduce functions are synchronous here (upstream
// gathers coroutines without side effects); the doc-page-source update keeps
// the stable row id as its update identity.
// ---------------------------------------------------------------------------

fn normalize_entity_type(entity_type: &Value) -> String {
    let first = match entity_type {
        Value::Array(items) => items
            .first()
            .cloned()
            .unwrap_or(Value::String(String::new())),
        other => other.clone(),
    };
    let text = first.as_str().map(str::trim).unwrap_or("").to_string();
    if text.is_empty() {
        "entity".to_string()
    } else {
        text
    }
}

fn claim_text_of(claim: &Value) -> String {
    ["statement", "text"]
        .iter()
        .find_map(|key| claim.get(*key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn claim_doc_of(claim: &Value) -> Option<String> {
    claim
        .get("source_doc_id")
        .and_then(Value::as_str)
        .filter(|doc| !doc.is_empty())
        .map(str::to_string)
}

/// `_wiki_reduce_entity`: additions/retractions vs the existing page.
#[allow(clippy::too_many_arguments)]
pub fn wiki_reduce_entity(
    entity_name: &str,
    new_claims: &[Value],
    existing_page: Option<&Value>,
    deleted_doc_ids: &BTreeSet<String>,
    invalidated_chunk_ids: Option<&BTreeSet<String>>,
    entity_type: &Value,
    aliases: Option<&[String]>,
    source_doc_ids: Option<&[String]>,
    source_chunk_ids: Option<&[String]>,
) -> Value {
    let entity_type = normalize_entity_type(entity_type);
    let aliases_value: Value = Value::Array(
        aliases
            .unwrap_or(&[])
            .iter()
            .cloned()
            .map(Value::String)
            .collect(),
    );
    let sorted_set = |values: &[String]| -> Vec<Value> {
        let set: BTreeSet<String> = values.iter().cloned().collect();
        set.into_iter().map(Value::String).collect()
    };

    let Some(existing_page) = existing_page else {
        let has_chunk_evidence = source_chunk_ids.map(|ids| !ids.is_empty()).unwrap_or(false);
        if new_claims.is_empty() && !has_chunk_evidence {
            return json!({
                "action": "noop",
                "entity_name": entity_name,
                "entity_type": entity_type,
                "aliases": aliases_value,
                "additions": [],
                "retractions": [],
                "retained_source_doc_ids": [],
                "has_delta": false,
            });
        }
        let mut retained_docs: BTreeSet<String> =
            source_doc_ids.unwrap_or(&[]).iter().cloned().collect();
        for claim in new_claims {
            if let Some(doc) = claim_doc_of(claim) {
                retained_docs.insert(doc);
            }
        }
        return json!({
            "action": "create",
            "entity_name": entity_name,
            "entity_type": entity_type,
            "aliases": aliases_value,
            "additions": new_claims,
            "source_chunk_ids": sorted_set(source_chunk_ids.unwrap_or(&[])),
            "retained_source_doc_ids": retained_docs.into_iter().collect::<Vec<String>>(),
            "has_delta": true,
        });
    };

    let existing_claims: Vec<Value> = match existing_page.get("claims") {
        Some(Value::String(text)) if !text.is_empty() => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default(),
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    let existing_claims: Vec<Value> = existing_claims
        .into_iter()
        .filter(Value::is_object)
        .collect();
    let invalidated_empty: BTreeSet<String> = BTreeSet::new();
    let invalidated = invalidated_chunk_ids.unwrap_or(&invalidated_empty);
    let existing_chunk_ids: BTreeSet<String> = as_str_list(existing_page.get("source_chunk_ids"))
        .into_iter()
        .collect();
    let all_page_evidence_invalidated = !existing_chunk_ids.is_empty()
        && existing_chunk_ids
            .iter()
            .all(|chunk| invalidated.contains(chunk));

    let is_retraction = |claim: &Value| -> bool {
        let deleted = claim_doc_of(claim)
            .map(|doc| deleted_doc_ids.contains(&doc))
            .unwrap_or(false);
        let claim_chunks = wiki_claim_chunk_ids(claim);
        let invalidated_hit = claim_chunks.iter().any(|chunk| invalidated.contains(chunk));
        let orphaned = all_page_evidence_invalidated && claim_chunks.is_empty();
        deleted || invalidated_hit || orphaned
    };
    let retractions: Vec<Value> = existing_claims
        .iter()
        .filter(|claim| is_retraction(claim))
        .cloned()
        .collect();
    let retained_claims: Vec<Value> = existing_claims
        .iter()
        .filter(|claim| !is_retraction(claim))
        .cloned()
        .collect();

    let retained_texts: BTreeSet<String> = retained_claims.iter().map(claim_text_of).collect();
    let additions: Vec<Value> = new_claims
        .iter()
        .filter(|claim| !retained_texts.contains(&claim_text_of(claim)))
        .cloned()
        .collect();

    let mut all_doc_ids: BTreeSet<String> = BTreeSet::new();
    for claim in &retained_claims {
        if let Some(doc) = claim_doc_of(claim) {
            all_doc_ids.insert(doc);
        }
    }
    for claim in &additions {
        if let Some(doc) = claim_doc_of(claim) {
            all_doc_ids.insert(doc);
        }
    }
    for doc in source_doc_ids.unwrap_or(&[]) {
        if !deleted_doc_ids.contains(doc) {
            all_doc_ids.insert(doc.clone());
        }
    }

    let current_chunk_ids: BTreeSet<String> = match source_chunk_ids {
        Some(ids) if !ids.is_empty() => ids.iter().cloned().collect(),
        _ => existing_chunk_ids
            .iter()
            .filter(|chunk| !invalidated.contains(*chunk))
            .cloned()
            .collect(),
    };
    let evidence_changed = current_chunk_ids != existing_chunk_ids;
    let current_chunk_values: Vec<Value> = current_chunk_ids
        .iter()
        .cloned()
        .map(Value::String)
        .collect();

    if all_doc_ids.is_empty() {
        return json!({
            "action": "delete",
            "entity_name": entity_name,
            "entity_type": entity_type,
            "aliases": aliases_value,
            "retractions": existing_claims,
            "source_chunk_ids": current_chunk_values,
            "has_delta": true,
        });
    }
    if !additions.is_empty() || !retractions.is_empty() || evidence_changed {
        return json!({
            "action": "update",
            "entity_name": entity_name,
            "entity_type": entity_type,
            "aliases": aliases_value,
            "additions": additions,
            "retractions": retractions,
            "source_chunk_ids": current_chunk_values,
            "retained_source_doc_ids": all_doc_ids.into_iter().collect::<Vec<String>>(),
            "has_delta": true,
        });
    }
    json!({
        "action": "noop",
        "entity_name": entity_name,
        "entity_type": entity_type,
        "aliases": aliases_value,
        "source_chunk_ids": current_chunk_values,
        "retained_source_doc_ids": all_doc_ids.into_iter().collect::<Vec<String>>(),
        "has_delta": false,
    })
}

fn names_from_value(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `_wiki_reduce_batch`: per-entity REDUCE over affected canonical names.
#[allow(clippy::too_many_arguments)]
pub fn wiki_reduce_batch(
    affected_names: &BTreeSet<String>,
    existing_pages: &Map<String, Value>,
    deleted_doc_ids: &BTreeSet<String>,
    invalidated_chunk_ids: Option<&BTreeSet<String>>,
    canonical_claims: Option<&Map<String, Value>>,
    canonical_map: Option<&Map<String, Value>>,
    name_resolution: Option<&Map<String, Value>>,
    map_results: Option<&[Value]>,
) -> Vec<Value> {
    let mut name_to_page: BTreeMap<String, Value> = BTreeMap::new();
    for (page_id, page) in existing_pages {
        for name in as_str_list(page.get("entity_names_kwd")) {
            name_to_page.insert(name, page.clone());
        }
        let slug = if page_id.contains('/') {
            page_id.rsplit('/').next().unwrap_or(page_id).to_string()
        } else {
            page_id.clone()
        };
        name_to_page.entry(slug).or_insert_with(|| page.clone());
    }

    let mut claims_source: Map<String, Value> = match canonical_claims {
        Some(claims) => claims.clone(),
        None => {
            let mut aggregated: Map<String, Value> = Map::new();
            for mr in map_results.unwrap_or(&[]) {
                if let Some(claims) = mr.get("claims").and_then(Value::as_array) {
                    for claim in claims {
                        let name = ["entity_name", "subject", "term"]
                            .iter()
                            .find_map(|key| claim.get(*key).and_then(Value::as_str))
                            .unwrap_or("")
                            .to_string();
                        if name.is_empty() {
                            continue;
                        }
                        let resolved = name_resolution
                            .and_then(|resolution| resolution.get(&name))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| name.clone());
                        let bucket = aggregated
                            .entry(resolved)
                            .or_insert_with(|| Value::Array(Vec::new()));
                        if let Some(items) = bucket.as_array_mut() {
                            items.push(claim.clone());
                        }
                    }
                }
            }
            aggregated
        }
    };
    for name in affected_names {
        claims_source
            .entry(name.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    let mut results: Vec<Value> = Vec::new();
    for name in affected_names {
        let claims = claims_source
            .get(name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let canonical = canonical_map.and_then(|map| map.get(name));
        let entity_type = canonical
            .and_then(|entry| entry.get("type"))
            .cloned()
            .unwrap_or(Value::String("entity".to_string()));
        let aliases: Vec<String> = canonical
            .map(|entry| names_from_value(entry.get("aliases")))
            .unwrap_or_default();
        let source_doc_ids: Vec<String> = canonical
            .map(|entry| names_from_value(entry.get("source_doc_ids")))
            .unwrap_or_default();
        let source_chunk_ids: Vec<String> = canonical
            .map(|entry| names_from_value(entry.get("source_chunk_ids")))
            .unwrap_or_default();
        let existing_page = name_to_page
            .get(name)
            .cloned()
            .or_else(|| existing_pages.get(name).cloned());
        let result = wiki_reduce_entity(
            name,
            &claims,
            existing_page.as_ref(),
            deleted_doc_ids,
            invalidated_chunk_ids,
            &entity_type,
            Some(&aliases),
            Some(&source_doc_ids),
            Some(&source_chunk_ids),
        );
        if result
            .get("has_delta")
            .map(crate::harness::knowlege_wiki::json_truthy)
            .unwrap_or(false)
        {
            results.push(result);
        }
    }
    results
}

fn doc_page_source_condition(doc_id: &str) -> Map<String, Value> {
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_DOC_PAGE_SOURCE_COMPILE_KWD.to_string()),
    );
    condition.insert("doc_id".to_string(), Value::String(doc_id.to_string()));
    condition
}

fn parse_json_or(value: Option<&Value>, fallback: Value) -> Value {
    match value {
        Some(Value::String(text)) if !text.is_empty() => {
            serde_json::from_str(text).unwrap_or(fallback)
        }
        Some(other) => other.clone(),
        None => fallback,
    }
}

/// `_wiki_update_doc_page_source`.
#[allow(clippy::too_many_arguments)]
pub fn wiki_update_doc_page_source(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
    page_ids: &[String],
    entity_names: Option<&[String]>,
    chunk_hashes: Option<&Map<String, Value>>,
    map_checksum: Option<&str>,
) {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let fields: Vec<String> = [
        "id",
        "page_ids",
        "entity_names",
        "source_chunk_hashes",
        "map_checksum",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let condition = doc_page_source_condition(doc_id);
    let existing_rows =
        inc_search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1).unwrap_or_default();
    let existing = existing_rows.into_iter().next();

    let mut entity_names_value: Value = Value::Array(
        entity_names
            .unwrap_or(&[])
            .iter()
            .cloned()
            .map(Value::String)
            .collect(),
    );
    let mut chunk_hashes_value: Value = chunk_hashes
        .map(|map| Value::Object(map.clone()))
        .unwrap_or_else(|| json!({}));
    let mut map_checksum_value = map_checksum.unwrap_or("").to_string();
    if let Some(row) = &existing {
        if entity_names.is_none() {
            entity_names_value = parse_json_or(row.get("entity_names"), Value::Array(Vec::new()));
        }
        if chunk_hashes.is_none() {
            let saved = row.get("source_chunk_hashes");
            chunk_hashes_value = match saved {
                Some(Value::String(text)) if !text.is_empty() => {
                    serde_json::from_str(text).unwrap_or_else(|_| json!({}))
                }
                Some(other) => other.clone(),
                None => json!({}),
            };
        }
        if map_checksum.is_none() {
            map_checksum_value = row
                .get("map_checksum")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
        }
    }

    let row_id = stable_row_id(&[
        WIKI_DOC_PAGE_SOURCE_COMPILE_KWD.to_string(),
        kb_id.to_string(),
        doc_id.to_string(),
    ]);
    let mut doc = Map::new();
    doc.insert("id".to_string(), Value::String(row_id.clone()));
    doc.insert("doc_id".to_string(), Value::String(doc_id.to_string()));
    doc.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
    doc.insert(
        "page_ids".to_string(),
        Value::String(
            Value::Array(page_ids.iter().cloned().map(Value::String).collect()).to_string(),
        ),
    );
    doc.insert(
        "entity_names".to_string(),
        Value::String(entity_names_value.to_string()),
    );
    doc.insert(
        "source_chunk_hashes".to_string(),
        Value::String(chunk_hashes_value.to_string()),
    );
    doc.insert(
        "map_checksum".to_string(),
        Value::String(map_checksum_value),
    );
    doc.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_DOC_PAGE_SOURCE_COMPILE_KWD.to_string()),
    );

    if existing.is_some() {
        let mut update_value = doc.clone();
        update_value.remove("id");
        let mut id_condition = Map::new();
        id_condition.insert("id".to_string(), Value::String(row_id));
        if let Err(err) = store.update(&id_condition, &update_value, &index, kb_id) {
            tracing::warn!(error = %err, doc_id = doc_id, "wiki: doc_page_source update failed");
        }
    } else if let Err(err) = store.insert(&[doc], &index, kb_id) {
        tracing::warn!(error = %err, doc_id = doc_id, "wiki: doc_page_source insert failed");
    }
}

/// `_wiki_load_doc_page_source`.
pub fn wiki_load_doc_page_source(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
) -> Option<Value> {
    let fields: Vec<String> = [
        "page_ids",
        "entity_names",
        "source_chunk_hashes",
        "map_checksum",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let condition = doc_page_source_condition(doc_id);
    let rows = inc_search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1).ok()?;
    let row = rows.into_iter().next()?;
    Some(json!({
        "page_ids": parse_json_or(row.get("page_ids"), Value::Array(Vec::new())),
        "entity_names": parse_json_or(row.get("entity_names"), Value::Array(Vec::new())),
        "source_chunk_hashes": parse_json_or(row.get("source_chunk_hashes"), json!({})),
        "map_checksum": row.get("map_checksum").and_then(Value::as_str).unwrap_or(""),
    }))
}

/// `_wiki_delete_doc_page_source`.
pub fn wiki_delete_doc_page_source(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
) {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let condition = doc_page_source_condition(doc_id);
    if let Err(err) = store.delete(&condition, &index, kb_id) {
        tracing::warn!(error = %err, doc_id = doc_id, "wiki: doc_page_source delete failed");
    }
}

#[cfg(test)]
mod wiki_incremental_part7_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;

    fn claim(statement: &str, doc: &str, chunks: &[&str]) -> Value {
        json!({"statement": statement, "source_doc_id": doc, "chunk_ids": chunks})
    }

    #[test]
    fn reduce_entity_matrix() {
        let empty = BTreeSet::new();
        let noop = wiki_reduce_entity(
            "Alpha",
            &[],
            None,
            &empty,
            None,
            &json!("entity"),
            None,
            None,
            None,
        );
        assert_eq!(noop["action"], json!("noop"));
        assert_eq!(noop["has_delta"], json!(false));

        let claims = vec![claim("s1", "d1", &["c1"]), claim("s2", "d2", &["c2"])];
        let created = wiki_reduce_entity(
            "Alpha",
            &claims,
            None,
            &empty,
            None,
            &json!("entity"),
            Some(&["A".to_string()]),
            Some(&["d1".to_string()]),
            Some(&["c1".to_string(), "c1".to_string()]),
        );
        assert_eq!(created["action"], json!("create"));
        assert_eq!(created["source_chunk_ids"], json!(["c1"]));
        assert_eq!(created["retained_source_doc_ids"], json!(["d1", "d2"]));

        let existing = json!({
            "claims": serde_json::to_string(&vec![claim("s1", "d1", &["c1"]), claim("s2", "d2", &["c2"])]).unwrap(),
            "source_chunk_ids": ["c1", "c2"]
        });
        let mut deleted = BTreeSet::new();
        deleted.insert("d1".to_string());
        let updated = wiki_reduce_entity(
            "Alpha",
            &[claim("s3", "d3", &["c3"])],
            Some(&existing),
            &deleted,
            None,
            &json!("entity"),
            None,
            Some(&["d1".to_string()]),
            Some(&["c1".to_string(), "c2".to_string(), "c3".to_string()]),
        );
        assert_eq!(updated["action"], json!("update"));
        assert_eq!(updated["retractions"].as_array().unwrap().len(), 1);
        assert_eq!(updated["retained_source_doc_ids"], json!(["d2", "d3"]));

        let mut deleted_all = BTreeSet::new();
        deleted_all.insert("d1".to_string());
        deleted_all.insert("d2".to_string());
        let deleted_result = wiki_reduce_entity(
            "Alpha",
            &[],
            Some(&existing),
            &deleted_all,
            None,
            &json!("entity"),
            None,
            None,
            None,
        );
        assert_eq!(deleted_result["action"], json!("delete"));

        let mut invalidated = BTreeSet::new();
        invalidated.insert("c1".to_string());
        invalidated.insert("c2".to_string());
        let invalidated_result = wiki_reduce_entity(
            "Alpha",
            &[],
            Some(&existing),
            &empty,
            Some(&invalidated),
            &json!("entity"),
            None,
            None,
            Some(&[]),
        );
        assert_eq!(invalidated_result["action"], json!("delete"));
        assert_eq!(invalidated_result["source_chunk_ids"], json!([]));
    }

    #[test]
    fn reduce_batch_uses_canonical_inputs() {
        let mut canonical_claims: Map<String, Value> = Map::new();
        canonical_claims.insert("Alpha".to_string(), json!([claim("s1", "d1", &["c1"])]));
        let mut canonical_map: Map<String, Value> = Map::new();
        canonical_map.insert(
            "Alpha".to_string(),
            json!({"type": "concept", "aliases": ["A"], "source_doc_ids": ["d1"], "source_chunk_ids": ["c1"]}),
        );
        let existing_pages: Map<String, Value> = Map::new();
        let mut affected = BTreeSet::new();
        affected.insert("Alpha".to_string());
        affected.insert("Ghost".to_string());
        let empty = BTreeSet::new();
        let results = wiki_reduce_batch(
            &affected,
            &existing_pages,
            &empty,
            None,
            Some(&canonical_claims),
            Some(&canonical_map),
            None,
            None,
        );
        assert_eq!(results.len(), 1);
        let alpha = results
            .iter()
            .find(|r| r["entity_name"] == json!("Alpha"))
            .unwrap();
        assert_eq!(alpha["action"], json!("create"));
        assert_eq!(alpha["entity_type"], json!("concept"));
        assert!(results.iter().all(|r| r["entity_name"] != json!("Ghost")));
    }

    #[test]
    fn doc_page_source_roundtrip() {
        let store = MemoryDocStore::new();
        assert!(wiki_load_doc_page_source(&store, "t1", "kb1", "d1").is_none());
        let mut hashes: Map<String, Value> = Map::new();
        hashes.insert("c1".to_string(), json!("h1"));
        wiki_update_doc_page_source(
            &store,
            "t1",
            "kb1",
            "d1",
            &["concept/alpha".to_string()],
            Some(&["Alpha".to_string()]),
            Some(&hashes),
            Some("checksum-1"),
        );
        let loaded = wiki_load_doc_page_source(&store, "t1", "kb1", "d1").expect("record");
        assert_eq!(loaded["page_ids"], json!(["concept/alpha"]));
        assert_eq!(loaded["entity_names"], json!(["Alpha"]));
        assert_eq!(loaded["source_chunk_hashes"], json!({"c1": "h1"}));
        assert_eq!(loaded["map_checksum"], json!("checksum-1"));

        wiki_update_doc_page_source(
            &store,
            "t1",
            "kb1",
            "d1",
            &["concept/alpha".to_string(), "concept/beta".to_string()],
            None,
            None,
            None,
        );
        let loaded2 = wiki_load_doc_page_source(&store, "t1", "kb1", "d1").expect("record2");
        assert_eq!(
            loaded2["page_ids"],
            json!(["concept/alpha", "concept/beta"])
        );
        assert_eq!(loaded2["entity_names"], json!(["Alpha"]));
        assert_eq!(loaded2["source_chunk_hashes"], json!({"c1": "h1"}));
        assert_eq!(loaded2["map_checksum"], json!("checksum-1"));

        wiki_delete_doc_page_source(&store, "t1", "kb1", "d1");
        assert!(wiki_load_doc_page_source(&store, "t1", "kb1", "d1").is_none());
    }
}

// ---------------------------------------------------------------------------
// Part 8a — Mode A prompt builders (`_build_source_chunks_block` ..
// `_wiki_entity_planning_text`).
//
// Adaptation note: prompts are assembled from literal line blocks so the
// templates need no brace escaping; the 50/30-item caps mirror upstream.
// ---------------------------------------------------------------------------

/// `_build_source_chunks_block`: verbatim source block for writer prompts.
pub fn build_source_chunks_block(source_chunks: &[Value], max_budget: usize) -> String {
    if source_chunks.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    let mut total = 0usize;
    for chunk in source_chunks {
        let cid = ["id", "chunk_id"]
            .iter()
            .find_map(|key| chunk.get(*key))
            .and_then(|value| match value {
                Value::String(text) if !text.is_empty() => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            });
        let text = ["content_with_weight", "text"]
            .iter()
            .find_map(|key| chunk.get(*key))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty());
        let (Some(cid), Some(text)) = (cid, text) else {
            continue;
        };
        let block = if chunk
            .get("_verbatim")
            .map(crate::harness::knowlege_wiki::json_truthy)
            .unwrap_or(false)
        {
            format!("[SOURCE {cid}]\n{text}")
        } else {
            format!("[CHUNK {cid}]\n{text}")
        };
        if total + block.chars().count() + 2 > max_budget {
            break;
        }
        parts.push(block.clone());
        total += block.chars().count() + 2;
    }
    if parts.is_empty() {
        return String::new();
    }
    if total >= max_budget {
        parts.push("[…further source chunks omitted to fit context budget…]".to_string());
    }
    parts.join("\n\n")
}

fn bullet_lines(values: &[String], limit: usize) -> String {
    values
        .iter()
        .take(limit)
        .map(|value| format!("- {value}"))
        .collect::<Vec<String>>()
        .join("\n")
}

fn claims_bullet_text(claims: &[Value]) -> String {
    claims
        .iter()
        .map(|claim| format!("- {}", claim_text_of(claim)))
        .collect::<Vec<String>>()
        .join("\n")
}

/// `_build_mode_a_generate_prompt`.
#[allow(clippy::too_many_arguments)]
pub fn build_mode_a_generate_prompt(
    page_id: &str,
    page_title: &str,
    claims: Option<&[Value]>,
    source_chunks: Option<&[Value]>,
    available_pages: Option<&[String]>,
    contextual_hints: &str,
    topic_candidates: Option<&[String]>,
    member_evidence: Option<&[Value]>,
) -> String {
    let chunks_text =
        build_source_chunks_block(source_chunks.unwrap_or(&[]), WIKI_SOURCE_BUDGET_CHARS);
    let claims = claims.unwrap_or(&[]);
    let claims_text = if claims.is_empty() {
        "(no claims)".to_string()
    } else {
        claims_bullet_text(claims)
    };
    let member_text = build_member_evidence_block(member_evidence);
    let topics_block = bullet_lines(
        topic_candidates.unwrap_or(&[]),
        WIKI_PAGE_TOPIC_CANDIDATE_LIMIT,
    );
    let topics_block = if topics_block.is_empty() {
        "(none; create a short canonical topic from the page evidence)".to_string()
    } else {
        topics_block
    };
    let pages_block = bullet_lines(available_pages.unwrap_or(&[]), 50);
    let pages_block = if pages_block.is_empty() {
        "(none)".to_string()
    } else {
        pages_block
    };
    let member_text = if member_text.is_empty() {
        "(single member page)".to_string()
    } else {
        member_text
    };
    let chunks_text = if chunks_text.is_empty() {
        "(no source chunks available)".to_string()
    } else {
        chunks_text
    };
    format!(
        "## Concept Page Identity\n- Page ID: {page_id}\n- Title: {page_title}\n\n## Required Page Members\n{member_text}\n\n## Source Chunks (verbatim source text — ground every fact in these)\n{chunks_text}\n\n## Extracted Claims (checklist)\n{claims_text}\n\n## Candidate Topics\n{topics_block}\n\n## Available Pages for [[wikilinks]]\n{pages_block}\n\n{contextual_hints}\n"
    )
}

/// `_build_mode_a_modify_prompt`.
#[allow(clippy::too_many_arguments)]
pub fn build_mode_a_modify_prompt(
    page_id: &str,
    page_title: &str,
    existing_page: Option<&Value>,
    additions: Option<&[Value]>,
    retractions: Option<&[Value]>,
    claims: Option<&[Value]>,
    source_chunks: Option<&[Value]>,
    available_pages: Option<&[String]>,
    contextual_hints: &str,
    topic_candidates: Option<&[String]>,
    force_full: bool,
    member_evidence: Option<&[Value]>,
) -> String {
    let existing_content = existing_page
        .and_then(|page| page.get("md_with_weight"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let existing_topic = existing_page
        .and_then(|page| page.get("topic_kwd"))
        .map(|value| match value {
            Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
            other => other.as_str().unwrap_or(""),
        })
        .unwrap_or("")
        .to_string();
    let topic_block = bullet_lines(
        topic_candidates.unwrap_or(&[]),
        WIKI_PAGE_TOPIC_CANDIDATE_LIMIT,
    );
    let topic_block = if topic_block.is_empty() {
        "(none; retain the current topic when it still fits, otherwise create a short canonical topic from the page evidence)".to_string()
    } else {
        topic_block
    };
    let member_text = build_member_evidence_block(member_evidence);
    let member_text = if member_text.is_empty() {
        "(single member page)".to_string()
    } else {
        member_text
    };

    if !force_full {
        let additions = additions.unwrap_or(&[]);
        let additions_text = if additions.is_empty() {
            "(none)".to_string()
        } else {
            claims_bullet_text(additions)
        };
        let retractions = retractions.unwrap_or(&[]);
        let retractions_text = if retractions.is_empty() {
            "(none)".to_string()
        } else {
            claims_bullet_text(retractions)
        };
        let chunks_text =
            build_source_chunks_block(source_chunks.unwrap_or(&[]), WIKI_SOURCE_BUDGET_CHARS);
        let pages_block = bullet_lines(available_pages.unwrap_or(&[]), 30);
        let pages_block = if pages_block.is_empty() {
            "(none)".to_string()
        } else {
            pages_block
        };
        let current = if existing_content.is_empty() {
            "(empty)".to_string()
        } else {
            truncate_chars_p8(&existing_content, 10000)
        };
        let current_topic = if existing_topic.is_empty() {
            "(none)".to_string()
        } else {
            existing_topic.clone()
        };
        return format!(
            "## Page Identity\n- Page ID: {page_id}\n- Title: {page_title}\n\n## Required Page Members\n{member_text}\n\n## Current Page\n{current}\n\n## Current Topic\n{current_topic}\n\n## Candidate Topics\n{topic_block}\n\n## New Claims to Add\n{additions_text}\n\n## Claims to Retract\n{retractions_text}\n\n## Source Chunks for New Information (verbatim source text — ground every fact in these)\n{chunks_text}\n\n## Available Pages for [[wikilinks]]\n{pages_block}\n\n{contextual_hints}\n"
        );
    }

    let chunks_text = build_source_chunks_block(source_chunks.unwrap_or(&[]), 120_000);
    let chunks_text = if chunks_text.is_empty() {
        "(none)".to_string()
    } else {
        chunks_text
    };
    let claims_text = claims_bullet_text(claims.unwrap_or(&[]));
    let claims_text = if claims_text.is_empty() {
        "(none)".to_string()
    } else {
        claims_text
    };
    let current_topic = if existing_topic.is_empty() {
        "(none)".to_string()
    } else {
        existing_topic.clone()
    };
    let pages_block = bullet_lines(available_pages.unwrap_or(&[]), 50);
    let pages_block = if pages_block.is_empty() {
        "(none)".to_string()
    } else {
        pages_block
    };
    format!(
        "## Page Identity\n- Page ID: {page_id}\n- Title: {page_title}\n\n## Required Page Members\n{member_text}\n\n## All Source Chunks (for full re-synthesis — verbatim source text)\n{chunks_text}\n\n## All Claims\n{claims_text}\n\n## Current Topic\n{current_topic}\n\n## Candidate Topics\n{topic_block}\n\n## Available Pages for [[wikilinks]]\n{pages_block}\n\n{contextual_hints}\n"
    )
}

fn truncate_chars_p8(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// `_build_member_evidence_block`.
pub fn build_member_evidence_block(member_evidence: Option<&[Value]>) -> String {
    let mut blocks: Vec<String> = Vec::new();
    for member in member_evidence.unwrap_or(&[]) {
        let name = member
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if name.is_empty() {
            continue;
        }
        let claims: Vec<Value> = member
            .get("claims")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let claims_text: String = {
            let lines: Vec<String> = claims
                .iter()
                .filter(|claim| claim.is_object() && !claim_text_of(claim).is_empty())
                .map(|claim| format!("- {}", claim_text_of(claim)))
                .collect();
            if lines.is_empty() {
                "(no extracted claims; use the member's source evidence)".to_string()
            } else {
                lines.join("\n")
            }
        };
        let chunk_ids = member
            .get("source_chunk_ids")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|cid| match cid {
                        Value::String(text) if !text.is_empty() => Some(text.clone()),
                        Value::Number(number) => Some(number.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<String>>()
                    .join(", ")
            })
            .unwrap_or_default();
        let chunk_ids = if chunk_ids.is_empty() {
            "(none)".to_string()
        } else {
            chunk_ids
        };
        blocks.push(format!(
            "### Member: {name}\nClaims:\n{claims_text}\nSource chunk IDs: {chunk_ids}"
        ));
    }
    blocks.join("\n\n")
}

/// `_WIKI_MODE_A_GENERATE_SYSTEM`.
pub const WIKI_MODE_A_GENERATE_SYSTEM: &str = "You are a wiki COMPILER. Generate a new wiki page for the given concept using the provided source chunks and extracted claims.\n\n## LANGUAGE\nWrite the ENTIRE page in the SAME LANGUAGE as the source chunks. If the source chunks are written in Chinese, write the page in Chinese. Do not switch to English, and do not translate entity names (keep them verbatim: e.g. keep \"张伟\", do not write \"Zhang Wei\").\n\n## RULES\n1. CONCEPT PAGE: This is a single-concept wiki page. Organize by THEME, not by entity.\n2. CROSS-DOCUMENT SYNTHESIS: Weave information from multiple sources into coherent paragraphs. Compare evidence, explain contradictions.\n3. OPENING PARAGRAPH: 2-4 sentences defining the concept. Mention key entities. No heading.\n4. SECTIONS: H2 headings, prose first, then sub-points if needed.\n   Markdown formatting is mandatory: put every heading on its own line and separate every paragraph with a blank line.\n5. WIKILINKS: Use ONLY the exact page IDs listed in \"Available Pages for [[wikilinks]]\" (they already carry the entity/ or concept/ prefix). Insert [[EXACT_PAGE_ID]] on first mention of a related concept/entity. NEVER invent a link target, NEVER drop the prefix, NEVER write English names.\n6. DICTIONARY PREVENTION: Do NOT group content by source document. Do NOT create one section per entity. Do NOT write flat bullet lists.\n7. MEMBER COVERAGE: If \"Required Page Members\" lists multiple members, the page MUST contain grounded factual content about EVERY listed member. Do not silently omit or replace any member. If the members are unrelated, keep them in clearly separated subsections while preserving all supported facts.\n\n## SOURCE GROUNDING (COMPILER, not writer)\n- The \"Source Chunks\" section contains VERBATIM source text. Stay close to the source wording — reuse the source's own sentences and facts where possible.\n- Every newly added factual claim, entity, or numerical value MUST be directly supported by the provided source chunks. Do NOT invent facts, figures, dates, or relationships not present in the sources.\n- Do NOT add rhetorical filler (e.g. \"旨在帮助…\", \"designed to…\", \"aims to provide…\") unless it appears verbatim in a source.\n- If the sources disagree, present both views and add a \"## Contradictions\" section rather than silently picking one.\n\n## OUTPUT\nReturn ONLY the complete markdown page.\nFirst line: SUMMARY: {one-sentence description, 15-40 words}\nSecond line: TITLE: {a concise title covering all required page members}\nThird line: TOPIC: {the best short canonical topic for this page}\nThen the page content.\n\nTITLE is the human-readable page title, not the page ID. When multiple\nmembers are merged, synthesize a title covering the combined subject. For a\nsingle-member page, keep the supplied title unchanged.\n\nChoose TOPIC by understanding the page subject and evidence. Prefer a fitting\nitem from Candidate Topics. If none fits, create a concise topic in the source\nlanguage. Do not choose by superficial character or word overlap.\n";

/// `_WIKI_MODE_A_MODIFY_SYSTEM`.
pub const WIKI_MODE_A_MODIFY_SYSTEM: &str = "You are a wiki editor. Update the existing page by integrating new information and removing retracted content.\n\n## LANGUAGE\nWrite the ENTIRE page in the SAME LANGUAGE as the source chunks. If the source chunks are written in Chinese, write the page in Chinese. Do not switch to English, and do not translate entity names (keep them verbatim: e.g. keep \"张伟\", do not write \"Zhang Wei\").\n\n## RULES\n1. CONCEPT PAGE: This is a single-concept wiki page. Organize by THEME, not by entity.\n2. CROSS-DOCUMENT SYNTHESIS: Connect new claims to existing content. Weave them into the SAME paragraphs.\n3. OPENING PARAGRAPH: Should reflect the FULL updated picture.\n4. WIKILINKS: Keep existing and add new [[page_id]] links where appropriate. Use ONLY the exact page IDs listed in \"Available Pages for [[wikilinks]]\" (they already carry the entity/ or concept/ prefix). NEVER invent a link target, NEVER drop the prefix, NEVER write English names.\n5. For FULL RE-SYNTHESIS: Use all source chunks + all claims to rewrite from scratch.\n6. For INCREMENTAL MODIFY: Integrate additions, remove retracted content, keep unchanged content.\n7. MARKDOWN FORMATTING: Put every heading on its own line and separate every paragraph with a blank line. Do not return the whole page as one line.\n8. MEMBER COVERAGE: If \"Required Page Members\" lists multiple members, the updated page MUST retain grounded factual content about EVERY listed member. Do not silently omit any member.\n\n## DICTIONARY PREVENTION\n- Do NOT group content by source document.\n- Do NOT simply append new claims at the end.\n- Do NOT create one section per entity.\n\n## SOURCE GROUNDING (COMPILER, not writer)\n- The \"Source Chunks\" section contains VERBATIM source text. Stay close to the source wording — reuse the source's own sentences and facts where possible.\n- Every newly added factual claim, entity, or numerical value MUST be directly supported by the provided source chunks. Do NOT invent facts, figures, dates, or relationships not present in the sources.\n- Do NOT add rhetorical filler (e.g. \"旨在帮助…\", \"designed to…\", \"aims to provide…\") unless it appears verbatim in a source.\n- If new sources contradict existing page content, present both views and add a \"## Contradictions / Updates\" section rather than silently overwriting.\n\n## OUTPUT\nReturn ONLY the complete updated markdown page.\nFirst line: SUMMARY: {one-sentence description of what changed, 15-40 words}\nSecond line: TITLE: {a concise title covering all required page members}\nThird line: TOPIC: {the best short canonical topic for the complete updated page}\nThen the updated page content.\n\nTITLE is the human-readable page title, not the page ID. When multiple\nmembers are merged, rewrite the title to cover the complete updated subject.\nFor a single-member page, keep the supplied title unchanged.\n\nChoose TOPIC by understanding the complete page subject and evidence. Prefer a\nfitting item from Candidate Topics; retain Current Topic when it remains the\nbest fit. If neither fits, create a concise topic in the source language. Do\nnot choose by superficial character or word overlap.\n";

/// `_wiki_build_contextual_hints`.
pub fn wiki_build_contextual_hints(
    _page_id: &str,
    existing_page: Option<&Value>,
    all_relations: &Map<String, Value>,
) -> String {
    let mut related: Vec<Value> = Vec::new();
    if let Some(page) = existing_page {
        if let Some(value) = page.get("related_kb_pages_kwd") {
            let parsed = match value {
                Value::String(text) if !text.is_empty() => serde_json::from_str::<Value>(text)
                    .ok()
                    .filter(Value::is_array)
                    .unwrap_or_else(|| Value::Array(vec![value.clone()])),
                Value::Array(_) => value.clone(),
                _ => Value::Null,
            };
            if let Value::Array(items) = parsed {
                related = items;
            }
        }
    }
    if related.is_empty() {
        if let Some(page) = existing_page {
            for name in as_str_list(page.get("entity_names_kwd")) {
                if let Some(items) = all_relations.get(&name).and_then(Value::as_array) {
                    related.extend(items.iter().cloned());
                }
            }
        }
    }
    if related.is_empty() {
        return String::new();
    }

    let mut lines: Vec<String> = vec![
        "## Context: Related Entities & Concepts".to_string(),
        "Reference them in the opening paragraph and relevant sections:".to_string(),
    ];
    for item in related.iter().take(10) {
        let (entity_name, relation) = if item.is_object() {
            let name = ["entity_name", "name", "slug"]
                .iter()
                .find_map(|key| item.get(*key).and_then(Value::as_str))
                .unwrap_or("")
                .to_string();
            let relation = ["relation", "type"]
                .iter()
                .find_map(|key| item.get(*key).and_then(Value::as_str))
                .unwrap_or("related")
                .to_string();
            (name, relation)
        } else {
            (
                item.as_str().unwrap_or("").trim().to_string(),
                "related".to_string(),
            )
        };
        if entity_name.is_empty() {
            continue;
        }
        lines.push(format!("- [[{entity_name}]] — {relation}"));
    }
    lines.join("\n")
}

/// `_wiki_entity_planning_text`.
pub fn wiki_entity_planning_text(entity: &Value, max_claims: usize) -> String {
    let name = ["entity_name", "name", "term"]
        .iter()
        .find_map(|key| entity.get(*key).and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let mut aliases_list = as_str_list(entity.get("aliases"));
    aliases_list.truncate(5);
    let aliases = aliases_list.join(", ");
    let description = ["definition_excerpt", "description"]
        .iter()
        .find_map(|key| entity.get(*key).and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let mut claims: Vec<String> = Vec::new();
    if let Some(items) = entity.get("claims").and_then(Value::as_array) {
        for claim in items.iter().take(max_claims) {
            let statement = ["statement", "text"]
                .iter()
                .find_map(|key| claim.get(*key).and_then(Value::as_str));
            if let Some(statement) = statement {
                claims.push(statement.to_string());
            }
        }
    }
    let mut parts: Vec<String> = vec![format!("name={name}")];
    if !aliases.is_empty() {
        parts.push(format!("aliases={aliases}"));
    }
    if !description.is_empty() {
        parts.push(format!("description={description}"));
    }
    if !claims.is_empty() {
        parts.push(format!("evidence={}", claims.join(" | ")));
    }
    let mut relations: Vec<String> = Vec::new();
    if let Some(items) = entity.get("relations").and_then(Value::as_array) {
        for relation in items.iter().take(8) {
            if !relation.is_object() {
                continue;
            }
            let counterpart = ["entity", "counterpart"]
                .iter()
                .find_map(|key| relation.get(*key).and_then(Value::as_str));
            let relation_type = relation
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("related");
            if let Some(counterpart) = counterpart {
                relations.push(format!("{relation_type}: {counterpart}"));
            }
        }
    }
    if !relations.is_empty() {
        parts.push(format!("relations={}", relations.join(" | ")));
    }
    parts.join("; ")
}

#[cfg(test)]
mod wiki_incremental_part8_tests {
    use super::*;

    #[test]
    fn source_block_labels_and_budget() {
        let chunks = vec![
            json!({"id": "c1", "text": "plain text", "_verbatim": true}),
            json!({"id": "c2", "content_with_weight": "condensed"}),
            json!({"id": "", "text": "skip"}),
            json!({"id": "c3", "text": ""}),
        ];
        let block = build_source_chunks_block(&chunks, WIKI_SOURCE_BUDGET_CHARS);
        assert!(block.contains("[SOURCE c1]\nplain text"));
        assert!(block.contains("[CHUNK c2]\ncondensed"));
        assert!(!block.contains("skip"));
        let tight = build_source_chunks_block(&chunks, 25);
        assert!(tight.contains("[SOURCE c1]"));
        assert!(!tight.contains("c2"));
        assert!(build_source_chunks_block(&[], 100).is_empty());
    }

    #[test]
    fn generate_prompt_sections() {
        let claims = vec![json!({"statement": "s1"}), json!({"text": "s2"})];
        let chunks = vec![json!({"id": "c1", "text": "body"})];
        let pages = vec!["entity/a".to_string(), "concept/b".to_string()];
        let topics = vec!["alpha".to_string()];
        let prompt = build_mode_a_generate_prompt(
            "concept/x",
            "X",
            Some(&claims),
            Some(&chunks),
            Some(&pages),
            "HINTS",
            Some(&topics),
            None,
        );
        assert!(prompt.contains("- Page ID: concept/x"));
        assert!(prompt.contains("## Required Page Members\n(single member page)"));
        assert!(prompt.contains("[CHUNK c1]\nbody"));
        assert!(prompt.contains("- s1\n- s2"));
        assert!(prompt.contains("- alpha"));
        assert!(prompt.contains("- entity/a\n- concept/b"));
        assert!(prompt.ends_with("HINTS\n"));
        let minimal = build_mode_a_generate_prompt("p", "T", None, None, None, "", None, None);
        assert!(minimal.contains("(no claims)"));
        assert!(minimal.contains("(no source chunks available)"));
        assert!(minimal.contains("(none; create a short canonical topic"));
        assert!(minimal.contains("\n(none)\n"));
    }

    #[test]
    fn modify_prompt_modes() {
        let existing = json!({"md_with_weight": "OLD", "topic_kwd": ["alpha"]});
        let additions = vec![json!({"statement": "new"})];
        let retractions = vec![json!({"statement": "old"})];
        let incremental = build_mode_a_modify_prompt(
            "concept/x",
            "X",
            Some(&existing),
            Some(&additions),
            Some(&retractions),
            None,
            None,
            None,
            "",
            None,
            false,
            None,
        );
        assert!(incremental.contains("## Current Page\nOLD"));
        assert!(incremental.contains("## Current Topic\nalpha"));
        assert!(incremental.contains("## New Claims to Add\n- new"));
        assert!(incremental.contains("## Claims to Retract\n- old"));
        let full = build_mode_a_modify_prompt(
            "concept/x",
            "X",
            Some(&existing),
            None,
            None,
            Some(&[json!({"statement": "all"})]),
            None,
            None,
            "",
            None,
            true,
            None,
        );
        assert!(full.contains("## All Source Chunks (for full re-synthesis"));
        assert!(full.contains("## All Claims\n- all"));
        assert!(!full.contains("## New Claims to Add"));
    }

    #[test]
    fn member_evidence_and_hints() {
        let members = vec![
            json!({"name": "Alpha", "claims": [{"statement": "s"}], "source_chunk_ids": ["c1", "c2"]}),
            json!({"name": "", "claims": []}),
            json!({"name": "Beta"}),
        ];
        let block = build_member_evidence_block(Some(&members));
        assert!(block.contains("### Member: Alpha\nClaims:\n- s\nSource chunk IDs: c1, c2"));
        assert!(block.contains("### Member: Beta\nClaims:\n(no extracted claims; use the member's source evidence)\nSource chunk IDs: (none)"));
        assert!(build_member_evidence_block(None).is_empty());

        let existing = json!({"related_kb_pages_kwd": "[{\"entity_name\": \"Gamma\", \"relation\": \"uses\"}]"});
        let relations: Map<String, Value> = Map::new();
        let hints = wiki_build_contextual_hints("concept/x", Some(&existing), &relations);
        assert!(hints.contains("- [[Gamma]] — uses"));
        let existing2 = json!({"entity_names_kwd": ["alpha"]});
        let mut relations2: Map<String, Value> = Map::new();
        relations2.insert(
            "alpha".to_string(),
            json!([{"entity_name": "Delta", "relation": "part_of"}]),
        );
        let hints2 = wiki_build_contextual_hints("concept/x", Some(&existing2), &relations2);
        assert!(hints2.contains("- [[Delta]] — part_of"));
        assert!(wiki_build_contextual_hints("p", None, &Map::new()).is_empty());
    }

    #[test]
    fn entity_planning_text_composition() {
        let entity = json!({
            "entity_name": "Alpha",
            "aliases": ["A", "B", "C", "D", "E", "F"],
            "definition_excerpt": "def",
            "claims": [{"statement": "s1"}, {"text": "s2"}, {"statement": "s3"}, {"statement": "s4"}],
            "relations": [{"entity": "Beta", "type": "uses"}, {"nope": 1}, {"counterpart": "Gamma"}]
        });
        let text = wiki_entity_planning_text(&entity, 3);
        assert_eq!(
            text,
            "name=Alpha; aliases=A, B, C, D, E; description=def; evidence=s1 | s2 | s3; relations=uses: Beta | related: Gamma"
        );
        assert_eq!(wiki_entity_planning_text(&json!({}), 3), "name=");
    }
}

// ---------------------------------------------------------------------------
// Part 8b — single-page REFINE (`_wiki_refine_page`).
//
// Adaptation note: the commit-history hooks (`FileCommitService`) are not
// ported (debug-logged as a documented divergence); delete/persist errors
// warn and keep state instead of raising; the topic-pool lock is unnecessary
// in the sequential port.
// ---------------------------------------------------------------------------

/// `_wiki_refine_page` arguments (upstream keyword-only parameters).
pub struct RefinePageArgs<'a> {
    pub store: &'a dyn DocStore,
    pub mode: &'a str,
    pub page_id: &'a str,
    pub page_title: &'a str,
    pub existing_page: Option<&'a Value>,
    pub page_type_kwd: &'a str,
    pub additions: Option<&'a [Value]>,
    pub retractions: Option<&'a [Value]>,
    pub source_chunks: Option<&'a [Value]>,
    pub claims: Option<&'a [Value]>,
    pub available_pages: Option<&'a [String]>,
    pub contextual_hints: &'a str,
    pub chat: &'a dyn HarnessChat,
    pub embd: Option<&'a dyn Embedder>,
    pub tenant_id: &'a str,
    pub kb_id: &'a str,
    pub page_version: i64,
    pub entity_names: Option<&'a [String]>,
    pub page_embedding: Option<&'a [f32]>,
    pub embed_routing_context: bool,
    pub source_doc_ids: Option<&'a [String]>,
    pub topic_candidates: Option<&'a [String]>,
    pub topic_selection_stats: Option<&'a mut Map<String, Value>>,
    pub topic_embeddings: Option<&'a mut Map<String, Value>>,
    pub topic_pool: Option<&'a mut Map<String, Value>>,
    pub member_evidence: Option<&'a [Value]>,
}

fn claim_dedupe_key(claim: &Value) -> (String, String, Vec<String>) {
    let statement = ["statement", "text"]
        .iter()
        .find_map(|key| claim.get(*key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let doc = claim
        .get("source_doc_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut chunks = wiki_claim_chunk_ids(claim);
    chunks.sort();
    (statement, doc, chunks)
}

fn existing_claims_of(page: Option<&Value>) -> Vec<Value> {
    let raw = page.and_then(|page| page.get("claims"));
    match raw {
        Some(Value::String(text)) if !text.is_empty() => serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default(),
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

fn truncate_chars_p8b(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// `_wiki_refine_page`: one Mode A REFINE action on a page.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_refine_page(args: RefinePageArgs<'_>) -> Option<Value> {
    let RefinePageArgs {
        store,
        mode,
        page_id,
        page_title,
        existing_page,
        page_type_kwd,
        additions,
        retractions,
        source_chunks,
        claims,
        available_pages,
        contextual_hints,
        chat,
        embd,
        tenant_id,
        kb_id,
        page_version,
        entity_names,
        page_embedding,
        embed_routing_context,
        source_doc_ids,
        topic_candidates,
        mut topic_selection_stats,
        mut topic_embeddings,
        topic_pool,
        member_evidence,
    } = args;

    if page_id.trim().is_empty() {
        return existing_page.cloned();
    }
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);

    let ranked_topics = wiki_rank_topic_candidates(
        embd,
        page_title,
        claims,
        source_chunks,
        existing_page,
        topic_candidates,
        topic_embeddings.as_deref_mut(),
    )
    .await;

    if mode == "delete" {
        let mut condition = Map::new();
        condition.insert(
            "compile_kwd".to_string(),
            Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
        );
        condition.insert("slug_kwd".to_string(), Value::String(page_id.to_string()));
        match store.delete(&condition, &index, kb_id) {
            Ok(count) if count > 0 => {
                tracing::debug!(page = page_id, "wiki: page version history hook not ported");
                return None;
            }
            Ok(_) => {
                tracing::warn!(page = page_id, "wiki: page deletion did not remove page");
                return existing_page.cloned();
            }
            Err(err) => {
                tracing::warn!(error = %err, page = page_id, "wiki: page deletion failed");
                return existing_page.cloned();
            }
        }
    }

    // WeKnora-style verbatim evidence: replace condensed claim text with the
    // actual source-chunk body when available.
    let mut enriched_chunks: Vec<Value> = source_chunks.unwrap_or(&[]).to_vec();
    if !enriched_chunks.is_empty() {
        let mut ids: Vec<String> = Vec::new();
        for chunk in &enriched_chunks {
            if let Some(cid) = ["id", "chunk_id"]
                .iter()
                .find_map(|key| chunk.get(*key))
                .and_then(|value| match value {
                    Value::String(text) if !text.is_empty() => Some(text.clone()),
                    Value::Number(number) => Some(number.to_string()),
                    _ => None,
                })
            {
                ids.push(cid);
            }
        }
        if !ids.is_empty() {
            let texts = wiki_load_chunk_texts(store, tenant_id, kb_id, &ids);
            if !texts.is_empty() {
                enriched_chunks = wiki_enrich_source_chunks(&enriched_chunks, &texts);
            }
        }
    }

    let (system_prompt, user_prompt) = if mode == "generate" {
        (
            WIKI_MODE_A_GENERATE_SYSTEM,
            build_mode_a_generate_prompt(
                page_id,
                page_title,
                claims,
                Some(&enriched_chunks),
                available_pages,
                contextual_hints,
                Some(&ranked_topics),
                member_evidence,
            ),
        )
    } else if mode == "re-synthesize" {
        (
            WIKI_MODE_A_MODIFY_SYSTEM,
            build_mode_a_modify_prompt(
                page_id,
                page_title,
                existing_page,
                additions,
                retractions,
                claims,
                Some(&enriched_chunks),
                available_pages,
                contextual_hints,
                Some(&ranked_topics),
                true,
                member_evidence,
            ),
        )
    } else {
        (
            WIKI_MODE_A_MODIFY_SYSTEM,
            build_mode_a_modify_prompt(
                page_id,
                page_title,
                existing_page,
                additions,
                retractions,
                claims,
                Some(&enriched_chunks),
                available_pages,
                contextual_hints,
                Some(&ranked_topics),
                false,
                member_evidence,
            ),
        )
    };

    let response = match chat_mdl_ask(chat, system_prompt, &user_prompt, 0.0).await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, page = page_id, "wiki: refine chat failed");
            return existing_page.cloned();
        }
    };
    if response.trim().is_empty() {
        return existing_page.cloned();
    }

    // Parse response metadata lines (SUMMARY/TITLE/TOPIC at the head).
    let mut content_lines: Vec<String> = response.trim().split('\n').map(str::to_string).collect();
    let mut summary = String::new();
    let mut title = String::new();
    let mut topic = String::new();
    while !content_lines.is_empty() {
        let line = content_lines[0].trim().to_string();
        if line.is_empty() && (!summary.is_empty() || !title.is_empty() || !topic.is_empty()) {
            content_lines.remove(0);
            continue;
        }
        let upper = line.to_uppercase();
        if upper.starts_with("SUMMARY:") && summary.is_empty() {
            summary = line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
            content_lines.remove(0);
            continue;
        }
        if upper.starts_with("TITLE:") && title.is_empty() {
            title = line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
            content_lines.remove(0);
            continue;
        }
        if upper.starts_with("TOPIC:") && topic.is_empty() {
            topic = line.splitn(2, ':').nth(1).unwrap_or("").trim().to_string();
            content_lines.remove(0);
            continue;
        }
        break;
    }
    let content = content_lines.join("\n").trim().to_string();
    if content.is_empty() {
        return existing_page.cloned();
    }

    let existing = existing_page.cloned().unwrap_or_else(|| json!({}));
    let member_names: BTreeSet<String> = entity_names
        .unwrap_or(&[])
        .iter()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();
    let fallback_title = existing
        .get("title_kwd")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| page_title.to_string());
    let title = if member_names.len() <= 1 || title.is_empty() {
        fallback_title.clone()
    } else {
        title
    };
    if topic.is_empty() {
        let existing_topic = existing
            .get("topic_kwd")
            .map(|value| match value {
                Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
                other => other.as_str().unwrap_or(""),
            })
            .unwrap_or("");
        topic = if existing_topic.is_empty() {
            WIKI_TOPIC_FALLBACK.to_string()
        } else {
            existing_topic.to_string()
        };
    }
    let topic_key = normalize_key(&topic);
    let candidate_keys: BTreeSet<String> = ranked_topics
        .iter()
        .map(|candidate| normalize_key(candidate))
        .collect();
    let is_new_topic = !topic.is_empty() && !candidate_keys.contains(&topic_key);
    let mut added_to_candidates = false;
    if is_new_topic {
        if let Some(pool) = topic_pool {
            added_to_candidates = !pool.contains_key(&topic_key);
            pool.entry(topic_key.clone())
                .or_insert_with(|| Value::String(topic.clone()));
            if let Some(embeddings) = topic_embeddings.as_deref_mut() {
                if !embeddings.contains_key(&topic) {
                    if let Some(embd) = embd {
                        match embd.embed(&[topic.as_str()]).await {
                            Ok(vectors) => {
                                if let Some(vector) = vectors.into_iter().next() {
                                    embeddings.insert(topic.clone(), json!(vector));
                                }
                            }
                            Err(error) => {
                                // Without a vector the topic drops to lexical-only matching
                                // for this run; a silent recall loss is hard to diagnose.
                                tracing::warn!(%error, %topic, "Topic embedding failed; semantic selection skipped for this topic");
                            }
                        }
                    }
                }
            }
            if added_to_candidates {
                if let Some(stats) = topic_selection_stats.as_deref_mut() {
                    let current = stats.get("new_added").and_then(Value::as_i64).unwrap_or(0);
                    stats.insert("new_added".to_string(), json!(current + 1));
                }
            }
        }
    }
    let topic_in_candidates = candidate_keys.contains(&topic_key);
    wiki_log_stats(
        "TOPIC",
        "page_selection",
        &[
            ("page_id", json!(page_id)),
            ("candidate_count", json!(candidate_keys.len())),
            (
                "candidates",
                Value::Array(
                    ranked_topics
                        .iter()
                        .take(WIKI_PAGE_TOPIC_CANDIDATE_LIMIT)
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            ("selected", json!(topic)),
            ("is_new", json!(!topic_in_candidates)),
            ("added_to_candidates", json!(added_to_candidates)),
        ],
    );
    if let Some(stats) = topic_selection_stats.as_deref_mut() {
        let selected = stats.get("selected").and_then(Value::as_i64).unwrap_or(0);
        stats.insert("selected".to_string(), json!(selected + 1));
        if !topic_in_candidates {
            let fresh = stats.get("new").and_then(Value::as_i64).unwrap_or(0);
            stats.insert("new".to_string(), json!(fresh + 1));
        }
    }

    let new_version = page_version + 1;
    let existing_claims = existing_claims_of(existing_page);
    let retraction_keys: BTreeSet<(String, String, Vec<String>)> = retractions
        .unwrap_or(&[])
        .iter()
        .filter(|claim| claim.is_object())
        .map(claim_dedupe_key)
        .collect();
    let mut effective_claims: Vec<Value> = if mode == "generate" {
        Vec::new()
    } else {
        existing_claims
            .iter()
            .filter(|claim| !retraction_keys.contains(&claim_dedupe_key(claim)))
            .cloned()
            .collect()
    };
    let mut seen_claims: BTreeSet<(String, String, Vec<String>)> =
        effective_claims.iter().map(claim_dedupe_key).collect();
    let mut incoming_claims: Vec<Value> = claims.unwrap_or(&[]).to_vec();
    incoming_claims.extend(additions.unwrap_or(&[]).iter().cloned());
    for claim in incoming_claims {
        if !claim.is_object() {
            continue;
        }
        let key = claim_dedupe_key(&claim);
        if seen_claims.insert(key) {
            effective_claims.push(claim);
        }
    }

    let mut doc_ids: Vec<String> = Vec::new();
    let mut source_chunk_set: BTreeSet<String> = BTreeSet::new();
    for claim in &effective_claims {
        if let Some(doc) = claim.get("source_doc_id").and_then(Value::as_str) {
            if !doc.is_empty() && !doc_ids.iter().any(|existing| existing == doc) {
                doc_ids.push(doc.to_string());
            }
        }
        for chunk in wiki_claim_chunk_ids(claim) {
            source_chunk_set.insert(chunk);
        }
    }
    for doc in source_doc_ids.unwrap_or(&[]) {
        if !doc.is_empty() && !doc_ids.iter().any(|existing| existing == doc) {
            doc_ids.push(doc.clone());
        }
    }
    for chunk in &enriched_chunks {
        if let Some(cid) = ["id", "chunk_id"]
            .iter()
            .find_map(|key| chunk.get(*key))
            .and_then(|value| match value {
                Value::String(text) if !text.is_empty() => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            })
        {
            source_chunk_set.insert(cid);
        }
        if let Some(doc) = ["doc_id", "source_doc_id"]
            .iter()
            .find_map(|key| chunk.get(*key))
            .and_then(Value::as_str)
        {
            if !doc.is_empty() && !doc_ids.iter().any(|existing| existing == doc) {
                doc_ids.push(doc.to_string());
            }
        }
    }

    let embedding: Option<Vec<f32>> = match page_embedding {
        Some(vector) => Some(vector.to_vec()),
        None => {
            let mut embedding_text = if summary.is_empty() {
                truncate_chars_p8b(&content, 200)
            } else {
                summary.clone()
            };
            if embed_routing_context {
                let mut parts: Vec<String> = Vec::new();
                if !page_title.is_empty() {
                    parts.push(format!("title={page_title}"));
                }
                if !summary.is_empty() {
                    parts.push(format!("summary={summary}"));
                }
                if let Some(names) = entity_names {
                    if !names.is_empty() {
                        let sorted: BTreeSet<String> = names.iter().cloned().collect();
                        parts.push(format!(
                            "members={}",
                            sorted.into_iter().collect::<Vec<String>>().join(", ")
                        ));
                    }
                }
                if !content.is_empty() {
                    parts.push(format!("content={}", truncate_chars_p8b(&content, 500)));
                }
                embedding_text = parts.join("; ");
            }
            match embd {
                Some(embd) => match embd.embed(&[embedding_text.as_str()]).await {
                    Ok(vectors) => vectors.into_iter().next(),
                    Err(err) => {
                        tracing::warn!(error = %err, page = page_id, "wiki: page embedding failed");
                        None
                    }
                },
                None => None,
            }
        }
    };
    let vec_dim = embedding.as_ref().map(|vector| vector.len()).unwrap_or(768);
    let (content_ltks, content_sm_ltks) = tokenize_for_search(&content);
    let (title_tks, _) = tokenize_for_search(&title);
    let entity_names_kwd: Vec<Value> = {
        let mut names: BTreeSet<String> = entity_names
            .unwrap_or(&[])
            .iter()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect();
        if names.is_empty() {
            names.insert(page_title.to_string());
        }
        names.into_iter().map(Value::String).collect()
    };
    let claims_json = if effective_claims.is_empty() {
        "[]".to_string()
    } else {
        Value::Array(effective_claims.clone()).to_string()
    };
    let synthesis_version = if mode == "generate" || mode == "re-synthesize" {
        new_version
    } else {
        existing
            .get("synthesis_version_int")
            .map(|value| as_int(Some(value), 0))
            .unwrap_or(0)
    };

    let mut page = Map::new();
    page.insert(
        "id".to_string(),
        Value::String(stable_row_id(&[
            WIKI_PAGE_COMPILE_KWD.to_string(),
            kb_id.to_string(),
            page_id.to_string(),
        ])),
    );
    page.insert("slug_kwd".to_string(), Value::String(page_id.to_string()));
    page.insert("title_kwd".to_string(), Value::String(title.clone()));
    page.insert("md_with_weight".to_string(), Value::String(content.clone()));
    page.insert(
        "summary_with_weight".to_string(),
        Value::String(if summary.is_empty() {
            title.clone()
        } else {
            summary.clone()
        }),
    );
    page.insert(
        "entity_names_kwd".to_string(),
        Value::Array(entity_names_kwd),
    );
    page.insert(
        "source_chunk_ids".to_string(),
        Value::Array(source_chunk_set.into_iter().map(Value::String).collect()),
    );
    page.insert(
        "source_doc_ids".to_string(),
        Value::Array(doc_ids.into_iter().map(Value::String).collect()),
    );
    page.insert("claims".to_string(), Value::String(claims_json));
    page.insert("page_version_int".to_string(), json!(new_version));
    page.insert(
        "synthesis_version_int".to_string(),
        json!(synthesis_version),
    );
    page.insert(
        "page_type_kwd".to_string(),
        Value::String(page_type_kwd.to_string()),
    );
    page.insert("topic_kwd".to_string(), Value::String(topic.clone()));
    page.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    page.insert(
        "knowledge_graph_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    page.insert("title_tks".to_string(), Value::String(title_tks.join(" ")));
    page.insert(
        "content_ltks".to_string(),
        Value::String(content_ltks.join(" ")),
    );
    page.insert(
        "content_sm_ltks".to_string(),
        Value::String(content_sm_ltks.join(" ")),
    );
    if let Some(embedding) = &embedding {
        page.insert(format!("q_{vec_dim}_vec"), json!(embedding));
        page.insert("embedding".to_string(), json!(embedding));
    }

    let page_value = Value::Object(page);
    let mut lookup_condition = Map::new();
    lookup_condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
    );
    lookup_condition.insert("slug_kwd".to_string(), Value::String(page_id.to_string()));
    let fields: Vec<String> = vec!["slug_kwd".to_string()];
    let exists = inc_search_page(store, tenant_id, kb_id, &fields, &lookup_condition, 0, 1)
        .map(|rows| !rows.is_empty())
        .unwrap_or(false);
    if exists {
        let mut update_value = page_value.as_object().cloned().unwrap_or_default();
        update_value.remove("id");
        let mut update_condition = Map::new();
        update_condition.insert("slug_kwd".to_string(), Value::String(page_id.to_string()));
        if let Err(err) = store.update(&update_condition, &update_value, &index, kb_id) {
            tracing::warn!(error = %err, page = page_id, "wiki: page update failed");
        }
    } else if let Err(err) = store.insert(
        &[page_value.as_object().cloned().unwrap_or_default()],
        &index,
        kb_id,
    ) {
        tracing::warn!(error = %err, page = page_id, "wiki: page insert failed");
    }
    tracing::debug!(
        page = page_id,
        "wiki: page version history hook not ported (FileCommitService)"
    );
    Some(page_value)
}

#[cfg(test)]
mod wiki_incremental_part9_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::embed::Embedder;
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

    struct CapturingEmb {
        texts: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl Embedder for CapturingEmb {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            for text in texts {
                self.texts.lock().unwrap().push((*text).to_string());
            }
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    fn base_args<'a>(
        store: &'a dyn DocStore,
        chat: &'a dyn HarnessChat,
        embd: Option<&'a dyn Embedder>,
    ) -> RefinePageArgs<'a> {
        RefinePageArgs {
            store,
            mode: "generate",
            page_id: "concept/alpha",
            page_title: "Alpha",
            existing_page: None,
            page_type_kwd: "concept",
            additions: None,
            retractions: None,
            source_chunks: None,
            claims: None,
            available_pages: None,
            contextual_hints: "",
            chat,
            embd,
            tenant_id: "t1",
            kb_id: "kb1",
            page_version: 0,
            entity_names: None,
            page_embedding: None,
            embed_routing_context: false,
            source_doc_ids: None,
            topic_candidates: None,
            topic_selection_stats: None,
            topic_embeddings: None,
            topic_pool: None,
            member_evidence: None,
        }
    }

    #[tokio::test]
    async fn generate_flow_parses_metadata_and_persists() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "SUMMARY: sum line\nTITLE: LLM Title\nTOPIC: alpha\n# Body\n\nGrounded text"
                .to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let emb = CapturingEmb {
            texts: Mutex::new(Vec::new()),
        };
        let names = vec!["Alpha".to_string()];
        let mut args = base_args(&store, &chat, Some(&emb));
        args.entity_names = Some(&names);
        let page = wiki_refine_page(args).await.expect("page");
        assert_eq!(page["slug_kwd"], json!("concept/alpha"));
        assert_eq!(page["title_kwd"], json!("Alpha"));
        assert_eq!(page["topic_kwd"], json!("alpha"));
        assert_eq!(page["summary_with_weight"], json!("sum line"));
        assert!(
            page["md_with_weight"]
                .as_str()
                .unwrap()
                .contains("Grounded text")
        );
        assert_eq!(page["claims"], json!("[]"));
        assert_eq!(page["page_version_int"], json!(1));
        assert_eq!(page["synthesis_version_int"], json!(1));
        assert_eq!(page["q_2_vec"], json!([1.0, 0.0]));
        assert!(page.get("embedding").is_some());
        assert_eq!(page["entity_names_kwd"], json!(["Alpha"]));
        let promp = chat.calls.lock().unwrap()[0].clone();
        assert!(promp.contains("- Page ID: concept/alpha"));
        assert!(promp.contains("## Extracted Claims (checklist)\n(no claims)"));
        let persisted = search_existing_pages(&store, "t1", "kb1", &["slug_kwd".to_string()]);
        assert_eq!(persisted.len(), 1);
    }

    #[tokio::test]
    async fn modify_flow_applies_claims_diff() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "TOPIC: alpha\n# Updated\n\nBody".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let existing = json!({
            "title_kwd": "Old Title",
            "md_with_weight": "OLD",
            "topic_kwd": "alpha",
            "synthesis_version_int": 2,
            "claims": serde_json::to_string(&vec![
                json!({"statement": "s1", "source_doc_id": "d1", "chunk_ids": ["c1"]}),
                json!({"statement": "s2", "source_doc_id": "d1", "chunk_ids": ["c2"]})
            ]).unwrap()
        });
        let retractions =
            vec![json!({"statement": "s1", "source_doc_id": "d1", "chunk_ids": ["c1"]})];
        let additions =
            vec![json!({"statement": "s3", "source_doc_id": "d2", "chunk_ids": ["c3"]})];
        let mut args = base_args(&store, &chat, None);
        args.mode = "modify";
        args.page_title = "Old Title";
        args.existing_page = Some(&existing);
        args.retractions = Some(&retractions);
        args.additions = Some(&additions);
        args.page_version = 3;
        let page = wiki_refine_page(args).await.expect("page");
        assert_eq!(page["title_kwd"], json!("Old Title"));
        assert_eq!(page["page_version_int"], json!(4));
        assert_eq!(page["synthesis_version_int"], json!(2));
        let claims: Value = serde_json::from_str(page["claims"].as_str().unwrap()).unwrap();
        let statements: Vec<&str> = claims
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|claim| claim.get("statement").and_then(Value::as_str))
            .collect();
        assert_eq!(statements, vec!["s2", "s3"]);
        assert_eq!(page["source_doc_ids"], json!(["d1", "d2"]));
        assert_eq!(page["source_chunk_ids"], json!(["c2", "c3"]));
    }

    #[tokio::test]
    async fn delete_flow_removes_row() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "unused".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "concept/alpha"
        });
        store
            .insert(
                &[row.as_object().cloned().unwrap()],
                &crate::harness::knowlege_dataset_nav::index_name("t1"),
                "kb1",
            )
            .unwrap();
        let mut args = base_args(&store, &chat, None);
        args.mode = "delete";
        let outcome = wiki_refine_page(args).await;
        assert!(outcome.is_none());
        let remaining = search_existing_pages(&store, "t1", "kb1", &["slug_kwd".to_string()]);
        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn empty_response_keeps_existing() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let existing = json!({"title_kwd": "Keep"});
        let mut args = base_args(&store, &chat, None);
        args.existing_page = Some(&existing);
        let outcome = wiki_refine_page(args).await.expect("existing");
        assert_eq!(outcome["title_kwd"], json!("Keep"));
    }

    #[tokio::test]
    async fn routing_context_embedding_text() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "SUMMARY: s\n# Body".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let emb = CapturingEmb {
            texts: Mutex::new(Vec::new()),
        };
        let names = vec!["Alpha".to_string()];
        let mut args = base_args(&store, &chat, Some(&emb));
        args.entity_names = Some(&names);
        args.embed_routing_context = true;
        let _ = wiki_refine_page(args).await.expect("page");
        let captured = emb.texts.lock().unwrap().clone();
        let embed_text = captured
            .iter()
            .find(|text| text.contains("title=Alpha"))
            .cloned();
        let embed_text = embed_text.expect("routing text");
        assert!(embed_text.contains("summary=s"));
        assert!(embed_text.contains("members=Alpha"));
        assert!(embed_text.contains("content="));
    }
}

// ---------------------------------------------------------------------------
// Part 9a — LLM grouping, routing helpers and deterministic clustering
// (`_wiki_llm_partition_candidate` .. `_wiki_expand_route_candidates` plus
// `_wiki_cluster_entities`).
//
// Adaptation note: numpy matrix work becomes plain Rust loops; concurrency is
// sequential (semaphore/gather semantics preserved observably).
// ---------------------------------------------------------------------------

/// `_wiki_llm_partition_candidate`.
pub async fn wiki_llm_partition_candidate(
    chat: &dyn HarnessChat,
    entities: &[Value],
) -> Option<Vec<Vec<Value>>> {
    if entities.len() <= 1 {
        return Some(vec![entities.to_vec()]);
    }
    let numbered = entities
        .iter()
        .enumerate()
        .map(|(idx, entity)| format!("{idx}: {}", wiki_entity_planning_text(entity, 3)))
        .collect::<Vec<String>>()
        .join("\n");
    let prompt = format!(
        "Group the following knowledge-base entities into coherent encyclopedia pages.\nEach page must have one clear subject. Group entities only when a reader would naturally expect them to be explained on the same page. Do not use entity types as grouping rules because types are user-defined.\n\nReturn ONLY a JSON array of arrays of integer IDs, for example [[0, 2], [1]].\nEvery ID from 0 through {} must appear exactly once. A group may contain at most {} IDs.\n\nEntities:\n{numbered}",
        entities.len() - 1,
        PAGE_CLUSTER_HARD_MAX_SIZE
    );
    let response = match chat_mdl_ask(
        chat,
        "You plan concise, semantically coherent encyclopedia pages.",
        &prompt,
        0.0,
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, "wiki: LLM page grouping call failed");
            return None;
        }
    };
    let raw_groups = parse_json_array(&response)?;
    let mut seen: BTreeSet<i64> = BTreeSet::new();
    let mut groups: Vec<Vec<Value>> = Vec::new();
    for raw_group in raw_groups {
        let Some(items) = raw_group.as_array() else {
            return None;
        };
        if items.is_empty() || items.len() > PAGE_CLUSTER_HARD_MAX_SIZE {
            return None;
        }
        let mut indices: Vec<usize> = Vec::new();
        for raw_idx in items {
            let Some(idx) = raw_idx.as_i64() else {
                return None;
            };
            if idx < 0 || idx as usize >= entities.len() || seen.contains(&idx) {
                return None;
            }
            seen.insert(idx);
            indices.push(idx as usize);
        }
        groups.push(
            indices
                .into_iter()
                .map(|idx| entities[idx].clone())
                .collect(),
        );
    }
    if seen != (0..entities.len() as i64).collect::<BTreeSet<i64>>() {
        return None;
    }
    Some(groups)
}

/// `_wiki_llm_group_entities`: embedding candidates + LLM final groups.
pub async fn wiki_llm_group_entities(
    chat: &dyn HarnessChat,
    entities: &[Value],
    embeddings: &[Vec<f32>],
    kb_id: &str,
) -> Vec<Vec<Value>> {
    if entities.len() <= 1 {
        wiki_log_stats(
            "PLAN",
            "group_summary",
            &[
                ("kb_id", json!(kb_id)),
                ("before", json!(entities.len())),
                ("after", json!(entities.len())),
                ("reduction_count", json!(0)),
                ("merged_group_count", json!(0)),
            ],
        );
        return vec![entities.to_vec()];
    }
    let candidate_count =
        1.max(((entities.len() as f64) / WIKI_GROUP_LLM_CANDIDATE_SIZE as f64).ceil() as usize);
    let candidates = match wiki_cluster_entities(entities, embeddings, Some(candidate_count)) {
        Ok(clusters) => clusters,
        Err(err) => {
            tracing::warn!(error = %err, "wiki: candidate clustering failed; falling back to single candidate");
            vec![entities.to_vec()]
        }
    };
    let mut all_groups: Vec<Vec<Value>> = Vec::new();
    for candidate in &candidates {
        let mut groups: Option<Vec<Vec<Value>>> = None;
        for attempt in 0..2 {
            groups = wiki_llm_partition_candidate(chat, candidate).await;
            if groups.is_some() {
                break;
            }
            tracing::warn!(
                attempt = attempt + 1,
                "wiki: LLM page grouping attempt failed"
            );
        }
        match groups {
            Some(groups) => {
                let merged_groups: Vec<Vec<String>> = groups
                    .iter()
                    .filter(|group| group.len() > 1)
                    .map(|group| {
                        group
                            .iter()
                            .map(|entity| {
                                ["entity_name", "term"]
                                    .iter()
                                    .find_map(|key| entity.get(*key).and_then(Value::as_str))
                                    .unwrap_or("")
                                    .to_string()
                            })
                            .collect()
                    })
                    .collect();
                for members in &merged_groups {
                    wiki_log_stats(
                        "PLAN",
                        "llm_page_group",
                        &[
                            ("kb_id", json!(kb_id)),
                            ("member_count", json!(members.len())),
                            (
                                "members",
                                Value::Array(members.iter().cloned().map(Value::String).collect()),
                            ),
                        ],
                    );
                }
                wiki_log_stats(
                    "PLAN",
                    "llm_group_candidate",
                    &[
                        ("kb_id", json!(kb_id)),
                        ("before", json!(candidate.len())),
                        ("after", json!(groups.len())),
                        (
                            "reduction_count",
                            json!(
                                groups
                                    .iter()
                                    .map(|group| group.len().saturating_sub(1))
                                    .sum::<usize>()
                            ),
                        ),
                        ("merged_group_count", json!(merged_groups.len())),
                    ],
                );
                all_groups.extend(groups);
            }
            None => {
                wiki_log_stats(
                    "PLAN",
                    "llm_group_unresolved",
                    &[
                        ("kb_id", json!(kb_id)),
                        ("before", json!(candidate.len())),
                        ("after", json!(candidate.len())),
                        ("retry_count", json!(2)),
                    ],
                );
                all_groups.extend(candidate.iter().cloned().map(|entity| vec![entity]));
            }
        }
    }
    wiki_log_stats(
        "PLAN",
        "group_summary",
        &[
            ("kb_id", json!(kb_id)),
            ("before", json!(entities.len())),
            ("after", json!(all_groups.len())),
            (
                "reduction_count",
                json!(
                    all_groups
                        .iter()
                        .map(|group| group.len().saturating_sub(1))
                        .sum::<usize>()
                ),
            ),
            (
                "merged_group_count",
                json!(all_groups.iter().filter(|group| group.len() > 1).count()),
            ),
        ],
    );
    all_groups
}

/// `_wiki_llm_route_batches`: existing-page or NEW per entity, batched.
pub async fn wiki_llm_route_batches(
    chat: &dyn HarnessChat,
    route_items: &[(Value, Vec<Value>)],
) -> BTreeMap<usize, String> {
    if route_items.is_empty() {
        return BTreeMap::new();
    }
    let indexed: Vec<(usize, Value, Vec<Value>)> = route_items
        .iter()
        .enumerate()
        .map(|(idx, (entity, candidates))| (idx, entity.clone(), candidates.clone()))
        .collect();
    let mut decisions = route_batch_group(chat, &indexed).await;
    let missing: Vec<(usize, Value, Vec<Value>)> = indexed
        .iter()
        .filter(|(id, _, _)| !decisions.contains_key(id))
        .cloned()
        .collect();
    if !missing.is_empty() {
        decisions.extend(route_batch_group(chat, &missing).await);
    }
    decisions
}

async fn route_batch_group(
    chat: &dyn HarnessChat,
    items: &[(usize, Value, Vec<Value>)],
) -> BTreeMap<usize, String> {
    let mut results: BTreeMap<usize, String> = BTreeMap::new();
    for batch in items.chunks(WIKI_ROUTE_LLM_BATCH_SIZE.max(1)) {
        results.extend(route_one_batch(chat, batch).await);
    }
    results
}

async fn route_one_batch(
    chat: &dyn HarnessChat,
    batch: &[(usize, Value, Vec<Value>)],
) -> BTreeMap<usize, String> {
    let mut lines: Vec<String> = Vec::new();
    let mut allowed: BTreeMap<usize, BTreeSet<String>> = BTreeMap::new();
    for (item_id, entity, candidates) in batch {
        let mut options: Vec<Value> = Vec::new();
        let mut allowed_set: BTreeSet<String> = BTreeSet::new();
        allowed_set.insert("NEW".to_string());
        for candidate in candidates {
            let Some(page_id) = candidate.get("page_id").and_then(Value::as_str) else {
                continue;
            };
            allowed_set.insert(page_id.to_string());
            let score = candidate
                .get("score")
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let rounded = (score * 10000.0).round() / 10000.0;
            options.push(json!({
                "page": page_id,
                "title": candidate.get("title").and_then(Value::as_str).unwrap_or(""),
                "summary": candidate.get("summary").and_then(Value::as_str).unwrap_or(""),
                "members": candidate.get("members").cloned().unwrap_or(Value::Array(Vec::new())),
                "similarity": rounded,
                "signals": candidate.get("signals").cloned().unwrap_or(Value::Array(Vec::new())),
                "cooccurrence_count": candidate.get("cooccurrence_count").and_then(Value::as_i64).unwrap_or(0),
            }));
        }
        allowed.insert(*item_id, allowed_set);
        lines.push(
            json!({
                "id": item_id,
                "entity": wiki_entity_planning_text(entity, 3),
                "options": options,
            })
            .to_string(),
        );
    }
    let prompt = format!(
        "Route each entity to the single existing encyclopedia page whose subject truly covers it, or choose NEW when none does. Similarity is candidate retrieval evidence, not proof. Prefer an existing page only when the semantic fit is clear.\n\nReturn ONLY a JSON array like [{{\"id\": 0, \"page\": \"entity/example\"}}, {{\"id\": 1, \"page\": \"NEW\"}}].\n\nItems:\n{}",
        lines.join("\n")
    );
    let response = match chat_mdl_ask(
        chat,
        "You route entities to semantically appropriate encyclopedia pages.",
        &prompt,
        0.0,
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(error = %err, "wiki: LLM page routing batch failed");
            return BTreeMap::new();
        }
    };
    let Some(decisions) = parse_json_array(&response) else {
        return BTreeMap::new();
    };
    let mut result: BTreeMap<usize, String> = BTreeMap::new();
    for decision in decisions {
        let Some(obj) = decision.as_object() else {
            continue;
        };
        let Some(item_id) = obj.get("id").and_then(Value::as_i64) else {
            continue;
        };
        let Some(page_id) = obj.get("page").and_then(Value::as_str) else {
            continue;
        };
        let key = item_id as usize;
        if let Some(allowed_set) = allowed.get(&key) {
            if allowed_set.contains(page_id) {
                result.insert(key, page_id.to_string());
            }
        }
    }
    result
}

/// `_wiki_route_page_candidate`.
pub fn wiki_route_page_candidate(page_id: &str, page: &Value, score: f64) -> Value {
    let title = page
        .get("title_kwd")
        .map(|value| match value {
            Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
            other => other.as_str().unwrap_or(""),
        })
        .unwrap_or("");
    let mut members = as_str_list(page.get("entity_names_kwd"));
    members.truncate(12);
    json!({
        "score": if score == 0.0 { 0.0 } else { score },
        "page_id": page_id,
        "title": title,
        "summary": page.get("summary_with_weight").and_then(Value::as_str).unwrap_or(""),
        "members": members.into_iter().map(Value::String).collect::<Vec<Value>>(),
        "signals": [],
        "cooccurrence_count": 0,
    })
}

/// `_wiki_expand_route_candidates`: merge retrieval, ownership and graph
/// evidence into a ranked candidate list.
pub fn wiki_expand_route_candidates(
    entity: &Value,
    dense_candidates: &[Value],
    existing_pages: &Map<String, Value>,
    entity_pages: &BTreeMap<String, BTreeSet<String>>,
    chunk_pages: &BTreeMap<String, BTreeSet<String>>,
    include_candidate_neighbors: bool,
) -> Vec<Value> {
    let mut candidates: BTreeMap<String, Value> = BTreeMap::new();
    for candidate in dense_candidates {
        let Some(page_id) = candidate.get("page_id").and_then(Value::as_str) else {
            continue;
        };
        if existing_pages.contains_key(page_id) {
            candidates.insert(page_id.to_string(), candidate.clone());
        }
    }

    let mut add = |candidates: &mut BTreeMap<String, Value>,
                   page_id: &str,
                   signal: &str,
                   cooccurrence_count: i64| {
        let Some(page) = existing_pages.get(page_id) else {
            return;
        };
        let entry = candidates
            .entry(page_id.to_string())
            .or_insert_with(|| wiki_route_page_candidate(page_id, page, 0.0));
        let mut signals: BTreeSet<String> = entry
            .get("signals")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        signals.insert(signal.to_string());
        let existing_count = entry
            .get("cooccurrence_count")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if let Some(obj) = entry.as_object_mut() {
            obj.insert(
                "signals".to_string(),
                Value::Array(signals.into_iter().map(Value::String).collect()),
            );
            obj.insert(
                "cooccurrence_count".to_string(),
                json!(existing_count.max(cooccurrence_count)),
            );
        }
    };

    let entity_name = ["entity_name", "term"]
        .iter()
        .find_map(|key| entity.get(*key).and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    if let Some(page_ids) = entity_pages.get(&normalize_key(&entity_name)) {
        for page_id in page_ids {
            add(&mut candidates, page_id, "current_owner", 0);
        }
    }
    if let Some(relations) = entity.get("relations").and_then(Value::as_array) {
        for relation in relations {
            let counterpart = ["entity", "counterpart"]
                .iter()
                .find_map(|key| relation.get(*key).and_then(Value::as_str))
                .unwrap_or("")
                .trim()
                .to_string();
            if let Some(page_ids) = entity_pages.get(&normalize_key(&counterpart)) {
                for page_id in page_ids {
                    add(&mut candidates, page_id, "relation", 0);
                }
            }
        }
    }
    let mut cooccurrence: BTreeMap<String, i64> = BTreeMap::new();
    for chunk_id in as_str_list(entity.get("source_chunk_ids")) {
        if let Some(page_ids) = chunk_pages.get(&chunk_id) {
            for page_id in page_ids {
                *cooccurrence.entry(page_id.clone()).or_insert(0) += 1;
            }
        }
    }
    for (page_id, count) in &cooccurrence {
        add(&mut candidates, page_id, "cooccurrence", *count);
    }

    if include_candidate_neighbors {
        let initial: Vec<String> = candidates.keys().cloned().collect();
        for page_id in initial {
            let page = existing_pages
                .get(&page_id)
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut neighbor_refs: Vec<String> = as_str_list(page.get("outlinks_kwd"));
            neighbor_refs.extend(as_str_list(page.get("related_kb_pages_kwd")));
            for neighbor_ref in neighbor_refs {
                if existing_pages.contains_key(&neighbor_ref) {
                    add(&mut candidates, &neighbor_ref, "candidate_neighbor", 0);
                    continue;
                }
                if let Some(page_ids) = entity_pages.get(&normalize_key(&neighbor_ref)) {
                    for neighbor_id in page_ids {
                        add(&mut candidates, neighbor_id, "candidate_neighbor", 0);
                    }
                }
            }
        }
    }

    let priority = |signal: &str| -> i32 {
        match signal {
            "current_owner" => 0,
            "relation" => 1,
            "cooccurrence" => 2,
            "embedding" => 3,
            _ => 4,
        }
    };
    let mut ranked: Vec<Value> = candidates.into_values().collect();
    ranked.sort_by(|left, right| {
        let rank_of = |candidate: &Value| -> i32 {
            candidate
                .get("signals")
                .and_then(Value::as_array)
                .map(|signals| {
                    signals
                        .iter()
                        .filter_map(Value::as_str)
                        .map(priority)
                        .min()
                        .unwrap_or(4)
                })
                .unwrap_or(4)
        };
        let left_rank = rank_of(left);
        let right_rank = rank_of(right);
        left_rank
            .cmp(&right_rank)
            .then_with(|| {
                let left_count = left
                    .get("cooccurrence_count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let right_count = right
                    .get("cooccurrence_count")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                right_count.cmp(&left_count)
            })
            .then_with(|| {
                let left_score = left.get("score").and_then(Value::as_f64).unwrap_or(0.0);
                let right_score = right.get("score").and_then(Value::as_f64).unwrap_or(0.0);
                right_score
                    .partial_cmp(&left_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                let left_id = left.get("page_id").and_then(Value::as_str).unwrap_or("");
                let right_id = right.get("page_id").and_then(Value::as_str).unwrap_or("");
                left_id.cmp(right_id)
            })
    });
    ranked.truncate(PAGE_ROUTER_MAX_CANDIDATES);
    ranked
}

/// `_wiki_normalize_rows`: L2-normalize each row (zero rows unchanged).
pub fn wiki_normalize_rows(matrix: &[Vec<f32>]) -> Vec<Vec<f32>> {
    matrix
        .iter()
        .map(|row| {
            let norm = (row.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>()).sqrt();
            if norm > 0.0 {
                row.iter().map(|x| (*x as f64 / norm) as f32).collect()
            } else {
                row.clone()
            }
        })
        .collect()
}

/// `_wiki_cluster_entities`: deterministic capacity-constrained spherical
/// k-means over entity embeddings.
pub fn wiki_cluster_entities(
    entities: &[Value],
    embeddings: &[Vec<f32>],
    target_count: Option<usize>,
) -> Result<Vec<Vec<Value>>, String> {
    if entities.len() <= 1 {
        return Ok(vec![entities.to_vec()]);
    }
    let n = entities.len();
    let dim = embeddings.first().map(Vec::len).unwrap_or(0);
    let shapes_ok =
        dim > 0 && embeddings.len() == n && embeddings.iter().all(|row| row.len() == dim);
    if !shapes_ok {
        return Err("entity embeddings must be a two-dimensional matrix".to_string());
    }
    let matrix = wiki_normalize_rows(embeddings);
    let target = match target_count {
        Some(target) => target,
        None => {
            if n <= PAGE_CLUSTER_MIN_PAGES {
                n
            } else {
                let raw = ((n as f64) / PAGE_CLUSTER_ITEMS_PER_PAGE as f64).round() as usize;
                raw.clamp(PAGE_CLUSTER_MIN_PAGES, PAGE_CLUSTER_MAX_PAGES)
            }
        }
    };
    let target = target.clamp(1, n);

    let names: Vec<String> = entities
        .iter()
        .map(|entity| {
            ["entity_name", "name", "term"]
                .iter()
                .find_map(|key| entity.get(*key).and_then(Value::as_str))
                .unwrap_or("")
                .to_string()
        })
        .collect();
    let evidence: Vec<usize> = entities
        .iter()
        .map(|entity| {
            entity
                .get("claims")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or(0)
        })
        .collect();
    let mut stable_order: Vec<usize> = (0..n).collect();
    stable_order.sort_by(|left, right| {
        names[*left]
            .to_lowercase()
            .cmp(&names[*right].to_lowercase())
            .then_with(|| names[*left].cmp(&names[*right]))
            .then_with(|| left.cmp(right))
    });

    let first = (0..n)
        .min_by(|left, right| {
            evidence[*right]
                .cmp(&evidence[*left])
                .then_with(|| {
                    names[*left]
                        .to_lowercase()
                        .cmp(&names[*right].to_lowercase())
                })
                .then_with(|| names[*left].cmp(&names[*right]))
                .then_with(|| left.cmp(right))
        })
        .unwrap_or(0);
    let mut center_indices: Vec<usize> = vec![first];
    let mut selected: BTreeSet<usize> = [first].into_iter().collect();
    while center_indices.len() < target {
        let mut nearest: Vec<f64> = vec![f64::NEG_INFINITY; n];
        for idx in 0..n {
            let best = center_indices
                .iter()
                .map(|center| dot_f32(&matrix[idx], &matrix[*center]) as f64)
                .fold(f64::NEG_INFINITY, f64::max);
            nearest[idx] = best;
        }
        let candidate = stable_order
            .iter()
            .filter(|idx| !selected.contains(*idx))
            .min_by(|left, right| {
                nearest[**left]
                    .partial_cmp(&nearest[**right])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        names[**left]
                            .to_lowercase()
                            .cmp(&names[**right].to_lowercase())
                    })
                    .then_with(|| names[**left].cmp(&names[**right]))
                    .then_with(|| left.cmp(right))
            });
        match candidate {
            Some(candidate) => {
                center_indices.push(*candidate);
                selected.insert(*candidate);
            }
            None => break,
        }
    }

    let mut centroids: Vec<Vec<f32>> = center_indices
        .iter()
        .map(|center| matrix[*center].clone())
        .collect();
    let hard_capacity =
        PAGE_CLUSTER_HARD_MAX_SIZE.max(((n as f64) / (target as f64)).ceil() as usize);
    let mut previous_assignments: Option<Vec<i64>> = None;
    let mut assignments: Vec<i64> = vec![0; n];

    for _ in 0..PAGE_CLUSTER_MAX_ITERATIONS {
        let mut scores: Vec<Vec<f64>> = vec![vec![0.0; target]; n];
        for idx in 0..n {
            for cid in 0..target {
                scores[idx][cid] = dot_f32(&matrix[idx], &centroids[cid]) as f64;
            }
        }
        let mut sizes: Vec<usize> = vec![0; target];
        assignments = vec![-1; n];
        let mut ranked_entities = stable_order.clone();
        ranked_entities.sort_by(|left, right| {
            let preference = |idx: &usize| -> f64 {
                if target > 1 {
                    let mut row = scores[*idx].clone();
                    row.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                    row[0] - row[1]
                } else {
                    scores[*idx][0]
                }
            };
            preference(right)
                .partial_cmp(&preference(left))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    names[*left]
                        .to_lowercase()
                        .cmp(&names[*right].to_lowercase())
                })
                .then_with(|| names[*left].cmp(&names[*right]))
                .then_with(|| left.cmp(right))
        });
        for idx in &ranked_entities {
            let mut ranked_clusters: Vec<usize> = (0..target).collect();
            ranked_clusters.sort_by(|left, right| {
                scores[*idx][*right]
                    .partial_cmp(&scores[*idx][*left])
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.cmp(right))
            });
            let chosen = ranked_clusters
                .iter()
                .find(|cid| sizes[**cid] < hard_capacity)
                .copied()
                .unwrap_or(ranked_clusters[0]);
            assignments[*idx] = chosen as i64;
            sizes[chosen] += 1;
        }

        let empty_clusters: Vec<usize> = (0..target).filter(|cid| sizes[*cid] == 0).collect();
        for empty_cid in empty_clusters {
            let movable: Vec<usize> = stable_order
                .iter()
                .copied()
                .filter(|idx| sizes[assignments[*idx] as usize] > 1)
                .collect();
            if movable.is_empty() {
                break;
            }
            let moved = movable
                .iter()
                .min_by(|left, right| {
                    let left_score = scores[**left][assignments[**left] as usize];
                    let right_score = scores[**right][assignments[**right] as usize];
                    left_score
                        .partial_cmp(&right_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| {
                            names[**left]
                                .to_lowercase()
                                .cmp(&names[**right].to_lowercase())
                        })
                        .then_with(|| names[**left].cmp(&names[**right]))
                        .then_with(|| left.cmp(right))
                })
                .copied()
                .unwrap_or(movable[0]);
            sizes[assignments[moved] as usize] -= 1;
            assignments[moved] = empty_cid as i64;
            sizes[empty_cid] = 1;
        }

        let mut new_centroids: Vec<Vec<f32>> = Vec::with_capacity(target);
        for cid in 0..target {
            let member_indices: Vec<usize> = (0..n)
                .filter(|idx| assignments[*idx] == cid as i64)
                .collect();
            if member_indices.is_empty() {
                new_centroids.push(centroids[cid].clone());
                continue;
            }
            let mut centroid = vec![0.0f32; dim];
            for idx in &member_indices {
                for (pos, value) in matrix[*idx].iter().enumerate() {
                    centroid[pos] += value;
                }
            }
            let count = member_indices.len() as f32;
            for value in centroid.iter_mut() {
                *value /= count;
            }
            let norm = (centroid
                .iter()
                .map(|x| (*x as f64) * (*x as f64))
                .sum::<f64>())
            .sqrt();
            if norm > 0.0 {
                for value in centroid.iter_mut() {
                    *value = (*value as f64 / norm) as f32;
                }
                new_centroids.push(centroid);
            } else {
                new_centroids.push(centroids[cid].clone());
            }
        }
        let movement = new_centroids
            .iter()
            .zip(centroids.iter())
            .map(|(new_centroid, old_centroid)| {
                new_centroid
                    .iter()
                    .zip(old_centroid.iter())
                    .map(|(a, b)| ((*a - *b) as f64).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(0.0f64, f64::max);
        let converged_assignments = previous_assignments.as_ref() == Some(&assignments);
        centroids = new_centroids;
        if converged_assignments || movement < PAGE_CLUSTER_CONVERGENCE_EPSILON {
            break;
        }
        previous_assignments = Some(assignments.clone());
    }

    let mut clusters: Vec<Vec<Value>> = Vec::new();
    for cid in 0..target {
        let members: Vec<Value> = stable_order
            .iter()
            .filter(|idx| assignments[**idx] == cid as i64)
            .map(|idx| entities[*idx].clone())
            .collect();
        if !members.is_empty() {
            clusters.push(members);
        }
    }
    clusters.sort_by(|left, right| {
        let left_name = left
            .first()
            .and_then(|entity| entity.get("entity_name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let right_name = right
            .first()
            .and_then(|entity| entity.get("entity_name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        left_name
            .to_lowercase()
            .cmp(&right_name.to_lowercase())
            .then_with(|| left_name.cmp(right_name))
    });
    Ok(clusters)
}

fn dot_f32(left: &[f32], right: &[f32]) -> f32 {
    left.iter().zip(right.iter()).map(|(a, b)| a * b).sum()
}

#[cfg(test)]
mod wiki_incremental_part10_tests {
    use super::*;
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

    fn entities(names: &[&str]) -> Vec<Value> {
        names
            .iter()
            .map(|name| json!({"entity_name": name, "claims": [], "source_chunk_ids": []}))
            .collect()
    }

    #[tokio::test]
    async fn partition_validation() {
        let chat = FakeChat {
            reply: "[[0, 2], [1]]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let groups = wiki_llm_partition_candidate(&chat, &entities(&["A", "B", "C"]))
            .await
            .expect("groups");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[0][0]["entity_name"], json!("A"));
        assert_eq!(groups[0][1]["entity_name"], json!("C"));
        assert_eq!(groups[1][0]["entity_name"], json!("B"));

        let bad = FakeChat {
            reply: "[[0, 9]]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        assert!(
            wiki_llm_partition_candidate(&bad, &entities(&["A", "B"]))
                .await
                .is_none()
        );
        let single = FakeChat {
            reply: "unused".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let one = wiki_llm_partition_candidate(&single, &entities(&["A"]))
            .await
            .expect("single");
        assert_eq!(one.len(), 1);
        assert!(single.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn group_entities_uses_clusters_and_llm() {
        let chat = FakeChat {
            reply: "[[0, 2], [1]]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let ents = entities(&["Alpha", "Beta", "Gamma"]);
        let embeddings = vec![vec![1.0f32, 0.0], vec![0.9, 0.1], vec![0.0, 1.0]];
        let groups = wiki_llm_group_entities(&chat, &ents, &embeddings, "kb1").await;
        let total: usize = groups.iter().map(Vec::len).sum();
        assert_eq!(total, 3);
        assert_eq!(groups.len(), 2);
    }

    #[tokio::test]
    async fn route_batches_validates_pages() {
        let chat = FakeChat {
            reply: "[{\"id\": 0, \"page\": \"entity/a\"}]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let candidates = vec![json!({"page_id": "entity/a", "title": "A", "score": 0.51})];
        let items = vec![(json!({"entity_name": "Alpha"}), candidates.clone())];
        let decisions = wiki_llm_route_batches(&chat, &items).await;
        assert_eq!(decisions.get(&0).map(String::as_str), Some("entity/a"));

        let invalid = FakeChat {
            reply: "[{\"id\": 0, \"page\": \"entity/z\"}]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let decisions2 = wiki_llm_route_batches(&invalid, &items).await;
        assert!(decisions2.is_empty());
    }

    #[test]
    fn expand_candidates_priorities() {
        let mut existing: Map<String, Value> = Map::new();
        existing.insert(
            "p1".to_string(),
            json!({"title_kwd": "One", "entity_names_kwd": ["X"]}),
        );
        existing.insert(
            "p2".to_string(),
            json!({"title_kwd": "Two", "entity_names_kwd": ["Alpha"]}),
        );
        existing.insert(
            "p3".to_string(),
            json!({"title_kwd": "Three", "entity_names_kwd": ["Beta"]}),
        );
        let mut entity_pages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        entity_pages.insert(
            "alpha".to_string(),
            ["p2"].into_iter().map(str::to_string).collect(),
        );
        entity_pages.insert(
            "beta".to_string(),
            ["p3"].into_iter().map(str::to_string).collect(),
        );
        let mut chunk_pages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        chunk_pages.insert(
            "c1".to_string(),
            ["p2", "p3"].into_iter().map(str::to_string).collect(),
        );
        let entity = json!({
            "entity_name": "Alpha",
            "relations": [{"entity": "Beta"}],
            "source_chunk_ids": ["c1"]
        });
        let dense = vec![
            json!({"page_id": "p1", "score": 0.9}),
            json!({"page_id": "px", "score": 0.8}),
        ];
        let ranked = wiki_expand_route_candidates(
            &entity,
            &dense,
            &existing,
            &entity_pages,
            &chunk_pages,
            false,
        );
        assert_eq!(ranked.len(), 3);
        assert_eq!(ranked[0]["page_id"], json!("p2"));
        let signals: Vec<String> = ranked[0]["signals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        assert!(signals.contains(&"current_owner".to_string()));
        assert!(signals.contains(&"cooccurrence".to_string()));
        assert_eq!(ranked[0]["cooccurrence_count"], json!(1));
        let p3 = ranked
            .iter()
            .find(|candidate| candidate["page_id"] == json!("p3"))
            .unwrap();
        let p3_signals: Vec<String> = p3["signals"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();
        assert!(p3_signals.contains(&"relation".to_string()));
        let p1 = ranked
            .iter()
            .find(|candidate| candidate["page_id"] == json!("p1"))
            .unwrap();
        assert_eq!(p1["score"], json!(0.9));
    }

    #[test]
    fn cluster_entities_deterministic() {
        let ents = entities(&["A", "B", "C", "D"]);
        let embeddings = vec![
            vec![1.0f32, 0.0],
            vec![0.9, 0.1],
            vec![0.0, 1.0],
            vec![0.1, 0.9],
        ];
        let first = wiki_cluster_entities(&ents, &embeddings, Some(2)).expect("clusters");
        let second = wiki_cluster_entities(&ents, &embeddings, Some(2)).expect("clusters2");
        assert_eq!(first.len(), 2);
        let total: usize = first.iter().map(Vec::len).sum();
        assert_eq!(total, 4);
        assert_eq!(first, second);
        assert!(wiki_cluster_entities(&ents, &embeddings[..2], Some(2)).is_err());
    }
}

// ---------------------------------------------------------------------------
// Part 9b — Mode B page router (`_wiki_page_router`).
//
// Adaptation note: the local dense search has no min-score filter, so scores
// are computed client-side from the returned vectors; Python's `id(entity)`
// embedding table becomes explicit `(entity, vector)` pairing; search fan-out
// is sequential.
// ---------------------------------------------------------------------------

fn push_assignment(assignments: &mut Map<String, Value>, key: &str, entity: &Value) {
    let bucket = assignments
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(items) = bucket.as_array_mut() {
        items.push(entity.clone());
    }
}

/// `_wiki_page_router`: KNN candidates then LLM decisions.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_page_router(
    store: &dyn DocStore,
    affected_entities: &mut [Value],
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    existing_pages: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let texts: Vec<String> = affected_entities.iter().map(entity_to_query_text).collect();
    let text_refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let embeddings: Vec<Vec<f32>> = match embd {
        Some(embd) => match embd.embed(&text_refs).await {
            Ok(vectors) if vectors.len() == affected_entities.len() => vectors,
            Ok(_) => {
                tracing::warn!(
                    "wiki: router embedding count mismatch; treating all entities as orphans"
                );
                vec![Vec::new(); affected_entities.len()]
            }
            Err(err) => {
                tracing::warn!(error = %err, "wiki: router embedding failed; treating all entities as orphans");
                vec![Vec::new(); affected_entities.len()]
            }
        },
        None => vec![Vec::new(); affected_entities.len()],
    };
    for (entity, vec) in affected_entities.iter_mut().zip(embeddings.iter()) {
        if let Some(obj) = entity.as_object_mut() {
            obj.insert("_embedding".to_string(), json!(vec));
        }
    }

    let mut assignments: Map<String, Value> = Map::new();
    let mut orphans: Vec<(Value, Vec<f32>)> = Vec::new();
    let existing: Map<String, Value> = existing_pages.cloned().unwrap_or_default();

    let mut entity_pages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut chunk_pages: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (page_id, page) in &existing {
        for entity_name in as_str_list(page.get("entity_names_kwd")) {
            entity_pages
                .entry(normalize_key(&entity_name))
                .or_default()
                .insert(page_id.clone());
        }
        for chunk_id in as_str_list(page.get("source_chunk_ids")) {
            chunk_pages
                .entry(chunk_id)
                .or_default()
                .insert(page_id.clone());
        }
    }

    if existing.is_empty() {
        // First build: no page can accept a routed entity.
        orphans = affected_entities
            .iter()
            .cloned()
            .zip(embeddings.iter().cloned())
            .collect();
        wiki_log_stats(
            "ROUTE",
            "summary",
            &[
                ("affected", json!(affected_entities.len())),
                ("llm_existing", json!(0)),
                ("llm_new", json!(0)),
                ("llm_missing", json!(0)),
                ("new_confirmed_existing", json!(0)),
                ("final_new", json!(orphans.len())),
            ],
        );
    } else {
        let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
        let mut route_items: Vec<(Value, Vec<Value>)> = Vec::new();
        for (entity, vec) in affected_entities.iter().zip(embeddings.iter()) {
            if entity.get("action").and_then(Value::as_str) == Some("delete") {
                push_assignment(&mut assignments, "_deleted", entity);
                continue;
            }
            let mut candidates: Vec<Value> = Vec::new();
            if !vec.is_empty() {
                let vec_field = format!("q_{}_vec", vec.len());
                let fields: Vec<String> = [
                    "slug_kwd",
                    "title_kwd",
                    "summary_with_weight",
                    "entity_names_kwd",
                    "embedding",
                    vec_field.as_str(),
                ]
                .iter()
                .map(|field| field.to_string())
                .collect();
                let mut condition = Map::new();
                condition.insert(
                    "compile_kwd".to_string(),
                    Value::String(WIKI_PAGE_COMPILE_KWD.to_string()),
                );
                let query = SearchQuery {
                    select_fields: fields.clone(),
                    condition,
                    match_expressions: vec![crate::doc_store::MatchExpr::dense(
                        &vec_field,
                        vec.clone(),
                        "cosine",
                        PAGE_ROUTER_TOP_K,
                    )],
                    offset: 0,
                    limit: PAGE_ROUTER_TOP_K,
                    index_names: vec![index.clone()],
                    dataset_ids: vec![kb_id.to_string()],
                    ..Default::default()
                };
                if let Ok(response) = store.search(&query) {
                    for (_, row) in store.get_fields(&response, &fields) {
                        let row_value = Value::Object(row);
                        let page_id = row_scalar_string(&row_value, "slug_kwd").trim().to_string();
                        if page_id.is_empty() {
                            continue;
                        }
                        let stored: Vec<f32> = row_value
                            .get(&vec_field)
                            .or_else(|| row_value.get("embedding"))
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| item.as_f64().map(|number| number as f32))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let score = crate::merge::cosine_similarity(vec, &stored) as f64;
                        let page_source = existing.get(&page_id).unwrap_or(&row_value);
                        let mut candidate = wiki_route_page_candidate(&page_id, page_source, score);
                        if let Some(obj) = candidate.as_object_mut() {
                            obj.insert(
                                "signals".to_string(),
                                Value::Array(vec![Value::String("embedding".to_string())]),
                            );
                        }
                        candidates.push(candidate);
                    }
                }
            }
            let expanded = wiki_expand_route_candidates(
                entity,
                &candidates,
                &existing,
                &entity_pages,
                &chunk_pages,
                false,
            );
            if expanded.is_empty() {
                orphans.push((entity.clone(), vec.clone()));
                continue;
            }
            route_items.push((entity.clone(), expanded));
        }

        let decisions = wiki_llm_route_batches(chat, &route_items).await;
        let first_new_count = decisions
            .values()
            .filter(|page_id| *page_id == "NEW")
            .count();
        let first_existing_count = decisions
            .values()
            .filter(|page_id| *page_id != "NEW")
            .count();
        let missing_count = route_items.len().saturating_sub(decisions.len());
        let mut second_pass_items: Vec<(usize, Value, Vec<Value>)> = Vec::new();
        let mut confirmed_existing_count = 0usize;
        for (item_id, (entity, candidates)) in route_items.iter().enumerate() {
            match decisions.get(&item_id) {
                Some(page_id) if page_id != "NEW" => {
                    push_assignment(&mut assignments, page_id, entity);
                }
                Some(_) => {
                    let expanded = wiki_expand_route_candidates(
                        entity,
                        candidates,
                        &existing,
                        &entity_pages,
                        &chunk_pages,
                        true,
                    );
                    let original_ids: BTreeSet<String> = candidates
                        .iter()
                        .filter_map(|candidate| {
                            candidate
                                .get("page_id")
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        })
                        .collect();
                    let added_ids: Vec<String> = expanded
                        .iter()
                        .filter_map(|candidate| {
                            let page_id = candidate.get("page_id").and_then(Value::as_str)?;
                            if original_ids.contains(page_id) {
                                None
                            } else {
                                Some(page_id.to_string())
                            }
                        })
                        .collect();
                    let entity_name = ["entity_name", "term"]
                        .iter()
                        .find_map(|key| entity.get(*key).and_then(Value::as_str))
                        .unwrap_or("");
                    wiki_log_stats(
                        "ROUTE",
                        "new_confirmation_candidates",
                        &[
                            ("entity", json!(entity_name)),
                            ("initial_candidate_count", json!(candidates.len())),
                            ("added_candidate_count", json!(added_ids.len())),
                            ("added_to_candidates", json!(!added_ids.is_empty())),
                            (
                                "added_page_ids",
                                Value::Array(added_ids.into_iter().map(Value::String).collect()),
                            ),
                            ("confirmation_candidate_count", json!(expanded.len())),
                        ],
                    );
                    second_pass_items.push((item_id, entity.clone(), expanded));
                }
                None => {
                    let owner = candidates.iter().find(|candidate| {
                        candidate
                            .get("signals")
                            .and_then(Value::as_array)
                            .map(|signals| {
                                signals
                                    .iter()
                                    .any(|signal| signal.as_str() == Some("current_owner"))
                            })
                            .unwrap_or(false)
                    });
                    match owner {
                        Some(owner) => {
                            if let Some(page_id) = owner.get("page_id").and_then(Value::as_str) {
                                push_assignment(&mut assignments, page_id, entity);
                            }
                        }
                        None => orphans.push((entity.clone(), Vec::new())),
                    }
                }
            }
        }

        if !second_pass_items.is_empty() {
            let confirmation_items: Vec<(Value, Vec<Value>)> = second_pass_items
                .iter()
                .map(|(_, entity, candidates)| (entity.clone(), candidates.clone()))
                .collect();
            let confirmations = wiki_llm_route_batches(chat, &confirmation_items).await;
            for (confirmation_id, (_, entity, candidates)) in second_pass_items.iter().enumerate() {
                let page_id = confirmations.get(&confirmation_id);
                match page_id {
                    Some(page_id) if page_id != "NEW" => {
                        push_assignment(&mut assignments, page_id, entity);
                        confirmed_existing_count += 1;
                    }
                    decided => {
                        let owner = candidates.iter().find(|candidate| {
                            candidate
                                .get("signals")
                                .and_then(Value::as_array)
                                .map(|signals| {
                                    signals
                                        .iter()
                                        .any(|signal| signal.as_str() == Some("current_owner"))
                                })
                                .unwrap_or(false)
                        });
                        match (owner, decided) {
                            (Some(owner), None) => {
                                if let Some(page_id) = owner.get("page_id").and_then(Value::as_str)
                                {
                                    push_assignment(&mut assignments, page_id, entity);
                                }
                            }
                            _ => orphans.push((entity.clone(), Vec::new())),
                        }
                    }
                }
            }
        }
        wiki_log_stats(
            "ROUTE",
            "summary",
            &[
                ("affected", json!(affected_entities.len())),
                ("llm_existing", json!(first_existing_count)),
                ("llm_new", json!(first_new_count)),
                ("llm_missing", json!(missing_count)),
                ("new_confirmed_existing", json!(confirmed_existing_count)),
                ("final_new", json!(orphans.len())),
            ],
        );
    }

    // Orphans: cluster by similarity and create grouped pages. Deleting
    // entities must not create pages just to delete them again.
    let mut orphan_entities: Vec<Value> = Vec::new();
    let mut orphan_embeddings: Vec<Vec<f32>> = Vec::new();
    for (entity, vec) in orphans {
        if entity.get("action").and_then(Value::as_str) == Some("delete") {
            continue;
        }
        orphan_entities.push(entity);
        orphan_embeddings.push(vec);
    }
    if !orphan_entities.is_empty() {
        let clusters =
            wiki_llm_group_entities(chat, &orphan_entities, &orphan_embeddings, kb_id).await;
        let mut used_page_ids: BTreeSet<String> = existing.keys().cloned().collect();
        for key in assignments.keys() {
            if let Some(rest) = key.strip_prefix("_new_") {
                used_page_ids.insert(rest.to_string());
            }
        }
        for cluster in clusters {
            if cluster.is_empty() {
                continue;
            }
            let evidence_and_name = |entity: &Value| -> (usize, String) {
                let claims = entity
                    .get("claims")
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0);
                let name = ["entity_name", "term"]
                    .iter()
                    .find_map(|key| entity.get(*key).and_then(Value::as_str))
                    .unwrap_or("")
                    .to_string();
                (claims, name)
            };
            let representative = cluster
                .iter()
                .min_by(|left, right| {
                    let (left_claims, left_name) = evidence_and_name(left);
                    let (right_claims, right_name) = evidence_and_name(right);
                    right_claims
                        .cmp(&left_claims)
                        .then_with(|| left_name.to_lowercase().cmp(&right_name.to_lowercase()))
                        .then_with(|| left_name.cmp(&right_name))
                })
                .cloned()
                .unwrap_or_else(|| cluster[0].clone());
            let mut ordered: Vec<Value> = vec![representative.clone()];
            for entity in &cluster {
                if entity != &representative {
                    ordered.push(entity.clone());
                }
            }
            let names: Vec<String> = ordered
                .iter()
                .map(|entity| {
                    ["entity_name", "term"]
                        .iter()
                        .find_map(|key| entity.get(*key).and_then(Value::as_str))
                        .unwrap_or("")
                        .to_string()
                })
                .collect();
            if names.is_empty() {
                continue;
            }
            let base_page_id = derive_page_id(&names[0], "entity");
            if base_page_id.is_empty() {
                continue;
            }
            let mut page_id = base_page_id.clone();
            let mut suffix = 2usize;
            while used_page_ids.contains(&page_id) {
                page_id = format!("{base_page_id}-{suffix}");
                suffix += 1;
            }
            used_page_ids.insert(page_id.clone());
            assignments.insert(format!("_new_{page_id}"), Value::Array(ordered));
        }
    }

    assignments
}

#[cfg(test)]
mod wiki_incremental_part11_tests {
    use super::*;
    use crate::doc_store::{DocStore, MemoryDocStore};
    use crate::embed::Embedder;
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

    struct MapEmb {
        vector: Vec<f32>,
    }

    #[async_trait::async_trait]
    impl Embedder for MapEmb {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| self.vector.clone()).collect())
        }
    }

    fn entity(name: &str) -> Value {
        json!({"entity_name": name, "claims": [], "source_chunk_ids": [], "action": "create"})
    }

    #[tokio::test]
    async fn first_build_clusters_all_orphans() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "[[0, 1, 2]]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let emb = MapEmb {
            vector: vec![1.0, 0.0],
        };
        let mut entities = vec![entity("Alpha"), entity("Beta"), entity("Gamma")];
        let assignments =
            wiki_page_router(&store, &mut entities, &chat, Some(&emb), "t1", "kb1", None).await;
        let new_keys: Vec<&String> = assignments
            .keys()
            .filter(|key| key.starts_with("_new_"))
            .collect();
        assert_eq!(new_keys.len(), 1);
        let group = assignments[new_keys[0]].as_array().unwrap();
        assert_eq!(group.len(), 3);
        assert_eq!(new_keys[0], &"_new_entity/alpha".to_string());
        assert_eq!(entities[0]["_embedding"], json!([1.0, 0.0]));
    }

    #[tokio::test]
    async fn routes_to_existing_page_via_llm() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "entity/alpha",
            "title_kwd": "Alpha",
            "summary_with_weight": "about alpha",
            "entity_names_kwd": ["Alpha"],
            "source_chunk_ids": ["c1"],
            "embedding": [1.0, 0.0],
            "q_2_vec": [1.0, 0.0]
        });
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
        let mut existing: Map<String, Value> = Map::new();
        existing.insert(
            "entity/alpha".to_string(),
            json!({"title_kwd": "Alpha", "entity_names_kwd": ["Alpha"], "source_chunk_ids": ["c1"], "summary_with_weight": "about alpha"}),
        );
        let chat = FakeChat {
            reply: "[{\"id\": 0, \"page\": \"entity/alpha\"}]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let emb = MapEmb {
            vector: vec![1.0, 0.0],
        };
        let mut entities = vec![entity("Alpha")];
        let assignments = wiki_page_router(
            &store,
            &mut entities,
            &chat,
            Some(&emb),
            "t1",
            "kb1",
            Some(&existing),
        )
        .await;
        let assigned = assignments["entity/alpha"].as_array().unwrap();
        assert_eq!(assigned.len(), 1);
        assert!(assignments.keys().all(|key| !key.starts_with("_new_")));
    }

    #[tokio::test]
    async fn missing_decision_falls_back_to_current_owner() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "entity/alpha",
            "title_kwd": "Alpha",
            "entity_names_kwd": ["Alpha"],
            "embedding": [1.0, 0.0],
            "q_2_vec": [1.0, 0.0]
        });
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
        let mut existing: Map<String, Value> = Map::new();
        existing.insert(
            "entity/alpha".to_string(),
            json!({"title_kwd": "Alpha", "entity_names_kwd": ["Alpha"]}),
        );
        let chat = FakeChat {
            reply: "no json here".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let emb = MapEmb {
            vector: vec![1.0, 0.0],
        };
        let mut entities = vec![entity("Alpha")];
        let assignments = wiki_page_router(
            &store,
            &mut entities,
            &chat,
            Some(&emb),
            "t1",
            "kb1",
            Some(&existing),
        )
        .await;
        assert_eq!(assignments["entity/alpha"].as_array().unwrap().len(), 1);
    }
}

// ---------------------------------------------------------------------------
// Part 10 — shared FINALIZE (`_wiki_finalize`).
//
// Adaptation note: `refresh_idx` has no local counterpart (skipped); the
// disabled-document set is host-injected; page updates match by stable row id.
// ---------------------------------------------------------------------------

fn char_to_byte_index_p10(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

fn replace_first(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    match haystack.find(needle) {
        Some(pos) => {
            let mut out = String::with_capacity(haystack.len() + replacement.len());
            out.push_str(&haystack[..pos]);
            out.push_str(replacement);
            out.push_str(&haystack[pos + needle.len()..]);
            out
        }
        None => haystack.to_string(),
    }
}

fn slug_suffix(value: &str) -> String {
    if value.contains('/') {
        value.rsplit('/').next().unwrap_or(value).to_string()
    } else {
        value.to_string()
    }
}

fn relation_entry(entity_name: &str, relation: &str) -> Value {
    json!({"entity_name": entity_name, "relation": relation})
}

/// `_wiki_finalize`: dead-link cleanup + cross-reference update.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_finalize(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    disabled_doc_ids: &BTreeSet<String>,
    _page_ids: Option<&[String]>,
    chunk_state: Option<&Map<String, Value>>,
) {
    let fields: Vec<String> = [
        "slug_kwd",
        "title_kwd",
        "md_with_weight",
        "outlinks_kwd",
        "related_kb_pages_kwd",
        "entity_names_kwd",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let all_pages = search_existing_pages(store, tenant_id, kb_id, &fields);
    if all_pages.is_empty() {
        return;
    }
    let valid_ids: BTreeSet<String> = all_pages.keys().cloned().collect();

    let canonical_index = load_canonical_entities(store, tenant_id, kb_id);
    let mut canonical_names: BTreeSet<String> = BTreeSet::new();
    for (cname, entry) in &canonical_index {
        canonical_names.insert(cname.clone());
        for alias in entry
            .get("aliases")
            .and_then(Value::as_array)
            .map(|items| items.to_vec())
            .unwrap_or_default()
        {
            if let Some(alias) = alias.as_str() {
                canonical_names.insert(alias.to_string());
            }
        }
    }

    let mut relation_map: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut outlink_map: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut name_slug: BTreeMap<String, String> = BTreeMap::new();
    for (pid, page) in &all_pages {
        let plain = slug_suffix(pid);
        if !plain.is_empty() {
            name_slug.insert(plain.clone(), pid.clone());
        }
        let title = page
            .get("title_kwd")
            .map(|value| match value {
                Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
                other => other.as_str().unwrap_or(""),
            })
            .unwrap_or("");
        if !title.is_empty() && title != plain {
            name_slug.insert(title.to_string(), pid.clone());
        }
        for entity_name in as_str_list(page.get("entity_names_kwd")) {
            if !entity_name.is_empty() && entity_name != plain {
                name_slug.insert(entity_name, pid.clone());
            }
        }
    }
    let mut ordered_names: Vec<String> = name_slug.keys().cloned().collect();
    ordered_names.sort_by(|left, right| {
        right
            .chars()
            .count()
            .cmp(&left.chars().count())
            .then_with(|| left.cmp(right))
    });

    let map_relations =
        load_map_relations(store, tenant_id, kb_id, Some(disabled_doc_ids), chunk_state);
    let mut relation_edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for rel in &map_relations {
        let from_name = rel.get("from").and_then(Value::as_str).unwrap_or("");
        let to_name = rel.get("to").and_then(Value::as_str).unwrap_or("");
        let from_pg = name_slug.get(from_name).cloned();
        let to_pg = name_slug.get(to_name).cloned();
        if let (Some(from_pg), Some(to_pg)) = (from_pg, to_pg) {
            if from_pg != to_pg {
                relation_edges
                    .entry(from_pg.clone())
                    .or_default()
                    .insert(to_pg.clone());
                relation_edges.entry(to_pg).or_default().insert(from_pg);
            }
        }
    }

    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let page_ids: Vec<String> = all_pages.keys().cloned().collect();
    for pid in page_ids {
        let Some(page) = all_pages.get(&pid) else {
            continue;
        };
        let original = page
            .get("md_with_weight")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut content = original.clone();

        let matches: Vec<regex::Captures> = wikilink_re().captures_iter(&original).collect();
        for caps in &matches {
            let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string();
            let link = caps
                .get(1)
                .map(|m| m.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let (target, display) = match link.split_once('|') {
                Some((target, display)) => (target.trim().to_string(), display.trim().to_string()),
                None => (link.clone(), String::new()),
            };
            if valid_ids.contains(&target) && target != pid {
                let entity_name = if !display.is_empty() {
                    display.clone()
                } else {
                    slug_suffix(&target)
                };
                relation_map
                    .entry(pid.clone())
                    .or_default()
                    .push(relation_entry(&entity_name, "see_also"));
                outlink_map.entry(pid.clone()).or_default().push(target);
            } else if canonical_names.contains(&target) {
                let replacement = if !display.is_empty() {
                    display
                } else {
                    target.clone()
                };
                content = replace_first(&content, &whole, &replacement);
            } else {
                let resolved = resolve_dead_slug(&target, &valid_ids, &name_slug);
                match resolved {
                    Some(resolved) => {
                        let resolved_link = if !display.is_empty() {
                            format!("[[{resolved}|{display}]]")
                        } else {
                            format!("[[{resolved}]]")
                        };
                        content = replace_first(&content, &whole, &resolved_link);
                        let entity_name = if !display.is_empty() {
                            display.clone()
                        } else {
                            slug_suffix(&resolved)
                        };
                        relation_map
                            .entry(pid.clone())
                            .or_default()
                            .push(relation_entry(&entity_name, "see_also"));
                        let outlinks = outlink_map.entry(pid.clone()).or_default();
                        if !outlinks.iter().any(|existing| existing == &resolved) {
                            outlinks.push(resolved);
                        }
                    }
                    None => {
                        let replacement = if !display.is_empty() {
                            display
                        } else {
                            target.clone()
                        };
                        content = replace_first(&content, &whole, &replacement);
                    }
                }
            }
        }

        // AUTO-LINK first standalone mention of other pages' names.
        let mut existing_links: BTreeSet<String> = wikilink_re()
            .captures_iter(&content)
            .filter_map(|caps| {
                caps.get(1).map(|m| {
                    m.as_str()
                        .split('|')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string()
                })
            })
            .collect();
        if let Ok(artifact_re) =
            Regex::new(&format!(r"\]\(artifact{}/([^)]+)\)", regex::escape(kb_id)))
        {
            for caps in artifact_re.captures_iter(&content) {
                if let Some(group) = caps.get(1) {
                    existing_links.insert(group.as_str().to_string());
                }
            }
        }
        for name in &ordered_names {
            let Some(target) = name_slug.get(name).cloned() else {
                continue;
            };
            if target == pid || existing_links.contains(&target) {
                continue;
            }
            let idx = find_unlinked_mention(&content, name);
            if idx < 0 {
                continue;
            }
            let idx = idx as usize;
            let name_len = name.chars().count();
            let start_byte = char_to_byte_index_p10(&content, idx);
            let end_byte = char_to_byte_index_p10(&content, idx + name_len);
            content = format!(
                "{}[[{}|{}]]{}",
                &content[..start_byte],
                target,
                name,
                &content[end_byte..]
            );
            existing_links.insert(target.clone());
            let outlinks = outlink_map.entry(pid.clone()).or_default();
            if !outlinks.iter().any(|existing| existing == &target) {
                outlinks.push(target.clone());
            }
            relation_map
                .entry(pid.clone())
                .or_default()
                .push(relation_entry(name, "see_also"));
        }

        // Merge semantic relation edges and inject the Related section.
        let mut rel_targets: Vec<String> = Vec::new();
        if let Some(targets) = relation_edges.get(&pid) {
            for target in targets {
                if target == &pid {
                    continue;
                }
                let outlinks = outlink_map.entry(pid.clone()).or_default();
                if !outlinks.iter().any(|existing| existing == target) {
                    outlinks.push(target.clone());
                    relation_map
                        .entry(pid.clone())
                        .or_default()
                        .push(relation_entry(&slug_suffix(target), "related"));
                }
                if !existing_links.contains(target) {
                    rel_targets.push(target.clone());
                    existing_links.insert(target.clone());
                }
            }
        }
        if !rel_targets.is_empty() {
            if !content.trim_end().ends_with("## 相关页面") {
                content = format!("{}\n\n## 相关页面\n", content.trim_end());
            }
            content += &rel_targets
                .iter()
                .map(|target| format!("- [[{target}]]"))
                .collect::<Vec<String>>()
                .join("\n");
            content.push('\n');
        }

        let mut rendered_content = content.clone();
        if !rendered_content.is_empty() {
            let mut link_targets: BTreeSet<String> = outlink_map.keys().cloned().collect();
            for targets in outlink_map.values() {
                link_targets.extend(targets.iter().cloned());
            }
            rendered_content = render_links(&rendered_content, kb_id, &link_targets);
        }

        let mut update = Map::new();
        if rendered_content != original {
            update.insert(
                "md_with_weight".to_string(),
                Value::String(rendered_content),
            );
        }
        let relations = relation_map.get(&pid).cloned().unwrap_or_default();
        if !relations.is_empty() {
            let related: Vec<Value> = relations
                .iter()
                .take(20)
                .map(|entry| {
                    let name = entry
                        .get("entity_name")
                        .and_then(Value::as_str)
                        .or_else(|| entry.get("slug").and_then(Value::as_str))
                        .map(str::to_string)
                        .unwrap_or_else(|| entry.to_string());
                    Value::String(name)
                })
                .collect();
            update.insert("related_kb_pages_kwd".to_string(), Value::Array(related));
        } else if page
            .get("related_kb_pages_kwd")
            .map(crate::harness::knowlege_wiki::json_truthy)
            .unwrap_or(false)
        {
            update.insert("related_kb_pages_kwd".to_string(), Value::Array(Vec::new()));
        }
        let outlinks = outlink_map.get(&pid).cloned().unwrap_or_default();
        update.insert(
            "outlinks_kwd".to_string(),
            Value::Array(outlinks.iter().cloned().map(Value::String).collect()),
        );
        update.insert("outlinks_int".to_string(), json!(outlinks.len()));

        let page_id_value = page.get("id").cloned().unwrap_or(Value::Null);
        let mut condition = Map::new();
        condition.insert("id".to_string(), page_id_value);
        if let Err(err) = store.update(&condition, &update, &index, kb_id) {
            tracing::warn!(error = %err, page = pid.as_str(), "wiki: finalize update failed");
        }
    }
}

#[cfg(test)]
mod wiki_incremental_part12_tests {
    use super::*;
    use crate::doc_store::{DocStore, MemoryDocStore};
    use crate::harness::knowlege_wiki::{build_resume_doc, commit_active_map_state};

    fn page_row(id: &str, slug: &str, title: &str, md: &str, entity_names: &[&str]) -> DocRow {
        json!({
            "id": id,
            "compile_kwd": "wiki_page",
            "slug_kwd": slug,
            "title_kwd": title,
            "md_with_weight": md,
            "entity_names_kwd": entity_names,
            "source_chunk_ids": [],
            "related_kb_pages_kwd": []
        })
        .as_object()
        .cloned()
        .unwrap()
    }

    #[tokio::test]
    async fn finalize_links_dead_and_autolink() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let rows = vec![
            page_row(
                "p1",
                "entity/alpha",
                "Alpha",
                "Alpha links [[entity/beta]] and [[张伟]] and [[entity/zzz]].",
                &["Alpha"],
            ),
            page_row("p2", "entity/beta", "Beta", "Beta page prose.", &["Beta"]),
            page_row(
                "p3",
                "concept/gamma",
                "Gamma",
                "Gamma mentions Beta in prose.",
                &["Gamma"],
            ),
        ];
        store.insert(&rows, &index, "kb1").unwrap();
        save_canonical_entity(
            &store,
            "t1",
            "kb1",
            "张伟",
            "entity",
            &[],
            &[],
            1,
            None,
            None,
        );
        let disabled: BTreeSet<String> = BTreeSet::new();
        wiki_finalize(&store, "t1", "kb1", &disabled, None, None).await;

        let full_fields: Vec<String> = [
            "slug_kwd",
            "md_with_weight",
            "outlinks_kwd",
            "outlinks_int",
            "related_kb_pages_kwd",
        ]
        .iter()
        .map(|f| f.to_string())
        .collect();
        let by_slug = search_existing_pages(&store, "t1", "kb1", &full_fields);
        let find = |slug: &str| -> Value {
            by_slug
                .values()
                .find(|page| page["slug_kwd"] == json!(slug))
                .cloned()
                .expect("page")
        };
        let alpha = find("entity/alpha");
        let alpha_md = alpha["md_with_weight"].as_str().unwrap();
        assert!(alpha_md.contains("artifact/kb1/entity/beta"));
        assert!(!alpha_md.contains("[[entity/zzz]]"));
        assert!(alpha_md.contains("entity/zzz"));
        assert!(!alpha_md.contains("[[张伟]]"));
        assert!(alpha_md.contains("张伟"));
        let related = alpha["related_kb_pages_kwd"].as_array().unwrap();
        assert!(related.iter().any(|value| value == &json!("beta")));
        assert!(alpha["outlinks_int"].as_i64().unwrap() >= 1);

        let gamma = find("concept/gamma");
        let gamma_md = gamma["md_with_weight"].as_str().unwrap();
        assert!(gamma_md.contains("artifact/kb1/entity/beta"));
    }

    #[tokio::test]
    async fn finalize_applies_relation_edges_and_section() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let rows = vec![
            page_row("p1", "entity/alpha", "Alpha", "Alpha body.", &["Alpha"]),
            page_row("p3", "concept/gamma", "Gamma", "Gamma body.", &["Gamma"]),
        ];
        store.insert(&rows, &index, "kb1").unwrap();
        let extract = json!({
            "entities": [], "concepts": [], "claims": [],
            "relations": [{"from": "Alpha", "to": "Gamma", "type": "uses"}],
            "topics": []
        });
        let row = build_resume_doc("c1", "d1", &extract, "h1");
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
        let mut state = Map::new();
        state.insert("c1".to_string(), json!({"doc_id": "d1", "hash": "h1"}));
        commit_active_map_state(&store, "t1", "kb1", &state).unwrap();
        let disabled: BTreeSet<String> = BTreeSet::new();
        wiki_finalize(&store, "t1", "kb1", &disabled, None, None).await;
        let fields: Vec<String> = ["slug_kwd", "md_with_weight", "outlinks_kwd"]
            .iter()
            .map(|f| f.to_string())
            .collect();
        let by_slug = search_existing_pages(&store, "t1", "kb1", &fields);
        let gamma = by_slug
            .values()
            .find(|page| page["slug_kwd"] == json!("concept/gamma"))
            .cloned()
            .expect("gamma");
        let gamma_md = gamma["md_with_weight"].as_str().unwrap();
        assert!(gamma_md.contains("## 相关页面"));
        assert!(gamma_md.contains("artifact/kb1/entity/alpha"));
        let alpha = by_slug
            .values()
            .find(|page| page["slug_kwd"] == json!("entity/alpha"))
            .cloned()
            .expect("alpha");
        let alpha_md = alpha["md_with_weight"].as_str().unwrap();
        assert!(alpha_md.contains("## 相关页面"));
        assert!(alpha_md.contains("artifact/kb1/concept/gamma"));
    }
}

// ---------------------------------------------------------------------------
// Part 11 — Mode A run (`_wiki_mode_a_run`).
//
// Adaptation note: the refine fan-out is sequential with the same ~20-slot
// progress cadence; `page_deltas` uses an ordered map for deterministic
// iteration.
// ---------------------------------------------------------------------------

struct ModeAPageDelta {
    page_id: String,
    page_title: String,
    existing_page: Option<Value>,
    additions: Vec<Value>,
    retractions: Vec<Value>,
    claims: Vec<Value>,
    source_chunks: Vec<Value>,
    source_doc_ids: BTreeSet<String>,
    action: String,
}

fn delta_claim_text(claim: &Value) -> String {
    ["statement", "text"]
        .iter()
        .find_map(|key| claim.get(*key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// `_wiki_mode_a_run`: every canonical entity/concept compiles to one page.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_mode_a_run(
    store: &dyn DocStore,
    deltas: &[Value],
    existing_pages: &Map<String, Value>,
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    incremental: bool,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
    canonical_claims: Option<&Map<String, Value>>,
    doc_to_entities: Option<&Map<String, Value>>,
    doc_topics: Option<&Map<String, Value>>,
    topic_embeddings: Option<&mut Map<String, Value>>,
    topic_pool: Option<&mut Map<String, Value>>,
) -> Value {
    let report = |message: &str| {
        if let Some(callback) = callback {
            callback(0.7, &format!("wiki REFINE A: {message}"));
        }
    };
    let mut pages_created = 0usize;
    let mut pages_modified = 0usize;
    let mut pages_deleted = 0usize;
    let mut errors: Vec<Value> = Vec::new();

    let mut name_to_page: BTreeMap<String, String> = BTreeMap::new();
    for (pid, page) in existing_pages {
        for name in as_str_list(page.get("entity_names_kwd")) {
            name_to_page.insert(name, pid.clone());
        }
    }

    let mut page_deltas: BTreeMap<String, ModeAPageDelta> = BTreeMap::new();
    for delta in deltas {
        let name = delta
            .get("entity_name")
            .and_then(Value::as_str)
            .unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let entity_type = normalize_entity_type(
            delta
                .get("entity_type")
                .unwrap_or(&Value::String(String::new())),
        );
        let prefix = if entity_type == "concept" {
            "concept"
        } else {
            "entity"
        };
        let page_id = name_to_page
            .get(name)
            .cloned()
            .unwrap_or_else(|| derive_page_id(name, prefix));
        let entry = page_deltas
            .entry(page_id.clone())
            .or_insert_with(|| ModeAPageDelta {
                page_id: page_id.clone(),
                page_title: name.to_string(),
                existing_page: existing_pages.get(&page_id).cloned(),
                additions: Vec::new(),
                retractions: Vec::new(),
                claims: Vec::new(),
                source_chunks: Vec::new(),
                source_doc_ids: BTreeSet::new(),
                action: "noop".to_string(),
            });
        if let Some(additions) = delta.get("additions").and_then(Value::as_array) {
            entry.additions.extend(additions.iter().cloned());
        }
        if let Some(retractions) = delta.get("retractions").and_then(Value::as_array) {
            entry.retractions.extend(retractions.iter().cloned());
        }
        let mut delta_claims: Vec<Value> = delta
            .get("claims")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        delta_claims.extend(
            delta
                .get("additions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        let delta_claims = wiki_dedupe_claims(&delta_claims);
        entry.claims.extend(delta_claims.iter().cloned());
        for doc in delta
            .get("retained_source_doc_ids")
            .and_then(Value::as_array)
            .map(|items| items.to_vec())
            .unwrap_or_default()
        {
            if let Some(doc) = doc.as_str() {
                if !doc.is_empty() {
                    entry.source_doc_ids.insert(doc.to_string());
                }
            }
        }
        for cid in as_str_list(delta.get("source_chunk_ids")) {
            entry.source_chunks.push(json!({"id": cid, "text": ""}));
        }
        for claim in &delta_claims {
            for cid in wiki_claim_chunk_ids(claim) {
                entry.source_chunks.push(json!({
                    "id": cid,
                    "text": delta_claim_text(claim),
                    "source_doc_id": claim.get("source_doc_id").cloned().unwrap_or(Value::Null),
                }));
            }
        }
        let action = delta.get("action").and_then(Value::as_str).unwrap_or("");
        if action == "delete" {
            entry.action = "delete".to_string();
        } else if entry.action != "delete" {
            entry.action = action.to_string();
        }
    }

    if let Some(canonical_claims) = canonical_claims {
        let mut chunk_claims: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for claims in canonical_claims.values() {
            if let Some(claims) = claims.as_array() {
                for claim in claims {
                    for cid in wiki_claim_chunk_ids(claim) {
                        chunk_claims
                            .entry(cid)
                            .or_default()
                            .push(delta_claim_text(claim));
                    }
                }
            }
        }
        for entry in page_deltas.values_mut() {
            let page_chunk_ids: BTreeSet<String> = entry
                .source_chunks
                .iter()
                .filter_map(|chunk| chunk.get("id").and_then(Value::as_str).map(str::to_string))
                .collect();
            if page_chunk_ids.is_empty() {
                continue;
            }
            for cid in page_chunk_ids {
                if let Some(texts) = chunk_claims.get(&cid) {
                    if let Some(first) = texts.first() {
                        entry
                            .source_chunks
                            .push(json!({"id": cid, "text": first.clone()}));
                    }
                }
            }
        }
    }

    if !incremental && existing_pages.is_empty() {
        let concept_entries: Vec<&ModeAPageDelta> = page_deltas
            .values()
            .filter(|entry| entry.page_id.starts_with("concept/"))
            .collect();
        if !concept_entries.is_empty() {
            let concepts: Vec<Value> = concept_entries
                .iter()
                .map(|entry| {
                    let source_docs: BTreeSet<String> = entry
                        .claims
                        .iter()
                        .filter_map(|claim| claim.get("source_doc_id").and_then(Value::as_str))
                        .map(str::to_string)
                        .collect();
                    json!({
                        "term": entry.page_title,
                        "claims": entry.claims,
                        "source_doc_ids": source_docs.into_iter().collect::<Vec<String>>(),
                    })
                })
                .collect();
            let deep_ids: BTreeSet<String> = wiki_decide_concept_pages(&concepts)
                .iter()
                .filter_map(|page| {
                    page.get("page_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            page_deltas.retain(|pid, _| !pid.starts_with("concept/") || deep_ids.contains(pid));
        }
        if page_deltas.is_empty() {
            report("No pages to compile. Skipping.");
            return json!({
                "pages_created": 0,
                "pages_modified": 0,
                "pages_deleted": 0,
                "errors": [],
            });
        }
    }

    let all_page_ids: Vec<String> = existing_pages.keys().cloned().collect();
    let mut doc_updates: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut topic_selection_stats: Map<String, Value> = Map::new();
    topic_selection_stats.insert("selected".to_string(), json!(0));
    topic_selection_stats.insert("new".to_string(), json!(0));
    topic_selection_stats.insert("new_added".to_string(), json!(0));
    let mut topic_embeddings = topic_embeddings;
    let mut topic_pool = topic_pool;

    let total = page_deltas.len();
    if total > 0 {
        report(&format!("{total} pages ..."));
    }
    let report_every =
        1.max((total + WIKI_REFINE_PROGRESS_UPDATES - 1) / WIKI_REFINE_PROGRESS_UPDATES);
    let mut completed = 0usize;
    let entries: Vec<ModeAPageDelta> = page_deltas.into_values().collect();
    for entry in entries {
        let pid = entry.page_id.clone();
        let page_type = if pid.starts_with("concept/") {
            "concept"
        } else {
            "entity"
        };
        let existing_value = entry.existing_page.clone();
        let page_version = existing_value
            .as_ref()
            .map(|page| as_int(page.get("page_version_int"), 0))
            .unwrap_or(0);

        if entry.action == "delete" {
            let _ = wiki_refine_page(RefinePageArgs {
                store,
                mode: "delete",
                page_id: &pid,
                page_title: &entry.page_title,
                existing_page: existing_value.as_ref(),
                page_type_kwd: page_type,
                additions: None,
                retractions: None,
                source_chunks: Some(&[]),
                claims: Some(&[]),
                available_pages: Some(&all_page_ids),
                contextual_hints: "",
                chat,
                embd,
                tenant_id,
                kb_id,
                page_version,
                entity_names: None,
                page_embedding: None,
                embed_routing_context: false,
                source_doc_ids: None,
                topic_candidates: None,
                topic_selection_stats: None,
                topic_embeddings: None,
                topic_pool: None,
                member_evidence: None,
            })
            .await;
            pages_deleted += 1;
            wiki_clear_refine_failure(store, tenant_id, kb_id, &pid);
            completed += 1;
            if completed % report_every == 0 || completed == total {
                report(&format!("{completed}/{total} pages completed."));
            }
            continue;
        }

        let next_version = page_version + 1;
        let new_doc_ids: BTreeSet<String> = entry
            .additions
            .iter()
            .filter_map(|claim| claim.get("source_doc_id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let refine_mode = match existing_value.as_ref() {
            Some(existing) if should_re_synthesize(existing, &new_doc_ids, next_version) => {
                "re-synthesize"
            }
            Some(_) => "modify",
            None => "generate",
        };
        let source_docs: Vec<String> = entry.source_doc_ids.iter().cloned().collect();
        let topic_candidates =
            wiki_topics_for_docs(&source_docs, doc_topics, topic_pool.as_deref());
        let result = wiki_refine_page(RefinePageArgs {
            store,
            mode: refine_mode,
            page_id: &pid,
            page_title: &entry.page_title,
            existing_page: existing_value.as_ref(),
            page_type_kwd: page_type,
            additions: Some(&entry.additions),
            retractions: Some(&entry.retractions),
            source_chunks: Some(&entry.source_chunks),
            claims: Some(&entry.claims),
            available_pages: Some(&all_page_ids),
            contextual_hints: "",
            chat,
            embd,
            tenant_id,
            kb_id,
            page_version,
            entity_names: None,
            page_embedding: None,
            embed_routing_context: false,
            source_doc_ids: Some(&source_docs),
            topic_candidates: Some(&topic_candidates),
            topic_selection_stats: Some(&mut topic_selection_stats),
            topic_embeddings: topic_embeddings.as_deref_mut(),
            topic_pool: topic_pool.as_deref_mut(),
            member_evidence: None,
        })
        .await;
        match result {
            Some(_page) => {
                wiki_clear_refine_failure(store, tenant_id, kb_id, &pid);
                if refine_mode == "generate" {
                    pages_created += 1;
                } else {
                    pages_modified += 1;
                }
                for did in &source_docs {
                    doc_updates
                        .entry(did.clone())
                        .or_default()
                        .push(pid.clone());
                }
            }
            None => {
                let mut names: Vec<String> = existing_value
                    .as_ref()
                    .map(|page| as_str_list(page.get("entity_names_kwd")))
                    .unwrap_or_default();
                if names.is_empty() {
                    names.push(entry.page_title.clone());
                }
                wiki_record_refine_failure(
                    store,
                    tenant_id,
                    kb_id,
                    &pid,
                    &names,
                    &format!("REFINE returned no page content for {pid}"),
                );
                errors.push(Value::String(format!("REFINE_FAILED:{pid}")));
            }
        }
        completed += 1;
        if completed % report_every == 0 || completed == total {
            report(&format!("{completed}/{total} pages completed."));
        }
    }

    wiki_log_stats(
        "TOPIC",
        "selection_summary",
        &[
            ("mode", json!("A")),
            (
                "selected",
                topic_selection_stats
                    .get("selected")
                    .cloned()
                    .unwrap_or(json!(0)),
            ),
            (
                "new",
                topic_selection_stats
                    .get("new")
                    .cloned()
                    .unwrap_or(json!(0)),
            ),
            (
                "new_added",
                topic_selection_stats
                    .get("new_added")
                    .cloned()
                    .unwrap_or(json!(0)),
            ),
        ],
    );

    for (did, pids) in &doc_updates {
        let existing_dps =
            wiki_load_doc_page_source(store, tenant_id, kb_id, did).unwrap_or_else(|| json!({}));
        let mut existing_pids: Vec<String> = as_str_list(existing_dps.get("page_ids"));
        for pid in pids {
            if !existing_pids.iter().any(|existing| existing == pid) {
                existing_pids.push(pid.clone());
            }
        }
        let mut doc_entity_names: Vec<String> = doc_to_entities
            .and_then(|map| map.get(did))
            .map(|value| as_str_list(Some(value)))
            .unwrap_or_default();
        if doc_entity_names.is_empty() {
            doc_entity_names = as_str_list(existing_dps.get("entity_names"));
        }
        let chunk_hashes: Map<String, Value> = existing_dps
            .get("source_chunk_hashes")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let map_checksum = existing_dps
            .get("map_checksum")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        wiki_update_doc_page_source(
            store,
            tenant_id,
            kb_id,
            did,
            &existing_pids,
            if doc_entity_names.is_empty() {
                None
            } else {
                Some(&doc_entity_names)
            },
            Some(&chunk_hashes),
            Some(&map_checksum),
        );
    }

    report(&format!(
        "done: +{pages_created} ~{pages_modified} -{pages_deleted}"
    ));
    json!({
        "pages_created": pages_created,
        "pages_modified": pages_modified,
        "pages_deleted": pages_deleted,
        "errors": errors,
    })
}

#[cfg(test)]
mod wiki_incremental_part13_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::embed::Embedder;
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

    struct TestEmb;

    #[async_trait::async_trait]
    impl Embedder for TestEmb {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    fn delta_create(name: &str, entity_type: &str) -> Value {
        json!({
            "action": "create",
            "entity_name": name,
            "entity_type": entity_type,
            "additions": [{"statement": "s", "source_doc_id": "d1", "chunk_ids": ["c1"]}],
            "retractions": [],
            "claims": [],
            "source_chunk_ids": ["c1"],
            "retained_source_doc_ids": ["d1"]
        })
    }

    #[tokio::test]
    async fn mode_a_creates_pages_and_tracks_docs() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let rows: Vec<DocRow> = vec![
            json!({"id": "c1", "doc_id": "d1", "content_with_weight": "alpha source"})
                .as_object()
                .cloned()
                .unwrap(),
        ];
        store.insert(&rows, &index, "kb1").unwrap();
        let chat = FakeChat {
            reply: "TOPIC: alpha\n# Body\n\nGrounded.".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let emb = TestEmb;
        let mut stats: Map<String, Value> = Map::new();
        stats.insert("selected".to_string(), json!(0));
        stats.insert("new".to_string(), json!(0));
        stats.insert("new_added".to_string(), json!(0));
        let deltas = vec![
            delta_create("Alpha", "entity"),
            delta_create("Beta", "concept"),
        ];
        let summary = wiki_mode_a_run(
            &store,
            &deltas,
            &Map::new(),
            &chat,
            Some(&emb),
            "t1",
            "kb1",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(summary["pages_created"], json!(2));
        assert_eq!(summary["pages_modified"], json!(0));
        assert_eq!(summary["pages_deleted"], json!(0));
        let pages = search_existing_pages(&store, "t1", "kb1", &["slug_kwd".to_string()]);
        assert_eq!(pages.len(), 2);
        assert!(pages.contains_key("entity/alpha"));
        assert!(pages.contains_key("concept/beta"));
        // Incremental Mode A persists wiki_page rows directly (no drafts).
        let drafts = crate::harness::knowlege_wiki::wiki_load_refine_resume(&store, "t1", "kb1");
        assert!(drafts.is_empty());
        let dps = wiki_load_doc_page_source(&store, "t1", "kb1", "d1").expect("doc source");
        let page_ids: Vec<String> = as_str_list(dps.get("page_ids"));
        assert!(page_ids.contains(&"entity/alpha".to_string()));
        assert!(page_ids.contains(&"concept/beta".to_string()));
    }

    #[tokio::test]
    async fn mode_a_modifies_and_deletes_existing() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "entity/alpha",
            "title_kwd": "Alpha",
            "md_with_weight": "OLD BODY",
            "entity_names_kwd": ["Alpha"],
            "page_version_int": 3,
            "synthesis_version_int": 1,
            "topic_kwd": "alpha"
        });
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
        let mut existing: Map<String, Value> = Map::new();
        existing.insert(
            "entity/alpha".to_string(),
            json!({"title_kwd": "Alpha", "entity_names_kwd": ["Alpha"], "page_version_int": 3, "synthesis_version_int": 1, "topic_kwd": "alpha", "md_with_weight": "OLD BODY", "claims": "[]"}),
        );
        let chat = FakeChat {
            reply: "TOPIC: alpha\n# Updated\n\nFresh body.".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let mut delta = delta_create("Alpha", "entity");
        delta["action"] = json!("update");
        let summary = wiki_mode_a_run(
            &store,
            &[delta.clone()],
            &existing,
            &chat,
            None,
            "t1",
            "kb1",
            true,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(summary["pages_modified"], json!(1));
        let pages = search_existing_pages(
            &store,
            "t1",
            "kb1",
            &["slug_kwd".to_string(), "md_with_weight".to_string()],
        );
        let alpha = pages.get("entity/alpha").cloned().expect("alpha");
        assert!(
            alpha["md_with_weight"]
                .as_str()
                .unwrap()
                .contains("Fresh body")
        );

        let mut delete_delta = delta_create("Alpha", "entity");
        delete_delta["action"] = json!("delete");
        let summary2 = wiki_mode_a_run(
            &store,
            &[delete_delta],
            &existing,
            &chat,
            None,
            "t1",
            "kb1",
            true,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(summary2["pages_deleted"], json!(1));
        let remaining = search_existing_pages(&store, "t1", "kb1", &["slug_kwd".to_string()]);
        assert!(!remaining.contains_key("entity/alpha"));
    }

    #[tokio::test]
    async fn mode_a_records_refine_failure() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let summary = wiki_mode_a_run(
            &store,
            &[delta_create("Alpha", "entity")],
            &Map::new(),
            &chat,
            None,
            "t1",
            "kb1",
            false,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(summary["pages_created"], json!(0));
        let errors = summary["errors"].as_array().unwrap();
        assert!(errors.contains(&json!("REFINE_FAILED:entity/alpha")));
        let failures = wiki_load_refine_failures(&store, "t1", "kb1");
        assert_eq!(failures.len(), 1);
        assert!(failures.contains_key("entity/alpha"));
    }
}

// ---------------------------------------------------------------------------
// Part 12 — claim parsing, cohesion and unstable-page splitting plus the
// Mode B plan-group store (`_wiki_parse_claims` .. `_wiki_load_plan_group_members`).
//
// Adaptation note: numpy matrix work becomes plain Rust; the semaphore fan-out
// is sequential.
// ---------------------------------------------------------------------------

/// `_wiki_parse_claims`.
pub fn wiki_parse_claims(raw_claims: Option<&Value>) -> Vec<Value> {
    let resolved = match raw_claims {
        Some(Value::String(text)) => {
            if text.is_empty() {
                Value::Array(Vec::new())
            } else {
                serde_json::from_str::<Value>(text).unwrap_or(Value::Array(Vec::new()))
            }
        }
        Some(Value::Null) | None => Value::Array(Vec::new()),
        Some(other) => other.clone(),
    };
    match resolved {
        Value::Array(items) => items.into_iter().filter(Value::is_object).collect(),
        _ => Vec::new(),
    }
}

/// `_wiki_claims_for_entity`: claims owned by one page member.
pub fn wiki_claims_for_entity(page: &Value, entity_name: &str) -> Vec<Value> {
    let claims = wiki_parse_claims(page.get("claims"));
    let member_names = as_str_list(page.get("entity_names_kwd"));
    if member_names.len() == 1 && normalize_key(&member_names[0]) == normalize_key(entity_name) {
        return claims;
    }
    let target = normalize_key(entity_name);
    claims
        .into_iter()
        .filter(|claim| {
            let name = ["entity_name", "subject", "term"]
                .iter()
                .find_map(|key| claim.get(*key).and_then(Value::as_str))
                .unwrap_or("");
            normalize_key(name) == target
        })
        .collect()
}

/// `_wiki_embedding_cohesion`: mean cosine of rows vs the centroid.
pub fn wiki_embedding_cohesion(matrix: &[Vec<f32>]) -> f64 {
    if matrix.len() <= 1 {
        return 1.0;
    }
    let dim = matrix.first().map(Vec::len).unwrap_or(0);
    if dim == 0 {
        return 1.0;
    }
    let mut centroid = vec![0.0f64; dim];
    for row in matrix {
        for (pos, value) in row.iter().enumerate() {
            centroid[pos] += *value as f64;
        }
    }
    let count = matrix.len() as f64;
    for value in centroid.iter_mut() {
        *value /= count;
    }
    let norm = (centroid.iter().map(|x| x * x).sum::<f64>()).sqrt();
    if norm <= 0.0 {
        return 0.0;
    }
    let unit: Vec<f64> = centroid.iter().map(|x| x / norm).collect();
    let mut total = 0.0f64;
    for row in matrix {
        total += row
            .iter()
            .zip(unit.iter())
            .map(|(value, unit_value)| (*value as f64) * unit_value)
            .sum::<f64>();
    }
    total / count
}

/// `_wiki_split_unstable_page_assignments`.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_split_unstable_page_assignments(
    store: &dyn DocStore,
    assignments: &Map<String, Value>,
    existing_pages: &Map<String, Value>,
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    kb_id: &str,
) -> Map<String, Value> {
    let _ = store;
    if assignments.is_empty() {
        return assignments.clone();
    }

    let mut candidates: BTreeMap<String, Value> = BTreeMap::new();
    let mut all_members: Vec<Value> = Vec::new();
    for (page_id, incoming_value) in assignments {
        let existing = if !page_id.starts_with("_new_") {
            existing_pages.get(page_id).cloned()
        } else {
            None
        };
        let Some(existing) = existing else { continue };
        let incoming: Vec<Value> = incoming_value.as_array().cloned().unwrap_or_default();
        let old_names = as_str_list(existing.get("entity_names_kwd"));
        let mut deleted_names: BTreeSet<String> = BTreeSet::new();
        for entity in &incoming {
            if entity.get("action").and_then(Value::as_str) == Some("delete") {
                if let Some(name) = entity.get("entity_name").and_then(Value::as_str) {
                    deleted_names.insert(name.to_string());
                }
            }
        }
        let mut incoming_by_name: BTreeMap<String, Value> = BTreeMap::new();
        for entity in &incoming {
            if let Some(name) = entity.get("entity_name").and_then(Value::as_str) {
                incoming_by_name.insert(name.to_string(), entity.clone());
            }
        }
        let mut member_names: BTreeSet<String> = old_names.iter().cloned().collect();
        member_names.extend(incoming_by_name.keys().cloned());
        for deleted in &deleted_names {
            member_names.remove(deleted);
        }
        if member_names.len() <= 1 {
            continue;
        }
        let mut members: Vec<Value> = Vec::new();
        for name in &member_names {
            let incoming_entity = incoming_by_name
                .get(name)
                .cloned()
                .unwrap_or_else(|| json!({}));
            let mut member_claims = wiki_claims_for_entity(&existing, name);
            member_claims.extend(
                incoming_entity
                    .get("claims")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            members.push(json!({
                "entity_name": name,
                "entity_type": incoming_entity.get("entity_type").and_then(Value::as_str).unwrap_or("entity"),
                "aliases": incoming_entity.get("aliases").cloned().unwrap_or(Value::Array(Vec::new())),
                "claims": member_claims,
                "retractions": incoming_entity.get("retractions").cloned().unwrap_or(Value::Array(Vec::new())),
                "source_chunk_ids": incoming_entity.get("source_chunk_ids").cloned().unwrap_or(Value::Array(Vec::new())),
                "source_doc_ids": incoming_entity.get("source_doc_ids").cloned().unwrap_or(Value::Array(Vec::new())),
                "action": incoming_entity.get("action").and_then(Value::as_str).unwrap_or("update"),
            }));
        }
        let start = all_members.len();
        all_members.extend(members.iter().cloned());
        let removed_retractions: Vec<Value> = incoming
            .iter()
            .filter(|entity| entity.get("action").and_then(Value::as_str) == Some("delete"))
            .flat_map(|entity| {
                entity
                    .get("retractions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        candidates.insert(
            page_id.clone(),
            json!({
                "existing": existing,
                "old_names": old_names,
                "members": members,
                "removed_retractions": removed_retractions,
                "vector_slice": [start, all_members.len()],
            }),
        );
    }

    let normalized_matrix: Vec<Vec<f32>> = if all_members.is_empty() {
        Vec::new()
    } else {
        match embd {
            Some(embd) => {
                let texts: Vec<String> = all_members.iter().map(entity_to_query_text).collect();
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                match embd.embed(&refs).await {
                    Ok(vectors) if vectors.len() == all_members.len() => {
                        wiki_normalize_rows(&vectors)
                    }
                    _ => vec![Vec::new(); all_members.len()],
                }
            }
            None => vec![Vec::new(); all_members.len()],
        }
    };

    let mut reconsidered: BTreeMap<String, Option<Vec<Vec<Value>>>> = BTreeMap::new();
    for (page_id, record) in &candidates {
        let slice = record
            .get("vector_slice")
            .and_then(Value::as_array)
            .map(|items| {
                (
                    items.first().and_then(Value::as_i64).unwrap_or(0) as usize,
                    items.get(1).and_then(Value::as_i64).unwrap_or(0) as usize,
                )
            })
            .unwrap_or((0, 0));
        let member_matrix: Vec<Vec<f32>> =
            if slice.1 > slice.0 && slice.1 <= normalized_matrix.len() {
                normalized_matrix[slice.0..slice.1].to_vec()
            } else {
                Vec::new()
            };
        let old_name_set: BTreeSet<String> = record
            .get("old_names")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let members: Vec<Value> = record
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let old_member_indices: Vec<usize> = members
            .iter()
            .enumerate()
            .filter(|(_, member)| {
                member
                    .get("entity_name")
                    .and_then(Value::as_str)
                    .map(|name| old_name_set.contains(name))
                    .unwrap_or(false)
            })
            .map(|(idx, _)| idx)
            .collect();
        let combined_cohesion = wiki_embedding_cohesion(&member_matrix);
        let old_cohesion = if old_member_indices.is_empty() {
            1.0
        } else {
            let subset: Vec<Vec<f32>> = old_member_indices
                .iter()
                .filter_map(|idx| member_matrix.get(*idx).cloned())
                .collect();
            wiki_embedding_cohesion(&subset)
        };
        let over_capacity = members.len() > PAGE_CLUSTER_HARD_MAX_SIZE;
        let degraded = old_member_indices.len() >= 2 && combined_cohesion < old_cohesion - 0.05;
        if !over_capacity && !degraded {
            reconsidered.insert(page_id.clone(), None);
            continue;
        }
        let clusters = wiki_llm_group_entities(chat, &members, &member_matrix, kb_id).await;
        reconsidered.insert(page_id.clone(), Some(clusters));
    }

    let mut result: Map<String, Value> = Map::new();
    let mut used_page_ids: BTreeSet<String> = existing_pages.keys().cloned().collect();
    for key in assignments.keys() {
        if let Some(rest) = key.strip_prefix("_new_") {
            used_page_ids.insert(rest.to_string());
        }
    }
    for (page_id, incoming) in assignments {
        let clusters = reconsidered.get(page_id).and_then(|entry| entry.clone());
        let Some(clusters) = clusters.filter(|clusters| clusters.len() > 1) else {
            result.insert(page_id.clone(), incoming.clone());
            continue;
        };
        let record = candidates
            .get(page_id)
            .cloned()
            .unwrap_or_else(|| json!({}));
        let existing = record.get("existing").cloned().unwrap_or_else(|| json!({}));
        let removed_retractions: Vec<Value> = record
            .get("removed_retractions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let page_title = existing
            .get("title_kwd")
            .map(|value| match value {
                Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
                other => other.as_str().unwrap_or(""),
            })
            .unwrap_or("")
            .to_string();
        let retained_idx = {
            let title_match = if page_title.is_empty() {
                None
            } else {
                clusters.iter().position(|cluster| {
                    cluster.iter().any(|member| {
                        member.get("entity_name").and_then(Value::as_str)
                            == Some(page_title.as_str())
                    })
                })
            };
            title_match.unwrap_or_else(|| {
                let mut best_idx = 0usize;
                let mut best_len = 0usize;
                for (idx, cluster) in clusters.iter().enumerate() {
                    if cluster.len() > best_len {
                        best_len = cluster.len();
                        best_idx = idx;
                    }
                }
                best_idx
            })
        };
        let mut moved_claims: Vec<Value> = Vec::new();
        for (idx, cluster) in clusters.iter().enumerate() {
            if idx == retained_idx {
                continue;
            }
            for member in cluster {
                if let Some(claims) = member.get("claims").and_then(Value::as_array) {
                    moved_claims.extend(claims.iter().cloned());
                }
            }
        }
        let mut retained_cluster = clusters[retained_idx].clone();
        if !retained_cluster.is_empty()
            && (!moved_claims.is_empty() || !removed_retractions.is_empty())
        {
            if let Some(first) = retained_cluster.first_mut() {
                let mut retractions: Vec<Value> = first
                    .get("retractions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                retractions.extend(moved_claims.iter().cloned());
                retractions.extend(removed_retractions.iter().cloned());
                if let Some(obj) = first.as_object_mut() {
                    obj.insert("retractions".to_string(), Value::Array(retractions));
                }
            }
        }
        result.insert(page_id.clone(), Value::Array(retained_cluster));

        for (idx, cluster) in clusters.iter().enumerate() {
            if idx == retained_idx {
                continue;
            }
            if cluster.is_empty() {
                continue;
            }
            let representative = cluster
                .iter()
                .min_by(|left, right| {
                    let left_claims = left
                        .get("claims")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0);
                    let right_claims = right
                        .get("claims")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0);
                    right_claims
                        .cmp(&left_claims)
                        .then_with(|| {
                            let left_name = left
                                .get("entity_name")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let right_name = right
                                .get("entity_name")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            left_name.to_lowercase().cmp(&right_name.to_lowercase())
                        })
                        .then_with(|| {
                            let left_name = left
                                .get("entity_name")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let right_name = right
                                .get("entity_name")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            left_name.cmp(right_name)
                        })
                })
                .cloned()
                .unwrap_or_else(|| cluster[0].clone());
            let mut ordered: Vec<Value> = vec![representative.clone()];
            for entity in cluster {
                if entity != &representative {
                    ordered.push(entity.clone());
                }
            }
            let prefix = page_id
                .split_once('/')
                .map(|(prefix, _)| prefix)
                .unwrap_or("entity");
            let representative_name = representative
                .get("entity_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let base_id = derive_page_id(representative_name, prefix);
            let mut candidate_id = base_id.clone();
            let mut suffix = 2usize;
            while used_page_ids.contains(&candidate_id) {
                candidate_id = format!("{base_id}-{suffix}");
                suffix += 1;
            }
            used_page_ids.insert(candidate_id.clone());
            for entity in ordered.iter_mut() {
                if let Some(obj) = entity.as_object_mut() {
                    obj.insert("action".to_string(), Value::String("create".to_string()));
                    obj.insert("retractions".to_string(), Value::Array(Vec::new()));
                }
            }
            result.insert(format!("_new_{candidate_id}"), Value::Array(ordered));
        }
    }
    result
}

fn plan_group_condition(page_id: &str) -> Map<String, Value> {
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PLAN_GROUP_COMPILE_KWD.to_string()),
    );
    condition.insert("page_id".to_string(), Value::String(page_id.to_string()));
    condition
}

/// `_wiki_update_plan_group`.
pub fn wiki_update_plan_group(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    page_id: &str,
    entity_names: &[String],
    page_version: i64,
) {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let row_id = stable_row_id(&[
        WIKI_PLAN_GROUP_COMPILE_KWD.to_string(),
        kb_id.to_string(),
        page_id.to_string(),
    ]);
    let mut doc = Map::new();
    doc.insert("id".to_string(), Value::String(row_id));
    doc.insert("kb_id".to_string(), Value::String(kb_id.to_string()));
    doc.insert("page_id".to_string(), Value::String(page_id.to_string()));
    doc.insert(
        "entity_names".to_string(),
        Value::String(
            Value::Array(entity_names.iter().cloned().map(Value::String).collect()).to_string(),
        ),
    );
    doc.insert("page_version_int".to_string(), json!(page_version));
    doc.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PLAN_GROUP_COMPILE_KWD.to_string()),
    );
    let fields: Vec<String> = vec!["page_id".to_string()];
    let condition = plan_group_condition(page_id);
    let exists = inc_search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1)
        .map(|rows| !rows.is_empty())
        .unwrap_or(false);
    if exists {
        let mut update_value = doc.clone();
        update_value.remove("id");
        let mut update_condition = Map::new();
        update_condition.insert("page_id".to_string(), Value::String(page_id.to_string()));
        if let Err(err) = store.update(&update_condition, &update_value, &index, kb_id) {
            tracing::warn!(error = %err, page = page_id, "wiki: plan_group update failed");
        }
    } else if let Err(err) = store.insert(&[doc], &index, kb_id) {
        tracing::warn!(error = %err, page = page_id, "wiki: plan_group insert failed");
    }
}

/// `_wiki_delete_plan_group`.
pub fn wiki_delete_plan_group(store: &dyn DocStore, tenant_id: &str, kb_id: &str, page_id: &str) {
    let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
    let condition = plan_group_condition(page_id);
    if let Err(err) = store.delete(&condition, &index, kb_id) {
        tracing::warn!(error = %err, page = page_id, "wiki: plan_group delete failed");
    }
}

/// `_wiki_load_plan_group_members`: `{page_id: [names]}`.
pub fn wiki_load_plan_group_members(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
) -> Map<String, Value> {
    let fields: Vec<String> = ["page_id", "entity_names"]
        .iter()
        .map(|field| field.to_string())
        .collect();
    let mut condition = Map::new();
    condition.insert(
        "compile_kwd".to_string(),
        Value::String(WIKI_PLAN_GROUP_COMPILE_KWD.to_string()),
    );
    let mut result: Map<String, Value> = Map::new();
    let mut offset = 0usize;
    let page_size = 1000usize;
    loop {
        let rows = match inc_search_page(
            store, tenant_id, kb_id, &fields, &condition, offset, page_size,
        ) {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(error = %err, kb = kb_id, "wiki: failed to load plan group members");
                return result;
            }
        };
        if rows.is_empty() {
            break;
        }
        let row_count = rows.len();
        for row in &rows {
            let page_id = row
                .get("page_id")
                .map(|value| match value {
                    Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
                    other => other.as_str().unwrap_or(""),
                })
                .unwrap_or("")
                .to_string();
            let raw_names = match row.get("entity_names") {
                Some(Value::String(text)) if !text.is_empty() => {
                    serde_json::from_str::<Value>(text).unwrap_or(Value::Array(Vec::new()))
                }
                Some(other) => other.clone(),
                None => Value::Array(Vec::new()),
            };
            let mut names: BTreeSet<String> = BTreeSet::new();
            if let Value::Array(items) = raw_names {
                for name in items {
                    if let Some(name) = name.as_str() {
                        if !name.is_empty() {
                            names.insert(name.to_string());
                        }
                    }
                }
            }
            if !page_id.is_empty() && !names.is_empty() {
                result.insert(
                    page_id,
                    Value::Array(names.into_iter().map(Value::String).collect()),
                );
            }
        }
        if row_count < page_size {
            break;
        }
        offset += page_size;
    }
    result
}

#[cfg(test)]
mod wiki_incremental_part14_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::embed::Embedder;
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

    struct TestEmb;

    #[async_trait::async_trait]
    impl Embedder for TestEmb {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[test]
    fn parse_claims_and_member_filtering() {
        assert!(wiki_parse_claims(Some(&json!("not json"))).is_empty());
        assert!(wiki_parse_claims(None).is_empty());
        let parsed = wiki_parse_claims(Some(&json!("[{\"statement\": \"s\"}, 5]")));
        assert_eq!(parsed.len(), 1);

        let single = json!({
            "claims": "[{\"statement\": \"s1\", \"subject\": \"Alpha\"}]",
            "entity_names_kwd": ["Alpha"]
        });
        assert_eq!(wiki_claims_for_entity(&single, "alpha").len(), 1);
        let multi = json!({
            "claims": "[{\"statement\": \"s1\", \"subject\": \"Alpha\"}, {\"statement\": \"s2\", \"subject\": \"Beta\"}]",
            "entity_names_kwd": ["Alpha", "Beta"]
        });
        let alpha_claims = wiki_claims_for_entity(&multi, "Alpha");
        assert_eq!(alpha_claims.len(), 1);
        assert_eq!(alpha_claims[0]["statement"], json!("s1"));
        assert!(wiki_claims_for_entity(&multi, "Gamma").is_empty());
    }

    #[test]
    fn cohesion_shapes() {
        assert_eq!(wiki_embedding_cohesion(&[]), 1.0);
        assert_eq!(wiki_embedding_cohesion(&[vec![1.0, 0.0]]), 1.0);
        let identical = vec![vec![1.0f32, 0.0], vec![1.0, 0.0]];
        assert!((wiki_embedding_cohesion(&identical) - 1.0).abs() < 1e-6);
        let opposite = vec![vec![1.0f32, 0.0], vec![-1.0, 0.0]];
        assert_eq!(wiki_embedding_cohesion(&opposite), 0.0);
    }

    #[tokio::test]
    async fn split_over_capacity_page() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "[[0,1,2,3,4],[5,6,7,8,9]]".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let emb = TestEmb;
        let mut existing: Map<String, Value> = Map::new();
        existing.insert(
            "entity/alpha".to_string(),
            json!({"title_kwd": "Alpha", "entity_names_kwd": ["Alpha"], "claims": "[]"}),
        );
        let incoming: Vec<Value> = (0..10)
            .map(|idx| {
                json!({
                    "entity_name": if idx == 0 { "Alpha".to_string() } else { format!("E{idx}") },
                    "entity_type": "entity",
                    "action": "update",
                    "claims": [],
                    "retractions": [],
                    "source_chunk_ids": [],
                    "source_doc_ids": []
                })
            })
            .collect();
        let mut assignments: Map<String, Value> = Map::new();
        assignments.insert("entity/alpha".to_string(), Value::Array(incoming));
        let split = wiki_split_unstable_page_assignments(
            &store,
            &assignments,
            &existing,
            &chat,
            Some(&emb),
            "kb1",
        )
        .await;
        assert!(split.contains_key("entity/alpha"));
        let new_keys: Vec<&String> = split
            .keys()
            .filter(|key| key.starts_with("_new_"))
            .collect();
        assert_eq!(new_keys.len(), 1);
        let new_cluster = split[new_keys[0]].as_array().unwrap();
        assert_eq!(new_cluster.len(), 5);
        assert!(
            new_cluster
                .iter()
                .all(|entity| entity["action"] == json!("create"))
        );
        let retained = split["entity/alpha"].as_array().unwrap();
        assert_eq!(retained.len(), 5);
    }

    #[test]
    fn plan_group_crud() {
        let store = MemoryDocStore::new();
        assert!(wiki_load_plan_group_members(&store, "t1", "kb1").is_empty());
        wiki_update_plan_group(
            &store,
            "t1",
            "kb1",
            "entity/alpha",
            &["Alpha".to_string(), "Beta".to_string()],
            3,
        );
        let members = wiki_load_plan_group_members(&store, "t1", "kb1");
        assert_eq!(members.len(), 1);
        assert_eq!(members["entity/alpha"], json!(["Alpha", "Beta"]));
        wiki_update_plan_group(
            &store,
            "t1",
            "kb1",
            "entity/alpha",
            &["Alpha".to_string()],
            4,
        );
        let members2 = wiki_load_plan_group_members(&store, "t1", "kb1");
        assert_eq!(members2.len(), 1);
        assert_eq!(members2["entity/alpha"], json!(["Alpha"]));
        wiki_delete_plan_group(&store, "t1", "kb1", "entity/alpha");
        assert!(wiki_load_plan_group_members(&store, "t1", "kb1").is_empty());
    }
}

// ---------------------------------------------------------------------------
// Part 13 — page-move reconciliation and Mode B run
// (`_wiki_reconcile_page_moves`, `_wiki_mode_b_run`).
//
// Adaptation note: the refine fan-out is sequential with the same progress
// cadence; doc_page_source updates run serially as upstream.
// ---------------------------------------------------------------------------

/// `_wiki_reconcile_page_moves`.
pub fn wiki_reconcile_page_moves(
    assignments: &Map<String, Value>,
    existing_pages: &Map<String, Value>,
) -> Map<String, Value> {
    let mut previous_pages: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (page_id, page) in existing_pages {
        for name in as_str_list(page.get("entity_names_kwd")) {
            previous_pages
                .entry(normalize_key(&name))
                .or_default()
                .push((page_id.clone(), name));
        }
    }
    let mut result: Map<String, Value> = Map::new();
    for (target_id, entities) in assignments {
        let target_key = target_id
            .strip_prefix("_new_")
            .unwrap_or(target_id)
            .to_string();
        let entries: Vec<Value> = entities.as_array().cloned().unwrap_or_default();
        for entity in entries {
            let name = entity
                .get("entity_name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let old_memberships = previous_pages
                .get(&normalize_key(&name))
                .cloned()
                .unwrap_or_default();
            let action = entity.get("action").and_then(Value::as_str).unwrap_or("");
            if action == "delete" {
                for (old_page_id, stored_name) in &old_memberships {
                    let mut removal = entity.as_object().cloned().unwrap_or_default();
                    removal.insert(
                        "entity_name".to_string(),
                        Value::String(stored_name.clone()),
                    );
                    removal.insert("claims".to_string(), Value::Array(Vec::new()));
                    let mut retractions: Vec<Value> = entity
                        .get("retractions")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(old_page) = existing_pages.get(old_page_id) {
                        retractions.extend(wiki_claims_for_entity(old_page, stored_name));
                    }
                    removal.insert("retractions".to_string(), Value::Array(retractions));
                    let bucket = result
                        .entry(old_page_id.clone())
                        .or_insert_with(|| Value::Array(Vec::new()));
                    if let Some(items) = bucket.as_array_mut() {
                        items.push(Value::Object(removal));
                    }
                }
                continue;
            }
            let bucket = result
                .entry(target_id.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(items) = bucket.as_array_mut() {
                items.push(entity.clone());
            }
            for (old_page_id, stored_name) in &old_memberships {
                if old_page_id == &target_key {
                    continue;
                }
                let mut retractions: Vec<Value> = Vec::new();
                if let Some(old_page) = existing_pages.get(old_page_id) {
                    retractions = wiki_claims_for_entity(old_page, stored_name);
                }
                let removal = json!({
                    "entity_name": stored_name,
                    "entity_type": entity.get("entity_type").and_then(Value::as_str).unwrap_or("entity"),
                    "aliases": entity.get("aliases").cloned().unwrap_or(Value::Array(Vec::new())),
                    "claims": [],
                    "retractions": retractions,
                    "action": "delete",
                });
                let bucket = result
                    .entry(old_page_id.clone())
                    .or_insert_with(|| Value::Array(Vec::new()));
                if let Some(items) = bucket.as_array_mut() {
                    items.push(removal);
                }
            }
        }
    }
    result.retain(|_, entities| {
        entities
            .as_array()
            .map(|items| !items.is_empty())
            .unwrap_or(false)
    });
    result
}

fn names_value_to_vec(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `_wiki_mode_b_run`: Page Router + per-page REFINE.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_mode_b_run(
    store: &dyn DocStore,
    deltas: &[Value],
    existing_pages: &Map<String, Value>,
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
    doc_to_entities: Option<&Map<String, Value>>,
    entity_evidence: Option<&Map<String, Value>>,
    entity_relations: Option<&Map<String, Value>>,
    doc_topics: Option<&Map<String, Value>>,
    topic_embeddings: Option<&mut Map<String, Value>>,
    topic_pool: Option<&mut Map<String, Value>>,
) -> Value {
    let report = |message: &str| {
        if let Some(callback) = callback {
            callback(0.7, &format!("wiki REFINE B: {message}"));
        }
    };
    let mut pages_created = 0usize;
    let mut pages_modified = 0usize;
    let mut pages_deleted = 0usize;
    let mut errors: Vec<Value> = Vec::new();

    let mut affected_entities: Vec<Value> = Vec::new();
    for delta in deltas {
        let Some(name) = delta.get("entity_name").and_then(Value::as_str) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let mut delta_claims: Vec<Value> = delta
            .get("additions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        delta_claims.extend(
            delta
                .get("claims")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        );
        let relations = entity_relations
            .and_then(|map| map.get(name))
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new()));
        affected_entities.push(json!({
            "entity_name": name,
            "entity_type": delta.get("entity_type").and_then(Value::as_str).unwrap_or("entity"),
            "aliases": delta.get("aliases").cloned().unwrap_or(Value::Array(Vec::new())),
            "claims": wiki_dedupe_claims(&delta_claims),
            "retractions": delta.get("retractions").cloned().unwrap_or(Value::Array(Vec::new())),
            "source_chunk_ids": delta.get("source_chunk_ids").cloned().unwrap_or(Value::Array(Vec::new())),
            "source_doc_ids": delta.get("retained_source_doc_ids").cloned().unwrap_or(Value::Array(Vec::new())),
            "relations": relations,
            "action": delta.get("action").and_then(Value::as_str).unwrap_or(""),
        }));
    }
    if affected_entities.is_empty() {
        report("No affected entities. Skipping.");
        return json!({
            "pages_created": 0,
            "pages_modified": 0,
            "pages_deleted": 0,
            "errors": [],
        });
    }

    report(&format!(
        "Page Router: routing {} entities ...",
        affected_entities.len()
    ));
    let assignments = wiki_page_router(
        store,
        &mut affected_entities,
        chat,
        embd,
        tenant_id,
        kb_id,
        Some(existing_pages),
    )
    .await;
    let assignments = wiki_reconcile_page_moves(&assignments, existing_pages);
    let assignments = wiki_split_unstable_page_assignments(
        store,
        &assignments,
        existing_pages,
        chat,
        embd,
        kb_id,
    )
    .await;
    if assignments.is_empty() {
        report("Page Router: no assignments. Skipping.");
        return json!({
            "pages_created": 0,
            "pages_modified": 0,
            "pages_deleted": 0,
            "errors": [],
        });
    }

    let all_page_ids: Vec<String> = existing_pages.keys().cloned().collect();

    let mut page_source_chunks: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for (pid, entities) in &assignments {
        let page_key = pid.strip_prefix("_new_").unwrap_or(pid).to_string();
        let mut chunks: Vec<Value> = Vec::new();
        for entity in entities.as_array().cloned().unwrap_or_default() {
            for cid in as_str_list(entity.get("source_chunk_ids")) {
                chunks.push(json!({"id": cid, "text": ""}));
            }
            if let Some(claims) = entity.get("claims").and_then(Value::as_array) {
                for claim in claims {
                    for cid in wiki_claim_chunk_ids(claim) {
                        chunks.push(json!({
                            "id": cid,
                            "text": delta_claim_text(claim),
                            "source_doc_id": claim.get("source_doc_id").cloned().unwrap_or(Value::Null),
                        }));
                    }
                }
            }
        }
        if !chunks.is_empty() {
            page_source_chunks.insert(page_key, chunks);
        }
    }

    let mut doc_updates: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut doc_removals: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut topic_selection_stats: Map<String, Value> = Map::new();
    topic_selection_stats.insert("selected".to_string(), json!(0));
    topic_selection_stats.insert("new".to_string(), json!(0));
    topic_selection_stats.insert("new_added".to_string(), json!(0));
    let mut topic_embeddings = topic_embeddings;
    let mut topic_pool = topic_pool;

    let total = assignments.len();
    report(&format!("{total} pages ..."));
    let report_every =
        1.max((total + WIKI_REFINE_PROGRESS_UPDATES - 1) / WIKI_REFINE_PROGRESS_UPDATES);
    let mut completed = 0usize;
    let assignment_entries: Vec<(String, Vec<Value>)> = assignments
        .iter()
        .map(|(pid, entities)| {
            (
                pid.clone(),
                entities.as_array().cloned().unwrap_or_default(),
            )
        })
        .collect();
    for (page_identifier, entities) in assignment_entries {
        let is_new = page_identifier.starts_with("_new_");
        let page_key = page_identifier
            .strip_prefix("_new_")
            .unwrap_or(&page_identifier)
            .trim()
            .to_string();
        if page_key.is_empty() {
            completed += 1;
            continue;
        }
        let existing = if !is_new {
            existing_pages.get(&page_key).cloned()
        } else {
            None
        };
        let page_type = match existing.as_ref() {
            Some(existing) => existing
                .get("page_type_kwd")
                .map(|value| match value {
                    Value::Array(items) => {
                        items.first().and_then(Value::as_str).unwrap_or("entity")
                    }
                    other => other.as_str().unwrap_or("entity"),
                })
                .unwrap_or("entity")
                .to_string(),
            None => {
                if page_key.starts_with("concept/") {
                    "concept".to_string()
                } else {
                    "entity".to_string()
                }
            }
        };

        let mut additions: Vec<Value> = Vec::new();
        let mut retractions: Vec<Value> = Vec::new();
        let mut page_source_doc_ids: BTreeSet<String> = BTreeSet::new();
        let mut member_evidence: Vec<Value> = Vec::new();
        let mut action = if is_new { "create" } else { "update" }.to_string();
        for entity in &entities {
            let ent_claims: Vec<Value> = entity
                .get("claims")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            additions.extend(ent_claims.iter().cloned());
            retractions.extend(
                entity
                    .get("retractions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
            for doc in as_str_list(entity.get("source_doc_ids")) {
                page_source_doc_ids.insert(doc);
            }
            member_evidence.push(json!({
                "name": entity.get("entity_name").and_then(Value::as_str).unwrap_or(""),
                "claims": ent_claims,
                "source_chunk_ids": entity.get("source_chunk_ids").cloned().unwrap_or(Value::Array(Vec::new())),
            }));
        }
        let existing_names = existing
            .as_ref()
            .map(|page| as_str_list(page.get("entity_names_kwd")))
            .unwrap_or_default();
        let added_names: Vec<String> = entities
            .iter()
            .filter(|entity| entity.get("action").and_then(Value::as_str) != Some("delete"))
            .filter_map(|entity| entity.get("entity_name").and_then(Value::as_str))
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
        let deleted_names: BTreeSet<String> = entities
            .iter()
            .filter(|entity| entity.get("action").and_then(Value::as_str) == Some("delete"))
            .filter_map(|entity| entity.get("entity_name").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let mut member_names: BTreeSet<String> = existing_names.iter().cloned().collect();
        member_names.extend(added_names);
        for deleted in &deleted_names {
            member_names.remove(deleted);
        }
        let member_names: Vec<String> = member_names.into_iter().collect();
        if member_names.is_empty() {
            action = "delete".to_string();
        }

        let mut member_source_chunks: Vec<Value> = page_source_chunks
            .get(&page_key)
            .cloned()
            .unwrap_or_default();
        for member_name in &member_names {
            let evidence = entity_evidence
                .and_then(|map| map.get(member_name))
                .cloned()
                .unwrap_or_else(|| json!({}));
            for doc in as_str_list(evidence.get("source_doc_ids")) {
                page_source_doc_ids.insert(doc);
            }
            let evidence_chunks = as_str_list(evidence.get("source_chunk_ids"));
            for cid in &evidence_chunks {
                member_source_chunks.push(json!({"id": cid, "text": ""}));
            }
            let already = member_evidence
                .iter()
                .any(|item| item.get("name").and_then(Value::as_str) == Some(member_name.as_str()));
            if !already {
                let claims = existing
                    .as_ref()
                    .map(|page| wiki_claims_for_entity(page, member_name))
                    .unwrap_or_default();
                member_evidence.push(json!({
                    "name": member_name,
                    "claims": claims,
                    "source_chunk_ids": evidence_chunks.iter().cloned().map(Value::String).collect::<Vec<Value>>(),
                }));
            }
        }

        if action == "delete" {
            let page_title = existing
                .as_ref()
                .and_then(|page| page.get("title_kwd"))
                .map(|value| match value {
                    Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
                    other => other.as_str().unwrap_or(""),
                })
                .unwrap_or(page_key.as_str())
                .to_string();
            let page_version = existing
                .as_ref()
                .map(|page| as_int(page.get("page_version_int"), 0))
                .unwrap_or(0);
            let deleted_page = wiki_refine_page(RefinePageArgs {
                store,
                mode: "delete",
                page_id: &page_key,
                page_title: &page_title,
                existing_page: existing.as_ref(),
                page_type_kwd: &page_type,
                additions: None,
                retractions: None,
                source_chunks: Some(&[]),
                claims: Some(&[]),
                available_pages: Some(&all_page_ids),
                contextual_hints: "",
                chat,
                embd,
                tenant_id,
                kb_id,
                page_version,
                entity_names: None,
                page_embedding: None,
                embed_routing_context: false,
                source_doc_ids: None,
                topic_candidates: None,
                topic_selection_stats: None,
                topic_embeddings: None,
                topic_pool: None,
                member_evidence: None,
            })
            .await;
            if deleted_page.is_none() {
                wiki_clear_refine_failure(store, tenant_id, kb_id, &page_key);
                wiki_delete_plan_group(store, tenant_id, kb_id, &page_key);
                if let Some(existing) = existing.as_ref() {
                    for did in as_str_list(existing.get("source_doc_ids")) {
                        doc_removals.entry(did).or_default().push(page_key.clone());
                    }
                }
                pages_deleted += 1;
            }
            completed += 1;
            if completed % report_every == 0 || completed == total {
                report(&format!("{completed}/{total} pages completed."));
            }
            continue;
        }

        let page_title = match existing.as_ref() {
            Some(existing) => existing
                .get("title_kwd")
                .map(|value| match value {
                    Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
                    other => other.as_str().unwrap_or(""),
                })
                .unwrap_or(page_key.as_str())
                .to_string(),
            None => entities
                .first()
                .and_then(|entity| entity.get("entity_name").and_then(Value::as_str))
                .unwrap_or(page_key.as_str())
                .to_string(),
        };
        let page_version = existing
            .as_ref()
            .map(|page| as_int(page.get("page_version_int"), 0))
            .unwrap_or(0);
        let next_version = page_version + 1;
        let new_doc_ids: BTreeSet<String> = additions
            .iter()
            .filter_map(|claim| claim.get("source_doc_id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        let mut refine_mode = if is_new { "generate" } else { "modify" };
        if let Some(existing) = existing.as_ref() {
            if should_re_synthesize(existing, &new_doc_ids, next_version) {
                refine_mode = "re-synthesize";
            }
        }
        let source_docs: Vec<String> = page_source_doc_ids.iter().cloned().collect();
        let topic_candidates =
            wiki_topics_for_docs(&source_docs, doc_topics, topic_pool.as_deref());
        let contextual_hints =
            wiki_build_contextual_hints(&page_key, existing.as_ref(), &Map::new());
        let result = wiki_refine_page(RefinePageArgs {
            store,
            mode: refine_mode,
            page_id: &page_key,
            page_title: &page_title,
            existing_page: existing.as_ref(),
            page_type_kwd: &page_type,
            additions: Some(&additions),
            retractions: Some(&retractions),
            source_chunks: Some(&member_source_chunks),
            claims: Some(&additions),
            available_pages: Some(&all_page_ids),
            contextual_hints: &contextual_hints,
            chat,
            embd,
            tenant_id,
            kb_id,
            page_version,
            entity_names: Some(&member_names),
            page_embedding: None,
            embed_routing_context: true,
            source_doc_ids: Some(&source_docs),
            topic_candidates: Some(&topic_candidates),
            topic_selection_stats: Some(&mut topic_selection_stats),
            topic_embeddings: topic_embeddings.as_deref_mut(),
            topic_pool: topic_pool.as_deref_mut(),
            member_evidence: Some(&member_evidence),
        })
        .await;
        match result {
            Some(result_page) => {
                wiki_clear_refine_failure(store, tenant_id, kb_id, &page_key);
                if is_new {
                    pages_created += 1;
                } else {
                    pages_modified += 1;
                }
                let result_page_version = as_int(result_page.get("page_version_int"), 1);
                wiki_update_plan_group(
                    store,
                    tenant_id,
                    kb_id,
                    &page_key,
                    &member_names,
                    result_page_version,
                );
                for entity in &entities {
                    if let Some(claims) = entity.get("claims").and_then(Value::as_array) {
                        for claim in claims {
                            if let Some(did) = claim.get("source_doc_id").and_then(Value::as_str) {
                                doc_updates
                                    .entry(did.to_string())
                                    .or_default()
                                    .push(page_key.clone());
                            }
                        }
                    }
                }
                for did in &source_docs {
                    doc_updates
                        .entry(did.clone())
                        .or_default()
                        .push(page_key.clone());
                }
                if let Some(existing) = existing.as_ref() {
                    let old_doc_ids: BTreeSet<String> = as_str_list(existing.get("source_doc_ids"))
                        .into_iter()
                        .collect();
                    let new_doc_ids: BTreeSet<String> =
                        as_str_list(result_page.get("source_doc_ids"))
                            .into_iter()
                            .collect();
                    for did in old_doc_ids.difference(&new_doc_ids) {
                        doc_removals
                            .entry(did.clone())
                            .or_default()
                            .push(page_key.clone());
                    }
                }
            }
            None => {
                let failed_page_id = page_identifier
                    .strip_prefix("_new_")
                    .unwrap_or(&page_identifier)
                    .to_string();
                let names: Vec<String> = entities
                    .iter()
                    .filter_map(|entity| entity.get("entity_name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect();
                wiki_record_refine_failure(
                    store,
                    tenant_id,
                    kb_id,
                    &failed_page_id,
                    &names,
                    &format!("REFINE returned no page content for {page_key}"),
                );
                errors.push(Value::String(format!("REFINE_FAILED:{page_identifier}")));
            }
        }
        completed += 1;
        if completed % report_every == 0 || completed == total {
            report(&format!("{completed}/{total} pages completed."));
        }
    }

    wiki_log_stats(
        "TOPIC",
        "selection_summary",
        &[
            ("mode", json!("B")),
            (
                "selected",
                topic_selection_stats
                    .get("selected")
                    .cloned()
                    .unwrap_or(json!(0)),
            ),
            (
                "new",
                topic_selection_stats
                    .get("new")
                    .cloned()
                    .unwrap_or(json!(0)),
            ),
            (
                "new_added",
                topic_selection_stats
                    .get("new_added")
                    .cloned()
                    .unwrap_or(json!(0)),
            ),
        ],
    );

    let mut touched_docs: BTreeSet<String> = doc_updates.keys().cloned().collect();
    touched_docs.extend(doc_removals.keys().cloned());
    for did in touched_docs {
        let existing_dps =
            wiki_load_doc_page_source(store, tenant_id, kb_id, &did).unwrap_or_else(|| json!({}));
        let removed: BTreeSet<String> = doc_removals
            .get(&did)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut existing_pids: Vec<String> = as_str_list(existing_dps.get("page_ids"));
        existing_pids.retain(|pid| !removed.contains(pid));
        for pid in doc_updates.get(&did).cloned().unwrap_or_default() {
            if !existing_pids.iter().any(|existing| existing == &pid) {
                existing_pids.push(pid);
            }
        }
        let mut doc_entity_names: Vec<String> = doc_to_entities
            .and_then(|map| map.get(&did))
            .map(|value| names_value_to_vec(Some(value)))
            .unwrap_or_default();
        if doc_entity_names.is_empty() {
            doc_entity_names = as_str_list(existing_dps.get("entity_names"));
        }
        let chunk_hashes: Map<String, Value> = existing_dps
            .get("source_chunk_hashes")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let map_checksum = existing_dps
            .get("map_checksum")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        wiki_update_doc_page_source(
            store,
            tenant_id,
            kb_id,
            &did,
            &existing_pids,
            if doc_entity_names.is_empty() {
                None
            } else {
                Some(&doc_entity_names)
            },
            Some(&chunk_hashes),
            Some(&map_checksum),
        );
    }

    report(&format!(
        "done: +{pages_created} ~{pages_modified} -{pages_deleted}"
    ));
    json!({
        "pages_created": pages_created,
        "pages_modified": pages_modified,
        "pages_deleted": pages_deleted,
        "errors": errors,
    })
}

#[cfg(test)]
mod wiki_incremental_part15_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::embed::Embedder;
    use crate::harness::HarnessChat;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct SequentialChat {
        replies: Mutex<VecDeque<String>>,
        calls: Mutex<Vec<String>>,
    }

    impl SequentialChat {
        fn new(replies: &[&str]) -> Self {
            Self {
                replies: Mutex::new(replies.iter().map(|reply| (*reply).to_string()).collect()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl HarnessChat for SequentialChat {
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
            let mut queue = self.replies.lock().unwrap();
            Ok(queue.pop_front().unwrap_or_default())
        }

        fn max_length(&self) -> usize {
            100_000
        }
    }

    struct TestEmb;

    #[async_trait::async_trait]
    impl Embedder for TestEmb {
        async fn embed(&self, texts: &[&str]) -> crate::Result<Vec<Vec<f32>>> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
    }

    #[test]
    fn reconcile_moves_routes_deletes_and_removals() {
        let mut existing: Map<String, Value> = Map::new();
        existing.insert(
            "entity/old".to_string(),
            json!({
                "entity_names_kwd": ["Alpha", "Beta"],
                "claims": "[{\"statement\": \"sA\", \"subject\": \"Alpha\"}]"
            }),
        );
        let mut assignments: Map<String, Value> = Map::new();
        assignments.insert(
            "_new_entity/alpha".to_string(),
            json!([{"entity_name": "Alpha", "entity_type": "entity", "action": "update", "claims": [], "retractions": []}]),
        );
        assignments.insert(
            "entity/old".to_string(),
            json!([{"entity_name": "Beta", "entity_type": "entity", "action": "delete", "retractions": [{"statement": "gone"}]}]),
        );
        let reconciled = wiki_reconcile_page_moves(&assignments, &existing);
        assert!(reconciled.contains_key("_new_entity/alpha"));
        let old_entries = reconciled["entity/old"].as_array().unwrap();
        assert_eq!(old_entries.len(), 2);
        let beta_removal = old_entries
            .iter()
            .find(|entry| entry["entity_name"] == json!("Beta"))
            .unwrap();
        assert_eq!(beta_removal["action"], json!("delete"));
        assert_eq!(beta_removal["retractions"].as_array().unwrap().len(), 1);
        let alpha_removal = old_entries
            .iter()
            .find(|entry| entry["entity_name"] == json!("Alpha"))
            .unwrap();
        assert_eq!(alpha_removal["action"], json!("delete"));
        assert_eq!(alpha_removal["retractions"].as_array().unwrap().len(), 1);
        assert_eq!(alpha_removal["retractions"][0]["statement"], json!("sA"));
    }

    #[tokio::test]
    async fn mode_b_routes_updates_and_tracks_plan_group() {
        let store = MemoryDocStore::new();
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "entity/alpha",
            "title_kwd": "Alpha",
            "page_type_kwd": "entity",
            "md_with_weight": "OLD",
            "entity_names_kwd": ["Alpha"],
            "source_chunk_ids": ["c1"],
            "source_doc_ids": ["d0"],
            "page_version_int": 2,
            "synthesis_version_int": 1,
            "topic_kwd": "alpha",
            "claims": "[]",
            "embedding": [1.0, 0.0],
            "q_2_vec": [1.0, 0.0]
        });
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
        let mut existing: Map<String, Value> = Map::new();
        existing.insert(
            "entity/alpha".to_string(),
            json!({
                "title_kwd": "Alpha",
                "page_type_kwd": "entity",
                "md_with_weight": "OLD",
                "entity_names_kwd": ["Alpha"],
                "source_chunk_ids": ["c1"],
                "source_doc_ids": ["d0"],
                "page_version_int": 2,
                "synthesis_version_int": 1,
                "topic_kwd": "alpha",
                "claims": "[]"
            }),
        );
        let chat = SequentialChat::new(&[
            "[{\"id\": 0, \"page\": \"entity/alpha\"}]",
            "TOPIC: alpha\n# Updated B\n\nBody from B.",
        ]);
        let emb = TestEmb;
        let delta = json!({
            "entity_name": "Alpha",
            "entity_type": "entity",
            "aliases": [],
            "additions": [{"statement": "s1", "source_doc_id": "d1", "chunk_ids": ["c1"]}],
            "retractions": [],
            "claims": [],
            "source_chunk_ids": ["c1"],
            "retained_source_doc_ids": ["d1"],
            "action": "update"
        });
        let summary = wiki_mode_b_run(
            &store,
            &[delta],
            &existing,
            &chat,
            Some(&emb),
            "t1",
            "kb1",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(summary["pages_modified"], json!(1));
        assert_eq!(summary["pages_created"], json!(0));
        let pages = search_existing_pages(
            &store,
            "t1",
            "kb1",
            &["slug_kwd".to_string(), "md_with_weight".to_string()],
        );
        let alpha = pages.get("entity/alpha").cloned().expect("alpha");
        assert!(
            alpha["md_with_weight"]
                .as_str()
                .unwrap()
                .contains("Body from B")
        );
        let groups = wiki_load_plan_group_members(&store, "t1", "kb1");
        assert_eq!(groups["entity/alpha"], json!(["Alpha"]));
        let dps = wiki_load_doc_page_source(&store, "t1", "kb1", "d1").expect("doc source");
        let page_ids: Vec<String> = as_str_list(dps.get("page_ids"));
        assert!(page_ids.contains(&"entity/alpha".to_string()));
        assert_eq!(chat.calls.lock().unwrap().len(), 2);
    }
}

// ---------------------------------------------------------------------------
// Part 14 — the public entry point (`wiki_compile_incremental`).
//
// Adaptation note: state loading, entity matching, canonical persistence,
// REDUCE dispatch and the mode runs are the ported helpers; the disabled-doc
// set is host-injected into FINALIZE.
// ---------------------------------------------------------------------------

fn chunk_id_set(chunk_delta: &Map<String, Value>, key: &str) -> BTreeSet<String> {
    chunk_delta
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn summary_zero() -> Value {
    json!({
        "pages_created": 0,
        "pages_modified": 0,
        "pages_deleted": 0,
        "errors": [],
    })
}

/// `wiki_compile_incremental`: main entry for dual-mode wiki compilation.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_compile_incremental(
    store: &dyn DocStore,
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    tenant_id: &str,
    kb_id: &str,
    mode: &str,
    chunk_delta: &Map<String, Value>,
    previous_chunk_state: &Map<String, Value>,
    current_chunk_state: &Map<String, Value>,
    incremental: bool,
    deleted_doc_ids: Option<&BTreeSet<String>>,
    disabled_doc_ids: &BTreeSet<String>,
    callback: Option<&(dyn Fn(f64, &str) + Send + Sync)>,
) -> Value {
    let report = |message: &str| {
        if let Some(callback) = callback {
            callback(0.5, message);
        }
    };
    let mut incremental = incremental;

    let changed_chunks = chunk_id_set(chunk_delta, "changed_chunk_ids");
    let deleted_chunks = chunk_id_set(chunk_delta, "deleted_chunk_ids");
    let new_chunks = chunk_id_set(chunk_delta, "new_chunk_ids");
    let mut invalidated_chunk_ids: BTreeSet<String> = changed_chunks.clone();
    invalidated_chunk_ids.extend(deleted_chunks.iter().cloned());
    let mut delta_current_chunk_ids: BTreeSet<String> = new_chunks.clone();
    delta_current_chunk_ids.extend(changed_chunks.iter().cloned());

    let map_results = crate::harness::knowlege_wiki::load_map_extracts_for_state(
        store,
        tenant_id,
        kb_id,
        current_chunk_state,
        None,
    );
    let mut delta_after_results: Vec<Value> = Vec::new();
    if !delta_current_chunk_ids.is_empty() {
        delta_after_results = crate::harness::knowlege_wiki::load_map_extracts_for_state(
            store,
            tenant_id,
            kb_id,
            current_chunk_state,
            Some(&delta_current_chunk_ids),
        );
    }
    let mut delta_before_results: Vec<Value> = Vec::new();
    if !invalidated_chunk_ids.is_empty() {
        delta_before_results = crate::harness::knowlege_wiki::load_map_extracts_for_state(
            store,
            tenant_id,
            kb_id,
            previous_chunk_state,
            Some(&invalidated_chunk_ids),
        );
    }
    if map_results.is_empty() && delta_before_results.is_empty() {
        report("No MAP results found. Skipping wiki compilation.");
        return summary_zero();
    }

    // Correct the incremental flag for interrupted first builds.
    if incremental && !wiki_has_any_pages(store, tenant_id, kb_id) {
        report(
            "No compiled wiki pages found; treating as first build (previous build was interrupted).",
        );
        incremental = false;
    }

    report("Entity Matching: deduplicating entities and concepts ...");
    let (raw_entities, claim_index) = extract_raw_entities(&map_results);
    let (before_raw_entities, _) = extract_raw_entities(&delta_before_results);
    let before_raw_names: BTreeSet<String> = before_raw_entities
        .iter()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let (after_raw_entities, _) = extract_raw_entities(&delta_after_results);
    let after_raw_names: BTreeSet<String> = after_raw_entities
        .iter()
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();

    let mut doc_topics: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut raw_topic_count = 0usize;
    let mut raw_relations: Vec<Value> = Vec::new();
    for mr in &map_results {
        let doc_id = mr
            .get("doc_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if doc_id.is_empty() {
            continue;
        }
        let mut seen_topics: BTreeSet<String> = BTreeSet::new();
        if let Some(topics) = mr.get("topics").and_then(Value::as_array) {
            for topic in topics {
                if let Some(topic) = topic.as_str() {
                    raw_topic_count += 1;
                    let topic = topic.trim();
                    let key = topic.to_lowercase();
                    if !topic.is_empty() && seen_topics.insert(key) {
                        doc_topics
                            .entry(doc_id.clone())
                            .or_default()
                            .push(topic.to_string());
                    }
                }
            }
        }
        if let Some(relations) = mr.get("relations").and_then(Value::as_array) {
            for relation in relations {
                let relation = match relation {
                    Value::String(text) => {
                        serde_json::from_str::<Value>(text).unwrap_or(Value::Null)
                    }
                    other => other.clone(),
                };
                let from = relation.get("from").and_then(Value::as_str).unwrap_or("");
                let to = relation.get("to").and_then(Value::as_str).unwrap_or("");
                if !from.is_empty() && !to.is_empty() {
                    raw_relations.push(json!({
                        "from": from,
                        "to": to,
                        "type": relation.get("type").and_then(Value::as_str).unwrap_or("related"),
                    }));
                }
            }
        }
    }
    let unique_topics: Vec<String> = {
        let set: BTreeSet<String> = doc_topics
            .values()
            .flat_map(|topics| topics.iter().cloned())
            .collect();
        let mut sorted: Vec<String> = set.into_iter().collect();
        sorted.sort_by_key(|value| (value.to_lowercase(), value.clone()));
        sorted
    };
    wiki_log_stats(
        "TOPIC",
        "map_summary",
        &[
            ("document_count", json!(doc_topics.len())),
            ("raw_count", json!(raw_topic_count)),
            ("unique_count", json!(unique_topics.len())),
            (
                "topics",
                Value::Array(unique_topics.iter().cloned().map(Value::String).collect()),
            ),
        ],
    );

    let canonical_entities = load_canonical_entities(store, tenant_id, kb_id);
    let (canonical_map_matched, name_resolution) = wiki_match_entities(
        store,
        embd,
        Some(chat),
        tenant_id,
        kb_id,
        &raw_entities,
        &canonical_entities,
        incremental,
        None,
    )
    .await;
    let mut canonical_map = canonical_map_matched;

    let mut canonical_resolution: BTreeMap<String, String> = BTreeMap::new();
    for (raw_name, cname) in &name_resolution {
        if let Some(cname) = cname.as_str() {
            canonical_resolution.insert(raw_name.clone(), cname.to_string());
        }
    }
    for (canonical_name, canonical_entry) in &canonical_entities {
        canonical_resolution
            .entry(canonical_name.clone())
            .or_insert_with(|| canonical_name.clone());
        for alias in as_str_list(canonical_entry.get("aliases")) {
            canonical_resolution
                .entry(alias)
                .or_insert_with(|| canonical_name.clone());
        }
    }
    let mut entity_relations: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut seen_relations: BTreeSet<(String, String, String)> = BTreeSet::new();
    for relation in &raw_relations {
        let from = relation.get("from").and_then(Value::as_str).unwrap_or("");
        let to = relation.get("to").and_then(Value::as_str).unwrap_or("");
        let source = canonical_resolution
            .get(from)
            .cloned()
            .unwrap_or_else(|| from.to_string());
        let target = canonical_resolution
            .get(to)
            .cloned()
            .unwrap_or_else(|| to.to_string());
        let relation_type = relation
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("related")
            .to_string();
        if source.is_empty() || target.is_empty() || source == target {
            continue;
        }
        for (owner, counterpart) in [(&source, &target), (&target, &source)] {
            let key = (owner.clone(), counterpart.clone(), relation_type.clone());
            if !seen_relations.insert(key) {
                continue;
            }
            entity_relations
                .entry(owner.clone())
                .or_default()
                .push(json!({"entity": counterpart, "type": relation_type}));
        }
    }

    let mut current_evidence: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>, i64)> =
        BTreeMap::new();
    for entry in &raw_entities {
        let raw_name = entry.get("name").and_then(Value::as_str).unwrap_or("");
        let cname = name_resolution
            .get(raw_name)
            .and_then(Value::as_str)
            .unwrap_or(raw_name)
            .to_string();
        let evidence = current_evidence
            .entry(cname)
            .or_insert_with(|| (BTreeSet::new(), BTreeSet::new(), 0));
        for doc in as_str_list(entry.get("source_doc_ids")) {
            evidence.0.insert(doc);
        }
        for chunk in as_str_list(entry.get("source_chunk_ids")) {
            evidence.1.insert(chunk);
        }
        evidence.2 += as_int(entry.get("claim_count"), 0);
    }
    let canonical_names_vec: Vec<String> = canonical_map.keys().cloned().collect();
    for cname in canonical_names_vec {
        let Some((docs, chunks, claims)) = current_evidence.get(&cname).cloned() else {
            continue;
        };
        if let Some(centry) = canonical_map.get_mut(&cname).and_then(Value::as_object_mut) {
            centry.insert(
                "source_doc_ids".to_string(),
                Value::Array(docs.into_iter().map(Value::String).collect()),
            );
            centry.insert(
                "source_chunk_ids".to_string(),
                Value::Array(chunks.into_iter().map(Value::String).collect()),
            );
            centry.insert("claim_count".to_string(), json!(claims));
        }
    }

    if canonical_map.is_empty() && before_raw_names.is_empty() {
        report("Entity Matching: no canonical entities found. Skipping.");
        return summary_zero();
    }
    report(&format!("Entity Matching: {}", name_resolution.len()));

    let mut changed_items: Vec<(String, Value)> = Vec::new();
    let mut new_items: Vec<(String, Value, String)> = Vec::new();
    for (cname, centry) in &canonical_map {
        match canonical_entities.get(cname) {
            Some(existing) => {
                let old_docs: BTreeSet<String> = as_str_list(existing.get("source_doc_ids"))
                    .into_iter()
                    .collect();
                let new_docs: BTreeSet<String> = as_str_list(centry.get("source_doc_ids"))
                    .into_iter()
                    .collect();
                let old_chunks: BTreeSet<String> = as_str_list(existing.get("source_chunk_ids"))
                    .into_iter()
                    .collect();
                let new_chunks: BTreeSet<String> = as_str_list(centry.get("source_chunk_ids"))
                    .into_iter()
                    .collect();
                let old_aliases: BTreeSet<String> =
                    as_str_list(existing.get("aliases")).into_iter().collect();
                let new_aliases: BTreeSet<String> =
                    as_str_list(centry.get("aliases")).into_iter().collect();
                let claim_count = as_int(centry.get("claim_count"), 0);
                let mention_count = as_int(existing.get("mention_count_int"), 0);
                if old_docs != new_docs
                    || old_chunks != new_chunks
                    || old_aliases != new_aliases
                    || claim_count > mention_count
                {
                    changed_items.push((cname.clone(), centry.clone()));
                }
            }
            None => {
                new_items.push((cname.clone(), centry.clone(), entity_to_query_text(centry)));
            }
        }
    }
    for (cname, centry) in &changed_items {
        let entity_type = centry
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("entity")
            .to_string();
        let aliases = as_str_list(centry.get("aliases"));
        let source_docs = as_str_list(centry.get("source_doc_ids"));
        let source_chunks = as_str_list(centry.get("source_chunk_ids"));
        let claim_count = as_int(centry.get("claim_count"), 0);
        update_canonical_entity(
            store,
            tenant_id,
            kb_id,
            cname,
            &entity_type,
            &aliases,
            &source_docs,
            claim_count,
            Some(&source_chunks),
        );
    }
    if !new_items.is_empty() {
        let embeddings: Vec<Option<Vec<f32>>> = match embd {
            Some(embd) => {
                let texts: Vec<String> =
                    new_items.iter().map(|(_, _, text)| text.clone()).collect();
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                match embd.embed(&refs).await {
                    Ok(vectors) if vectors.len() == new_items.len() => {
                        vectors.into_iter().map(Some).collect()
                    }
                    _ => vec![None; new_items.len()],
                }
            }
            None => vec![None; new_items.len()],
        };
        let index = crate::harness::knowlege_dataset_nav::index_name(tenant_id);
        let mut rows: Vec<DocRow> = Vec::new();
        for ((cname, centry, _), embedding) in new_items.iter().zip(embeddings.iter()) {
            let entity_type = centry
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("entity");
            let aliases = as_str_list(centry.get("aliases"));
            let source_docs = as_str_list(centry.get("source_doc_ids"));
            let source_chunks = as_str_list(centry.get("source_chunk_ids"));
            let claim_count = as_int(centry.get("claim_count"), 0);
            let doc = build_canonical_entity_doc(
                kb_id,
                cname,
                entity_type,
                &aliases,
                &source_docs,
                claim_count,
                embedding.as_deref(),
                Some(&source_chunks),
            );
            if let Some(map) = doc.as_object().cloned() {
                rows.push(map);
            }
        }
        if !rows.is_empty() {
            if let Err(err) = store.insert(&rows, &index, kb_id) {
                tracing::warn!(error = %err, "wiki: canonical insert failed");
            }
        }
    }

    if !invalidated_chunk_ids.is_empty() {
        for (cname, existing) in &canonical_entities {
            if canonical_map.contains_key(cname) {
                continue;
            }
            let old_chunks: BTreeSet<String> = as_str_list(existing.get("source_chunk_ids"))
                .into_iter()
                .collect();
            if old_chunks.is_disjoint(&invalidated_chunk_ids) {
                continue;
            }
            let remaining: BTreeSet<String> = old_chunks
                .difference(&invalidated_chunk_ids)
                .cloned()
                .collect();
            if remaining.is_empty() {
                delete_canonical_entity(store, tenant_id, kb_id, cname);
            } else {
                let entity_type = existing
                    .get("entity_type_kwd")
                    .and_then(Value::as_str)
                    .unwrap_or("entity")
                    .to_string();
                let aliases = as_str_list(existing.get("aliases"));
                let source_docs = as_str_list(existing.get("source_doc_ids"));
                let mention_count = as_int(existing.get("mention_count_int"), 0);
                let remaining_vec: Vec<String> = remaining.into_iter().collect();
                update_canonical_entity(
                    store,
                    tenant_id,
                    kb_id,
                    cname,
                    &entity_type,
                    &aliases,
                    &source_docs,
                    mention_count,
                    Some(&remaining_vec),
                );
            }
        }
    }

    if let Some(deleted_doc_ids) = deleted_doc_ids {
        let names: Vec<String> = canonical_map.keys().cloned().collect();
        for cname in names {
            let keep = {
                let Some(entry) = canonical_map.get(&cname) else {
                    continue;
                };
                let docs: Vec<String> = as_str_list(entry.get("source_doc_ids"))
                    .into_iter()
                    .filter(|doc| !deleted_doc_ids.contains(doc))
                    .collect();
                let claim_count = as_int(entry.get("claim_count"), 0);
                if let Some(obj) = canonical_map.get_mut(&cname).and_then(Value::as_object_mut) {
                    obj.insert(
                        "source_doc_ids".to_string(),
                        Value::Array(docs.iter().cloned().map(Value::String).collect()),
                    );
                }
                !(docs.is_empty() && claim_count <= 0)
            };
            if !keep {
                delete_canonical_entity(store, tenant_id, kb_id, &cname);
                canonical_map.remove(&cname);
            }
        }
    }

    report("REDUCE: computing per-entity changes ...");
    let canonical_names: BTreeSet<String> = canonical_map.keys().cloned().collect();
    let mut affected_names: BTreeSet<String> = if incremental {
        let mut existing_aliases: BTreeMap<String, String> = BTreeMap::new();
        for (cname, centry) in &canonical_entities {
            existing_aliases.insert(normalize_key(cname), cname.clone());
            for alias in as_str_list(centry.get("aliases")) {
                existing_aliases.insert(normalize_key(&alias), cname.clone());
            }
        }
        let mut affected: BTreeSet<String> = BTreeSet::new();
        for raw_name in before_raw_names.union(&after_raw_names) {
            let resolved = name_resolution
                .get(raw_name)
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| existing_aliases.get(&normalize_key(raw_name)).cloned())
                .unwrap_or_else(|| raw_name.clone());
            if !resolved.is_empty() {
                affected.insert(resolved);
            }
        }
        affected
    } else {
        canonical_names.clone()
    };
    affected_names.remove(&String::new());

    let page_fields: Vec<String> = [
        "slug_kwd",
        "title_kwd",
        "md_with_weight",
        "claims",
        "source_chunk_ids",
        "source_doc_ids",
        "page_version_int",
        "synthesis_version_int",
        "entity_names_kwd",
        "outlinks_kwd",
        "related_kb_pages_kwd",
        "page_type_kwd",
        "topic_kwd",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let mut existing_pages = search_existing_pages(store, tenant_id, kb_id, &page_fields);
    let fallback_key = normalize_key(WIKI_TOPIC_FALLBACK);
    let mut topic_pool: Map<String, Value> = Map::new();
    for page in existing_pages.values() {
        for topic in as_str_list(page.get("topic_kwd")) {
            if !topic.is_empty() && normalize_key(&topic) != fallback_key {
                topic_pool.insert(normalize_key(&topic), Value::String(topic));
            }
        }
    }
    if mode == "topic" && !existing_pages.is_empty() {
        let plan_members = wiki_load_plan_group_members(store, tenant_id, kb_id);
        for (page_id, names) in &plan_members {
            if let Some(names) = names.as_array().filter(|items| !items.is_empty()) {
                if existing_pages.contains_key(page_id) {
                    if let Some(obj) = existing_pages
                        .get_mut(page_id)
                        .and_then(Value::as_object_mut)
                    {
                        obj.insert("entity_names_kwd".to_string(), Value::Array(names.clone()));
                    }
                }
            }
        }
    }

    let refine_failures = wiki_load_refine_failures(store, tenant_id, kb_id);
    let mut retry_names: BTreeSet<String> = BTreeSet::new();
    for failure in refine_failures.values() {
        for name in as_str_list(failure.get("entity_names")) {
            let name = name.trim().to_string();
            if !name.is_empty() {
                retry_names.insert(name);
            }
        }
    }
    if incremental {
        affected_names.extend(retry_names);
    }

    let mut canonical_claims: Map<String, Value> = Map::new();
    for (raw_name, claims) in &claim_index {
        let cname = name_resolution
            .get(raw_name)
            .and_then(Value::as_str)
            .unwrap_or(raw_name)
            .to_string();
        if affected_names.contains(&cname) {
            let bucket = canonical_claims
                .entry(cname)
                .or_insert_with(|| Value::Array(Vec::new()));
            if let (Some(items), Some(claims)) = (bucket.as_array_mut(), claims.as_array()) {
                items.extend(claims.iter().cloned());
            }
        }
    }
    for name in &affected_names {
        canonical_claims
            .entry(name.clone())
            .or_insert_with(|| Value::Array(Vec::new()));
    }

    let deleted_set: BTreeSet<String> = deleted_doc_ids.cloned().unwrap_or_default();
    let deltas = wiki_reduce_batch(
        &affected_names,
        &existing_pages,
        &deleted_set,
        Some(&invalidated_chunk_ids),
        Some(&canonical_claims),
        Some(&canonical_map),
        Some(&name_resolution),
        None,
    );
    if deltas.is_empty() {
        report("REDUCE: no changes detected.");
        return summary_zero();
    }

    let mut doc_to_entities: Map<String, Value> = Map::new();
    let mut entity_evidence: Map<String, Value> = Map::new();
    for (cname, centry) in &canonical_map {
        let docs = as_str_list(centry.get("source_doc_ids"));
        let chunks = as_str_list(centry.get("source_chunk_ids"));
        entity_evidence.insert(
            cname.clone(),
            json!({
                "source_doc_ids": docs,
                "source_chunk_ids": chunks,
            }),
        );
        for did in docs {
            let bucket = doc_to_entities
                .entry(did)
                .or_insert_with(|| Value::Array(Vec::new()));
            if let Some(items) = bucket.as_array_mut() {
                items.push(Value::String(cname.clone()));
            }
        }
    }

    let topic_pool_values: Vec<String> = topic_pool
        .values()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let mut topic_embeddings = wiki_prepare_topic_embeddings(
        embd,
        &doc_topics
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    Value::Array(v.iter().cloned().map(Value::String).collect()),
                )
            })
            .collect(),
        Some(&topic_pool_values),
    )
    .await;

    let summary = if mode == "topic" {
        wiki_mode_b_run(
            store,
            &deltas,
            &existing_pages,
            chat,
            embd,
            tenant_id,
            kb_id,
            callback,
            Some(&doc_to_entities),
            Some(&entity_evidence),
            Some(
                &entity_relations
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::Array(v.clone())))
                    .collect(),
            ),
            Some(
                &doc_topics
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            Value::Array(v.iter().cloned().map(Value::String).collect()),
                        )
                    })
                    .collect(),
            ),
            Some(&mut topic_embeddings),
            Some(&mut topic_pool),
        )
        .await
    } else {
        wiki_mode_a_run(
            store,
            &deltas,
            &existing_pages,
            chat,
            embd,
            tenant_id,
            kb_id,
            incremental,
            callback,
            Some(&canonical_claims),
            Some(&doc_to_entities),
            Some(
                &doc_topics
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            Value::Array(v.iter().cloned().map(Value::String).collect()),
                        )
                    })
                    .collect(),
            ),
            Some(&mut topic_embeddings),
            Some(&mut topic_pool),
        )
        .await
    };

    report("FINALIZE: updating cross-references ...");
    wiki_finalize(
        store,
        tenant_id,
        kb_id,
        disabled_doc_ids,
        None,
        Some(current_chunk_state),
    )
    .await;

    summary
}

#[cfg(test)]
mod wiki_incremental_part16_tests {
    use super::*;
    use crate::doc_store::MemoryDocStore;
    use crate::harness::HarnessChat;
    use crate::harness::knowlege_wiki::{build_resume_doc, commit_active_map_state};
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

    fn seed_map(store: &MemoryDocStore, relation: bool) -> Map<String, Value> {
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let extract = json!({
            "entities": [{"name": "Alpha", "type": "entity", "chunk_ids": ["c1"]}],
            "concepts": [],
            "claims": [{"entity_name": "Alpha", "statement": "s1", "source_doc_id": "d1", "chunk_ids": ["c1"]}],
            "relations": if relation { json!([{"from": "Alpha", "to": "Alpha", "type": "self"}]) } else { json!([]) },
            "topics": ["t1"]
        });
        let row = build_resume_doc("c1", "d1", &extract, "h1");
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
        let mut state = Map::new();
        state.insert("c1".to_string(), json!({"doc_id": "d1", "hash": "h1"}));
        commit_active_map_state(store, "t1", "kb1", &state).unwrap();
        state
    }

    #[tokio::test]
    async fn main_entry_first_build_mode_a() {
        let store = MemoryDocStore::new();
        let state = seed_map(&store, false);
        let chat = FakeChat {
            reply: "TOPIC: alpha\n# Body\n\nGrounded text.".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let chunk_delta: Map<String, Value> = Map::new();
        let summary = wiki_compile_incremental(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            "entity",
            &chunk_delta,
            &Map::new(),
            &state,
            false,
            None,
            &BTreeSet::new(),
            None,
        )
        .await;
        assert_eq!(summary["pages_created"], json!(1));
        assert_eq!(summary["errors"], json!([]));
        let pages = search_existing_pages(&store, "t1", "kb1", &["slug_kwd".to_string()]);
        assert!(pages.contains_key("entity/alpha"));
        let canonical = load_canonical_entities(&store, "t1", "kb1");
        assert!(canonical.contains_key("Alpha"));
        assert_eq!(canonical["Alpha"]["mention_count_int"], json!(1));
    }

    #[tokio::test]
    async fn main_entry_skips_when_no_map() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "unused".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let summary = wiki_compile_incremental(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            "entity",
            &Map::new(),
            &Map::new(),
            &Map::new(),
            false,
            None,
            &BTreeSet::new(),
            None,
        )
        .await;
        assert_eq!(summary["pages_created"], json!(0));
        assert!(chat.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn main_entry_incremental_falls_back_to_full_build() {
        let store = MemoryDocStore::new();
        let state = seed_map(&store, false);
        let chat = FakeChat {
            reply: "TOPIC: alpha\n# Body\n\nGrounded.".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let summary = wiki_compile_incremental(
            &store,
            &chat,
            None,
            "t1",
            "kb1",
            "entity",
            &Map::new(),
            &Map::new(),
            &state,
            true,
            None,
            &BTreeSet::new(),
            None,
        )
        .await;
        assert_eq!(summary["pages_created"], json!(1));
        let pages = search_existing_pages(&store, "t1", "kb1", &["slug_kwd".to_string()]);
        assert!(pages.contains_key("entity/alpha"));
    }
}

// ---------------------------------------------------------------------------
// Part 15 — document deletion cleanup (`wiki_handle_document_deleted`).
//
// Adaptation note: sequential execution; the disabled-doc set is
// host-injected for FINALIZE; the final FINALIZE call carries no chunk state
// (it reloads the active MAP state internally).
// ---------------------------------------------------------------------------

fn scalar_string_or(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(Value::Array(items)) => items
            .first()
            .and_then(Value::as_str)
            .unwrap_or(fallback)
            .to_string(),
        Some(Value::String(text)) => text.clone(),
        _ => fallback.to_string(),
    }
}

/// `wiki_handle_document_deleted`: clean up pages + canonical entities.
#[allow(clippy::too_many_arguments)]
pub async fn wiki_handle_document_deleted(
    store: &dyn DocStore,
    tenant_id: &str,
    kb_id: &str,
    doc_id: &str,
    chat: &dyn HarnessChat,
    embd: Option<&dyn Embedder>,
    mode: &str,
    disabled_doc_ids: &BTreeSet<String>,
) -> Value {
    let mut pages_modified = 0usize;
    let mut pages_deleted = 0usize;
    let mut errors: Vec<Value> = Vec::new();

    let Some(dps) = wiki_load_doc_page_source(store, tenant_id, kb_id, doc_id) else {
        return json!({
            "pages_modified": 0,
            "pages_deleted": 0,
            "errors": [],
        });
    };

    // Step 1: canonical entity index — drop the document reference.
    let entity_names = as_str_list(dps.get("entity_names"));
    if !entity_names.is_empty() {
        let canonical_index = load_canonical_entities(store, tenant_id, kb_id);
        for ename in &entity_names {
            let Some(centry) = canonical_index.get(ename) else {
                continue;
            };
            let mut src_ids: Vec<String> = as_str_list(centry.get("source_doc_ids"));
            if let Some(pos) = src_ids.iter().position(|existing| existing == doc_id) {
                src_ids.remove(pos);
            }
            if src_ids.is_empty() {
                delete_canonical_entity(store, tenant_id, kb_id, ename);
            } else {
                let entity_type = centry
                    .get("entity_type_kwd")
                    .and_then(Value::as_str)
                    .unwrap_or("entity")
                    .to_string();
                let aliases = as_str_list(centry.get("aliases"));
                let default_count = src_ids.len() as i64;
                let mention_count = as_int(centry.get("mention_count_int"), default_count);
                let source_chunks = as_str_list(centry.get("source_chunk_ids"));
                save_canonical_entity(
                    store,
                    tenant_id,
                    kb_id,
                    ename,
                    &entity_type,
                    &aliases,
                    &src_ids,
                    mention_count,
                    None,
                    Some(&source_chunks),
                );
            }
        }
    }

    let affected_page_ids = as_str_list(dps.get("page_ids"));
    if affected_page_ids.is_empty() {
        return json!({
            "pages_modified": pages_modified,
            "pages_deleted": pages_deleted,
            "errors": errors,
        });
    }

    // Step 2: update wiki pages.
    let page_fields: Vec<String> = [
        "slug_kwd",
        "title_kwd",
        "md_with_weight",
        "claims",
        "source_doc_ids",
        "page_version_int",
        "entity_names_kwd",
        "page_type_kwd",
        "topic_kwd",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let all_existing_pages = search_existing_pages(store, tenant_id, kb_id, &page_fields);
    let all_page_ids: Vec<String> = all_existing_pages.keys().cloned().collect();

    for page_id in &affected_page_ids {
        let Some(existing) = all_existing_pages.get(page_id).cloned() else {
            continue;
        };
        let mut source_doc_ids: Vec<String> = as_str_list(existing.get("source_doc_ids"));
        if let Some(pos) = source_doc_ids
            .iter()
            .position(|existing| existing == doc_id)
        {
            source_doc_ids.remove(pos);
        }
        let default_type = if mode == "entity" {
            "concept"
        } else {
            "entity"
        };
        let page_type = scalar_string_or(existing.get("page_type_kwd"), default_type);
        let page_title = scalar_string_or(existing.get("title_kwd"), page_id);
        let page_version = as_int(existing.get("page_version_int"), 0);

        if source_doc_ids.is_empty() {
            let _ = wiki_refine_page(RefinePageArgs {
                store,
                mode: "delete",
                page_id,
                page_title: &page_title,
                existing_page: Some(&existing),
                page_type_kwd: &page_type,
                additions: None,
                retractions: None,
                source_chunks: Some(&[]),
                claims: Some(&[]),
                available_pages: Some(&[]),
                contextual_hints: "",
                chat,
                embd,
                tenant_id,
                kb_id,
                page_version,
                entity_names: None,
                page_embedding: None,
                embed_routing_context: false,
                source_doc_ids: None,
                topic_candidates: None,
                topic_selection_stats: None,
                topic_embeddings: None,
                topic_pool: None,
                member_evidence: None,
            })
            .await;
            pages_deleted += 1;
        } else {
            let existing_claims = wiki_parse_claims(existing.get("claims"));
            let retractions: Vec<Value> = existing_claims
                .iter()
                .filter(|claim| claim.get("source_doc_id").and_then(Value::as_str) == Some(doc_id))
                .cloned()
                .collect();
            let retained: Vec<Value> = existing_claims
                .iter()
                .filter(|claim| claim.get("source_doc_id").and_then(Value::as_str) != Some(doc_id))
                .cloned()
                .collect();
            let contextual_hints =
                wiki_build_contextual_hints(page_id, Some(&existing), &Map::new());
            let _ = wiki_refine_page(RefinePageArgs {
                store,
                mode: "modify",
                page_id,
                page_title: &page_title,
                existing_page: Some(&existing),
                page_type_kwd: &page_type,
                additions: Some(&[]),
                retractions: Some(&retractions),
                source_chunks: Some(&[]),
                claims: Some(&retained),
                available_pages: Some(&all_page_ids),
                contextual_hints: &contextual_hints,
                chat,
                embd,
                tenant_id,
                kb_id,
                page_version,
                entity_names: None,
                page_embedding: None,
                embed_routing_context: false,
                source_doc_ids: None,
                topic_candidates: None,
                topic_selection_stats: None,
                topic_embeddings: None,
                topic_pool: None,
                member_evidence: None,
            })
            .await;
            pages_modified += 1;
        }

        if mode == "topic" {
            let fields: Vec<String> = ["entity_names"]
                .iter()
                .map(|field| field.to_string())
                .collect();
            let condition = plan_group_condition(page_id);
            let rows = inc_search_page(store, tenant_id, kb_id, &fields, &condition, 0, 1)
                .unwrap_or_default();
            if let Some(row) = rows.into_iter().next() {
                let entity_names_parsed: Vec<String> = match row.get("entity_names") {
                    Some(Value::String(text)) if !text.is_empty() => {
                        match serde_json::from_str::<Value>(text) {
                            Ok(Value::Array(items)) => items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect(),
                            _ => Vec::new(),
                        }
                    }
                    Some(Value::Array(items)) => items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    _ => Vec::new(),
                };
                wiki_update_plan_group(
                    store,
                    tenant_id,
                    kb_id,
                    page_id,
                    &entity_names_parsed,
                    page_version,
                );
            }
        }
    }

    wiki_delete_doc_page_source(store, tenant_id, kb_id, doc_id);

    wiki_finalize(store, tenant_id, kb_id, disabled_doc_ids, None, None).await;

    json!({
        "pages_modified": pages_modified,
        "pages_deleted": pages_deleted,
        "errors": errors,
    })
}

#[cfg(test)]
mod wiki_incremental_part17_tests {
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

    fn seed_page(store: &MemoryDocStore, source_docs: &[&str]) {
        let index = crate::harness::knowlege_dataset_nav::index_name("t1");
        let claims = json!([
            {"statement": "s1", "source_doc_id": "d1", "chunk_ids": ["c1"]},
            {"statement": "s2", "source_doc_id": "d2", "chunk_ids": ["c2"]}
        ]);
        let row = json!({
            "id": "p1",
            "compile_kwd": "wiki_page",
            "slug_kwd": "entity/alpha",
            "title_kwd": "Alpha",
            "page_type_kwd": "entity",
            "md_with_weight": "OLD BODY",
            "entity_names_kwd": ["Alpha"],
            "source_doc_ids": source_docs,
            "source_chunk_ids": ["c1", "c2"],
            "page_version_int": 2,
            "synthesis_version_int": 1,
            "topic_kwd": "alpha",
            "claims": claims.to_string()
        });
        store
            .insert(&[row.as_object().cloned().unwrap()], &index, "kb1")
            .unwrap();
    }

    #[tokio::test]
    async fn no_source_record_returns_zero() {
        let store = MemoryDocStore::new();
        let chat = FakeChat {
            reply: "unused".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let summary = wiki_handle_document_deleted(
            &store,
            "t1",
            "kb1",
            "d1",
            &chat,
            None,
            "entity",
            &BTreeSet::new(),
        )
        .await;
        assert_eq!(summary["pages_modified"], json!(0));
        assert_eq!(summary["pages_deleted"], json!(0));
        assert!(chat.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deletes_sole_source_page() {
        let store = MemoryDocStore::new();
        seed_page(&store, &["d1"]);
        save_canonical_entity(
            &store,
            "t1",
            "kb1",
            "Alpha",
            "entity",
            &[],
            &["d1".to_string()],
            1,
            None,
            Some(&["c1".to_string()]),
        );
        wiki_update_doc_page_source(
            &store,
            "t1",
            "kb1",
            "d1",
            &["entity/alpha".to_string()],
            Some(&["Alpha".to_string()]),
            None,
            None,
        );
        let chat = FakeChat {
            reply: "unused".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let summary = wiki_handle_document_deleted(
            &store,
            "t1",
            "kb1",
            "d1",
            &chat,
            None,
            "entity",
            &BTreeSet::new(),
        )
        .await;
        assert_eq!(summary["pages_deleted"], json!(1));
        let pages = search_existing_pages(&store, "t1", "kb1", &["slug_kwd".to_string()]);
        assert!(pages.is_empty());
        assert!(load_canonical_entities(&store, "t1", "kb1").is_empty());
        assert!(wiki_load_doc_page_source(&store, "t1", "kb1", "d1").is_none());
    }

    #[tokio::test]
    async fn modifies_shared_page_and_plan_group() {
        let store = MemoryDocStore::new();
        seed_page(&store, &["d1", "d2"]);
        save_canonical_entity(
            &store,
            "t1",
            "kb1",
            "Alpha",
            "entity",
            &[],
            &["d1".to_string(), "d2".to_string()],
            2,
            None,
            Some(&["c1".to_string(), "c2".to_string()]),
        );
        wiki_update_doc_page_source(
            &store,
            "t1",
            "kb1",
            "d1",
            &["entity/alpha".to_string()],
            Some(&["Alpha".to_string()]),
            None,
            None,
        );
        wiki_update_plan_group(
            &store,
            "t1",
            "kb1",
            "entity/alpha",
            &["Alpha".to_string(), "Beta".to_string()],
            2,
        );
        let chat = FakeChat {
            reply: "TOPIC: alpha\n# Updated\n\nBody after delete.".to_string(),
            calls: Mutex::new(Vec::new()),
        };
        let summary = wiki_handle_document_deleted(
            &store,
            "t1",
            "kb1",
            "d1",
            &chat,
            None,
            "topic",
            &BTreeSet::new(),
        )
        .await;
        assert_eq!(summary["pages_modified"], json!(1));
        assert_eq!(summary["pages_deleted"], json!(0));
        let pages = search_existing_pages(
            &store,
            "t1",
            "kb1",
            &["slug_kwd".to_string(), "md_with_weight".to_string()],
        );
        let alpha = pages.get("entity/alpha").cloned().expect("alpha");
        assert!(
            alpha["md_with_weight"]
                .as_str()
                .unwrap()
                .contains("Body after delete")
        );
        let canonical = load_canonical_entities(&store, "t1", "kb1");
        assert_eq!(canonical["Alpha"]["source_doc_ids"], json!(["d2"]));
        assert_eq!(canonical["Alpha"]["mention_count_int"], json!(2));
        let groups = wiki_load_plan_group_members(&store, "t1", "kb1");
        assert_eq!(groups["entity/alpha"], json!(["Alpha", "Beta"]));
        assert!(wiki_load_doc_page_source(&store, "t1", "kb1", "d1").is_none());
    }
}
