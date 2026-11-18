//! Agentic-RAG capability layer — RAGFlow v0.27.2
//! `rag/advanced_rag/agentic_rag.py`.
//!
//! `RagTools` bundles every retrieval primitive the agentic-search graph
//! (`agentic_rag_graph`) needs — question formalisation, document scoping,
//! keyword analysis, KB / web retrieval, a sufficiency judge and follow-up
//! generation — plus the two tools the outer LLM may bind (`rag` and
//! `summarize_document`).
//!
//! Port mapping: the Python class holds a live `LLMBundle` and DB services;
//! the Rust port keeps the DATA model here and receives IO backends explicitly
//! (the same injected-contract pattern the action-session port uses). The
//! `citation_prompt` wrapper renders the local `PromptLibrary` template when no
//! user override is configured.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;

use crate::common::misc_utils::hash_str2int;
use crate::harness::HarnessChat;
use crate::harness::action_session::{ActionLlmBackend, ActionSearchBackend, ActionToolBackend};
use crate::harness::chunk_utils;
use crate::harness::keywords::extract_weighted_keywords;
use crate::harness::orchestrator::direct::DirectTools;
use crate::harness::orchestrator::direct::Kbinfos;
use crate::harness::stats::StatsHandle;
use crate::harness::tools::search::{SearchSettings, resolve_rerank_candidates, resolve_top_k};
use crate::harness::tools::text_processing::compact_keywords;
use crate::harness::{form_message, message_fit_in};

/// Tokens held back from the model's context when fitting retrieved evidence
/// into the sufficiency / follow-up prompts.
pub const EVIDENCE_PROMPT_RESERVE_TOKENS: usize = 1024;

/// Fixed evidence budget for `fit_evidence` (sufficiency judge / follow-ups /
/// formalise-answer trimming). ~8000 tokens ≈ 32K chars.
pub const EVIDENCE_BUDGET_TOKENS: usize = 8000;

/// Significant-keyword overlap above which a new `rag` question reuses a
/// cached answer (0.6 + `>=2` shared words collapses the re-ask pattern while
/// leaving genuinely different questions untouched).
pub const RAG_CACHE_MIN_OVERLAP: f64 = 0.6;
pub const RAG_CACHE_MIN_SHARED: usize = 2;

/// Lightweight stopwords for the cross-`rag`-call dedup only.
pub const RAG_CACHE_STOPWORDS: [&str; 57] = [
    "the", "a", "an", "is", "was", "were", "what", "which", "when", "where", "who", "how", "of",
    "in", "to", "for", "and", "or", "but", "on", "at", "by", "be", "as", "it", "that", "this",
    "about", "with", "their", "its", "have", "has", "had", "been", "being", "from", "over",
    "under", "do", "does", "did", "not", "no", "yes", "can", "could", "should", "would", "also",
    "only", "very", "much", "more", "most", "some", "any",
];

fn is_stopword(token: &str) -> bool {
    RAG_CACHE_STOPWORDS.contains(&token)
}

/// `_question_keywords`: `(significant words, numeric tokens)` of a question.
/// For English plain tokenisation suffices; CJK text falls back to the
/// whole-token as a single significant unit. Numeric tokens are returned
/// separately so the cache can refuse to collapse questions that differ in the
/// number being asked about.
pub fn question_keywords(question: &str) -> (HashSet<String>, HashSet<String>) {
    let token_re = Regex::new(r"[a-zA-Z0-9\u{4e00}-\u{9fff}]+").expect("token regex");
    let lowered = question.to_lowercase();
    let tokens: Vec<String> = token_re
        .find_iter(&lowered)
        .map(|m| m.as_str().to_string())
        .collect();
    let numbers: HashSet<String> = tokens
        .iter()
        .filter(|token| token.chars().all(|c| c.is_ascii_digit()))
        .cloned()
        .collect();
    let mut significant: HashSet<String> = tokens
        .iter()
        .filter(|token| {
            !is_stopword(token)
                && token.chars().count() > 1
                && !token.chars().all(|c| c.is_ascii_digit())
        })
        .cloned()
        .collect();
    if significant.is_empty() {
        significant = tokens
            .iter()
            .filter(|token| token.chars().count() > 1 && !token.chars().all(|c| c.is_ascii_digit()))
            .cloned()
            .collect();
    }
    (significant, numbers)
}

/// `_cache_similar`: true when a new question's significant words mostly
/// overlap a cached one (`shared / min(cardinality) >= 0.6` and `>= 2` shared
/// words). The numeric sets must either be both empty or identical.
pub fn cache_similar(
    a: &(HashSet<String>, HashSet<String>),
    b: &(HashSet<String>, HashSet<String>),
) -> bool {
    let (aw, an) = a;
    let (bw, bn) = b;
    if aw.is_empty() || bw.is_empty() {
        return false;
    }
    if (!an.is_empty() || !bn.is_empty()) && an != bn {
        return false;
    }
    let shared = aw.intersection(bw).count();
    if shared < RAG_CACHE_MIN_SHARED {
        return false;
    }
    shared as f64 / aw.len().min(bw.len()) as f64 >= RAG_CACHE_MIN_OVERLAP
}

/// `_resolve_effective_question`: prefer the user's ORIGINAL, complete question
/// over the outer model's rewritten `question` argument, but only when the two
/// are clearly the SAME user turn (`>=2` shared significant keywords). The
/// outer rewrite frequently drops the FINAL target of a multi-hop question.
pub fn resolve_effective_question(question: &str, original_user_question: &str) -> String {
    if question.is_empty() || original_user_question.is_empty() {
        return question.to_string();
    }
    let original = original_user_question.trim();
    if original.is_empty() {
        return question.to_string();
    }
    let (question_keys, _) = question_keywords(question);
    let (original_keys, _) = question_keywords(original);
    if !question_keys.is_empty()
        && !original_keys.is_empty()
        && question_keys.intersection(&original_keys).count() >= 2
    {
        return original.to_string();
    }
    question.to_string()
}

/// `citation_prompt(user_defined_prompts)`: the citation rules the final answer
/// must follow. A user-configured `citation_guidelines` replaces the built-in
/// template; the illustrative-ID note is always appended.
pub fn get_citation_guidelines(user_defined_prompts: &HashMap<String, String>) -> String {
    let rendered = match user_defined_prompts.get("citation_guidelines") {
        Some(custom) => custom.clone(),
        None => {
            let empty: HashMap<&str, &str> = HashMap::new();
            crate::prompts::PromptLibrary::citation_prompt().render(&empty)
        }
    };
    format!(
        "{rendered}\n\nIMPORTANT: The example IDs above (45, 46, 78, etc.) are illustrative only. Use the actual chunk IDs from the provided knowledge blocks."
    )
}

