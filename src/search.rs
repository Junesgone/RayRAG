//! Vector search module — cosine similarity retrieval over embedded chunks.
//!
//! Implements the search functionality that replaces RAGFlow's ES retrieval.
//! Stores chunks in-memory or loads from a JSON index file.
//!
//! CLI: `rayrag search --query "hello" --index index.json`

use crate::nlp::Bm25Scorer;
use crate::{Chunk, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DocumentAggregation {
    pub doc_name: String,
    pub doc_id: String,
    pub count: usize,
}

/// A search result with score.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    /// The matched chunk
    pub chunk: IndexedChunk,
    /// Cosine similarity score (0.0–1.0, higher is better)
    pub score: f32,
    /// Rank position (0-based)
    pub rank: usize,
}

/// A search result after term/vector score fusion.
#[derive(Debug, Clone, Serialize)]
pub struct HybridSearchResult {
    /// The matched chunk.
    pub chunk: IndexedChunk,
    /// Final weighted score.
    pub score: f32,
    /// Cosine similarity score.
    pub vector_score: f32,
    /// Normalized BM25 term score.
    pub term_score: f32,
    /// Normalized lexical score used by RAGFlow's external-model branch.
    pub model_term_score: f32,
    /// RAGFlow-compatible tag cosine boost plus chunk pagerank.
    pub rank_feature_score: f32,
    /// Rank position (0-based).
    pub rank: usize,
}

pub struct HybridSearchQuery<'a> {
    pub query: &'a str,
    pub query_embedding: Option<&'a [f32]>,
    pub top_k: usize,
    pub kb_ids: &'a [String],
    pub vector_weight: f32,
    pub doc_ids: Option<&'a [String]>,
    pub rank_feature: Option<&'a HashMap<String, f32>>,
}

/// A chunk stored in the search index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedChunk {
    /// Chunk ID
    pub id: String,
    /// Document name
    pub doc_name: String,
    /// Content text
    pub content: String,
    /// Embedding vector (384-dim for all-MiniLM-L6-v2)
    pub embedding: Vec<f32>,
    /// Token count
    pub token_count: usize,
    /// Position in document
    pub position: usize,
    /// Extra metadata
    pub metadata: HashMap<String, String>,
}

impl From<Chunk> for IndexedChunk {
    fn from(chunk: Chunk) -> Self {
        Self {
            id: chunk.id,
            doc_name: String::new(),
            content: chunk.content,
            embedding: chunk.embedding.unwrap_or_default(),
            token_count: chunk.token_count,
            position: chunk.position,
            metadata: chunk.metadata,
        }
    }
}

/// Simple in-memory vector search engine.
///
/// Stores chunks and performs cosine similarity search against
/// a query embedding vector.
pub struct SearchEngine {
    chunks: Vec<IndexedChunk>,
}

pub fn aggregate_documents(results: &[HybridSearchResult]) -> Vec<DocumentAggregation> {
    let mut counts: HashMap<String, (String, usize)> = HashMap::new();
    for result in results {
        let doc_name = result.chunk.doc_name.clone();
        let doc_id = result
            .chunk
            .metadata
            .get("doc_id")
            .cloned()
            .unwrap_or_default();
        let entry = counts.entry(doc_name).or_insert((doc_id, 0));
        entry.1 += 1;
    }
    let mut aggregations: Vec<DocumentAggregation> = counts
        .into_iter()
        .map(|(doc_name, (doc_id, count))| DocumentAggregation {
            doc_name,
            doc_id,
            count,
        })
        .collect();
    aggregations.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.doc_name.cmp(&b.doc_name))
    });
    aggregations
}

