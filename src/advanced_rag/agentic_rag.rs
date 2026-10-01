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

use regex::Regex;
use serde_json::Value;

use crate::harness::HarnessChat;
use crate::harness::keywords::extract_weighted_keywords;
use crate::harness::orchestrator::direct::Kbinfos;
use crate::harness::tools::text_processing::compact_keywords;
use crate::harness::{form_message, message_fit_in};
use crate::prompts::RAGFLOW_CITATION_PROMPT;

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
    if !an.is_empty() || !bn.is_empty() {
        if an != bn {
            return false;
        }
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
    /// Structured (SQL) KB ids (`parser_config.field_map` present).
    pub sql_kbs: Vec<String>,
    /// Unstructured KB ids.
    pub kbs: Vec<String>,
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
                tools.sql_kbs.push(kb.id.clone());
            } else {
                tools.kbs.push(kb.id.clone());
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn question_keywords_splits_significant_and_numbers() {
        let (words, numbers) = question_keywords("What is the legal population of Paris in 2019?");
        assert!(words.contains("legal"));
        assert!(words.contains("population"));
        assert!(words.contains("paris"));
        assert!(!words.contains("the"));
        assert!(!words.contains("what"));
        assert!(numbers.contains("2019"));
        // CJK text falls back to whole tokens.
        let (cjk, _) = question_keywords("巴黎人口");
        assert!(cjk.contains("巴黎人口"));
        // All-stopword input falls back to the length filter only.
        let (fallback, _) = question_keywords("the of and");
        assert_eq!(
            fallback.len(),
            3,
            "the stopword-only fallback keeps the long tokens"
        );
    }

    #[test]
    fn cache_similar_matches_reask_patterns() {
        let a = question_keywords("population of Paris 2019");
        let b = question_keywords("legal population of Paris 2019");
        assert!(cache_similar(&a, &b), "3/5 overlap collapses the re-ask");
        let other = question_keywords("population of Brown County 2019");
        assert!(!cache_similar(&b, &other));
        let different_year = question_keywords("population of Paris 2015");
        assert!(
            !cache_similar(&a, &different_year),
            "numbers must match exactly"
        );
        let empty = (HashSet::new(), HashSet::new());
        assert!(!cache_similar(&empty, &a));
    }

    #[test]
    fn resolve_effective_question_prefers_original_same_turn() {
        let original = "Who was the purchaser of the OmiyaSoft stake?";
        let rewrite = "Who was the purchaser of the OmiyaSoft stake";
        assert_eq!(resolve_effective_question(rewrite, original), original);
        let different = "What is the population of Paris?";
        assert_eq!(resolve_effective_question(different, original), different);
        assert_eq!(resolve_effective_question("", original), "");
        assert_eq!(resolve_effective_question("q", ""), "q");
        assert_eq!(resolve_effective_question("q", "   "), "q");
    }

    #[test]
    fn citation_guidelines_appends_illustrative_note() {
        let default = get_citation_guidelines(&HashMap::new());
        assert!(default.contains("[ID:"));
        assert!(default.contains("illustrative only"));
        let mut custom = HashMap::new();
        custom.insert(
            "citation_guidelines".to_string(),
            "CUSTOM RULES".to_string(),
        );
        let overridden = get_citation_guidelines(&custom);
        assert!(overridden.starts_with("CUSTOM RULES\n\nIMPORTANT:"));
    }

    #[test]
    fn construction_groups_kbs_and_dedupes_scope() {
        let tools = RagTools::new(RagToolsConfig {
            chat_max_length: 8192,
            doc_scope: Some(vec!["d1".to_string(), "d2".to_string(), "d1".to_string()]),
            kbs_by_ids: vec![
                KbRef {
                    id: "kb1".to_string(),
                    field_map: None,
                },
                KbRef {
                    id: "kb2".to_string(),
                    field_map: Some(serde_json::Map::from_iter([(
                        "city".to_string(),
                        json!("text"),
                    )])),
                },
            ],
            ..RagToolsConfig::default()
        });
        assert_eq!(tools.kb_ids, vec!["kb1".to_string()]);
        assert_eq!(tools.sql_kbs, vec!["kb2".to_string()]);
        assert_eq!(tools.field_map.get("city"), Some(&json!("text")));
        assert_eq!(
            tools.doc_scope,
            Some(vec!["d1".to_string(), "d2".to_string()]),
            "scope is deduped in order"
        );
        assert!(tools.has_unstructured());
        assert!(tools.has_structured());
        assert!(!tools.has_web());
        assert!(tools.has_llm());
    }

    #[test]
    fn scoped_doc_ids_matrix() {
        let bound = RagTools::new(RagToolsConfig {
            doc_scope: Some(vec!["d1".to_string(), "d2".to_string()]),
            ..RagToolsConfig::default()
        });
        assert_eq!(
            bound.scoped_doc_ids(None),
            Some(vec!["d1".to_string(), "d2".to_string()])
        );
        assert_eq!(
            bound.scoped_doc_ids(Some(&[])),
            Some(vec!["d1".to_string(), "d2".to_string()])
        );
        assert_eq!(
            bound.scoped_doc_ids(Some(&["d2".to_string(), "d9".to_string()])),
            Some(vec!["d2".to_string()]),
            "the bound scope is the hard constraint"
        );
        let unbound = RagTools::new(RagToolsConfig::default());
        assert_eq!(unbound.scoped_doc_ids(None), None);
        assert_eq!(
            unbound.scoped_doc_ids(Some(&["d5".to_string()])),
            Some(vec!["d5".to_string()])
        );
    }

    #[test]
    fn fit_messages_keeps_the_pair_within_budget() {
        let tools = RagTools::new(RagToolsConfig {
            chat_max_length: 128,
            ..RagToolsConfig::default()
        });
        let long = "x".repeat(5000);
        let messages = tools.fit_messages("system", &long);
        assert_eq!(messages.len(), 2);
        let content_len = messages[1]["content"].as_str().unwrap().chars().count();
        assert!(
            content_len < 5000,
            "the fitter trims the oversized user turn"
        );
    }
}
#[cfg(test)]
mod formalize_tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Mutex;

    struct MockChat {
        reply: Result<String, String>,
        max_length: usize,
        calls: Mutex<Vec<String>>,
    }

    impl MockChat {
        fn ok(reply: &str) -> Self {
            Self {
                reply: Ok(reply.to_string()),
                max_length: 4096,
                calls: Mutex::new(Vec::new()),
            }
        }
        fn err() -> Self {
            Self {
                reply: Err("boom".to_string()),
                max_length: 4096,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl crate::harness::HarnessChat for MockChat {
        async fn chat(
            &self,
            _system: &str,
            _history: &[Value],
            _gen_conf: &Value,
        ) -> Result<String, String> {
            self.calls.lock().unwrap().push("chat".to_string());
            self.reply.clone()
        }
        fn max_length(&self) -> usize {
            self.max_length
        }
    }

    fn tools_with_kb() -> RagTools {
        RagTools::new(RagToolsConfig {
            kbs: vec![KbRef {
                id: "kb1".to_string(),
                field_map: None,
            }],
            chat_max_length: 4096,
            ..RagToolsConfig::default()
        })
    }

    #[test]
    fn sys_prompt_router_matrix() {
        let tools = RagTools::new(RagToolsConfig {
            chat_max_length: 4096,
            ..RagToolsConfig::default()
        });
        let plain = tools.sys_prompt();
        assert!(plain.starts_with("You are a smart agent."));
        assert!(
            !plain.contains("summarize_document"),
            "no KB -> no summarize line"
        );

        let with_kb = tools_with_kb().sys_prompt();
        assert!(with_kb.contains("Call `summarize_document` ONLY"));

        let custom = RagTools::new(RagToolsConfig {
            system_prompt: "CUSTOM".to_string(),
            chat_max_length: 4096,
            ..RagToolsConfig::default()
        });
        assert!(
            custom
                .sys_prompt()
                .starts_with("CUSTOM\n\nYou are a smart agent.")
        );
    }

    #[tokio::test]
    async fn formalize_single_turn_keeps_question_verbatim() {
        let tools = tools_with_kb();
        let chat = MockChat::err();
        let messages = vec![json!({"role": "user", "content": "  Who discovered radium?  "})];
        let (question, keywords) = tools.formalize(&chat, &messages).await;
        assert_eq!(question, "Who discovered radium?", "trimmed, not rewritten");
        assert!(keywords.is_empty() || keywords == "Who discovered radium?");
    }

    #[tokio::test]
    async fn formalize_multi_turn_parses_think_and_fences() {
        let tools = tools_with_kb();
        let chat = MockChat::ok(
            "<think>hmm</think>```json\n{\"question\": \"standalone?\", \"keywords\": \"a, b, a, c\"}\n```",
        );
        let messages = vec![
            json!({"role": "user", "content": "first"}),
            json!({"role": "assistant", "content": "second"}),
            json!({"role": "user", "content": "third"}),
        ];
        let (question, keywords) = tools.formalize(&chat, &messages).await;
        assert_eq!(question, "standalone?");
        assert_eq!(keywords, "a b c", "deduped and space-joined");
        assert_eq!(chat.calls.lock().unwrap().len(), 1, "one LLM round-trip");
    }

    #[tokio::test]
    async fn formalize_multi_turn_falls_back_to_last_user() {
        let tools = tools_with_kb();
        let chat = MockChat::ok("no json here");
        let messages = vec![
            json!({"role": "user", "content": "first"}),
            json!({"role": "user", "content": "second"}),
        ];
        let (question, keywords) = tools.formalize(&chat, &messages).await;
        assert_eq!(question, "second");
        assert!(keywords.is_empty());
    }

    #[tokio::test]
    async fn extract_keywords_empty_question_returns_empty() {
        let tools = tools_with_kb();
        let chat = MockChat::err();
        assert!(tools.extract_keywords(&chat, "").await.is_empty());
        assert!(
            chat.calls.lock().unwrap().is_empty(),
            "no LLM call for empty input"
        );
    }
}