/// One knowledge base as the grouping step sees it (upstream `Knowledgebase`).
/// `field_map` being `Some` mirrors `"field_map" in kb.parser_config` — the
/// KEY's presence routes the KB to the structured list, even when its map is
/// empty.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KbRef {
    pub id: String,
    /// Upstream `kb.tenant_id` (used by the structured retrieve).
    pub tenant_id: String,
    pub field_map: Option<serde_json::Map<String, Value>>,
}

/// Constructor mirror of upstream `RAGTools.__init__` keyword arguments.
#[derive(Debug, Clone, Default)]
pub struct RagToolsConfig {
    pub tenant_ids: Vec<String>,
    pub similarity_threshold: Option<f64>,
    pub vector_similarity_weight: Option<f64>,
    pub top_n: Option<usize>,
    pub rerank_candidates_count: Option<usize>,
    pub top_k: Option<usize>,
    pub original_user_question: String,
    pub thinking_mode: String,
    /// `kb_ids` path: resolved KB descriptors (upstream `KnowledgebaseService.
    /// get_by_ids`).
    pub kbs_by_ids: Vec<KbRef>,
    /// `kbs` path: descriptors handed in directly.
    pub kbs: Vec<KbRef>,
    /// Upstream stores the provider object; the port records its presence
    /// (the web backend itself is injected at the call site).
    pub web_search: Option<Value>,
    pub meta_data_filter: Option<Value>,
    pub doc_scope: Option<Vec<String>>,
    pub user_defined_prompts: HashMap<String, String>,
    pub empty_response: String,
    /// Upstream `do_refer: bool | None = True`; `None` is falsy at use sites.
    pub do_refer: Option<bool>,
    pub text_attachments_content: String,
    pub system_prompt: String,
    /// `chat_mdl.max_length` (the chat surface itself is injected per call).
    pub chat_max_length: usize,
}

/// `RAGTools`: the agentic-RAG capability data model (chunk 1: construction,
/// flags, scoping, message fitting).
#[derive(Debug, Clone)]
pub struct RagTools {
    pub tenant_ids: Vec<String>,
    pub similarity_threshold: Option<f64>,
    pub vector_similarity_weight: Option<f64>,
    pub top_n: Option<usize>,
    pub rerank_candidates_count: Option<usize>,
    pub top_k: Option<usize>,
    pub original_user_question: String,
    pub thinking_mode: String,
    pub field_map: serde_json::Map<String, Value>,
    /// Structured (SQL) KB descriptors (`parser_config.field_map` present).
    pub sql_kbs: Vec<KbRef>,
    /// Unstructured KB descriptors.
    pub kbs: Vec<KbRef>,
    pub kb_ids: Vec<String>,
    pub web_search: Option<Value>,
    pub meta_data_filter: Option<Value>,
    pub doc_scope: Option<Vec<String>>,
    pub user_defined_prompts: HashMap<String, String>,
    pub empty_response: String,
    pub do_refer: Option<bool>,
    pub text_attachments_content: String,
    pub system_prompt: String,
    pub chat_max_length: usize,
    /// Citation pool shared with the final-answer node (chunks + doc_aggs).
    pub kbinfos: Kbinfos,
    /// Cross-`rag`-call cache: question -> (answer, significant/numeric keys).
    pub rag_cache: HashMap<String, (String, (HashSet<String>, HashSet<String>))>,
    /// Sufficiency verdict of the most recent agentic-graph run.
    pub rag_verdict: Option<Value>,
    /// Consecutive rag calls that ended UNANSWERABLE / INSUFFICIENT.
    pub consecutive_unanswerable: usize,
    /// Per-request search cache keyed by effective query + scope.
    pub search_cache: HashMap<String, Value>,
    /// Memoised flattened doc metadata (`_metas_cache`).
    pub metas_cache: Option<Value>,
}

impl RagTools {
    /// `RAGTools.__init__`: group the KBs, dedupe the doc scope, default the
    /// prompts and initialise the caches.
    pub fn new(config: RagToolsConfig) -> Self {
        let mut tools = Self {
            tenant_ids: config.tenant_ids,
            similarity_threshold: config.similarity_threshold,
            vector_similarity_weight: config.vector_similarity_weight,
            top_n: config.top_n,
            rerank_candidates_count: config.rerank_candidates_count,
            top_k: config.top_k,
            original_user_question: config.original_user_question,
            thinking_mode: config.thinking_mode,
            field_map: serde_json::Map::new(),
            sql_kbs: Vec::new(),
            kbs: Vec::new(),
            kb_ids: Vec::new(),
            web_search: config.web_search,
            meta_data_filter: config.meta_data_filter,
            doc_scope: config
                .doc_scope
                .map(|scope| dedupe_preserving_order(&scope)),
            user_defined_prompts: config.user_defined_prompts,
            empty_response: config.empty_response,
            do_refer: config.do_refer,
            text_attachments_content: config.text_attachments_content,
            system_prompt: config.system_prompt,
            chat_max_length: config.chat_max_length,
            kbinfos: Kbinfos::default(),
            rag_cache: HashMap::new(),
            rag_verdict: None,
            consecutive_unanswerable: 0,
            search_cache: HashMap::new(),
            metas_cache: None,
        };
        // `kb_ids` wins over `kbs` (upstream if/elif).
        if !tools.kb_ids.is_empty() || !config.kbs_by_ids.is_empty() {
            // nothing: kb_ids path uses kbs_by_ids below
        }
        let source: Vec<KbRef> = if !config.kbs_by_ids.is_empty() {
            config.kbs_by_ids
        } else {
            config.kbs
        };
        for kb in &source {
            if let Some(map) = &kb.field_map {
                tools.field_map.extend(map.clone());
                tools.sql_kbs.push(kb.clone());
            } else {
                tools.kbs.push(kb.clone());
                tools.kb_ids.push(kb.id.clone());
            }
        }
        tools
    }

    /// `has_unstructured`.
    pub fn has_unstructured(&self) -> bool {
        !self.kb_ids.is_empty()
    }

