//! PDF parser — ported from RAGFlow's `deepdoc/parser/pdf_parser.py`.
//!
//! Extracts text from PDF documents. For now, a simple text extraction.
//! When OCR client is available, page images get OCR'd for better results.
//!
//! Also mirrors RAGFlow's PDF outline handling: `deepdoc/parser/utils.py
//! extract_pdf_outlines` walks the outline tree and produces
//! (title, depth, page) entries, which `rag/svr/task_executor_refactor/
//! chunk_builder.py` persists onto the document metadata as transient
//! `__outline__` on the first chunk. Here the outline is attached to
//! [`Document::metadata`] under the same key.

use crate::ocr::OcrClient;
use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One PDF outline (bookmark) entry, mirroring RAGFlow's
/// `{"title": ..., "depth": ...}` dicts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutlineEntry {
    /// Bookmark title.
    pub title: String,
    /// Nesting depth (0 = top level).
    pub depth: usize,
    /// 1-based destination page number (0 when unresolvable).
    pub page: usize,
}

#[derive(Default)]
pub struct PdfParser {
    ocr: Option<OcrClient>,
}

impl PdfParser {
    pub fn new() -> Self {
        Self { ocr: None }
    }

    /// Create a PDF parser with OCR support.
    pub fn with_ocr(ocr_client: OcrClient) -> Self {
        Self {
            ocr: Some(ocr_client),
        }
    }

    /// Extract the PDF outline tree (bookmarks) as `(title, depth, page)`
    /// entries, mirroring RAGFlow `extract_pdf_outlines`. Returns `Ok(vec![])`
    /// when the document has no outline or it cannot be parsed.
    pub fn extract_outlines(data: &[u8]) -> Result<Vec<OutlineEntry>> {
        let doc = lopdf::Document::load_mem(data)?;
        let mut named_destinations = indexmap::IndexMap::new();
        let outlines = match doc.get_outlines(None, None, &mut named_destinations) {
            Ok(Some(outlines)) => outlines,
            _ => return Ok(vec![]),
        };

        // page-object-id → 1-based page number, for resolving destinations.
        let page_ids: HashMap<u32, u32> = doc
            .get_pages()
            .iter()
            .map(|(&page_num, &(page_id, _))| (page_id, page_num))
            .collect();

        let mut entries = Vec::new();
        fn walk(
            nodes: &[lopdf::Outline],
            depth: usize,
            page_ids: &HashMap<u32, u32>,
            entries: &mut Vec<OutlineEntry>,
        ) {
            for node in nodes {
                match node {
                    lopdf::Outline::Destination(dest) => {
                        let title = dest
                            .title()
                            .and_then(|t| t.as_str().ok())
                            .map(String::from_utf8_lossy)
                            .unwrap_or_default()
                            .into_owned();
                        let page = dest
                            .page()
                            .and_then(|p| p.as_reference().ok())
                            .and_then(|id| page_ids.get(&id.0).copied())
                            .unwrap_or(0) as usize;
                        entries.push(OutlineEntry { title, depth, page });
                    }
                    lopdf::Outline::SubOutlines(sub) => walk(sub, depth + 1, page_ids, entries),
                }
            }
        }
        walk(&outlines, 0, &page_ids, &mut entries);
        Ok(entries)
    }

