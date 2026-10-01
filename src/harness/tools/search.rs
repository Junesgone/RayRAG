//! Search tools: hybrid, vector, BM25, web, structured — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/tools/search.py`.
//!
//! Retrieval legs plus the grep-style locate layered on top of BM25. The
//! tool-facing schemas and dispatch live in the action-session module; this
//! module owns the retrieval implementations they call. The store call is
//! injected through [`HarnessRetriever`] so the orchestration logic is testable
//! without a live knowledge base.

use async_trait::async_trait;
use regex::Regex;
use serde_json::Value;

use crate::harness::grep_sed_narrow::narrow_by_terms;
use crate::harness::orchestrator::direct::Kbinfos;
use crate::harness::tools::text_processing::narrow_or_keep;

// Fallbacks for callers that supply no retrieval configuration.
pub const DEFAULT_SIMILARITY_THRESHOLD: f64 = 0.2;
pub const DEFAULT_HYBRID_VECTOR_WEIGHT: f64 = 0.3;
pub const DEFAULT_TOP_N: usize = 12;
pub const DEFAULT_RERANK_CANDIDATES: usize = 64;
pub const DEFAULT_TOP_K: usize = 1024;
/// Grep-style exact search caps.
pub const GREP_TERMS_MAX: usize = 10;
pub const GREP_OUT_CHARS_PER_CHUNK: usize = 700;
pub const GREP_OUT_TOTAL_CHARS: usize = 8000;
pub const LIST_CHUNKS_MAX_CHUNKS: usize = 80;
/// Chars of a chunk's text returned by `search_chunks` in snippet mode.
pub const SEARCH_SNIPPET_CHARS: usize = 300;

/// `_TOOL_NAME_BY_FUNC`: public tool names the model calls.
pub const TOOL_NAME_BY_FUNC: [(&str, &str); 8] = [
    ("_think_impl", "think"),
    ("_todo_write_impl", "todo_write"),
    ("_grep_chunks_impl", "grep_chunks"),
    ("_search_chunks_impl", "search_chunks"),
    ("_list_chunks_impl", "list_chunks"),
    ("_calculate_impl", "calculate"),
    ("_navigate_tree_impl", "navigate_tree"),
    ("_navigate_structure_impl", "navigate_structure"),
];

/// Thinking modes that enable the knowledge-compilation tools.
pub const COMPILED_TOOL_MODES: [&str; 2] = ["high", "ultra"];

/// Retrieval settings a caller may supply (`_setting` reads).
#[derive(Debug, Clone, Default)]
pub struct SearchSettings {
    pub top_n: Option<usize>,
    pub top_k: Option<usize>,
    pub rerank_candidates_count: Option<usize>,
    pub vector_similarity_weight: Option<f64>,
    pub similarity_threshold: Option<f64>,
}

/// `_resolve_top_n`: explicit argument wins, then the caller's configuration.
pub fn resolve_top_n(settings: &SearchSettings, top_n: Option<usize>) -> usize {
    top_n.unwrap_or(settings.top_n.unwrap_or(DEFAULT_TOP_N))
}

/// `_resolve_top_k`: size of the approximate-kNN candidate pool.
pub fn resolve_top_k(settings: &SearchSettings) -> usize {
    settings.top_k.unwrap_or(DEFAULT_TOP_K)
}

/// `_resolve_rerank_candidates`: never smaller than the page it must fill.
pub fn resolve_rerank_candidates(settings: &SearchSettings, top_n: usize) -> usize {
    settings
        .rerank_candidates_count
        .unwrap_or(DEFAULT_RERANK_CANDIDATES)
        .max(top_n)
}

/// The effective query shared by every retrieval leg: append the
/// entity-weighted retrieval query (or the keyword union) and cap at 400 chars.
pub fn effective_query(query: &str, keywords: &str, retrieval_query: &str) -> String {
    let combined = if !retrieval_query.is_empty() {
        format!("{query} {retrieval_query}")
    } else if !keywords.is_empty() {
        format!("{query} {keywords}")
    } else {
        query.to_string()
    };
    let trimmed = combined.trim();
    trimmed.chars().take(400).collect()
}

