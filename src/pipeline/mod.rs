//! Parsing pipeline — orchestration of parse → chunk → embed → store.
//!
//! Ported from RAGFlow's `rag/app/naive.py` `chunk()` function.
//! This is the main entry point for document processing.

use crate::chunk::{self, ChunkStrategy};
use crate::embed::Embedder;
use crate::parser;
use crate::store::ZvecStore;
use crate::{Chunk, ParserConfig, Result};

/// The full RayRAG parsing pipeline.
pub struct Pipeline {
    config: ParserConfig,
    chunker: Box<dyn ChunkStrategy>,
    embedder: Option<Box<dyn Embedder>>,
    store: Option<ZvecStore>,
}

impl Pipeline {
    /// Create a new pipeline with default naive chunker.
    pub fn new(config: ParserConfig) -> Self {
        Self {
            config,
            chunker: chunk::default_chunker(),
            embedder: None,
            store: None,
        }
    }

    /// Set a custom chunking strategy.
    pub fn with_chunker(mut self, chunker: Box<dyn ChunkStrategy>) -> Self {
        self.chunker = chunker;
        self
    }

    /// Set an embedder for generating embeddings.
    pub fn with_embedder(mut self, embedder: Box<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Set a zvec store for persisting chunks.
    pub fn with_store(mut self, store: ZvecStore) -> Self {
        self.store = Some(store);
        self
    }

    /// Run the full pipeline on a file: parse → chunk → embed → store.
    pub async fn process(&self, file_path: &str) -> Result<Vec<Chunk>> {
        let name = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(file_path);

        // 1. Detect MIME type and parse
        let mime = parser::mime_from_extension(name)
            .ok_or_else(|| anyhow::anyhow!("Unsupported file type: {}", name))?;

        let data = std::fs::read(file_path)?;
        let parser = parser::get_parser(mime)
            .ok_or_else(|| anyhow::anyhow!("No parser for MIME: {}", mime))?;

        let doc = parser.parse(name, &data)?;
        tracing::info!("Parsed: {} ({} bytes)", doc.name, doc.size);

        // 2. Chunk
        let mut chunks = self.chunker.chunk(&doc, &self.config)?;
        tracing::info!("Chunked: {} chunks", chunks.len());

        // 3. Embed (if embedder is configured)
        if let Some(ref embedder) = self.embedder {
            embedder.embed_chunks(&mut chunks).await?;
            tracing::info!("Embedded: {} vectors", chunks.len());
        }

        // 4. Store (if store is configured)
        if let Some(ref store) = self.store {
            store.insert(&chunks)?;
            tracing::info!("Stored: {} chunks", chunks.len());
        }

        Ok(chunks)
    }
}
