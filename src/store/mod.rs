//! zvec vector database integration.
//!
//! Stores and retrieves chunks with embeddings using zvec-rust bindings.
//! Replaces RAGFlow's Elasticsearch-based chunk storage.

use crate::{Chunk, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// zvec vector store wrapper.
pub struct ZvecStore {
    /// Schema name / collection
    collection: String,
    /// Embedding dimension
    dimension: usize,
}

/// Search result with score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub chunk: Chunk,
    pub score: f32,
}

impl ZvecStore {
    /// Create a new zvec store for a given collection.
    pub fn new(collection: &str, dimension: usize) -> Result<Self> {
        Ok(Self {
            collection: collection.to_string(),
            dimension,
        })
    }

    /// Initialize the zvec collection/schema.
    /// TODO: Implement with zvec-rust when ready.
    pub fn init(&self) -> Result<()> {
        tracing::info!(
            "ZvecStore initialized: collection={}, dim={}",
            self.collection,
            self.dimension
        );
        // TODO: zvec-rust schema setup
        Ok(())
    }

    /// Insert chunks with their embeddings into the store.
    /// TODO: Implement with zvec-rust when ready.
    pub fn insert(&self, chunks: &[Chunk]) -> Result<usize> {
        let count = chunks
            .iter()
            .filter(|c| c.embedding.is_some())
            .count();

        tracing::info!("ZvecStore: inserting {} chunks into {}", count, self.collection);

        // TODO: zvec-rust insert
        // for chunk in chunks {
        //     if let Some(emb) = &chunk.embedding {
        //         // Convert chunk metadata to zvec fields
        //         let fields = chunk_to_fields(chunk);
        //         self.client.insert(&self.collection, emb, &fields)?;
        //     }
        // }

        Ok(count)
    }

    /// Search for similar chunks by embedding vector.
    /// TODO: Implement with zvec-rust when ready.
    pub fn search(
        &self,
        query_embedding: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>> {
        tracing::info!(
            "ZvecStore: searching {} (top_k={})",
            self.collection,
            top_k
        );

        // TODO: zvec-rust ANN search
        // let results = self.client.search(
        //     &self.collection,
        //     query_embedding,
        //     top_k,
        // )?;

        Ok(vec![])
    }
}

/// Convert a Chunk to a flat key-value metadata map for zvec.
fn _chunk_to_fields(chunk: &Chunk) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    fields.insert("chunk_id".to_string(), chunk.id.clone());
    fields.insert("content".to_string(), chunk.content.clone());
    fields.insert("doc_id".to_string(), chunk.doc_id.to_string());
    fields.insert("position".to_string(), chunk.position.to_string());
    fields.insert("token_count".to_string(), chunk.token_count.to_string());
    for (k, v) in &chunk.metadata {
        fields.insert(format!("meta_{}", k), v.clone());
    }
    fields
}