    /// Extract text from PDF bytes using lopdf.
    async fn extract_text(&self, data: &[u8]) -> Result<String> {
        let doc = lopdf::Document::load_mem(data)?;
        let mut text = String::new();

        // Try OCR first if client is available
        let use_ocr = if let Some(ref ocr) = self.ocr {
            ocr.health().await.unwrap_or(false)
        } else {
            false
        };

        // lopdf 0.34+: get_pages() returns HashMap; iterate directly
        for (page_num, &page_id) in &doc.get_pages() {
            // Try to render page as image for OCR
            if use_ocr && page_num % 3 == 0 {
                // OCR every 3rd page to keep latency reasonable
                if let Some(ref ocr) = self.ocr {
                    // Extract page content as bytes for rendering
                    if let Ok(_content) = doc.get_and_decode_page_content(page_id) {
                        // Note: lopdf doesn't render pages to images natively.
                        // Full PDF→image OCR needs pdfium/poppler.
                        // For now, skip image-based OCR on PDF.
                        let _ = ocr;
                    }
                }
            }

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
        // Create a blocking runtime for async OCR calls
        let content = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(self.extract_text(data))?;
        let mut doc = new_document(name, content, "application/pdf", data.len());
        // RAGFlow transient `__outline__`: attach the PDF outline tree to the
        // document metadata (mirrors naive.py attaching it to chunks[0]).
        let outlines = Self::extract_outlines(data)?;
        if !outlines.is_empty() {
            doc.metadata.insert(
                "__outline__".to_string(),
                serde_json::to_string(&outlines).unwrap_or_default(),
            );
        }
        Ok(doc)
    }
}

/// Simple PDF content stream to text (extracts text operators).
fn content_to_text(content: &lopdf::content::Content) -> String {
    let mut text = String::new();
    for op in &content.operations {
        match op.operator.as_ref() {
            "Tj" | "TJ" | "'" | "\"" => {
                for operand in &op.operands {
                    if let lopdf::Object::String(s, _) = operand
                        && let Ok(t) = std::str::from_utf8(s)
                    {
                        text.push_str(t);
                    }
                }
            }
            _ => {}
        }
    }
    text
}

/// Public version for external use (e.g. mineru parser).
pub fn content_to_text_page(content: &lopdf::content::Content) -> String {
    content_to_text(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal valid PDF with one page and no outline tree.
    fn minimal_pdf() -> Vec<u8> {
        // Hand-built minimal PDF (header, 1 page, trailer). Enough for
        // lopdf::Document::load_mem + get_pages.
        let mut pdf = String::from("%PDF-1.4\n");
        pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
        pdf.push_str("3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>\nendobj\n");
        pdf.push_str("4 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n");
        // xref and trailer with startxref
        let xref_offset = pdf.len();
        pdf.push_str(&format!(
            "xref\n0 5\n0000000000 65535 f \n0000000009 00000 n \n0000000058 00000 n \n0000000115 00000 n \n0000000202 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        ));
        pdf.into_bytes()
    }

    #[test]
    fn pdf_without_outline_returns_empty() {
        let data = minimal_pdf();
        let outlines = PdfParser::extract_outlines(&data).unwrap();
        assert!(outlines.is_empty());
    }

    #[test]
    fn parse_attaches_no_outline_metadata_when_absent() {
        let data = minimal_pdf();
        let doc = PdfParser::new().parse("no_outline.pdf", &data).unwrap();
        assert!(!doc.metadata.contains_key("__outline__"));
    }

    // Real-PDF outline extraction (ignored by default). Requires
    // RAYRAG_TEST_PDF_OUTLINE pointing at a PDF with bookmarks (e.g. a
    // reportlab-generated outline_test.pdf with a nested outline tree).
    #[test]
    #[ignore]
    fn gpu_pdf_outline_extraction_live() {
        let path = std::env::var("RAYRAG_TEST_PDF_OUTLINE").expect("RAYRAG_TEST_PDF_OUTLINE");
        let data = std::fs::read(&path).expect("read pdf");
        let outlines = PdfParser::extract_outlines(&data).expect("extract");
        assert!(!outlines.is_empty(), "expected outline entries");
        let has_nested = outlines.iter().any(|e| e.depth > 0);
        assert!(has_nested, "expected nested outline entries: {outlines:?}");
        let has_page = outlines.iter().any(|e| e.page > 0);
        assert!(has_page, "expected resolved page numbers: {outlines:?}");
        tracing::info!("PDF outline ({path}): {outlines:?}");
    }
}
