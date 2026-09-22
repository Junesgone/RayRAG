//! Advanced RAG + Tree-structured query decomposition.
//! Multi-hop retrieval: decompose → retrieve → refine → fusion.
//! Tree Decomposition ↔ RAGFlow's `advanced_rag/tree_structured_query_decomposition_retrieval.py` (DeepResearcher).
//!
//! Algorithm:
//! 1. Tree Decomposition: break complex query into hierarchical sub-queries
//! 2. Multi-hop: retrieve for each sub-query, cascade results
//! 3. Fusion: deduplicate + score-merge results across hops

use crate::Result;
use crate::search::SearchEngine;
use crate::search::SearchResult;
use std::collections::HashMap;

/// A single retrieval hop result.
#[derive(Debug, Clone)]
struct HopResult {
    #[allow(dead_code)]
    query: String,
    results: Vec<SearchResult>,
    #[allow(dead_code)]
    hop: usize,
}

/// Multi-hop retrieval strategy.
///
/// Algorithm:
/// 1. Decompose the original query into sub-queries
/// 2. For each sub-query, retrieve relevant chunks
/// 3. Extract key terms from retrieved chunks
/// 4. Build a refined query using original + extracted terms
/// 5. Second hop: search with refined query
/// 6. Fuse and deduplicate results
pub struct MultiHopStrategy {
    /// Max number of hops
    max_hops: usize,
    /// Max results per hop
    results_per_hop: usize,
    /// Similarity threshold for deduplication
    dedup_threshold: f32,
}

impl Default for MultiHopStrategy {
    fn default() -> Self {
        Self {
            max_hops: 3,
            results_per_hop: 10,
            dedup_threshold: 0.85,
        }
    }
}

impl MultiHopStrategy {
    /// Create a new multi-hop strategy.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set max hops.
    pub fn max_hops(mut self, n: usize) -> Self {
        self.max_hops = n.min(5);
        self
    }

    /// Set results per hop.
    pub fn results_per_hop(mut self, n: usize) -> Self {
        self.results_per_hop = n.min(50);
        self
    }

    /// Execute multi-hop search.
    ///
    /// `engine` provides the search capability.
    /// `embed_fn` embeds a query string → Vec<f32> (async).
    pub fn search<F>(
        &self,
        query: &str,
        engine: &SearchEngine,
        embed_fn: F,
    ) -> Result<Vec<SearchResult>>
    where
        F: Fn(&str) -> Result<Vec<f32>>,
    {
        let mut all_results: Vec<SearchResult> = Vec::new();
        let mut seen_ids: HashMap<String, f32> = HashMap::new();

        // Step 1: Decompose query
        let sub_queries = decompose_query(query);

        // Step 2-3: First hop — search with each sub-query
        let mut hop_results = Vec::new();
        for sub_q in &sub_queries {
            let embedding = embed_fn(sub_q)?;
            let results = engine.search(&embedding, self.results_per_hop);
            hop_results.push(HopResult {
                query: sub_q.clone(),
                results,
                hop: 1,
            });
        }

        // Collect first-hop results
        for hr in &hop_results {
            for r in &hr.results {
                if r.score > 0.3 {
                    all_results.push(r.clone());
                    seen_ids.insert(r.chunk.id.clone(), r.score);
                }
            }
        }

        // Step 4: Extract key terms from top results for query refinement
        let key_terms = extract_key_terms(&hop_results, 5);

        // Step 5: Second hop with refined query
        if !key_terms.is_empty() {
            let refined_query = format!("{} {}", query, key_terms.join(" "));
            let embedding = embed_fn(&refined_query)?;
            let second_results = engine.search(&embedding, self.results_per_hop);

            for r in &second_results {
                // Deduplicate: skip if we already have a similar chunk
                if let Some(&existing_score) = seen_ids.get(&r.chunk.id) {
                    // Keep the higher score
                    if r.score > existing_score {
                        seen_ids.insert(r.chunk.id.clone(), r.score);
                    }
                } else if r.score > 0.25 {
                    all_results.push(r.clone());
                    seen_ids.insert(r.chunk.id.clone(), r.score);
                }
            }
        }

        // Step 6: Third hop — extract phrases from top-3 and search
        if self.max_hops >= 3 && all_results.len() >= 3 {
            let phrases = extract_key_phrases(&all_results, 3);
            if !phrases.is_empty() {
                let phase_query = phrases.join(". ");
                let embedding = embed_fn(&phase_query)?;
                let third_results = engine.search(&embedding, self.results_per_hop);

                for r in &third_results {
                    if !seen_ids.contains_key(&r.chunk.id) && r.score > 0.2 {
                        all_results.push(r.clone());
                        seen_ids.insert(r.chunk.id.clone(), r.score);
                    }
                }
            }
        }

        // Sort by score descending
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Deduplicate by near-duplicate content
        let deduped = deduplicate_results(&all_results, self.dedup_threshold);

        Ok(deduped)
    }
}

