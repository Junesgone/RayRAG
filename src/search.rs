//! Vector search module — cosine similarity retrieval over embedded chunks.
//!
//! Implements the search functionality that replaces RAGFlow's ES retrieval.
//! Stores chunks in-memory or loads from a JSON index file.
//!
//! CLI: `rayrag search --query "hello" --index index.json`

use crate::{Chunk, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

impl SearchEngine {
    /// Create a new empty search engine.
    pub fn new() -> Self {
        Self { chunks: Vec::new() }
    }

    /// Load chunks from a JSON index file.
    pub fn from_file(path: &str) -> Result<Self> {
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

    /// Number of indexed chunks.
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Check if index is empty.
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Save index to a JSON file.
    pub fn save(&self, path: &str) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.chunks)?;
        std::fs::write(path, data)?;
        tracing::info!("Saved {} chunks to {}", self.chunks.len(), path);
        Ok(())
    }

    /// Search for chunks similar to the query embedding.
    ///
    /// Uses cosine similarity: score = dot(query, chunk) / (|query| * |chunk|)
    pub fn search(&self, query_embedding: &[f32], top_k: usize) -> Vec<SearchResult> {
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
            .enumerate()
            .filter_map(|(_i, chunk)| {
                if chunk.embedding.is_empty() {
                    return None;
                }
                let chunk_norm = l2_norm(&chunk.embedding);
                if chunk_norm == 0.0 {
                    return None;
                }
                let dot = dot_product(query_embedding, &chunk.embedding);
                let score = dot / (query_norm * chunk_norm);

                Some(SearchResult {
                    chunk: chunk.clone(),
                    score,
                    rank: 0, // Will be set after sorting
                })
            })
            .collect();

        // Sort by score descending
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Truncate to top_k and set ranks
        results.truncate(top_k);
        for (i, result) in results.iter_mut().enumerate() {
            result.rank = i;
        }

        results
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
}