/// `_search_cache_key`: key a retrieval by what determines its result.
pub fn search_cache_key(
    effective_query: &str,
    target_ids: &[String],
    top_n: usize,
    doc_scope: &[String],
) -> String {
    let mut targets: Vec<String> = target_ids.to_vec();
    targets.sort();
    let mut scope: Vec<String> = doc_scope.to_vec();
    scope.sort();
    let normalized_query = effective_query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    format!(
        "{normalized_query}\x1f{}\x1f{top_n}\x1f{}",
        targets.join("\x1e"),
        scope.join("\x1e")
    )
}

/// `_is_table_chunk`: HTML table markup or >=3 pipe rows.
pub fn is_table_chunk(chunk: &Value) -> bool {
    let text = crate::harness::chunk_utils::chunk_text(chunk).to_lowercase();
    if text.contains("<table") || text.contains("<tr") {
        return true;
    }
    let pipe_rows = crate::harness::chunk_utils::chunk_text(chunk)
        .lines()
        .filter(|line| line.matches('|').count() >= 2)
        .count();
    pipe_rows >= 3
}

/// `_grep_terms_from_query`: bare alnum words of length >= 2, deduped and
/// capped; numbers/ids preserved as-is.
pub fn grep_terms_from_query(query: &str, max_terms: usize) -> Vec<String> {
    if query.is_empty() {
        return Vec::new();
    }
    let finder = Regex::new(r"[A-Za-z0-9][A-Za-z0-9_.\-]+").unwrap();
    let mut terms: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for found in finder.find_iter(query) {
        let trimmed = found
            .as_str()
            .trim_matches(|ch| ch == '.' || ch == '_' || ch == '-');
        if trimmed.chars().count() < 2 {
            continue;
        }
        let lower = trimmed.to_lowercase();
        if !seen.insert(lower) {
            continue;
        }
        terms.push(trimmed.to_string());
        if terms.len() >= max_terms {
            break;
        }
    }
    terms
}

/// `_query_to_terms`: derive plain grep terms from a regex query.
pub fn query_to_terms(query: &str) -> Vec<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let stripped = trimmed.replace("^\\(?i\\)", "");
    let stripped = stripped.replace("\\b", "").replace("(?i)", "");
    let mut terms: Vec<String> = Vec::new();
    for part in stripped.split('|') {
        let cleaned = Regex::new(r"[.*+?^$()\[\]{}]")
            .unwrap()
            .replace_all(part, " ")
            .trim()
            .to_string();
        if cleaned.is_empty() {
            continue;
        }
        for token in cleaned.split_whitespace() {
            if token.chars().count() >= 2 && !terms.iter().any(|term| term == token) {
                terms.push(token.to_string());
            }
        }
        if terms.len() >= 16 {
            break;
        }
    }
    terms
}

/// `_rank_chunks_by_terms`: zero-LLM keyword relevance for a small candidate
/// scope; most-relevant first.
pub fn rank_chunks_by_terms(candidates: &[Value], queries: &[String]) -> Vec<Value> {
    let tokenizer = Regex::new(r"[A-Za-z0-9_]{2,}").unwrap();
    let mut terms: Vec<String> = Vec::new();
    for query in queries {
        for found in tokenizer.find_iter(&query.to_lowercase()) {
            let token = found.as_str().to_string();
            if !terms.contains(&token) {
                terms.push(token);
            }
        }
    }
    if terms.is_empty() {
        return candidates.to_vec();
    }
    let mut scored: Vec<(usize, Value)> = Vec::new();
    for chunk in candidates {
        let text = crate::harness::chunk_utils::chunk_text(chunk).to_lowercase();
        let hits = terms
            .iter()
            .filter(|term| text.contains(term.as_str()))
            .count();
        if hits > 0 {
            scored.push((hits, chunk.clone()));
        }
    }
    scored.sort_by(|left, right| right.0.cmp(&left.0));
    scored.into_iter().map(|(_, chunk)| chunk).collect()
}

/// One retrieval call (`settings.retriever.retrieval(...)`).
#[derive(Debug, Clone, Default)]
pub struct RetrievalRequest {
    pub query: String,
    pub use_embedding: bool,
    pub tenant_ids: Vec<String>,
    pub dataset_ids: Vec<String>,
    pub page: usize,
    pub page_size: usize,
    pub similarity_threshold: f64,
    pub vector_similarity_weight: f64,
    pub knn_top_k: usize,
    pub aggs: bool,
    pub highlight: bool,
    pub doc_ids: Vec<String>,
    /// Plain retrieval = document chunks only; compiled products have their own tools.
    pub must_not_compile_kwd: bool,
    pub rerank_candidates_count: usize,
}

