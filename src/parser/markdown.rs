//! Markdown parser — ported from RAGFlow's `deepdoc/parser/markdown_parser.py`.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};

#[derive(Default)]
pub struct MarkdownParser;

impl MarkdownParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parse for MarkdownParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = String::from_utf8(data.to_vec())
            .map_err(|e| anyhow::anyhow!("UTF-8 decode error: {}", e))?;
        // Markdown is already text; pass through as-is.
        // Future: could strip formatting for cleaner chunks.
        Ok(new_document(name, content, "text/markdown", data.len()))
    }
}
