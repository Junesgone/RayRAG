//! PDF parser — ported from RAGFlow's `deepdoc/parser/pdf_parser.py`.
//!
//! Extracts text from PDF documents. For now, a simple text extraction.
//! Future: integrate with PDFium/poppler for layout-aware extraction.

use crate::parser::{new_document, Parse};
use crate::{Document, Result};

pub struct PdfParser;

impl PdfParser {
    pub fn new() -> Self {
        Self
    }

    /// Extract text from PDF bytes using lopdf.
    fn extract_text(&self, data: &[u8]) -> Result<String> {
        let doc = lopdf::Document::load_mem(data)?;
        let mut text = String::new();

        // lopdf 0.34+: get_pages() returns HashMap; iterate directly
        for (_, &page_id) in &doc.get_pages() {
            if let Ok(content) = doc.get_and_decode_page_content(page_id) {
                text.push_str(&content_to_text(&content));
                text.push('\n');
            }
        }

        Ok(text)
    }
}

impl Parse for PdfParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(data)?;
        Ok(new_document(name, content, "application/pdf", data.len()))
    }
}

/// Simple PDF content stream to text (extracts text operators).
fn content_to_text(content: &lopdf::content::Content) -> String {
    let mut text = String::new();
    for op in &content.operations {
        match op.operator.as_ref() {
            "Tj" | "TJ" | "'" | "\"" => {
                for operand in &op.operands {
                    if let lopdf::Object::String(s, _) = operand {
                        if let Ok(t) = std::str::from_utf8(s) {
                            text.push_str(t);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    text
}
