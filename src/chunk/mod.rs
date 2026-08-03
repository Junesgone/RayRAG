//! Chunking strategies — ported from RAGFlow's `rag/nlp/`.
//!
//! Splits parsed document text into embeddable chunks.

mod naive;
mod position;
mod tokenizer;

pub use naive::NaiveChunker;
pub use tokenizer::token_count;

use crate::{Chunk, Document, ParserConfig, Result};
use uuid::Uuid;

/// Internal parser-to-chunker signal matching RAGFlow's `if pdf_parser` branch.
pub(crate) const RAGFLOW_POSITION_TAGS_METADATA: &str = "_rayrag_ragflow_position_tags";

/// Chunking strategy trait.
pub trait ChunkStrategy: Send + Sync {
    /// Split document content into chunks.
    fn chunk(&self, doc: &Document, config: &ParserConfig) -> Result<Vec<Chunk>>;
}

/// Get the default chunking strategy (naive token-based).
pub fn default_chunker() -> Box<dyn ChunkStrategy> {
    Box::new(NaiveChunker::new())
}

/// Estimate token count using a simple heuristic:
/// ~4 characters per token for English, ~1.5 for CJK.
pub fn estimate_tokens(text: &str) -> usize {
    token_count(text)
}

/// Generate a chunk ID from document ID and position.
pub fn chunk_id(doc_id: &Uuid, position: usize) -> String {
    format!("{}_{}", doc_id, position)
}