pub fn highlight_content(content: &str, query: &str) -> String {
    let escaped = escape_html(content);
    let terms: Vec<&str> = query
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .collect();
    let pattern = terms
        .iter()
        .map(|term| regex::escape(term))
        .collect::<Vec<_>>()
        .join("|");
    if pattern.is_empty() {
        return escaped;
    }
    regex::RegexBuilder::new(&format!("({pattern})"))
        .case_insensitive(true)
        .build()
        .map(|regex| regex.replace_all(&escaped, "<em>$1</em>").into_owned())
        .unwrap_or(escaped)
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

impl SearchEngine {
    /// Create a new empty search engine.
    pub fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    /// Restore an engine from an already validated chunk snapshot.
    pub fn from_chunks(chunks: Vec<IndexedChunk>) -> Self {
        Self { chunks }
    }

    /// Load chunks from a JSON index file.
    pub fn from_file(path: &str) -> Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        let data = std::fs::read_to_string(path)?;
        let chunks: Vec<IndexedChunk> = serde_json::from_str(&data)?;
        tracing::info!("Loaded {} chunks from {}", chunks.len(), path);
        Ok(Self { chunks })
    }

    /// Add a chunk to the index.
    pub fn add(&mut self, chunk: IndexedChunk) {
        self.chunks.push(chunk);
    }

    /// Index multiple chunks at once.
    pub fn index(&mut self, chunks: Vec<IndexedChunk>) {
        self.chunks.extend(chunks);
    }

    /// Remove all chunks belonging to a document.
    pub fn remove_document(&mut self, doc_id: &str) -> usize {
        let before = self.chunks.len();
        self.chunks
            .retain(|chunk| chunk.metadata.get("doc_id").map(String::as_str) != Some(doc_id));
        before - self.chunks.len()
    }

    /// Remove a chunk by its stable primary key.
    pub fn remove_chunk(&mut self, chunk_id: &str) -> bool {
        let before = self.chunks.len();
        self.chunks.retain(|chunk| chunk.id != chunk_id);
        self.chunks.len() != before
    }

    /// Replace all chunks belonging to a document atomically in memory.
    pub fn replace_document(&mut self, doc_id: &str, chunks: Vec<IndexedChunk>) {
        self.remove_document(doc_id);
        self.index(chunks);
    }

    /// Number of indexed chunks.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Get a copy of all indexed chunks.
    pub fn to_vec(&self) -> Vec<IndexedChunk> {
        self.chunks.clone()
    }

    /// Adjust one chunk's persisted pagerank feature within the supported range.
    pub fn adjust_chunk_pagerank(
        &mut self,
        chunk_id: &str,
        kb_id: &str,
        delta: i32,
    ) -> Option<(f32, f32)> {
        let chunk = self.chunks.iter_mut().find(|chunk| {
            chunk.id == chunk_id && chunk.metadata.get("kb_id").map(String::as_str) == Some(kb_id)
        })?;
        let previous = chunk
            .metadata
            .get("pagerank_fea")
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(0.0);
        let updated = (previous + delta as f32).clamp(0.0, 100.0);
        if updated == 0.0 {
            chunk.metadata.remove("pagerank_fea");
        } else {
            chunk
                .metadata
                .insert("pagerank_fea".into(), updated.to_string());
        }
        Some((previous, updated))
    }

    /// Check if index is empty.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Save index to a JSON file.
    pub fn save(&self, path: &str) -> Result<()> {
        let data = serde_json::to_vec_pretty(&self.chunks)?;
        let path = std::path::Path::new(path);
        crate::persistence::atomic_write(path, &data)?;
        tracing::info!("Saved {} chunks to {}", self.chunks.len(), path.display());
        Ok(())
    }

    /// Search for chunks similar to the query embedding.
    ///
    /// Uses cosine similarity: score = dot(query, chunk) / (|query| * |chunk|)
    pub fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<SearchResult> {
        self.search_filtered(query_embedding, top_k, None)
    }

    /// Search while enforcing knowledge-base isolation.
    pub fn search_kbs(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        kb_ids: &[String],
    ) -> Vec<SearchResult> {
        self.search_filtered(query_embedding, top_k, Some(kb_ids))
    }

    /// Hybrid retrieval matching RAGFlow's term/vector weighted rerank.
    ///
    /// `vector_weight=0` performs term-only retrieval and does not require a
    /// query embedding. BM25 scores are normalized by the strongest candidate
    /// so they can be blended with cosine similarity on a common scale.
    pub fn hybrid_search_kbs(&self, request: HybridSearchQuery<'_>) -> Vec<HybridSearchResult> {
        let HybridSearchQuery {
            query,
            query_embedding,
            top_k,
            kb_ids,
            vector_weight,
            doc_ids,
            rank_feature,
        } = request;
        if query.trim().is_empty() || top_k == 0 || kb_ids.is_empty() {
            return Vec::new();
        }

        let vector_weight = vector_weight.clamp(0.0, 1.0);
        let term_weight = 1.0 - vector_weight;
        let candidates: Vec<&IndexedChunk> = self
            .chunks
            .iter()
            .filter(|chunk| {
                chunk_available(chunk)
                    && chunk
                        .metadata
                        .get("kb_id")
                        .is_some_and(|kb_id| kb_ids.iter().any(|id| id == kb_id))
                    && doc_ids.is_none_or(|ids| {
                        chunk
                            .metadata
                            .get("doc_id")
                            .is_some_and(|doc_id| ids.iter().any(|id| id == doc_id))
                    })
            })
            .collect();
        if candidates.is_empty() {
            return Vec::new();
        }

        let body_scores = field_bm25_scores(query, &candidates, |chunk| &chunk.content);
        let title_scores = field_bm25_scores(query, &candidates, |chunk| &chunk.doc_name);
        let keyword_scores = field_bm25_scores(query, &candidates, |chunk| {
            chunk
                .metadata
                .get("important_kwd")
                .map(String::as_str)
                .unwrap_or("")
        });
        let question_scores = field_bm25_scores(query, &candidates, |chunk| {
            chunk
                .metadata
                .get("question_tks")
                .map(String::as_str)
                .unwrap_or("")
        });
        let raw_term_scores: Vec<f32> = (0..candidates.len())
            .map(|index| {
                body_scores[index]
                    + 2.0 * title_scores[index]
                    + 5.0 * keyword_scores[index]
                    + 6.0 * question_scores[index]
            })
            .collect();
        let raw_model_term_scores: Vec<f32> = (0..candidates.len())
            .map(|index| body_scores[index] + title_scores[index] + keyword_scores[index])
            .collect();
        let max_term_score = raw_term_scores.iter().copied().fold(0.0_f32, f32::max);
        let max_model_term_score = raw_model_term_scores
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);

        let query_vector = query_embedding.filter(|embedding| !embedding.is_empty());
        let query_norm = query_vector.map(l2_norm).unwrap_or(0.0);
        let mut results: Vec<HybridSearchResult> = candidates
            .into_iter()
            .zip(raw_term_scores)
            .zip(raw_model_term_scores)
            .filter_map(|((chunk, raw_term_score), raw_model_term_score)| {
                let term_score = if max_term_score > 0.0 {
                    raw_term_score / max_term_score
                } else {
                    0.0
                };
                let model_term_score = if max_model_term_score > 0.0 {
                    raw_model_term_score / max_model_term_score
                } else {
                    0.0
                };
                let vector_score = query_vector
                    .filter(|embedding| {
                        query_norm > 0.0 && embedding.len() == chunk.embedding.len()
                    })
                    .map(|embedding| cosine_similarity(embedding, &chunk.embedding, query_norm))
                    .unwrap_or(0.0);
                let rank_feature_score = rank_feature_score(chunk, rank_feature);
                let score =
                    term_weight * term_score + vector_weight * vector_score + rank_feature_score;
                (score > 0.0).then(|| HybridSearchResult {
                    chunk: chunk.clone(),
                    score,
                    vector_score,
                    term_score,
                    model_term_score,
                    rank_feature_score,
                    rank: 0,
                })
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.chunk.id.cmp(&b.chunk.id))
        });
        results.truncate(top_k);
        for (rank, result) in results.iter_mut().enumerate() {
            result.rank = rank;
        }
        results
    }

    fn search_filtered(
        &self,
        query_embedding: &[f32],
        top_k: usize,
        kb_ids: Option<&[String]>,
    ) -> Vec<SearchResult> {
        if self.chunks.is_empty() || query_embedding.is_empty() {
            return Vec::new();
        }

        let query_norm = l2_norm(query_embedding);
        if query_norm == 0.0 {
            return Vec::new();
        }

        let mut results: Vec<SearchResult> = self
            .chunks
            .iter()
            .filter(|chunk| {
                chunk_available(chunk)
                    && kb_ids.is_none_or(|ids| {
                        chunk
                            .metadata
                            .get("kb_id")
                            .is_some_and(|kb_id| ids.iter().any(|id| id == kb_id))
                    })
            })
            .filter_map(|chunk| {
                if chunk.embedding.is_empty() || chunk.embedding.len() != query_embedding.len() {
                    return None;
                }
                let score = cosine_similarity(query_embedding, &chunk.embedding, query_norm);
                if score == 0.0 && l2_norm(&chunk.embedding) == 0.0 {
                    return None;
                }

                Some(SearchResult {
                    chunk: chunk.clone(),
                    score,
                    rank: 0, // Will be set after sorting
                })
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Truncate to top_k and set ranks
        results.truncate(top_k);
        for (i, result) in results.iter_mut().enumerate() {
            result.rank = i;
        }

        results
    }
}

fn chunk_available(chunk: &IndexedChunk) -> bool {
    chunk
        .metadata
        .get("available_int")
        .map(String::as_str)
        .is_none_or(|value| value != "0" && !value.eq_ignore_ascii_case("false"))
}

fn rank_feature_score(chunk: &IndexedChunk, query_features: Option<&HashMap<String, f32>>) -> f32 {
    let pagerank = chunk
        .metadata
        .get("pagerank_fea")
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(0.0);
    let Some(query_features) = query_features else {
        return pagerank;
    };
    let query_norm = query_features
        .iter()
        .filter(|(tag, score)| tag.as_str() != "pagerank_fea" && score.is_finite())
        .map(|(_, score)| score * score)
        .sum::<f32>()
        .sqrt();
    if query_norm == 0.0 {
        return pagerank;
    }

    let tag_features = chunk
        .metadata
        .get("tag_feas")
        .and_then(|raw| serde_json::from_str::<HashMap<String, f32>>(raw).ok())
        .unwrap_or_default();
    let mut dot = 0.0;
    let mut document_norm_squared = 0.0;
    for (tag, score) in tag_features {
        if !score.is_finite() {
            continue;
        }
        document_norm_squared += score * score;
        if let Some(query_score) = query_features.get(&tag).filter(|value| value.is_finite()) {
            dot += query_score * score;
        }
    }
    if document_norm_squared == 0.0 {
        pagerank
    } else {
        pagerank + 10.0 * dot / document_norm_squared.sqrt() / query_norm
    }
}

impl Default for SearchEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod pagerank_adjustment_tests {
    use super::*;

    fn chunk(id: &str, kb_id: &str, pagerank: Option<&str>) -> IndexedChunk {
        let mut metadata = HashMap::from([("kb_id".into(), kb_id.into())]);
        if let Some(pagerank) = pagerank {
            metadata.insert("pagerank_fea".into(), pagerank.into());
        }
        IndexedChunk {
            id: id.into(),
            doc_name: "document.txt".into(),
            content: "content".into(),
            embedding: vec![],
            token_count: 1,
            position: 0,
            metadata,
        }
    }

    #[test]
    fn pagerank_adjustment_is_scoped_and_clamped() {
        let mut engine = SearchEngine::from_chunks(vec![
            chunk("shared", "kb-a", Some("100")),
            chunk("shared", "kb-b", Some("invalid")),
        ]);

        assert_eq!(
            engine.adjust_chunk_pagerank("shared", "kb-a", 1),
            Some((100.0, 100.0))
        );
        assert_eq!(
            engine.adjust_chunk_pagerank("shared", "kb-b", -1),
            Some((0.0, 0.0))
        );
        assert_eq!(engine.adjust_chunk_pagerank("missing", "kb-a", 1), None);
        assert_eq!(engine.to_vec()[1].metadata.get("pagerank_fea"), None);
    }
}

/// Compute the L2 (Euclidean) norm of a vector.
fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// Compute the dot product of two vectors.
fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn cosine_similarity(query: &[f32], document: &[f32], query_norm: f32) -> f32 {
    let document_norm = l2_norm(document);
    if query_norm == 0.0 || document_norm == 0.0 {
        return 0.0;
    }
    (dot_product(query, document) / (query_norm * document_norm)).clamp(-1.0, 1.0)
}

fn field_bm25_scores<'a>(
    query: &str,
    candidates: &[&'a IndexedChunk],
    field: impl Fn(&'a IndexedChunk) -> &'a str,
) -> Vec<f32> {
    let documents: Vec<&str> = candidates.iter().map(|chunk| field(chunk)).collect();
    let mut bm25 = Bm25Scorer::new();
    bm25.index(&documents);
    // RAGFlow retrieval weights query terms (term_weight.py Dealer.weights):
    // entities/locations/numbers dominate, stop words drop out. Tokens not
    // in the map keep weight 1.0, so behavior degrades gracefully.
    let tw = crate::nlp::TermWeightComputer::new();
    let query_weights: Vec<(String, f64)> = tw
        .weights(&[query.to_string()], true)
        .into_iter()
        .map(|(t, w)| (t, w))
        .collect();
    let weight_refs: Vec<(&str, f64)> = query_weights
        .iter()
        .map(|(t, w)| (t.as_str(), *w))
        .collect();
    documents
        .iter()
        .map(|document| bm25.score_weighted(query, document, &weight_refs))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((dot_product(&a, &b) - 1.0).abs() < 1e-6);

        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((dot_product(&a, &b) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_search() {
        let mut engine = SearchEngine::new();
        engine.add(IndexedChunk {
            id: "1".into(),
            doc_name: "test".into(),
            content: "Rust is great".into(),
            embedding: vec![1.0, 0.0, 0.0],
            token_count: 3,
            position: 0,
            metadata: HashMap::new(),
        });
        engine.add(IndexedChunk {
            id: "2".into(),
            doc_name: "test".into(),
            content: "Python is ok".into(),
            embedding: vec![0.0, 1.0, 0.0],
            token_count: 3,
            position: 1,
            metadata: HashMap::new(),
        });

        // Query: "Rust" → embedding similar to [1,0,0]
        let results = engine.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(results.len(), 2);
        assert!((results[0].score - 1.0).abs() < 1e-6);
        assert_eq!(results[0].chunk.content, "Rust is great");
    }
    #[test]
    fn test_search_enforces_kb_filter() {
        let mut engine = SearchEngine::new();
        for (id, kb_id) in [("1", "kb-a"), ("2", "kb-b")] {
            let mut metadata = HashMap::new();
            metadata.insert("kb_id".into(), kb_id.into());
            engine.add(IndexedChunk {
                id: id.into(),
                doc_name: "test".into(),
                content: format!("content-{kb_id}"),
                embedding: vec![1.0, 0.0],
                token_count: 1,
                position: 0,
                metadata,
            });
        }
        let results = engine.search_kbs(&[1.0, 0.0], 10, &["kb-b".into()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk.metadata.get("kb_id").unwrap(), "kb-b");
    }

    #[test]
    fn hybrid_search_combines_term_and_vector_scores() {
        let mut engine = SearchEngine::new();
        for (id, content, embedding) in [
            ("term", "rust ownership borrowing", vec![0.0, 1.0]),
            ("vector", "python packaging", vec![1.0, 0.0]),
        ] {
            let mut metadata = HashMap::new();
            metadata.insert("kb_id".into(), "kb-a".into());
            engine.add(IndexedChunk {
                id: id.into(),
                doc_name: "test".into(),
                content: content.into(),
                embedding,
                token_count: 2,
                position: 0,
                metadata,
            });
        }

        let kb_ids = vec!["kb-a".into()];
        let term_first = engine.hybrid_search_kbs(HybridSearchQuery {
            query: "rust ownership",
            query_embedding: Some(&[1.0, 0.0]),
            top_k: 2,
            kb_ids: &kb_ids,
            vector_weight: 0.3,
            doc_ids: None,
            rank_feature: None,
        });
        assert_eq!(term_first[0].chunk.id, "term");
        assert!(term_first[0].term_score > term_first[1].term_score);
        assert!(term_first[1].vector_score > term_first[0].vector_score);

        let vector_first = engine.hybrid_search_kbs(HybridSearchQuery {
            query: "rust ownership",
            query_embedding: Some(&[1.0, 0.0]),
            top_k: 2,
            kb_ids: &kb_ids,
            vector_weight: 0.8,
            doc_ids: None,
            rank_feature: None,
        });
        assert_eq!(vector_first[0].chunk.id, "vector");
    }

    #[test]
    fn hybrid_search_supports_term_only_without_embedding() {
        let mut engine = SearchEngine::new();
        for (id, content) in [
            ("match", "aquaculture water quality"),
            ("other", "server logs"),
        ] {
            let mut metadata = HashMap::new();
            metadata.insert("kb_id".into(), "kb-a".into());
            engine.add(IndexedChunk {
                id: id.into(),
                doc_name: "test".into(),
                content: content.into(),
                embedding: Vec::new(),
                token_count: 2,
                position: 0,
                metadata,
            });
        }

        let kb_ids = vec!["kb-a".into()];
        let results = engine.hybrid_search_kbs(HybridSearchQuery {
            query: "water quality",
            query_embedding: None,
            top_k: 2,
            kb_ids: &kb_ids,
            vector_weight: 0.0,
            doc_ids: None,
            rank_feature: None,
        });
        assert_eq!(results[0].chunk.id, "match");
        assert_eq!(results[0].vector_score, 0.0);
        assert_eq!(results[0].term_score, 1.0);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn vector_search_rejects_dimension_mismatch() {
        let mut engine = SearchEngine::new();
        engine.add(IndexedChunk {
            id: "mismatch".into(),
            doc_name: "test".into(),
            content: "content".into(),
            embedding: vec![1.0],
            token_count: 1,
            position: 0,
            metadata: HashMap::new(),
        });
        assert!(engine.search(&[1.0, 0.0], 1).is_empty());
    }

    #[test]
    fn unavailable_chunks_are_hidden_from_vector_and_hybrid_search() {
        let mut chunk = IndexedChunk {
            id: "disabled".into(),
            doc_name: "disabled.txt".into(),
            content: "water quality".into(),
            embedding: vec![1.0, 0.0],
            token_count: 2,
            position: 0,
            metadata: HashMap::from([
                ("kb_id".into(), "kb-a".into()),
                ("available_int".into(), "0".into()),
            ]),
        };
        let mut engine = SearchEngine::new();
        engine.add(chunk.clone());
        assert!(
            engine
                .search_kbs(&[1.0, 0.0], 10, &["kb-a".into()])
                .is_empty()
        );
        assert!(
            engine
                .hybrid_search_kbs(HybridSearchQuery {
                    query: "water quality",
                    query_embedding: None,
                    top_k: 10,
                    kb_ids: &["kb-a".into()],
                    vector_weight: 0.0,
                    doc_ids: None,
                    rank_feature: None,
                })
                .is_empty()
        );

        chunk.metadata.insert("available_int".into(), "1".into());
        let engine = SearchEngine::from_chunks(vec![chunk]);
        assert_eq!(
            engine.search_kbs(&[1.0, 0.0], 10, &["kb-a".into()]).len(),
            1
        );
        assert_eq!(
            engine
                .hybrid_search_kbs(HybridSearchQuery {
                    query: "water quality",
                    query_embedding: None,
                    top_k: 10,
                    kb_ids: &["kb-a".into()],
                    vector_weight: 0.0,
                    doc_ids: None,
                    rank_feature: None,
                })
                .len(),
            1
        );
    }

    #[test]
    fn hybrid_search_applies_ragflow_field_weights() {
        let mut engine = SearchEngine::new();
        for (id, title, keyword) in [
            ("title", "water quality", None),
            ("keyword", "unrelated", Some("water quality")),
            ("body", "unrelated", None),
        ] {
            let mut metadata = HashMap::new();
            metadata.insert("kb_id".into(), "kb-a".into());
            if let Some(keyword) = keyword {
                metadata.insert("important_kwd".into(), keyword.into());
            }
            engine.add(IndexedChunk {
                id: id.into(),
                doc_name: title.into(),
                content: if id == "body" {
                    "water quality"
                } else {
                    "common text"
                }
                .into(),
                embedding: Vec::new(),
                token_count: 2,
                position: 0,
                metadata,
            });
        }

        let kb_ids = vec!["kb-a".into()];
        let results = engine.hybrid_search_kbs(HybridSearchQuery {
            query: "water quality",
            query_embedding: None,
            top_k: 3,
            kb_ids: &kb_ids,
            vector_weight: 0.0,
            doc_ids: None,
            rank_feature: None,
        });
        assert_eq!(results[0].chunk.id, "keyword");
        assert!(results[0].term_score > results[1].term_score);
        assert!(results[1].term_score > results[2].term_score);
    }

    #[test]
    fn hybrid_search_filters_documents_before_scoring() {
        let mut engine = SearchEngine::new();
        for (id, doc_id) in [("one", "doc-one"), ("two", "doc-two")] {
            let metadata = HashMap::from([
                ("kb_id".into(), "kb-a".into()),
                ("doc_id".into(), doc_id.into()),
            ]);
            engine.add(IndexedChunk {
                id: id.into(),
                doc_name: format!("{doc_id}.txt"),
                content: "water quality".into(),
                embedding: Vec::new(),
                token_count: 2,
                position: 0,
                metadata,
            });
        }

        let kb_ids = vec!["kb-a".into()];
        let doc_ids = vec!["doc-two".into()];
        let results = engine.hybrid_search_kbs(HybridSearchQuery {
            query: "water quality",
            query_embedding: None,
            top_k: 10,
            kb_ids: &kb_ids,
            vector_weight: 0.0,
            doc_ids: Some(&doc_ids),
            rank_feature: None,
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].chunk.id, "two");
    }

    #[test]
    fn hybrid_search_adds_tag_cosine_and_pagerank_features() {
        let mut engine = SearchEngine::new();
        for (id, tags, pagerank) in [
            ("base", r#"{"fish":1}"#, "0"),
            ("boosted", r#"{"fish":1,"pond":1}"#, "2"),
        ] {
            let metadata = HashMap::from([
                ("kb_id".into(), "kb-a".into()),
                ("tag_feas".into(), tags.into()),
                ("pagerank_fea".into(), pagerank.into()),
            ]);
            engine.add(IndexedChunk {
                id: id.into(),
                doc_name: "doc.txt".into(),
                content: "water quality".into(),
                embedding: Vec::new(),
                token_count: 2,
                position: 0,
                metadata,
            });
        }
        let query_features = HashMap::from([("fish".into(), 1.0), ("pond".into(), 1.0)]);

        let kb_ids = vec!["kb-a".into()];
        let results = engine.hybrid_search_kbs(HybridSearchQuery {
            query: "water quality",
            query_embedding: None,
            top_k: 10,
            kb_ids: &kb_ids,
            vector_weight: 0.0,
            doc_ids: None,
            rank_feature: Some(&query_features),
        });
        assert_eq!(results[0].chunk.id, "boosted");
        assert!((results[0].rank_feature_score - 12.0).abs() < 1e-5);
    }

    #[test]
    fn document_aggregation_uses_all_filtered_candidates() {
        let results = vec![
            hybrid_result("one", "doc-a", "A"),
            hybrid_result("two", "doc-a", "A"),
            hybrid_result("three", "doc-b", "B"),
        ];
        assert_eq!(
            aggregate_documents(&results),
            vec![
                DocumentAggregation {
                    doc_name: "A".into(),
                    doc_id: "doc-a".into(),
                    count: 2,
                },
                DocumentAggregation {
                    doc_name: "B".into(),
                    doc_id: "doc-b".into(),
                    count: 1,
                },
            ]
        );
    }

    #[test]
    fn highlight_escapes_content_and_marks_matches() {
        assert_eq!(
            highlight_content("Water <quality>", "water"),
            "<em>Water</em> &lt;quality&gt;"
        );
        assert_eq!(highlight_content("pond health", "missing"), "pond health");
    }

    fn hybrid_result(id: &str, doc_id: &str, doc_name: &str) -> HybridSearchResult {
        HybridSearchResult {
            chunk: IndexedChunk {
                id: id.into(),
                doc_name: doc_name.into(),
                content: "content".into(),
                embedding: Vec::new(),
                token_count: 1,
                position: 0,
                metadata: HashMap::from([("doc_id".into(), doc_id.into())]),
            },
            score: 1.0,
            vector_score: 0.0,
            term_score: 1.0,
            model_term_score: 1.0,
            rank_feature_score: 0.0,
            rank: 0,
        }
    }
}
