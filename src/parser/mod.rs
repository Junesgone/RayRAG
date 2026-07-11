//! Document parsers — ported from RAGFlow's `deepdoc/parser/`.
//!
//! Each parser takes raw bytes and produces a `Document` with
//! extracted text content and metadata.

pub mod docx;
pub mod html;
pub mod markdown;
pub mod pdf;
pub mod txt;

use crate::{Document, Result};
use std::collections::HashMap;
use uuid::Uuid;

/// Trait for all document parsers.
pub trait Parse {
    /// Parse raw bytes into a Document.
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document>;
}

/// Create a new document with common fields populated.
pub(crate) fn new_document(
    name: &str,
    content: String,
    mime_type: &str,
    size: usize,
) -> Document {
    Document {
        id: Uuid::new_v4(),
        name: name.to_string(),
        content,
        mime_type: mime_type.to_string(),
        size,
        metadata: HashMap::new(),
    }
}

/// Registry of available parsers, indexed by MIME type.
pub fn get_parser(mime_type: &str) -> Option<Box<dyn Parse>> {
    match mime_type {
        "application/pdf" => Some(Box::new(pdf::PdfParser::new())),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some(Box::new(docx::DocxParser::new()))
        }
        "text/plain" => Some(Box::new(txt::TxtParser::new())),
        "text/markdown" => Some(Box::new(markdown::MarkdownParser::new())),
        "text/html" => Some(Box::new(html::HtmlParser::new())),
        _ => None,
    }
}

/// Infer MIME type from file extension.
pub fn mime_from_extension(name: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext.to_lowercase().as_str() {
        "pdf" => Some("application/pdf"),
        "docx" => Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ),
        "txt" => Some("text/plain"),
        "md" | "markdown" => Some("text/markdown"),
        "html" | "htm" => Some("text/html"),
        "json" => Some("application/json"),
        _ => None,
    }
}