    /// `has_structured`.
    pub fn has_structured(&self) -> bool {
        !self.sql_kbs.is_empty() && !self.field_map.is_empty()
    }

    /// `has_web`.
    pub fn has_web(&self) -> bool {
        self.web_search.is_some()
    }

    /// `has_llm` (the chat surface is injected per call; presence is recorded
    /// by the caller through `chat_max_length` > 0).
    pub fn has_llm(&self) -> bool {
        self.chat_max_length > 0
    }

    /// `scoped_doc_ids`: intersect the caller's document scope with the bound
    /// one (the bound scope is the hard constraint when set).
    pub fn scoped_doc_ids(&self, doc_scope: Option<&[String]>) -> Option<Vec<String>> {
        match (&self.doc_scope, doc_scope) {
            (None, scope) => scope.map(|ids| ids.to_vec()),
            (Some(_), None) => Some(self.doc_scope.clone().unwrap_or_default()),
            (Some(own), Some(ids)) if ids.is_empty() => Some(own.clone()),
            (Some(own), Some(ids)) => Some(
                ids.iter()
                    .filter(|doc_id| own.contains(doc_id))
                    .cloned()
                    .collect(),
            ),
        }
    }

    /// `_fit_messages`: fit system+user messages into the model's context.
    pub fn fit_messages(&self, system: &str, user: &str) -> Vec<Value> {
        let (_, messages) = message_fit_in(form_message(system, user), self.chat_max_length);
        messages
    }

    /// `get_citation_guidelines`.
    pub fn get_citation_guidelines(&self) -> String {
        get_citation_guidelines(&self.user_defined_prompts)
    }

    /// `sys_prompt`: the thin router prompt for callers that bind `tools`.
    pub fn sys_prompt(&self) -> String {
        let summarize_line = if self.has_unstructured() {
            "- Call `summarize_document` ONLY when the user explicitly asks to summarise a specific document ('summarise the security audit', 'tldr the onboarding guide'). It needs a document ID.\n"
        } else {
            ""
        };
        let router_prompt = format!(
            "You are a smart agent. For any question that needs evidence from the knowledge bases or the web, call the `rag` tool with a self-contained question — it runs the full search-and-answer pipeline and returns a cited answer.\nAfter the `rag` tool returns, do not call `rag` again for the same user question. Use the returned cited answer as the final answer unless the user explicitly asks a new question.\nCRITICAL — preserve the full multi-hop structure when phrasing the `rag` question. A question that compares two or more DISTINCT targets or needs an arithmetic result across them (\"how much taller is X than Y\", \"how many days after A's death did B die\", \"which of these was discovered last\") MUST keep every target and relation in the question you pass to `rag`. Never rewrite a comparison into a single-entity question — dropping the second entity (e.g. the purchaser in \"how many days after his death did the man who purchased it in 1933 die\") makes the pipeline answer only the first part. Pass the complete comparison.\n{summarize_line}Do not invent facts and do not fabricate document IDs."
        );
        if self.system_prompt.is_empty() {
            router_prompt
        } else {
            format!("{}\n\n{router_prompt}", self.system_prompt)
        }
    }

    /// `extract_keywords`: the compact keyword string (deduped union of the four
    /// weighted aspects), falling back to the question when extraction fails.
    pub async fn extract_keywords(&self, chat: &dyn HarnessChat, question: &str) -> String {
        if question.is_empty() {
            return String::new();
        }
        let (_, union) = extract_weighted_keywords(chat, question).await;
        compact_keywords(&union, 15)
    }

    /// `_extract_keywords_weighted`: the four weighted keyword aspects as
    /// `(weighted_query, plain_union)`.
    pub async fn extract_keywords_weighted(
        &self,
        chat: &dyn HarnessChat,
        question: &str,
    ) -> (String, String) {
        extract_weighted_keywords(chat, question).await
    }