/// The injected retrieval backend.
#[async_trait]
pub trait HarnessRetriever: Send + Sync {
    async fn retrieval(&self, request: RetrievalRequest) -> Result<Kbinfos, String>;
}

/// The `RAGTools` surface the search legs read.
pub struct SearchContext<'a> {
    pub kb_ids: Vec<String>,
    pub sql_kb_ids: Vec<String>,
    pub tenant_ids: Vec<String>,
    pub has_embed_model: bool,
    pub settings: SearchSettings,
    pub retriever: &'a dyn HarnessRetriever,
    /// `scoped_doc_ids(doc_scope)` hook, when the caller provides one.
    pub scoped_doc_ids: Option<&'a (dyn Fn(Option<Vec<String>>) -> Vec<String> + Send + Sync)>,
    /// Per-request dedup cache (`tools.search_cache`).
    pub search_cache: Option<&'a std::sync::Mutex<std::collections::HashMap<String, Kbinfos>>>,
}

impl<'a> SearchContext<'a> {
    /// `_normalize` followed by the child-retrieval pass is applied by the
    /// retriever itself; the legs only resolve targets and settings here.
    fn targets(&self, kb_ids: &[String]) -> Vec<String> {
        if !kb_ids.is_empty() {
            return kb_ids.to_vec();
        }
        let mut targets = self.kb_ids.clone();
        for id in &self.sql_kb_ids {
            if !targets.contains(id) {
                targets.push(id.clone());
            }
        }
        targets
    }

    fn scope(&self, doc_scope: Option<Vec<String>>) -> Vec<String> {
        match self.scoped_doc_ids {
            Some(hook) => hook(doc_scope),
            None => doc_scope.unwrap_or_default(),
        }
    }
}

/// `hybrid_search`: one hybrid pass with keyword narrowing and (optionally)
/// compiled-structure expansion.
pub async fn hybrid_search(
    ctx: &SearchContext<'_>,
    query: &str,
    kb_ids: Option<Vec<String>>,
    top_n: Option<usize>,
    doc_scope: Option<Vec<String>>,
    keywords: &str,
    retrieval_query: &str,
    use_compiled: bool,
) -> Kbinfos {
    let top_n = resolve_top_n(&ctx.settings, top_n);
    let target_ids = ctx.targets(kb_ids.as_deref().unwrap_or(&[]));
    if target_ids.is_empty() {
        return Kbinfos::default();
    }
    let doc_scope = ctx.scope(doc_scope);
    let effective = effective_query(query, keywords, retrieval_query);
    let cache_key = search_cache_key(&effective, &target_ids, top_n, &doc_scope);
    if let Some(cache) = ctx.search_cache {
        if let Ok(cache) = cache.lock() {
            if let Some(cached) = cache.get(&cache_key) {
                return cached.clone();
            }
        }
    }
    let vector_weight = if ctx.has_embed_model {
        ctx.settings
            .vector_similarity_weight
            .unwrap_or(DEFAULT_HYBRID_VECTOR_WEIGHT)
    } else {
        0.0
    };
    let request = RetrievalRequest {
        query: effective,
        use_embedding: ctx.has_embed_model,
        tenant_ids: ctx.tenant_ids.clone(),
        dataset_ids: target_ids,
        page: 1,
        page_size: top_n,
        similarity_threshold: ctx
            .settings
            .similarity_threshold
            .unwrap_or(DEFAULT_SIMILARITY_THRESHOLD),
        vector_similarity_weight: vector_weight,
        knn_top_k: resolve_top_k(&ctx.settings),
        aggs: true,
        highlight: false,
        doc_ids: doc_scope.clone(),
        must_not_compile_kwd: true,
        rerank_candidates_count: resolve_rerank_candidates(&ctx.settings, top_n),
    };
    let mut kbinfos = ctx.retriever.retrieval(request).await.unwrap_or_default();
    kbinfos.chunks = narrow_or_keep(&kbinfos.chunks, keywords, "hybrid_search");
    let _ = use_compiled; // compiled expansion lands with the navigation batch
    if let Some(cache) = ctx.search_cache {
        if let Ok(mut cache) = cache.lock() {
            cache.insert(cache_key, kbinfos.clone());
        }
    }
    kbinfos
}

