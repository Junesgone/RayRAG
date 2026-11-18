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

/// Infinity-flavored options layered on the hybrid engine (see
/// [`SearchEngine::hybrid_search_kbs_with`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct HybridSearchOptions {
    /// match_dense `similarity` → `threshold`: candidates whose cosine
    /// similarity falls below this bound contribute no dense score to the
    /// fusion (Infinity drops them from the dense expression entirely).
    pub dense_threshold: Option<f32>,
    /// FusionExpr `weighted_sum` with `normalize=atan`: apply
    /// `atan(weighted) * 2/pi` to the term/vector weighted sum before adding
    /// rank features. The Infinity connector sets this because the default
    /// minmax normalization gives the last-ranked document a zero score.
    pub atan_fusion: bool,
}

/// Infinity `normalize=atan` for `weighted_sum` fusion: `atan(x) * 2/pi`
/// maps non-negative scores into `[0, 1)` with diminishing returns.
pub fn atan_normalize(score: f32) -> f32 {
    (2.0 / std::f32::consts::PI) * score.atan()
}

/// Retrieval-side post filter (`Dealer.retrieval`, search.py): candidates
/// whose fused score is below `threshold` are dropped, but only while vector
/// weighting is active (`vector_weight > 0`). Term-only retrieval
/// (`vector_weight <= 0`) keeps every candidate — a similarity threshold is
/// meaningless for term-only scores.
pub fn similarity_threshold_mask(scores: &[f32], threshold: f32, vector_weight: f32) -> Vec<bool> {
    let post_threshold = if vector_weight <= 0.0 { 0.0 } else { threshold };
    scores
        .iter()
        .map(|score| *score >= post_threshold)
        .collect()
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
        crate::store::search_mapping::validate_replacement_snapshot(&chunks)?;
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

    /// Summed `token_count` for the given documents of one KB — the `token_count`
    /// column a document row reports on the REST surface.
    pub fn token_counts_for(
        &self,
        kb_id: &str,
        doc_ids: &std::collections::HashSet<String>,
    ) -> std::collections::HashMap<String, usize> {
        let mut counts = std::collections::HashMap::new();
        for chunk in &self.chunks {
            if chunk.metadata.get("kb_id").map(String::as_str) != Some(kb_id) {
                continue;
            }
            let Some(doc_id) = chunk.metadata.get("doc_id") else {
                continue;
            };
            if !doc_ids.contains(doc_id) {
                continue;
            }
            *counts.entry(doc_id.clone()).or_insert(0) += chunk.token_count;
        }
        counts
    }

    /// Number of indexed chunks.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Get a copy of the chunks belonging to one document only.
    ///
    /// The index still holds the whole corpus, but a document commit only needs
    /// the before/after image of that document. Cloning the whole engine on every
    /// document would copy every embedding in the knowledge base.
    pub fn chunks_for_document(&self, doc_id: &str) -> Vec<IndexedChunk> {
        self.chunks
            .iter()
            .filter(|chunk| chunk.metadata.get("doc_id").map(String::as_str) == Some(doc_id))
            .cloned()
            .collect()
    }

    /// Get a copy of all indexed chunks.
    pub fn to_vec(&self) -> Vec<IndexedChunk> {
        self.chunks.clone()
    }

    /// Borrow all indexed chunks.
    ///
    /// The persistence path diffs two snapshots and writes them; it needs to *read*
    /// the current index, not own a copy of it. Borrowing here removes one full clone
    /// of the index (embeddings included) from every document commit.
    pub fn chunks(&self) -> &[IndexedChunk] {
        &self.chunks
    }

    /// How many chunks each knowledge base owns, without copying any chunk.
    ///
    /// The native mirror uses this to decide whether a collection already agrees
    /// with the index; comparing counts costs one pass over the chunk metadata
    /// instead of cloning every embedding.
    pub fn chunk_counts_by_kb(&self) -> HashMap<String, usize> {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for chunk in self
            .chunks
            .iter()
            .filter(|chunk| !chunk.embedding.is_empty())
        {
            if let Some(kb_id) = chunk
                .metadata
                .get("kb_id")
                .map(String::as_str)
                .filter(|value| !value.trim().is_empty())
            {
                *counts.entry(kb_id.to_string()).or_default() += 1;
            }
        }
        counts
    }

    /// Copy one knowledge base's chunks, in index order.
    ///
    /// Loading a single owner at a time keeps reconciliation memory bounded by the
    /// owner being repaired instead of by the whole corpus.
    pub fn chunks_for_kb(&self, kb_id: &str) -> Vec<IndexedChunk> {
        self.chunks
            .iter()
            .filter(|chunk| !chunk.embedding.is_empty())
            .filter(|chunk| chunk.metadata.get("kb_id").map(String::as_str) == Some(kb_id))
            .cloned()
            .collect()
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
        self.hybrid_search_kbs_with(request, HybridSearchOptions::default())
    }

    /// Hybrid retrieval with the Infinity search options layered on top of
    /// the RayRAG blend: a dense `similarity` threshold (match_dense
    /// `threshold` semantics) and the `atan` normalized weighted-sum fusion
    /// the Python connector applies for every `weighted_sum` FusionExpr.
    pub fn hybrid_search_kbs_with(
        &self,
        request: HybridSearchQuery<'_>,
        options: HybridSearchOptions,
    ) -> Vec<HybridSearchResult> {
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
                // Infinity match_dense `similarity` → `threshold`: candidates
                // below the bound contribute no dense score to the fusion.
                let vector_score = match (query_vector, options.dense_threshold) {
                    (Some(_), Some(threshold)) if vector_score < threshold => 0.0,
                    _ => vector_score,
                };
                let rank_feature_score = rank_feature_score(chunk, rank_feature);
                let weighted = term_weight * term_score + vector_weight * vector_score;
                // FusionExpr weighted_sum with normalize=atan: atan maps the
                // weighted sum into (0, 1); rank features stay outside the
                // normalization (Infinity `_score = SCORE + pagerank_fea`).
                let fused = if options.atan_fusion {
                    atan_normalize(weighted)
                } else {
                    weighted
                };
                let score = fused + rank_feature_score;
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
    let query_weights: Vec<(String, f64)> =
        tw.weights(&[query.to_string()], true).into_iter().collect();
    let weight_refs: Vec<(&str, f64)> = query_weights
        .iter()
        .map(|(t, w)| (t.as_str(), *w))
        .collect();
    documents
        .iter()
        .map(|document| bm25.score_weighted(query, document, &weight_refs))
        .collect()
}

/// Rerank candidates with RAGFlow's lexical+vector fusion —
/// mirrors `Dealer.rerank` (search.py:361-398) for the in-memory engine.
/// `cfield` tokens come from chunk metadata (content_ltks); title/important
/// keyword/question token fields are read from metadata when present.
/// Returns (fused_scores, tksim, vtsim) aligned with `candidates` order.
pub fn ragflow_rerank(
    candidates: &[HybridSearchResult],
    query: &str,
    tw: &crate::nlp::TermWeightComputer,
    tkweight: f64,
    vtweight: f64,
    rank_feature: Option<&HashMap<String, f32>>,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let keywords = tokenize_query_keywords(query);
    let mut ins_embd: Vec<Vec<f32>> = Vec::new();
    for c in candidates {
        ins_embd.push(c.chunk.embedding.clone());
    }
    let ins_tw: Vec<Vec<String>> = candidates
        .iter()
        .map(|c| {
            let content_ltks: Vec<String> = c
                .chunk
                .metadata
                .get("content_ltks")
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default();
            let title_tks: Vec<String> = c
                .chunk
                .metadata
                .get("title_tks")
                .map(|s| {
                    s.split_whitespace()
                        .map(String::from)
                        .filter(|t| !t.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let question_tks: Vec<String> = c
                .chunk
                .metadata
                .get("question_tks")
                .map(|s| {
                    s.split_whitespace()
                        .map(String::from)
                        .filter(|t| !t.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let important_kwd: Vec<String> = c
                .chunk
                .metadata
                .get("important_kwd")
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default();
            let mut tks = content_ltks;
            tks.extend(title_tks.iter().cloned());
            tks.extend(title_tks.iter().cloned());
            tks.extend(important_kwd.iter().cloned());
            tks.extend(important_kwd.iter().cloned());
            tks.extend(important_kwd.iter().cloned());
            tks.extend(important_kwd.iter().cloned());
            tks.extend(important_kwd.iter().cloned());
            tks.extend(question_tks.iter().cloned());
            tks.extend(question_tks.iter().cloned());
            tks.extend(question_tks.iter().cloned());
            tks.extend(question_tks.iter().cloned());
            tks.extend(question_tks.iter().cloned());
            tks.extend(question_tks.iter().cloned());
            tks
        })
        .collect();

    let rank_fea = ragflow_rank_feature_scores(rank_feature, candidates);
    let (sim, tksim, vtsim) = tw.hybrid_similarity(
        &query_embedding_zero(),
        &ins_embd,
        &keywords,
        &ins_tw,
        tkweight,
        vtweight,
    );
    let fused: Vec<f64> = sim
        .iter()
        .zip(rank_fea.iter())
        .map(|(s, r)| s + r)
        .collect();
    (fused, tksim, vtsim)
}

/// Rerank with an external reranker model —
/// mirrors `Dealer.rerank_by_model` (search.py:400-441):
/// fused = tkweight * tksim + vtweight * vtsim + rank_fea, where vtsim comes
/// from the reranker's relevance scores (aligned with candidates order).
pub fn ragflow_rerank_by_model(
    candidates: &[HybridSearchResult],
    query: &str,
    tw: &crate::nlp::TermWeightComputer,
    tkweight: f64,
    vtweight: f64,
    reranker_scores: &[f32],
    rank_feature: Option<&HashMap<String, f32>>,
) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    assert_eq!(
        candidates.len(),
        reranker_scores.len(),
        "reranker scores must align with candidates"
    );
    let keywords = tokenize_query_keywords(query);
    let ins_tw: Vec<Vec<String>> = candidates
        .iter()
        .map(|c| {
            let content_ltks: Vec<String> = c
                .chunk
                .metadata
                .get("content_ltks")
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default();
            let title_tks: Vec<String> = c
                .chunk
                .metadata
                .get("title_tks")
                .map(|s| {
                    s.split_whitespace()
                        .map(String::from)
                        .filter(|t| !t.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let important_kwd: Vec<String> = c
                .chunk
                .metadata
                .get("important_kwd")
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default();
            let mut tks = content_ltks;
            tks.extend(title_tks);
            tks.extend(important_kwd);
            tks
        })
        .collect();

    let tksim = tw.token_similarity(&keywords, &ins_tw);
    let rank_fea = ragflow_rank_feature_scores(rank_feature, candidates);
    let vtsim: Vec<f64> = reranker_scores.iter().map(|s| *s as f64).collect();
    let fused: Vec<f64> = tksim
        .iter()
        .zip(vtsim.iter())
        .zip(rank_fea.iter())
        .map(|((t, v), r)| tkweight * t + vtweight * v + r)
        .collect();
    (fused, tksim, vtsim)
}

/// Tokenize a query into keyword tokens (space-split, non-empty).
fn tokenize_query_keywords(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(String::from)
        .filter(|t| !t.is_empty())
        .collect()
}

/// A zero query vector (no embedding available) — mirrors the in-memory
/// engine's embedding-less path.
fn query_embedding_zero() -> Vec<f32> {
    Vec::new()
}

/// Rank-feature scores — mirrors `Dealer._rank_feature_scores`
/// (search.py:328-360): tag-feature cosine boost ×10 plus chunk pagerank.
pub fn ragflow_rank_feature_scores(
    query_features: Option<&HashMap<String, f32>>,
    candidates: &[HybridSearchResult],
) -> Vec<f64> {
    let pageranks: Vec<f64> = candidates
        .iter()
        .map(|c| {
            c.chunk
                .metadata
                .get("pagerank_fea")
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(0.0)
        })
        .collect();

    let Some(query_features) = query_features else {
        return pageranks;
    };
    let q_denor: f64 = query_features
        .iter()
        .filter(|(t, s)| *t != "pagerank_fea" && s.is_finite())
        .map(|(_, s)| (*s as f64) * (*s as f64))
        .sum::<f64>()
        .sqrt();
    if q_denor == 0.0 {
        return pageranks;
    }
    let mut rank_fea = Vec::with_capacity(candidates.len());
    for c in candidates {
        let tag_fea = c
            .chunk
            .metadata
            .get("tag_fea")
            .map(|s| parse_tag_features(s))
            .unwrap_or_default();
        if tag_fea.is_empty() {
            rank_fea.push(0.0);
            continue;
        }
        let mut nor = 0.0_f64;
        let mut denor = 0.0_f64;
        for (t, sc) in &tag_fea {
            if let Some(qs) = query_features.get(t) {
                nor += (*qs as f64) * sc;
            }
            denor += sc * sc;
        }
        if denor == 0.0 {
            rank_fea.push(0.0);
        } else {
            rank_fea.push(nor / denor.sqrt() / q_denor);
        }
    }
    rank_fea
        .into_iter()
        .zip(pageranks)
        .map(|(r, p)| r * 10.0 + p)
        .collect()
}

/// Parse a tag-feature string like `{"tag": 1.5, ...}` (JSON or Python dict).
fn parse_tag_features(s: &str) -> HashMap<String, f64> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return HashMap::new();
    }
    let json = trimmed
        .replace('(', "[")
        .replace(')', "]")
        .replace('\'', "\"");
    serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|obj| {
            obj.into_iter()
                .filter_map(|(k, v)| v.as_f64().map(|n| (k, n)))
                .collect()
        })
        .unwrap_or_default()
}




