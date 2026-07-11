//! RayRAG: RAGFlow parsing pipeline rewritten in Rust, powered by zvec.
//!
//! Port of RAGFlow's document parsing and chunking pipeline from Python to Rust,
//! with zvec as the vector database backend instead of Elasticsearch.
//!
//! # Architecture
//!
//! ```text
//! Document → parser → chunk → embed → store
//!   .pdf     pdf.rs   naive   openai   zvec.rs
//!   .docx    docx.rs  sentence
//!   .txt     txt.rs
//!   .md      markdown.rs
//!   .html    html.rs
//! ```

pub mod chunk;
pub mod embed;
pub mod parser;
pub mod pipeline;
pub mod server;
pub mod store;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A parsed document before chunking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    /// Unique document ID
    pub id: Uuid,
    /// Original filename
    pub name: String,
    /// Parsed text content
    pub content: String,
    /// MIME type (e.g., "application/pdf")
    pub mime_type: String,
    /// File size in bytes
    pub size: usize,
    /// Extracted metadata
    pub metadata: std::collections::HashMap<String, String>,
}

/// A text chunk ready for embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    /// Chunk ID (generated or from source doc)
    pub id: String,
    /// The chunk text content
    pub content: String,
    /// Source document ID
    pub doc_id: Uuid,
    /// Position within the document (for ordering)
    pub position: usize,
    /// Token count
    pub token_count: usize,
    /// Optional embedding vector (set after embedding)
    pub embedding: Option<Vec<f32>>,
    /// Chunk metadata (page numbers, sections, etc.)
    pub metadata: std::collections::HashMap<String, String>,
}

/// Parsing configuration, mirroring RAGFlow's parser_config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParserConfig {
    /// Chunk token size (default: 2048)
    pub chunk_token_num: usize,
    /// Overlap between chunks (0.0–1.0, default: 0.05)
    pub overlapped_percent: f32,
    /// Chunk delimiter (default: "\n")
    pub delimiter: String,
    /// Enable table context extraction
    pub table_context_size: usize,
    /// Enable image context extraction
    pub image_context_size: usize,
    /// OCR language (default: "English")
    pub ocr_lang: String,
    /// Layout recognizer: "DeepDOC" or "plain"
    pub layout_recognize: String,
    /// Enable metadata extraction
    pub enable_metadata: bool,
    /// Enable children chunks (parent-child hierarchy)
    pub enable_children: bool,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            chunk_token_num: 2048,
            overlapped_percent: 0.05,
            delimiter: "\n".to_string(),
            table_context_size: 256,
            image_context_size: 256,
            ocr_lang: "English".to_string(),
            layout_recognize: "DeepDOC".to_string(),
            enable_metadata: true,
            enable_children: true,
        }
    }
}

/// Result type alias for RayRAG operations.
pub type Result<T> = anyhow::Result<T>;