/// `vector_search`: meaning-only leg (weight 1.0, pure-cosine floor).
pub async fn vector_search(
    ctx: &SearchContext<'_>,
    query: &str,
    kb_ids: Option<Vec<String>>,
    top_n: Option<usize>,
    keywords: &str,
    retrieval_query: &str,
    doc_scope: Option<Vec<String>>,
) -> Kbinfos {
    let top_n = resolve_top_n(&ctx.settings, top_n);
    if !ctx.has_embed_model {
        return Kbinfos::default();
    }
    let target_ids = ctx.targets(kb_ids.as_deref().unwrap_or(&[]));
    let doc_scope = ctx.scope(doc_scope);
    let request = RetrievalRequest {
        query: effective_query(query, keywords, retrieval_query),
        use_embedding: true,
        tenant_ids: ctx.tenant_ids.clone(),
        dataset_ids: target_ids,
        page: 1,
        page_size: top_n,
        similarity_threshold: 0.2,
        vector_similarity_weight: 1.0,
        knn_top_k: resolve_top_k(&ctx.settings),
        aggs: false,
        highlight: false,
        doc_ids: doc_scope,
        must_not_compile_kwd: true,
        rerank_candidates_count: resolve_rerank_candidates(&ctx.settings, top_n),
    };
    let mut kbinfos = ctx.retriever.retrieval(request).await.unwrap_or_default();
    kbinfos.chunks = narrow_or_keep(&kbinfos.chunks, keywords, "Vector search");
    kbinfos
}

/// `bm25_search`: keyword-only leg (weight 0, threshold 0).
pub async fn bm25_search(
    ctx: &SearchContext<'_>,
    query: &str,
    kb_ids: Option<Vec<String>>,
    top_n: Option<usize>,
    keywords: &str,
    retrieval_query: &str,
    doc_scope: Option<Vec<String>>,
) -> Kbinfos {
    let top_n = resolve_top_n(&ctx.settings, top_n);
    let target_ids = ctx.targets(kb_ids.as_deref().unwrap_or(&[]));
    let doc_scope = ctx.scope(doc_scope);
    let request = RetrievalRequest {
        query: effective_query(query, keywords, retrieval_query),
        use_embedding: false,
        tenant_ids: ctx.tenant_ids.clone(),
        dataset_ids: target_ids,
        page: 1,
        page_size: top_n,
        similarity_threshold: 0.0,
        vector_similarity_weight: 0.0,
        knn_top_k: resolve_top_k(&ctx.settings),
        aggs: false,
        highlight: false,
        doc_ids: doc_scope,
        must_not_compile_kwd: true,
        rerank_candidates_count: resolve_rerank_candidates(&ctx.settings, top_n),
    };
    let mut kbinfos = ctx.retriever.retrieval(request).await.unwrap_or_default();
    kbinfos.chunks = narrow_or_keep(&kbinfos.chunks, keywords, "BM25 search");
    kbinfos
}

/// `grep_search`: BM25-first candidate pool, then a term locate on prose chunks
/// (tables pass through whole). When grep matches nothing the BM25 candidates
/// are returned unchanged so evidence is never dropped.
pub async fn grep_search(
    ctx: &SearchContext<'_>,
    query: &str,
    kb_ids: Option<Vec<String>>,
    top_n: usize,
    doc_scope: Option<Vec<String>>,
    keywords: Option<&str>,
) -> Kbinfos {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Kbinfos::default();
    }
    let terms = grep_terms_from_query(trimmed, GREP_TERMS_MAX);
    let hint = match keywords {
        Some(hint) if !hint.is_empty() => hint.to_string(),
        _ => terms.join(" "),
    };
    let mut result = bm25_search(ctx, trimmed, kb_ids, Some(top_n), &hint, "", doc_scope).await;
    let chunks = result.chunks.clone();
    if chunks.is_empty() || terms.is_empty() {
        return result;
    }
    let tables: Vec<Value> = chunks
        .iter()
        .filter(|chunk| is_table_chunk(chunk))
        .cloned()
        .collect();
    let prose: Vec<Value> = chunks
        .iter()
        .filter(|chunk| !is_table_chunk(chunk))
        .cloned()
        .collect();
    let mut kept: Vec<Value> = Vec::new();
    if !prose.is_empty() {
        let (narrowed, _stats) = narrow_by_terms(
            &prose,
            &terms,
            None,
            Some(1),
            Some(0),
            trimmed,
            GREP_OUT_CHARS_PER_CHUNK,
            GREP_OUT_TOTAL_CHARS,
        );
        kept = narrowed;
    }
    let mut combined = tables;
    combined.extend(kept);
    if !combined.is_empty() {
        result.chunks = combined;
    }
    result
}

