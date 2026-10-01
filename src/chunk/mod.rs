//! Chunking strategies — ported from RAGFlow's `rag/nlp/`.
//!
//! Splits parsed document text into embeddable chunks.

mod naive;
mod position;
mod title;
mod token;
pub mod tokenizer;
pub use tokenizer::token_count;

pub use naive::{NaiveChunker, SingleChunker};
pub use title::TitleChunker;
pub use token::TokenChunker;

use crate::{Chunk, Document, ParserConfig, Result};
use uuid::Uuid;

/// Internal parser-to-chunker signal matching RAGFlow's `if pdf_parser` branch.
pub(crate) const RAGFLOW_POSITION_TAGS_METADATA: &str = "_rayrag_ragflow_position_tags";

/// Transient PDF outline metadata attached by pdf.rs (JSON array of
/// `{title, depth, page}`); consumed by TitleChunker and popped by the
/// chunk builder before persistence.
pub(crate) const OUTLINE_METADATA: &str = "__outline__";

/// Chunking strategy trait.
pub trait ChunkStrategy: Send + Sync {
    /// Split document content into chunks.
    fn chunk(&self, doc: &Document, config: &ParserConfig) -> Result<Vec<Chunk>>;
}

/// Get the default chunking strategy (naive token-based).
pub fn default_chunker() -> Box<dyn ChunkStrategy> {
    Box::new(NaiveChunker::new())
}

/// Select a chunking strategy from `config.chunk_method`
/// (naive | token | title; unknown methods fall back to naive).
pub fn chunker_for(config: &ParserConfig) -> Box<dyn ChunkStrategy> {
    match config.chunk_method.as_str() {
        "token" => Box::new(TokenChunker::new()),
        "title" => Box::new(TitleChunker::new()),
        // RAGFlow rag/app/one.py: one file forms a single chunk.
        "one" => Box::new(SingleChunker::new()),
        _ => Box::new(NaiveChunker::new()),
    }
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