    /// `formalize`: rewrite the latest user message into a standalone question
    /// AND derive its search keywords (each with close synonyms) in one LLM call
    /// (the single-turn path keeps the question verbatim and extracts keywords).
    pub async fn formalize(&self, chat: &dyn HarnessChat, messages: &[Value]) -> (String, String) {
        if messages.is_empty() {
            return (String::new(), String::new());
        }
        let mut lines: Vec<String> = Vec::new();
        let mut last_user = String::new();
        for message in messages {
            if let Some(text) = message.as_str() {
                lines.push(text.to_string());
                last_user = text.to_string();
                continue;
            }
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user");
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if role == "user" {
                last_user = content.clone();
            }
            let prefix = match role {
                "user" => "User".to_string(),
                "assistant" => "Assistant".to_string(),
                other => capitalize(other),
            };
            lines.push(format!("{prefix}: {content}"));
        }
        let transcript = lines.join("\n");
        let user_turns = messages
            .iter()
            .filter(|message| {
                message.as_str().is_some()
                    || message
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("user")
                        == "user"
            })
            .count();
        let multi_turn = user_turns > 1;
        if !multi_turn && !last_user.is_empty() {
            // Single-turn self-contained question — kept verbatim (no rewrite).
            let keywords = self.extract_keywords(chat, &last_user).await;
            return (last_user.trim().to_string(), keywords);
        }
        let system = "You are given a conversation. Do BOTH of the following and return JSON only:\n1. Rewrite the LAST user message into a single, self-contained question that can be understood without the prior conversation — resolve pronouns, ellipses and follow-up shortcuts using the earlier turns. In most cases, it should be EXACTLY THE SAME as the last user query — only rewrite when there is something to resolve (a pronoun/ellipsis pointing back at an earlier turn). Preserve the original language.\n2. Extract keywords for a keyword search: the salient content words and phrases that literally appear in the (standalone) question — key nouns, named entities, domain terms — PLUS 2-3 close synonyms/abbreviations/aliases/alternative spellings of each, in the SAME language as the question. Maximize recall. Do NOT include terms that would be part of the answer.\n   Example — \"In which year did Apple acquire Beats?\" -> keywords = \"Apple, Apple Inc., AAPL, acquire, acquisition, acquired, Beats, Beats Electronics\".\n\nOutput ONLY JSON, no prose, no code fences: {\"question\": \"<standalone question>\", \"keywords\": \"<term1, term2, synonym1, ...>\"}";
        let user = format!("Conversation:\n{transcript}\n\nOutput JSON:");
        let (_, fitted) = message_fit_in(form_message(system, &user), self.chat_max_length);
        let system_content = fitted
            .first()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or(system)
            .to_string();
        let history: Vec<Value> = fitted.iter().skip(1).cloned().collect();
        let ans = chat
            .chat(
                &system_content,
                &history,
                &serde_json::json!({"temperature": 0.1}),
            )
            .await
            .unwrap_or_default();
        let cleaned = strip_think_prefix(&ans);
        let cleaned = strip_code_fences(&cleaned).trim().to_string();
        let data = serde_json::from_str::<Value>(&cleaned)
            .ok()
            .or_else(|| crate::structure_compile::repair_json_text(&cleaned))
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        let mut question = data
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_string();
        if question.is_empty() {
            question = last_user.trim().to_string();
        }
        let keywords = match data.get("keywords") {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| match item {
                    Value::String(text) => text.trim().to_string(),
                    other => other.to_string(),
                })
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join(", "),
            Some(value) => value
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| value.to_string()),
            None => String::new(),
        };
        (question, compact_keywords(keywords.trim(), 15))
    }

    /// `retrieve`: raw chunks from the unstructured KBs for one question.
    /// Returns `{"chunks", "doc_aggs"}` — no citation stamping, no pool
    /// accumulation (the graph owns merging so parallel retrieval is race-free).
    #[allow(clippy::too_many_arguments)]
    pub async fn retrieve(
        &self,
        backend: &dyn RagRetrievalBackend,
        question: &str,
        keywords: Option<&Value>,
        doc_scope: Option<&[String]>,
        top_n: Option<usize>,
        similarity_threshold: Option<f64>,
        using_embedding: bool,
    ) -> Value {
        if self.kb_ids.is_empty() {
            return empty_kbinfos();
        }
        let keyword_text = match keywords {
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join(","),
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        };
        // Explicit argument wins, then the caller's configuration, then this
        // method's own defaults (they differ from the search tools' on purpose).
        let top_n = top_n.unwrap_or(self.top_n.unwrap_or(6));
        let similarity_threshold =
            similarity_threshold.unwrap_or(self.similarity_threshold.unwrap_or(0.2));
        let mut doc_scope = self.scoped_doc_ids(doc_scope);
        if matches!(&doc_scope, Some(scope) if scope.len() == 1 && scope[0] == "-999") {
            return empty_kbinfos();
        }
        if let Some(scope) = &doc_scope {
            let candidates: Vec<String> = scope.clone();
            let known = backend.filter_known_doc_ids(&candidates);
            let valid: Vec<String> = candidates
                .iter()
                .filter(|doc_id| known.contains(*doc_id))
                .cloned()
                .collect();
            if !valid.is_empty() {
                doc_scope = Some(valid);
            } else {
                if self.doc_scope.is_some() {
                    return empty_kbinfos();
                }
                doc_scope = None;
            }
        }
        let mut search_terms = keyword_text.trim().to_string();
        let mut question_text = question.to_string();
        if search_terms.is_empty() {
            search_terms = question_text.clone();
        } else {
            question_text = format!("{question_text} {search_terms}");
        }
        let settings = SearchSettings {
            top_n: self.top_n,
            top_k: self.top_k,
            rerank_candidates_count: self.rerank_candidates_count,
            vector_similarity_weight: self.vector_similarity_weight,
            similarity_threshold: self.similarity_threshold,
        };
        let vector_weight = if using_embedding {
            self.vector_similarity_weight.unwrap_or(0.7)
        } else {
            0.0
        };
        let knn_top_k = resolve_top_k(&settings);
        let rerank_candidates_count = resolve_rerank_candidates(&settings, top_n);
        let request = RagRetrievalRequest {
            question: question_text.clone(),
            tenant_ids: self.tenant_ids.clone(),
            kb_ids: self.kb_ids.clone(),
            top_n,
            similarity_threshold,
            vector_similarity_weight: vector_weight,
            knn_top_k,
            rerank_candidates_count,
            doc_ids: doc_scope,
            rank_feature: backend.rank_feature(&question_text),
            using_embedding,
        };
        let Some(kbinfos) = backend.retrieval(&request).await else {
            return empty_kbinfos();
        };
        if !value_truthy(&kbinfos) {
            return empty_kbinfos();
        }
        let chunks_in = kbinfos
            .get("chunks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let chunks = backend.retrieval_by_children(&chunks_in, &self.tenant_ids);
        let doc_aggs = kbinfos
            .get("doc_aggs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        serde_json::json!({"chunks": chunks, "doc_aggs": doc_aggs})
    }

    /// `web_retrieve`: raw kbinfos shape from the public web.
    pub async fn web_retrieve(&self, backend: &dyn RagRetrievalBackend, query: &str) -> Value {
        if self.web_search.is_none() {
            return empty_kbinfos();
        }
        let Some(web_res) = backend.web_retrieve_chunks(query).await else {
            return empty_kbinfos();
        };
        let chunks = web_res
            .get("chunks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let doc_aggs = web_res
            .get("doc_aggs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        serde_json::json!({"chunks": chunks, "doc_aggs": doc_aggs})
    }

    /// `structured_retrieve`: query the tabular KBs by translating to SQL.
    /// Returns `{"answer", "chunks", "doc_aggs"}`.
    pub async fn structured_retrieve(
        &self,
        chat: &dyn HarnessChat,
        backend: &dyn RagRetrievalBackend,
        question: &str,
    ) -> Value {
        let empty = serde_json::json!({"answer": "", "chunks": [], "doc_aggs": []});
        if !self.has_structured() {
            return empty;
        }
        let sql_kb_ids: Vec<String> = self.sql_kbs.iter().map(|kb| kb.id.clone()).collect();
        let tenant_id = self
            .sql_kbs
            .first()
            .map(|kb| kb.tenant_id.clone())
            .unwrap_or_default();
        let doc_ids = self.scoped_doc_ids(None);
        let Some(ans) = backend
            .use_sql(
                question,
                &self.field_map,
                &tenant_id,
                chat,
                &sql_kb_ids,
                doc_ids,
            )
            .await
        else {
            return empty;
        };
        if !value_truthy(&ans) {
            return empty;
        }
        let reference = ans
            .get("reference")
            .cloned()
            .filter(Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        let answer = ans
            .get("answer")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let chunks = reference
            .get("chunks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let doc_aggs = reference
            .get("doc_aggs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        serde_json::json!({"answer": answer, "chunks": chunks, "doc_aggs": doc_aggs})
    }

    /// `_fit_evidence`: trim the evidence so question + evidence + the prompt
    /// template stay inside the FIXED evidence budget.
    pub fn fit_evidence(&self, question: &str, evidence_md: &str) -> String {
        if evidence_md.is_empty() {
            return evidence_md.to_string();
        }
        let (_, fitted) =
            message_fit_in(form_message(question, evidence_md), EVIDENCE_BUDGET_TOKENS);
        fitted
            .last()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or(evidence_md)
            .to_string()
    }

    /// `judge_sufficiency`: judge whether the evidence answers the question and
    /// pick the useful chunks (returns `{}` on failure).
    pub async fn judge_sufficiency(
        &self,
        chat: &dyn HarnessChat,
        question: &str,
        evidence_md: &str,
    ) -> Value {
        let evidence = self.fit_evidence(question, evidence_md);
        sufficiency_select(chat, question, &evidence).await
    }

    /// `gen_followups`: complementary (question, query) pairs for the gaps.
    pub async fn gen_followups(
        &self,
        chat: &dyn HarnessChat,
        question: &str,
        query: &str,
        missing: &[String],
        evidence_md: &str,
    ) -> Vec<Value> {
        let evidence = self.fit_evidence(question, evidence_md);
        let res = multi_queries_gen(
            chat,
            question,
            if query.is_empty() { question } else { query },
            missing,
            &evidence,
        )
        .await;
        res.get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|candidate| {
                candidate.is_object()
                    && candidate
                        .get("question")
                        .and_then(Value::as_str)
                        .map(|text| !text.trim().is_empty())
                        .unwrap_or(false)
            })
            .collect()
    }
    /// `_get_cached_metas`: memoised flattened doc metadata for the bound KBs.
    pub async fn get_cached_metas(&mut self, backend: &dyn RagRetrievalBackend) -> Value {
        if let Some(cached) = &self.metas_cache {
            return cached.clone();
        }
        if self.kb_ids.is_empty() {
            self.metas_cache = Some(serde_json::json!({}));
            return serde_json::json!({});
        }
        let metas = backend.flattened_meta_by_kbs(&self.kb_ids);
        let value = if value_truthy(&metas) {
            metas
        } else {
            serde_json::json!({})
        };
        self.metas_cache = Some(value.clone());
        value
    }

    /// `_collect_doc_titles`: `(doc_id, title)` pairs, or `None` once the
    /// `max_docs` cap is exceeded (upstream quirk).
    pub fn collect_doc_titles(
        &self,
        backend: &dyn RagRetrievalBackend,
        max_docs: usize,
    ) -> Option<Vec<(String, String)>> {
        let mut result: Vec<(String, String)> = Vec::new();
        for kb_id in &self.kb_ids {
            for (id, name) in backend.doc_titles_for_kb(kb_id) {
                result.push((id, name));
                if result.len() >= max_docs {
                    return None;
                }
            }
        }
        Some(result)
    }

    /// `_filter_known_doc_ids`.
    pub fn filter_known_doc_ids(
        &self,
        backend: &dyn RagRetrievalBackend,
        candidates: &[String],
    ) -> HashSet<String> {
        if candidates.is_empty() || self.kb_ids.is_empty() {
            return HashSet::new();
        }
        backend.filter_known_doc_ids(candidates)
    }

    /// `_resolve_doc_tenant`: `(kb_id, tenant_id)` for the document when it
    /// sits in one of the bound (unstructured) KBs.
    pub fn doc_tenant(
        &self,
        backend: &dyn RagRetrievalBackend,
        doc_id: &str,
    ) -> Option<(String, String)> {
        let kb_id = backend.doc_kb_id(doc_id)?;
        for kb in &self.kbs {
            if kb.id == kb_id {
                return Some((kb_id.clone(), kb.tenant_id.clone()));
            }
        }
        None
    }

    /// `fetch_full_document`: a whole document's chunks in reading order (raw
    /// kbinfos), capped by the model context budget. The paging loop breaks
    /// OUTER once the token budget is hit (the bare inner break used to run all
    /// ~79 pages).
    pub async fn fetch_full_document(
        &self,
        backend: &dyn RagRetrievalBackend,
        doc_id: &str,
    ) -> Value {
        let empty = empty_kbinfos();
        if self.kb_ids.is_empty() {
            return empty;
        }
        if let Some(scope) = &self.doc_scope
            && !scope.contains(&doc_id.to_string())
        {
            return empty;
        }
        let Some((kb_id, tenant_id)) = self.doc_tenant(backend, doc_id) else {
            return empty;
        };
        let mut cks: Vec<Value> = Vec::new();
        let mut tokens: usize = 0;
        let mut offset: usize = 0;
        while offset < 10000 {
            let page = backend
                .chunk_list(doc_id, &tenant_id, &kb_id, offset + 128, offset)
                .await;
            if page.is_empty() {
                break;
            }
            let mut budget_hit = false;
            for ck in &page {
                let num =
                    num_tokens_from_string(&chunk_utils::chunk_attr(ck, &["content_with_weight"]));
                if tokens + num > self.chat_max_length {
                    budget_hit = true;
                    break;
                }
                tokens += num;
                cks.push(ck.clone());
            }
            if budget_hit {
                break;
            }
            offset += 128;
        }
        if cks.is_empty() {
            return empty;
        }
        let doc_name = cks
            .iter()
            .find_map(|ck| {
                let name = chunk_utils::chunk_attr(ck, &["docnm_kwd"]);
                if name.is_empty() { None } else { Some(name) }
            })
            .unwrap_or_default();
        let count = cks.len();
        serde_json::json!({
            "chunks": cks,
            "doc_aggs": [{"doc_name": doc_name, "doc_id": doc_id, "count": count}],
        })
    }

    /// `summarize_document` (the bound tool): the document's chunk blocks in
    /// reading order, prefixed with the citation rules unless `do_refer` is off.
    pub async fn summarize_document(
        &mut self,
        backend: &dyn RagRetrievalBackend,
        doc_id: &str,
    ) -> Vec<String> {
        let kbinfos = self.fetch_full_document(backend, doc_id).await;
        let chunks = kbinfos
            .get("chunks")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if chunks.is_empty() {
            return Vec::new();
        }
        let start_idx = self.kbinfos.chunks.len();
        self.kbinfos.chunks.extend(chunks);
        if let Some(doc_aggs) = kbinfos.get("doc_aggs").and_then(Value::as_array) {
            self.kbinfos.doc_aggs.extend(doc_aggs.iter().cloned());
        }
        let blocks = kb_prompt(&self.kbinfos, self.chat_max_length, false);
        let tail: Vec<String> = if start_idx > 0 && start_idx <= blocks.len() {
            blocks[start_idx..].to_vec()
        } else {
            blocks
        };
        if self.do_refer != Some(true) {
            return tail;
        }
        let header = format!(
            "# Citation rules\nApply the following rules VERBATIM to your final answer.\n\n{}\n\n----\n\n",
            self.get_citation_guidelines().trim()
        );
        let mut out = vec![header];
        out.extend(tail);
        out
    }
}

/// One `retrieve` request against the host search service (upstream
/// `settings.retriever.retrieval(...)`; the port fixes `page=1`, `aggs=True`
/// and `highlight=True` exactly as the source does).
#[derive(Debug, Clone)]
pub struct RagRetrievalRequest {
    pub question: String,
    pub tenant_ids: Vec<String>,
    pub kb_ids: Vec<String>,
    pub top_n: usize,
    pub similarity_threshold: f64,
    pub vector_similarity_weight: f64,
    pub knn_top_k: usize,
    pub rerank_candidates_count: usize,
    pub doc_ids: Option<Vec<String>>,
    pub rank_feature: Value,
    pub using_embedding: bool,
}

/// The retrieval / KB services `RAGTools` calls into (RAGFlow's
/// `settings.retriever` + the peewee services + the web provider + `use_sql`).
#[async_trait]
pub trait RagRetrievalBackend: Send + Sync {
    /// `settings.retriever.retrieval(...)`; `None` stands for an empty result.
    async fn retrieval(&self, request: &RagRetrievalRequest) -> Option<Value>;
    /// `settings.retriever.retrieval_by_children(chunks, tenant_ids)`.
    fn retrieval_by_children(&self, chunks: &[Value], tenant_ids: &[String]) -> Vec<Value>;
    /// `label_question(question, kbs)` — the rank feature.
    fn rank_feature(&self, question: &str) -> Value;
    /// `self._filter_known_doc_ids(candidates)` (DB lookup).
    fn filter_known_doc_ids(&self, candidates: &[String]) -> HashSet<String>;
    /// `thread_pool_exec(self.web_search.retrieve_chunks, query)`.
    async fn web_retrieve_chunks(&self, query: &str) -> Option<Value>;
    /// `use_sql(question, field_map, tenant_id, chat_mdl, quota=True, kb_ids=…, doc_ids=…)`.
    async fn use_sql(
        &self,
        question: &str,
        field_map: &serde_json::Map<String, Value>,
        tenant_id: &str,
        chat: &dyn HarnessChat,
        kb_ids: &[String],
        doc_ids: Option<Vec<String>>,
    ) -> Option<Value>;
    /// `DocMetadataService.get_flatted_meta_by_kbs(kb_ids)`.
    fn flattened_meta_by_kbs(&self, kb_ids: &[String]) -> Value;
    /// `DocumentService.query(kb_id=kb_id)` -> `(doc_id, doc_name)` pairs.
    fn doc_titles_for_kb(&self, kb_id: &str) -> Vec<(String, String)>;
    /// `Document.select(kb_id).where(id == doc_id & kb_id in kb_ids)`.
    fn doc_kb_id(&self, doc_id: &str) -> Option<String>;
    /// `settings.retriever.chunk_list(doc_id, tenant_id, [kb_id],
    /// max_count=…, offset=…, fields=[…], sort_by_position=True, retrieve_all=False)`.
    async fn chunk_list(
        &self,
        doc_id: &str,
        tenant_id: &str,
        kb_id: &str,
        max_count: usize,
        offset: usize,
    ) -> Vec<Value>;
}

/// `num_tokens_from_string` (upstream `common/token_utils.py`, tiktoken BPE);
/// the crate-level estimator keeps the same "tokens in the text" contract.
pub fn num_tokens_from_string(text: &str) -> usize {
    crate::chunk::tokenizer::token_count(text)
}

/// `draw_node`: `"\n├── {k}: {line}"` with newline runs collapsed; an empty
/// line renders nothing.
fn draw_node(key: &str, line: &str) -> String {
    if line.is_empty() {
        return String::new();
    }
    let collapsed = Regex::new(r"\n+")
        .expect("newline regex")
        .replace_all(line, " ");
    format!("\n├── {key}: {collapsed}")
}

/// `kb_prompt(kbinfos, max_tokens, hash_id=False)`: format the retrieved chunks
/// as `ID: n` evidence blocks, honouring the token budget (upstream
/// `prompts/generator.py`).
pub fn kb_prompt(kbinfos: &Kbinfos, max_tokens: usize, hash_id: bool) -> Vec<String> {
    let chunks: &[Value] = &kbinfos.chunks;
    let knowledges: Vec<String> = chunks
        .iter()
        .map(|ck| chunk_utils::chunk_attr(ck, &["content", "content_with_weight"]))
        .collect();
    let mut used_token_count: usize = 0;
    let mut selected: Vec<&Value> = Vec::new();
    for (ck, content) in chunks.iter().zip(knowledges.iter()) {
        if content.is_empty() {
            continue;
        }
        let chunk_tokens = num_tokens_from_string(content);
        if (max_tokens as f64) * 0.97 < (used_token_count + chunk_tokens) as f64 {
            break;
        }
        used_token_count += chunk_tokens;
        selected.push(ck);
    }
    let mut out: Vec<String> = Vec::new();
    for (index, ck) in selected.iter().enumerate() {
        let mut block = if hash_id {
            let id = chunk_utils::chunk_attr(ck, &["id", "chunk_id"]);
            format!("\nID: {}", hash_str2int(&id, 500))
        } else {
            format!("\nID: {index}")
        };
        block.push_str(&draw_node(
            "Title",
            &chunk_utils::chunk_attr(ck, &["docnm_kwd", "document_name"]),
        ));
        block.push_str(&draw_node(
            "URL",
            ck.get("url").and_then(Value::as_str).unwrap_or(""),
        ));
        if let Some(meta) = ck.get("document_metadata").and_then(Value::as_object) {
            for (key, value) in meta {
                let line = value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string());
                block.push_str(&draw_node(key, &line));
            }
        }
        block.push_str("\n└── Content:\n");
        block.push_str(&chunk_utils::chunk_attr(
            ck,
            &["content", "content_with_weight"],
        ));
        out.push(block);
    }
    out
}

fn empty_kbinfos() -> Value {
    serde_json::json!({"chunks": [], "doc_aggs": []})
}

fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `gen_json`: run the chat model and parse ONE JSON value out of its reply
/// (think-block + fence stripping, one corrective retry — upstream
/// `prompts.generator.gen_json`). Returns `{}` when both attempts fail.
async fn gen_json(chat: &dyn HarnessChat, system: &str, user: &str) -> Value {
    let (_, mut fitted) = message_fit_in(form_message(system, user), chat.max_length());
    let mut ans = String::new();
    let mut err = String::new();
    for _ in 0..2 {
        if !ans.is_empty()
            && !err.is_empty()
            && let Some(last) = fitted.last_mut()
        {
            let content = last
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            last["content"] = serde_json::json!(format!(
                "{content}\nGenerated JSON is as following:\n{ans}\nBut exception while loading:\n{err}\nPlease reconsider and correct it."
            ));
        }
        let system_content = fitted
            .first()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or(system)
            .to_string();
        let history: Vec<Value> = fitted.iter().skip(1).cloned().collect();
        ans = chat
            .chat(&system_content, &history, &serde_json::json!({}))
            .await
            .unwrap_or_default();
        let cleaned = strip_gen_json_marks(&ans);
        match serde_json::from_str::<Value>(&cleaned)
            .ok()
            .or_else(|| crate::structure_compile::repair_json_text(&cleaned))
        {
            Some(value) => return value,
            None => {
                err = "parse failed".to_string();
            }
        }
    }
    serde_json::json!({})
}

/// `re.sub(r"(^.*</think>|```json\n|```\n*$)", "", ans, flags=re.DOTALL)`.
fn strip_gen_json_marks(text: &str) -> String {
    let re = Regex::new(r"(?s)^.*</think>|```json\n|```\n*$").expect("gen_json regex");
    re.replace_all(text, "").to_string()
}

/// `sufficiency_select`: render the template and parse the verdict JSON.
async fn sufficiency_select(chat: &dyn HarnessChat, question: &str, ret_content: &str) -> Value {
    let mut vars: HashMap<&str, &str> = HashMap::new();
    vars.insert("question", question);
    vars.insert("retrieved_docs", ret_content);
    let system = crate::prompts::PromptLibrary::sufficiency_check().render(&vars);
    let value = gen_json(chat, &system, "Output:\n").await;
    if value.is_object() {
        value
    } else {
        serde_json::json!({})
    }
}

/// `multi_queries_gen`: render the template and parse the follow-up JSON.
async fn multi_queries_gen(
    chat: &dyn HarnessChat,
    question: &str,
    query: &str,
    missing: &[String],
    ret_content: &str,
) -> Value {
    let missing_info = missing.join("\n - ");
    let mut vars: HashMap<&str, &str> = HashMap::new();
    vars.insert("original_question", question);
    vars.insert("original_query", query);
    vars.insert("missing_info", &missing_info);
    vars.insert("retrieved_docs", ret_content);
    let system = crate::prompts::PromptLibrary::multi_queries_gen().render(&vars);
    let value = gen_json(chat, &system, "Output:\n").await;
    if value.is_object() {
        value
    } else {
        serde_json::json!({})
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// `re.sub(r"^.*</think>", "", text, flags=re.DOTALL)`: drop a leading think
/// block (through the LAST closing tag).
fn strip_think_prefix(text: &str) -> String {
    let re = Regex::new(r"(?s)^.*</think>").expect("think regex");
    re.replace(text, "").to_string()
}

/// `re.sub(r"```(?:json)?\s*|\s*```", "", text)`: drop code fences.
fn strip_code_fences(text: &str) -> String {
    let re = Regex::new(r"```(?:json)?\s*|\s*```").expect("fence regex");
    re.replace_all(text, "").to_string()
}

fn json_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn dedupe_preserving_order(items: &[String]) -> Vec<String> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if seen.insert(item.as_str()) {
            out.push(item.clone());
        }
    }
    out
}

/// Backend bundle for the `rag` tool (the graph runtime without the `tools`
/// reference, which `rag` supplies from `&mut self` via a snapshot).
pub struct AgenticBackends<'a> {
    pub search: &'a crate::harness::tools::search::SearchContext<'a>,
    pub retrieval: &'a dyn RagRetrievalBackend,
    pub chat: &'a dyn HarnessChat,
    pub action_search: &'a dyn ActionSearchBackend,
    pub action_tools: &'a dyn ActionToolBackend,
    pub action_llm: &'a dyn ActionLlmBackend,
    pub direct: Option<&'a dyn DirectTools>,
    pub stats: StatsHandle,
}

impl RagTools {
    /// `pick_documents`: narrowing is disabled upstream (the method returns
    /// `None` before any lookup), so every caller searches everything. The
    /// helpers below it (`_select_by_titles`; the metadata pushdown path) stay
    /// unreachable there; `select_by_titles` is ported for source parity.
    pub async fn pick_documents(&self) -> Option<Vec<String>> {
        None
    }

    /// `_select_by_titles`: ask the model to filter a document catalogue.
    pub async fn select_by_titles(
        &self,
        chat: &dyn HarnessChat,
        question: &str,
        docs: &[(String, String)],
    ) -> Vec<String> {
        if docs.is_empty() {
            return Vec::new();
        }
        let catalogue = docs
            .iter()
            .map(|(id, title)| format!("docID: {id}, title: {title}"))
            .collect::<Vec<_>>()
            .join("\n");
        let system = "You filter a document catalogue to find which documents are relevant to a user's question. Use ONLY the titles in the catalogue — do not invent docIDs. Output ONLY a JSON array of the docIDs you consider relevant, e.g. [\"abc123\", \"def456\"]. If no document is clearly relevant, output []. No explanations, no Markdown, no code fences, no prose around the array.";
        let user = format!(
            "Question:\n{question}\n\nDocuments:\n{catalogue}\n\nRelevant docIDs (JSON array):"
        );
        let (_, fitted) = message_fit_in(form_message(system, &user), self.chat_max_length);
        let system_content = fitted
            .first()
            .and_then(|message| message.get("content"))
            .and_then(Value::as_str)
            .unwrap_or(system)
            .to_string();
        let history: Vec<Value> = fitted.iter().skip(1).cloned().collect();
        let ans = chat
            .chat(
                &system_content,
                &history,
                &serde_json::json!({"temperature": 0.1}),
            )
            .await
            .unwrap_or_default();
        let cleaned = strip_code_fences(&strip_think_prefix(&ans));
        let cleaned = cleaned.trim().to_string();
        let Some(ids) = serde_json::from_str::<Value>(&cleaned)
            .ok()
            .or_else(|| crate::structure_compile::repair_json_text(&cleaned))
            .filter(Value::is_array)
        else {
            return Vec::new();
        };
        let known: HashSet<String> = docs.iter().map(|(id, _)| id.clone()).collect();
        ids.as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .filter(|id| known.contains(id))
            .collect()
    }

    /// `rag` (the bound tool): answer via the full agentic-search pipeline.
    pub async fn rag(
        &mut self,
        backends: &AgenticBackends<'_>,
        question: &str,
        gen_conf: &Value,
    ) -> String {
        use crate::advanced_rag::agentic_rag_graph::{
            AgenticRuntime, ThinkKind, ThinkSplitter, run_agentic_rag,
        };
        // Near-identical re-ask cache: reuse the prior answer instead of
        // re-running the whole graph, unless the last round was insufficient.
        if !question.is_empty() && self.text_attachments_content.is_empty() {
            let qk = question_keywords(question);
            let last_status = self
                .rag_verdict
                .as_ref()
                .and_then(|verdict| verdict.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let cache_ok = last_status.is_empty() || last_status == "SUFFICIENT";
            if cache_ok && !self.rag_cache.is_empty() {
                for (cached_answer, cached_gram) in self.rag_cache.values() {
                    if !cached_gram.0.is_empty() && cache_similar(&qk, cached_gram) {
                        return cached_answer.clone();
                    }
                }
            }
        }
        let effective_q = resolve_effective_question(question, &self.original_user_question);
        let mut messages: Vec<Value> = Vec::new();
        if !effective_q.is_empty() {
            messages.push(serde_json::json!({"role": "user", "content": effective_q}));
            if !self.text_attachments_content.is_empty()
                && let Some(last) = messages.last_mut()
            {
                let content = last
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                last["content"] =
                    serde_json::json!(format!("{content}{}", self.text_attachments_content));
            }
        }
        let tools_snapshot = self.clone();
        let runtime = AgenticRuntime {
            tools: &tools_snapshot,
            search: backends.search,
            retrieval: backends.retrieval,
            chat: backends.chat,
            action_search: backends.action_search,
            action_tools: backends.action_tools,
            action_llm: backends.action_llm,
            direct: backends.direct,
            stats: StatsHandle(backends.stats.0.clone()),
        };
        let tokens = run_agentic_rag(&runtime, &mut self.kbinfos, &messages, 3, gen_conf).await;
        let mut splitter = ThinkSplitter::default();
        let mut final_text = String::new();
        for token in &tokens {
            for (kind, text) in splitter.push(token) {
                if kind == ThinkKind::Answer {
                    final_text.push_str(&text);
                }
            }
        }
        for (kind, text) in splitter.finish() {
            if kind == ThinkKind::Answer {
                final_text.push_str(&text);
            }
        }
        let id_marker = Regex::new(r"\(\**(ID:\d+)\**\)").expect("id regex");
        let mut final_answer = id_marker.replace_all(&final_text, "[$1]").to_string();
        if !question.is_empty()
            && !final_answer.is_empty()
            && self.text_attachments_content.is_empty()
        {
            self.rag_cache.insert(
                question.to_string(),
                (final_answer.clone(), question_keywords(question)),
            );
        }
        // Sufficiency feedback for the outer loop + the consecutive-insufficient
        // guardrail (bound useless re-runs).
        if let Some(verdict) = self.rag_verdict.clone().filter(Value::is_object) {
            let status = verdict
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !status.is_empty() && status != "SUFFICIENT" {
                let status_hint = match status.as_str() {
                    "USEFUL_BUT_INCOMPLETE" => {
                        "evidence is partially sufficient (gaps remain)".to_string()
                    }
                    "INSUFFICIENT" => "evidence is not yet sufficient".to_string(),
                    "CONFLICTING" => "evidence contains conflicts".to_string(),
                    other => format!("sufficiency status: {other}"),
                };
                let missing: Vec<String> = verdict
                    .get("missing_claims")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().take(3).map(json_text).collect())
                    .unwrap_or_default();
                let missing_txt = if missing.is_empty() {
                    String::new()
                } else {
                    format!("; missing: {}", missing.join("; "))
                };
                let hard: Vec<String> = verdict
                    .get("hard_violations")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().take(3).map(json_text).collect())
                    .unwrap_or_default();
                let hard_txt = if hard.is_empty() {
                    String::new()
                } else {
                    format!("; hard gaps: {}", hard.join(", "))
                };
                let conf_txt = verdict
                    .get("agent_confidence")
                    .and_then(Value::as_f64)
                    .map(|confidence| format!("; agent confidence: {confidence:.2}"))
                    .unwrap_or_default();
                let feedback = verdict
                    .get("feedback")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let fb_txt = if feedback.is_empty() {
                    String::new()
                } else {
                    let head: String = feedback.chars().take(200).collect();
                    format!("; feedback: {head}")
                };
                let insufficient = matches!(
                    status.as_str(),
                    "UNANSWERABLE" | "INSUFFICIENT" | "CONFLICTING"
                );
                if insufficient {
                    self.consecutive_unanswerable += 1;
                } else {
                    self.consecutive_unanswerable = 0;
                }
                if self.consecutive_unanswerable >= 2 {
                    final_answer = format!(
                        "{final_answer}\n\n[Research status] {status_hint}{missing_txt}{hard_txt}{conf_txt}{fb_txt}. STOP calling rag again: {} consecutive research rounds returned insufficient evidence. The sources likely lack the required data. Give your best answer from the evidence already gathered; do not re-run rag.",
                        self.consecutive_unanswerable
                    );
                } else {
                    final_answer = format!(
                        "{final_answer}\n\n[Research status] {status_hint}{missing_txt}{hard_txt}{conf_txt}{fb_txt}. If these gaps are material, call rag again with a question focused on them."
                    );
                }
            }
        }
        final_answer
    }
}






