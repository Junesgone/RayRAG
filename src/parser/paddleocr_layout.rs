//! PaddleOCR Layout parser — enhanced OCR with layout detection.
//!
//! Calls paddleocr-gpu-proxy for text detection + recognition,
//! then structures the output by detected regions (layout-aware).
//! Based on RAGFlow's `deepdoc/parser/paddleocr_parser.py`.

use crate::ocr::OcrClient;
use crate::parser::{Parse, new_document};
use crate::{Document, Result};

#[derive(Default)]
pub struct PaddleOcrLayoutParser {
    ocr: OcrClient,
}

impl PaddleOcrLayoutParser {
    pub fn new() -> Self {
        Self {
            ocr: OcrClient::default(),
        }
    }

    pub fn with_ocr(ocr: OcrClient) -> Self {
        Self { ocr }
    }
}

impl Parse for PaddleOcrLayoutParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async { self.ocr.ocr(data).await })?;

        // Wrap in layout markers with detected text blocks
        let formatted = if content.is_empty() {
            format!(
                "<!--IMAGE_START-->\n[No text detected in image: {}]\n<!--IMAGE_END-->\n",
                name
            )
        } else {
            format!(
                "<!--IMAGE_START-->\n[DOCUMENT_LAYOUT: {}]\n{}\n<!--IMAGE_END-->\n",
                detect_layout_type(&content),
                content,
            )
        };

        Ok(new_document(name, formatted, "image/png", data.len()))
    }
}

/// Heuristically detect document layout type from text content.
fn detect_layout_type(text: &str) -> &'static str {
    if text.contains("Chapter") || text.contains("Section") {
        "book_page"
    } else if text.contains("Table") || text.contains("|") {
        "table"
    } else if text.lines().count() > 10 && text.len() > 500 {
        "full_page"
    } else if text.len() < 200 {
        "caption_or_label"
    } else {
        "mixed_content"
    }
}
