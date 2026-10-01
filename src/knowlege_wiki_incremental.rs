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
            row.insert("id".to_string(), Value::String(row_id.clone()));
            results.insert(row_id, Value::Object(row));
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
    fn existing_pages_keyed_by_row_id() {
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
        assert_eq!(pages["p1"]["id"], json!("p1"));
        assert_eq!(pages["p2"]["slug_kwd"], json!("entity/b"));
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
