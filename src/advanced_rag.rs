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
