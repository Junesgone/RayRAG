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