/// Decompose a complex query into sub-queries.
///
/// Simple heuristic:
/// - Split on "and", "or", comma, question marks
/// - If no separators, return the original query
/// - For very short queries, don't decompose
fn decompose_query(query: &str) -> Vec<String> {
    // Short query — don't decompose
    if query.len() < 30 && !query.contains(" and ") && !query.contains(" or ") {
        return vec![query.to_string()];
    }

    let mut sub_queries = Vec::new();

    // Split on "and" / "or"
    for part in query.split_inclusive(&['?', '？', '.', '。', '\n']) {
        let trimmed = part.trim();
        if trimmed.len() > 3 {
            sub_queries.push(trimmed.to_string());
        }
    }

    // If still a single query, try splitting on "and"
    if sub_queries.len() <= 1 {
        let keywords = [" and ", " or ", ", ", ";"];
        for kw in &keywords {
            if query.contains(kw) {
                sub_queries = query
                    .split(kw)
                    .map(|s| s.trim().to_string())
                    .filter(|s| s.len() > 3)
                    .collect();
                break;
            }
        }
    }

    if sub_queries.is_empty() {
        vec![query.to_string()]
    } else {
        sub_queries
    }
}

/// Extract key terms from top results across all hops.
fn extract_key_terms(hop_results: &[HopResult], top_n: usize) -> Vec<String> {
    let mut term_freq: HashMap<String, usize> = HashMap::new();

    for hr in hop_results {
        for r in hr.results.iter().take(3) {
            for word in r.chunk.content.split_whitespace() {
                let cleaned = word
                    .trim_matches(|c: char| !c.is_alphanumeric())
                    .to_lowercase();
                if cleaned.len() > 3 && !is_stop_word(&cleaned) {
                    *term_freq.entry(cleaned).or_insert(0) += 1;
                }
            }
        }
    }

    let mut terms: Vec<(String, usize)> = term_freq.into_iter().collect();
    terms.sort_by_key(|term| std::cmp::Reverse(term.1));
    terms.truncate(top_n);

    terms.into_iter().map(|(t, _)| t).collect()
}

/// Extract key phrases (2-3 word sequences) from top results.
fn extract_key_phrases(results: &[SearchResult], top_n: usize) -> Vec<String> {
    let mut phrases: HashMap<String, usize> = HashMap::new();

    for r in results.iter().take(5) {
        let words: Vec<&str> = r.chunk.content.split_whitespace().collect();
        for window in words.windows(3) {
            let phrase = window.join(" ");
            if phrase.len() > 10 && phrase.len() < 80 {
                *phrases.entry(phrase).or_insert(0) += 1;
            }
        }
    }

    let mut sorted: Vec<(String, usize)> = phrases.into_iter().collect();
    sorted.sort_by_key(|phrase| std::cmp::Reverse(phrase.1));
    sorted.truncate(top_n);

    sorted.into_iter().map(|(p, _)| p).collect()
}

/// Deduplicate results by near-duplicate content (Jaccard similarity).
fn deduplicate_results(results: &[SearchResult], threshold: f32) -> Vec<SearchResult> {
    let mut kept: Vec<SearchResult> = Vec::new();

    for r in results {
        let is_dup = kept.iter().any(|existing| {
            jaccard_similarity(&r.chunk.content, &existing.chunk.content) > threshold
        });

        if !is_dup {
            kept.push(r.clone());
        }
    }

    kept
}