/// `list_chunks`: deep-read a document — its COMPLETE chunk list in reading order.
pub async fn list_chunks(
    fetch_full_document: Option<&(dyn Fn(&str) -> Result<Kbinfos, String> + Send + Sync)>,
    doc_id: &str,
) -> Kbinfos {
    let doc_id = doc_id.trim();
    if doc_id.is_empty() {
        return Kbinfos::default();
    }
    let Some(fetch) = fetch_full_document else {
        return Kbinfos::default();
    };
    match fetch(doc_id) {
        Ok(full) => Kbinfos {
            chunks: full
                .chunks
                .into_iter()
                .take(LIST_CHUNKS_MAX_CHUNKS)
                .collect(),
            doc_aggs: full.doc_aggs,
        },
        Err(_) => Kbinfos::default(),
    }
}

/// `_load_specific_chunks`: load chunks referenced by navigate_structure
/// outline pointers (requires doc_scope for the precise path).
pub async fn load_specific_chunks(
    fetch_full_document: &(dyn Fn(&str) -> Result<Kbinfos, String> + Send + Sync),
    chunk_ids: &[String],
    doc_scope: Option<&[String]>,
) -> Vec<Value> {
    let wanted: std::collections::HashSet<&str> = chunk_ids
        .iter()
        .map(|id| id.trim())
        .filter(|id| !id.is_empty())
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let Some(scope) = doc_scope else {
        return Vec::new();
    };
    let mut found: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for doc_id in scope.iter().take(8) {
        let Ok(full) = fetch_full_document(doc_id) else {
            continue;
        };
        for chunk in full.chunks {
            let id = crate::harness::chunk_utils::chunk_id(&chunk);
            if wanted.contains(id.as_str()) && seen.insert(id) {
                found.push(chunk);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct MockRetriever {
        result: Kbinfos,
        last: std::sync::Mutex<Option<RetrievalRequest>>,
    }

    #[async_trait]
    impl HarnessRetriever for MockRetriever {
        async fn retrieval(&self, request: RetrievalRequest) -> Result<Kbinfos, String> {
            *self.last.lock().unwrap() = Some(request);
            Ok(self.result.clone())
        }
    }

    fn context<'a>(retriever: &'a MockRetriever) -> SearchContext<'a> {
        SearchContext {
            kb_ids: vec!["kb-1".to_string()],
            sql_kb_ids: vec![],
            tenant_ids: vec!["tenant".to_string()],
            has_embed_model: true,
            settings: SearchSettings::default(),
            retriever,
            scoped_doc_ids: None,
            search_cache: None,
        }
    }

    #[test]
    fn helpers_mirror_upstream() {
        assert_eq!(
            resolve_top_n(&SearchSettings::default(), None),
            DEFAULT_TOP_N
        );
        assert_eq!(resolve_top_n(&SearchSettings::default(), Some(5)), 5);
        assert_eq!(resolve_top_k(&SearchSettings::default()), DEFAULT_TOP_K);
        assert_eq!(
            resolve_rerank_candidates(&SearchSettings::default(), 100),
            100
        );
        assert_eq!(
            effective_query("who?", "alpha", "alpha, alpha, 1999"),
            "who? alpha, alpha, 1999"
        );
        assert_eq!(effective_query("who?", "alpha", ""), "who? alpha");
        let key_a = search_cache_key("Q", &["b".into(), "a".into()], 12, &["d1".into()]);
        let key_b = search_cache_key("q", &["a".into(), "b".into()], 12, &["d1".into()]);
        assert_eq!(key_a, key_b);

        let terms = grep_terms_from_query("Culdect-Saga: the 2011 season?", GREP_TERMS_MAX);
        assert!(terms.contains(&"Culdect-Saga".to_string()));
        assert!(terms.contains(&"2011".to_string()));
        let regex_terms = query_to_terms("(?i)(Alpha|beta\\b|gamma)");
        assert!(regex_terms.contains(&"Alpha".to_string()));
        assert!(regex_terms.contains(&"beta".to_string()));
    }

    #[test]
    fn tables_detected_and_ranked() {
        assert!(is_table_chunk(
            &json!({"content": "| a | b |\n| - | - |\n| 1 | 2 |"})
        ));
        assert!(is_table_chunk(
            &json!({"content": "<table><tr></tr></table>"})
        ));
        assert!(!is_table_chunk(&json!({"content": "plain prose"})));

        let candidates = vec![
            json!({"chunk_id": "c1", "content": "nothing here"}),
            json!({"chunk_id": "c2", "content": "alpha and beta appear"}),
        ];
        let ranked = rank_chunks_by_terms(&candidates, &["alpha beta".to_string()]);
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0]["chunk_id"], json!("c2"));
    }

    #[tokio::test]
    async fn hybrid_legs_pass_flags_and_narrow() {
        let retriever = MockRetriever {
            result: Kbinfos {
                chunks: vec![
                    json!({"chunk_id": "c1", "content": "alpha wins the race"}),
                    json!({"chunk_id": "c2", "content": "unrelated filler about nothing at all"}),
                ],
                doc_aggs: vec![],
            },
            last: std::sync::Mutex::new(None),
        };
        let ctx = context(&retriever);
        // Keyword narrowing needs 3+ comma terms (upstream bigram behaviour).
        let result = hybrid_search(
            &ctx,
            "who won?",
            None,
            None,
            None,
            "alpha, wins, race",
            "",
            true,
        )
        .await;
        assert_eq!(result.chunks.len(), 1, "keyword narrowing drops the filler");
        let request = retriever.last.lock().unwrap().clone().unwrap();
        assert!(request.use_embedding);
        assert_eq!(
            request.vector_similarity_weight,
            DEFAULT_HYBRID_VECTOR_WEIGHT
        );
        assert!(request.aggs);
        assert!(request.must_not_compile_kwd);

        // Vector leg: weight 1.0 and no aggs; BM25 leg: weight 0.
        let _ = vector_search(&ctx, "q", None, None, "", "", None).await;
        let request = retriever.last.lock().unwrap().clone().unwrap();
        assert_eq!(request.vector_similarity_weight, 1.0);
        assert!(!request.aggs);
        let _ = bm25_search(&ctx, "q", None, None, "", "", None).await;
        let request = retriever.last.lock().unwrap().clone().unwrap();
        assert_eq!(request.vector_similarity_weight, 0.0);
    }

    #[tokio::test]
    async fn grep_search_keeps_tables_whole_and_falls_back() {
        let table = json!({"chunk_id": "t1", "content": "| rank | city |\n| --- | --- |\n| 1 | Tokyo |\n| 2 | Lima |"});
        let prose = json!({"chunk_id": "p1", "content": "The 2011 season standings list Culdect Saga at rank nineteen overall in that long table of results and more words to pass the short threshold for narrowing to apply at all here in this chunk of text."});
        let retriever = MockRetriever {
            result: Kbinfos {
                chunks: vec![table.clone(), prose.clone()],
                doc_aggs: vec![],
            },
            last: std::sync::Mutex::new(None),
        };
        let ctx = context(&retriever);
        let result = grep_search(&ctx, "Culdect Saga", None, 60, None, None).await;
        assert!(
            result
                .chunks
                .iter()
                .any(|chunk| chunk["chunk_id"] == json!("t1"))
        );
        assert!(
            result
                .chunks
                .iter()
                .any(|chunk| chunk["chunk_id"] == json!("p1"))
        );

        // No terms at all: the BM25 candidates come back untouched.
        let result = grep_search(&ctx, "!!!", None, 60, None, None).await;
        assert_eq!(result.chunks.len(), 2);
    }

    #[tokio::test]
    async fn deep_read_helpers() {
        let fetch = |doc_id: &str| -> Result<Kbinfos, String> {
            if doc_id == "d1" {
                Ok(Kbinfos {
                    chunks: vec![json!({"chunk_id": "c1"}), json!({"chunk_id": "c2"})],
                    doc_aggs: vec![json!({"doc_id": "d1"})],
                })
            } else {
                Err("missing".to_string())
            }
        };
        let full = list_chunks(Some(&fetch), "d1").await;
        assert_eq!(full.chunks.len(), 2);
        assert!(list_chunks(Some(&fetch), "").await.chunks.is_empty());

        let scope = vec!["d1".to_string()];
        let loaded = load_specific_chunks(&fetch, &["c2".to_string()], Some(&scope)).await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0]["chunk_id"], json!("c2"));
        assert!(
            load_specific_chunks(&fetch, &["c9".to_string()], Some(&scope))
                .await
                .is_empty()
        );
    }
}