/// Simple Jaccard similarity between two texts (word-level).
fn jaccard_similarity(a: &str, b: &str) -> f32 {
    let words_a: Vec<&str> = a.split_whitespace().collect();
    let words_b: Vec<&str> = b.split_whitespace().collect();

    if words_a.is_empty() || words_b.is_empty() {
        return 0.0;
    }

    let mut intersection = 0;
    for wa in &words_a {
        if words_b.contains(wa) {
            intersection += 1;
        }
    }

    let union = words_a.len() + words_b.len() - intersection;
    intersection as f32 / union as f32
}

/// Common English stop words.
fn is_stop_word(word: &str) -> bool {
    matches!(
        word,
        "the"
            | "and"
            | "that"
            | "this"
            | "with"
            | "for"
            | "from"
            | "are"
            | "was"
            | "were"
            | "been"
            | "have"
            | "has"
            | "had"
            | "not"
            | "but"
            | "they"
            | "them"
            | "their"
            | "what"
            | "when"
            | "where"
            | "which"
            | "will"
            | "would"
            | "could"
            | "should"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decompose_query() {
        let parts = decompose_query("What is Rust and how does it compare to Python?");
        assert!(!parts.is_empty());

        let simple = decompose_query("Rust");
        assert_eq!(simple.len(), 1);
    }

    #[test]
    fn test_jaccard() {
        let sim = jaccard_similarity("hello world", "hello world rust");
        assert!(sim > 0.5);
        assert!(sim < 1.0);

        let same = jaccard_similarity("hello world", "hello world");
        assert!((same - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_tree_decompose() {
        let tree = DecomposedQuery::build(
            "How does Rust's ownership model compare to Python's garbage collection?",
        );
        assert!(tree.root.is_some());
        assert!(tree.sub_queries.len() >= 2);
    }
}

// ── Tree-Structured Query Decomposition (DeepResearcher) ────────

/// Tree node in a decomposed query.
#[derive(Debug, Clone)]
pub struct QueryNode {
    /// Sub-query text
    pub query: String,
    /// Child nodes (deeper/specific sub-questions)
    pub children: Vec<QueryNode>,
    /// Depth in the tree (0 = root)
    pub depth: usize,
    /// Retrieved results for this node (filled during search)
    pub results: Vec<SearchResult>,
}

/// Decomposed query tree (RAGFlow's DeepResearcher equivalent).
pub struct DecomposedQuery {
    pub root: Option<QueryNode>,
    pub sub_queries: Vec<String>,
}

impl DecomposedQuery {
    /// Decompose a complex query into a tree of sub-queries.
    ///
    /// Strategy:
    /// - Identify comparison questions → split by "vs"/"compare"
    /// - Identify multi-part questions → split by "and"
    /// - Generate sub-questions for each aspect
    pub fn build(query: &str) -> Self {
        let mut sub_queries = Vec::new();

        // Level 0: original query as root
        sub_queries.push(query.to_string());

        // Level 1: split comparison/compound queries
        if let Some((a, b)) = split_comparison(query) {
            // Level 2: ask specifics about each side (clone before moving)
            sub_queries.push(format!("What are the details of: {}", a));
            sub_queries.push(format!("What are the details of: {}", b));
            sub_queries.push(a);
            sub_queries.push(b);
        } else {
            // Split on "and"/"or"
            for part in query
                .split(&['&', ','])
                .map(|s| s.trim().to_string())
                .filter(|s| s.len() > 5)
            {
                if part != query {
                    sub_queries.push(part);
                }
            }

            // Add aspect-based sub-queries
            if query.contains("how") || query.contains("How") {
                sub_queries.push(format!("What steps are involved in: {}", query));
            }
            if query.contains("why") || query.contains("Why") {
                sub_queries.push(format!("What are the reasons for: {}", query));
            }
        }

        Self {
            root: Some(QueryNode {
                query: query.to_string(),
                children: vec![],
                depth: 0,
                results: vec![],
            }),
            sub_queries,
        }
    }

    /// Execute the decomposed query tree against a search engine.
    pub fn execute<F>(
        &mut self,
        engine: &SearchEngine,
        embed_fn: &F,
        top_k: usize,
    ) -> crate::Result<Vec<SearchResult>>
    where
        F: Fn(&str) -> crate::Result<Vec<f32>>,
    {
        let mut all_results: Vec<SearchResult> = Vec::new();
        let mut seen = std::collections::HashSet::new();

        // Search each sub-query and collect results
        for sub_q in &self.sub_queries {
            if let Ok(embedding) = embed_fn(sub_q) {
                let results = engine.search(&embedding, top_k / 2);
                for r in results {
                    if r.score > 0.25 && seen.insert(r.chunk.id.clone()) {
                        all_results.push(r);
                    }
                }
            }
        }

        // Sort by score descending
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(all_results)
    }
}

/// Split a comparison query into two parts.
fn split_comparison(query: &str) -> Option<(String, String)> {
    let patterns = [" vs ", " versus ", " compare ", " and ", " differ from "];

    for pat in &patterns {
        if let Some(pos) = query.to_lowercase().find(pat) {
            let a = query[..pos].trim().to_string();
            let b = query[pos + pat.len()..].trim().to_string();
            if !a.is_empty() && !b.is_empty() && a.len() > 3 && b.len() > 3 {
                return Some((a, b));
            }
        }
    }
    None
}

// ── Tree-structured query decomposition retrieval ─────────────────
// Port of RAGFlow `rag/advanced_rag/tree_structured_query_decomposition_retrieval.py`
// (DeepResearchRetriever): decompose a complex question into sub-queries,
// retrieve for each, check sufficiency, and recursively drill down until
// the evidence is sufficient or the depth budget is exhausted.

use std::sync::Arc;

/// Verdict of the sufficiency checker (RAGFlow `SufficiencyVerdict`).
#[derive(Debug, Clone)]
pub struct SufficiencyVerdict {
    pub is_sufficient: bool,
    pub reasoning: String,
    pub missing_information: Vec<String>,
}

/// One follow-up question (RAGFlow `FollowUpQuestion`).
#[derive(Debug, Clone)]
pub struct FollowUpQuestion {
    pub question: String,
    pub query: String,
}

/// Follow-up plan produced by the multi-query generator
/// (RAGFlow `FollowUpPlan`).
#[derive(Debug, Clone)]
pub struct FollowUpPlan {
    pub reasoning: String,
    pub questions: Vec<FollowUpQuestion>,
}

/// Accumulated evidence across a deep-research run (RAGFlow
/// `ResearchChunkInfo`): unique chunks plus per-document aggregation.
#[derive(Debug, Clone, Default)]
pub struct ResearchChunkInfo {
    pub total: usize,
    pub chunks: Vec<crate::search::SearchResult>,
    pub doc_aggs: Vec<crate::search::DocumentAggregation>,
}

impl ResearchChunkInfo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Merge chunks in, deduplicating by chunk id, and refresh document
    /// aggregation counts (RAGFlow `merge_chunk_info`).
    pub fn merge(&mut self, chunks: &[crate::search::SearchResult]) {
        for chunk in chunks {
            if !self.chunks.iter().any(|c| c.chunk.id == chunk.chunk.id) {
                self.chunks.push(chunk.clone());
            }
        }
        let mut counts: std::collections::HashMap<String, (String, usize)> =
            std::collections::HashMap::new();
        for chunk in &self.chunks {
            let entry = counts
                .entry(chunk.chunk.id.clone())
                .or_insert_with(|| (chunk.chunk.doc_name.clone(), 0));
            entry.1 += 1;
        }
        self.doc_aggs = counts
            .into_iter()
            .map(
                |(doc_id, (doc_name, count))| crate::search::DocumentAggregation {
                    doc_name,
                    doc_id,
                    count,
                },
            )
            .collect();
        self.total = self.chunks.len();
    }
}

/// Outcome of one (recursive) research step: aggregated text plus every
/// chunk retrieved anywhere under this subtree.
#[derive(Debug, Clone, Default)]
pub struct ResearchOutcome {
    pub text: String,
    pub chunks: Vec<crate::search::SearchResult>,
}

/// Format retrieved chunks as the prompt evidence block (RAGFlow
/// `kb_prompt`, simplified): numbered, content truncated to a char budget.
pub fn kb_prompt(
    results: &[crate::search::SearchResult],
    max_chunks: usize,
    max_chars_per_chunk: usize,
) -> String {
    let mut out = String::new();
    for (i, result) in results.iter().take(max_chunks).enumerate() {
        let content: String = result
            .chunk
            .content
            .chars()
            .take(max_chars_per_chunk)
            .collect();
        out.push_str(&format!("[{}] {}\n", i + 1, content));
    }
    out
}

/// Extract the first JSON object/array from an LLM response, tolerating
/// markdown fences and surrounding prose.
fn parse_llm_json(text: &str) -> Option<serde_json::Value> {
    for open in ['{', '['] {
        let close = if open == '{' { '}' } else { ']' };
        if let Some(start) = text.find(open)
            && let Some(end) = text[start..].rfind(close) {
                let candidate = &text[start..start + end + 1];
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(candidate) {
                    return Some(value);
                }
            }
    }
    None
}

/// Tree-structured deep-research retriever (RAGFlow `DeepResearchRetriever`).
pub struct DeepResearchRetriever {
    chat: Arc<dyn crate::llm::ChatModel>,
    retrieve: Arc<dyn Fn(&str) -> anyhow::Result<Vec<crate::search::SearchResult>> + Send + Sync>,
    /// Maximum recursion depth for follow-up questions.
    pub max_depth: usize,
    /// Maximum number of chunks fed to the sufficiency/query LLMs.
    pub max_chunks_in_prompt: usize,
    /// Maximum characters per chunk in the evidence block.
    pub max_chars_per_chunk: usize,
    sufficiency_prompt: String,
    followup_prompt: String,
}

impl DeepResearchRetriever {
    pub fn new(
        chat: Arc<dyn crate::llm::ChatModel>,
        retrieve: impl Fn(&str) -> anyhow::Result<Vec<crate::search::SearchResult>>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            chat,
            retrieve: Arc::new(retrieve),
            max_depth: 2,
            max_chunks_in_prompt: 32,
            max_chars_per_chunk: 1200,
            sufficiency_prompt: DEFAULT_SUFFICIENCY_PROMPT.to_string(),
            followup_prompt: DEFAULT_FOLLOWUP_PROMPT.to_string(),
        }
    }

    pub fn with_prompts(mut self, sufficiency: &str, followup: &str) -> Self {
        self.sufficiency_prompt = sufficiency.to_string();
        self.followup_prompt = followup.to_string();
        self
    }

    /// Ask the LLM whether the retrieved evidence answers `question`.
    async fn check_sufficiency(
        &self,
        question: &str,
        evidence: &str,
    ) -> anyhow::Result<SufficiencyVerdict> {
        let prompt = self
            .sufficiency_prompt
            .replace("{question}", question)
            .replace("{retrieved_docs}", evidence);
        let answer = self.chat.chat(&prompt, &[]).await?;
        let json = parse_llm_json(&answer)
            .ok_or_else(|| anyhow::anyhow!("sufficiency check returned no JSON: {answer}"))?;
        Ok(SufficiencyVerdict {
            is_sufficient: json["is_sufficient"].as_bool().unwrap_or(false),
            reasoning: json["reasoning"].as_str().unwrap_or_default().to_string(),
            missing_information: json["missing_information"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Generate follow-up questions for the missing information.
    async fn gen_followups(
        &self,
        question: &str,
        query: &str,
        missing: &[String],
        evidence: &str,
    ) -> anyhow::Result<FollowUpPlan> {
        let prompt = self
            .followup_prompt
            .replace("{question}", question)
            .replace("{query}", query)
            .replace("{missing_information}", &missing.join("\n"))
            .replace("{retrieved_docs}", evidence);
        let answer = self.chat.chat(&prompt, &[]).await?;
        let json = parse_llm_json(&answer)
            .ok_or_else(|| anyhow::anyhow!("follow-up generation returned no JSON: {answer}"))?;
        let questions = json["queries"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|q| {
                        let question = q["question"].as_str()?;
                        let query = q["query"].as_str().unwrap_or(question);
                        Some(FollowUpQuestion {
                            question: question.to_string(),
                            query: query.to_string(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(FollowUpPlan {
            reasoning: json["reasoning"].as_str().unwrap_or_default().to_string(),
            questions,
        })
    }

    /// Recursive research: retrieve → check sufficiency → decompose →
    /// recurse on sub-queries (RAGFlow `DeepResearchRetriever.research`).
    pub async fn research(
        &self,
        question: &str,
        query: &str,
        depth: usize,
    ) -> anyhow::Result<ResearchOutcome> {
        if depth == 0 {
            return Ok(ResearchOutcome::default());
        }
        let results = (self.retrieve)(query)?;
        let evidence = kb_prompt(
            &results,
            self.max_chunks_in_prompt,
            self.max_chars_per_chunk,
        );
        let mut outcome = ResearchOutcome {
            text: evidence.clone(),
            chunks: results,
        };
        let verdict = self.check_sufficiency(question, &evidence).await?;
        if !verdict.is_sufficient {
            let plan = self
                .gen_followups(question, query, &verdict.missing_information, &evidence)
                .await?;
            let futures: Vec<_> = plan
                .questions
                .iter()
                .map(|q| self.research(&q.question, &q.query, depth - 1))
                .collect();
            let results = futures_util::future::join_all(futures).await;
            for sub in results.into_iter().flatten() {
                if !sub.text.trim().is_empty() {
                    outcome.text.push('\n');
                    outcome.text.push_str(&sub.text);
                }
                outcome.chunks.extend(sub.chunks);
            }
        }
        Ok(outcome)
    }

    /// One-shot deep research: runs the recursion and merges all evidence
    /// into a `ResearchChunkInfo` (RAGFlow `DeepResearcher.astream` flow).
    pub async fn deep_research(
        &self,
        question: &str,
        query: &str,
    ) -> anyhow::Result<(ResearchOutcome, ResearchChunkInfo)> {
        let outcome = self.research(question, query, self.max_depth).await?;
        let mut info = ResearchChunkInfo::new();
        info.merge(&outcome.chunks);
        Ok((outcome, info))
    }
}

/// Default sufficiency-check prompt, ported from
/// `rag/prompts/sufficiency_check.md`.
pub const DEFAULT_SUFFICIENCY_PROMPT: &str = r#"You are an expert research assistant. Your task is to determine whether the retrieved documents are sufficient to answer the user's question.

Guidelines:
- If the documents contain enough information to fully answer the question, set "is_sufficient" to true.
- If information is missing or incomplete, set "is_sufficient" to false and list the specific missing information.
- Base your decision solely on the provided documents, not on external knowledge.

Respond in JSON format only:
{"is_sufficient": true/false, "reasoning": "...", "missing_information": ["..."]}

Question: {question}

Retrieved Documents:
{retrieved_docs}"#;

/// Default follow-up question generator, ported from
/// `rag/prompts/multi_queries_gen.md`.
pub const DEFAULT_FOLLOWUP_PROMPT: &str = r#"You are an expert research assistant. The retrieved documents do not fully answer the original question. Generate follow-up questions to gather the missing information.

Guidelines:
- Generate 1-3 follow-up questions that target the missing information.
- Each question must be a concrete search query that can be used to retrieve more documents.
- Questions should be independent and cover different aspects of the missing information.

Respond in JSON format only:
{"reasoning": "...", "queries": [{"question": "...", "query": "..."}]}

Original Question: {question}
Current Query: {query}
Missing Information: {missing_information}
Retrieved Documents:
{retrieved_docs}"#;

#[cfg(test)]
mod deep_research_tests {
    use super::*;
    use crate::llm::{ChatMessage, ChatModel};
    use crate::search::{IndexedChunk, SearchResult};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeChatModel {
        calls: Arc<AtomicUsize>,
        sufficient_after: usize,
    }

    #[async_trait::async_trait]
    impl ChatModel for FakeChatModel {
        fn model_name(&self) -> &str {
            "fake-chat"
        }
        async fn chat(&self, system: &str, _history: &[ChatMessage]) -> anyhow::Result<String> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if system.contains("sufficient") {
                if n <= self.sufficient_after {
                    Ok(r#"{"is_sufficient": false, "reasoning": "need more", "missing_information": ["more details"]}"#.into())
                } else {
                    Ok(r#"{"is_sufficient": true, "reasoning": "enough", "missing_information": []}"#.into())
                }
            } else {
                Ok(r#"{"reasoning": "split", "queries": [{"question": "sub q1", "query": "sub query 1"}, {"question": "sub q2", "query": "sub query 2"}]}"#.into())
            }
        }
        async fn chat_stream(
            &self,
            _system: &str,
            _history: &[ChatMessage],
            _on_chunk: Box<dyn for<'a> FnMut(&'a str) + Send>,
        ) -> anyhow::Result<String> {
            Ok(String::new())
        }
    }

    fn fake_retrieve(
        calls: Arc<AtomicUsize>,
    ) -> impl Fn(&str) -> anyhow::Result<Vec<SearchResult>> {
        move |query: &str| {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(vec![SearchResult {
                chunk: IndexedChunk {
                    id: format!("c{n}"),
                    doc_name: format!("doc{n}"),
                    content: format!("content for {query} ({n})"),
                    embedding: vec![],
                    token_count: 0,
                    position: 0,
                    metadata: Default::default(),
                },
                score: 0.9,
                rank: 0,
            }])
        }
    }

    #[tokio::test]
    async fn sufficient_evidence_stops_after_one_level() {
        let chat_calls = Arc::new(AtomicUsize::new(0));
        let ret_calls = Arc::new(AtomicUsize::new(0));
        let chat: Arc<dyn ChatModel> = Arc::new(FakeChatModel {
            calls: chat_calls.clone(),
            sufficient_after: 0,
        });
        let retriever = DeepResearchRetriever::new(chat, fake_retrieve(ret_calls.clone()));
        let (outcome, info) = retriever
            .deep_research("what is x", "x")
            .await
            .expect("deep research runs");
        assert_eq!(
            chat_calls.load(Ordering::SeqCst),
            1,
            "only sufficiency check"
        );
        assert_eq!(ret_calls.load(Ordering::SeqCst), 1, "only one retrieval");
        assert!(outcome.text.contains("content for x"));
        assert_eq!(info.total, 1);
        assert_eq!(info.doc_aggs.len(), 1);
    }

    #[tokio::test]
    async fn insufficient_evidence_recurses_into_followups() {
        let chat_calls = Arc::new(AtomicUsize::new(0));
        let ret_calls = Arc::new(AtomicUsize::new(0));
        let chat: Arc<dyn ChatModel> = Arc::new(FakeChatModel {
            calls: chat_calls.clone(),
            sufficient_after: 1,
        });
        let retriever = DeepResearchRetriever::new(chat, fake_retrieve(ret_calls.clone()));
        let (outcome, info) = retriever
            .deep_research("what is x", "x")
            .await
            .expect("deep research runs");
        // 1 sufficiency (insufficient) + 1 followup gen + 2 sufficiency checks
        assert_eq!(chat_calls.load(Ordering::SeqCst), 4);
        // 1 root retrieval + 2 follow-up retrievals, deduplicated
        assert_eq!(ret_calls.load(Ordering::SeqCst), 3);
        assert_eq!(info.total, 3);
        assert!(outcome.text.contains("content for sub query 1"));
        assert!(outcome.text.contains("content for sub query 2"));
    }

    #[test]
    fn kb_prompt_numbers_and_truncates_chunks() {
        let results = vec![
            SearchResult {
                chunk: IndexedChunk {
                    id: "a".into(),
                    doc_name: "d".into(),
                    content: "hello world".into(),
                    embedding: vec![],
                    token_count: 0,
                    position: 0,
                    metadata: Default::default(),
                },
                score: 1.0,
                rank: 0,
            },
            SearchResult {
                chunk: IndexedChunk {
                    id: "b".into(),
                    doc_name: "d".into(),
                    content: "second chunk".into(),
                    embedding: vec![],
                    token_count: 0,
                    position: 0,
                    metadata: Default::default(),
                },
                score: 0.5,
                rank: 1,
            },
        ];
        let prompt = kb_prompt(&results, 1, 5);
        assert_eq!(prompt, "[1] hello\n");
        let all = kb_prompt(&results, 10, 1000);
        assert!(all.contains("[1] hello world"));
        assert!(all.contains("[2] second chunk"));
    }

    #[test]
    fn parse_llm_json_handles_markdown_fences() {
        let raw = "Here you go:\n```json\n{\"is_sufficient\": true, \"reasoning\": \"ok\", \"missing_information\": []}\n```";
        let json = parse_llm_json(raw).expect("json extracted");
        assert_eq!(json["is_sufficient"], true);
        let arr = parse_llm_json("[1, 2, 3] trailing");
        assert_eq!(arr.unwrap()[1], 2);
        assert!(parse_llm_json("no json here").is_none());
    }
}
