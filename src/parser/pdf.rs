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
use crate::parser::pdf_text;
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
    ///
    /// The positioned pass (`pdf_text`) wins whenever it finds text, because it
    /// decodes subset fonts through `/ToUnicode` and keeps each line's box, which
    /// the chunker projects into `position_int`. The raw operator walk stays as
    /// the fallback for streams the interpreter cannot follow.
    async fn extract_text(&self, data: &[u8]) -> Result<String> {
        let doc = lopdf::Document::load_mem(data)?;
        let tagged = pdf_text::tagged_content(&pdf_text::extract_positioned_lines(&doc));
        if !tagged.trim().is_empty() {
            return Ok(tagged);
        }
        let mut text = String::new();
        // Pages whose stream was refused by the size cap. Reported at the end when
        // they are the reason the document produced nothing: "DONE, 0 chunks" would
        // otherwise hide a bomb (or a broken file) behind a successful status.
        let mut refused_streams = 0usize;

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
                    // Extract page content as bytes for rendering (capped decode).
                    if let Ok(_content) = super::pdf_stream::page_content_limited(
                        &doc,
                        page_id,
                        super::pdf_stream::stream_limit_bytes(),
                        "PDF page content (OCR probe)",
                    ) {
                        // Note: lopdf doesn't render pages to images natively.
                        // Full PDF→image OCR needs pdfium/poppler.
                        // For now, skip image-based OCR on PDF.
                        let _ = ocr;
                    }
                }
            }

            // Decoded with a size cap: `get_and_decode_page_content` inflates the page
            // stream without a ceiling, so a few kilobytes of PDF can describe
            // gigabytes of content.
            match super::pdf_stream::page_content_limited(
                &doc,
                page_id,
                super::pdf_stream::stream_limit_bytes(),
                &format!("PDF page {page_num} content"),
            )
            .and_then(|bytes| {
                lopdf::content::Content::decode(&bytes)
                    .map_err(|error| anyhow::anyhow!("PDF page {page_num}: {error}"))
            }) {
                Ok(content) => {
                    text.push_str(&content_to_text(&content));
                    text.push('\n');
                }
                // A refusal must be visible: silence here would look like an empty page.
                Err(error) => {
                    refused_streams += 1;
                    tracing::warn!(%error, page = page_num, "Skipping PDF page content");
                }
            }
        }

        if text.trim().is_empty() && refused_streams > 0 {
            anyhow::bail!(
                "PDF content could not be decoded: {refused_streams} page stream(s) exceeded the {} MiB safety limit",
                super::pdf_stream::stream_limit_bytes() / (1024 * 1024)
            );
        }
        Ok(text)
    }

    /// Drive [`Self::extract_text`] from the synchronous `Parse` interface.
    ///
    /// Parsing runs on the server's own worker threads, so a nested
    /// `Runtime::block_on` is a hard panic ("Cannot start a runtime from within a
    /// runtime"): without an OCR client there is nothing to await, and with one
    /// the current runtime is borrowed through `block_in_place` instead.
    fn extract_text_blocking(&self, data: &[u8]) -> Result<String> {
        if self.ocr.is_none() {
            return futures_lite_block_on(self.extract_text(data));
        }
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(self.extract_text(data))),
            Err(_) => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(self.extract_text(data)),
        }
    }
}

/// Run a future that never actually awaits anything external to completion.
///
/// `extract_text` is async only because of the optional OCR health probe; on the
/// no-OCR path every awaited call resolves immediately, so a parked, unpolled
/// future still yields its value. The helper keeps that path free of a nested
/// runtime entirely.
fn futures_lite_block_on<F: std::future::Future>(mut future: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

impl Parse for PdfParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text_blocking(data)?;
        let mut doc = new_document(name, content, "application/pdf", data.len());
        // Position tags in the content flip the chunker into position-aware mode
        // (`_rayrag_ragflow_position_tags`): `naive_merge` then strips the tags
        // from the visible text and writes `position_int` / `page_num_int` /
        // `top_int` onto every chunk, exactly like upstream's DeepDOC parsers.
        if doc.content.contains("@@") && doc.content.contains("##") {
            doc.metadata.insert(
                crate::chunk::RAGFLOW_POSITION_TAGS_METADATA.to_string(),
                "true".to_string(),
            );
        }
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

    /// A one-page PDF with two 12 pt lines at known baselines.
    fn positioned_pdf() -> Vec<u8> {
        use lopdf::{Document as PdfDocument, Object, Stream, dictionary};
        let mut doc = PdfDocument::with_version("1.7");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "FirstChar" => 32,
            "LastChar" => 126,
            "Widths" => (32..=126)
                .map(|code| Object::Real(if code == 32 { 250.0 } else { 500.0 }))
                .collect::<Vec<Object>>(),
            "Encoding" => "WinAnsiEncoding",
        });
        let content_id = doc.add_object(Stream::new(
            dictionary! {},
            b"BT /F1 12 Tf 72 700 Td (positioned line one) Tj 0 -200 Td (positioned line two) Tj ET"
                .to_vec(),
        ));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).expect("save generated pdf");
        bytes
    }

    #[test]
    fn parse_emits_position_tags_and_marks_the_document_position_aware() {
        let data = positioned_pdf();
        let doc = PdfParser::new().parse("positioned.pdf", &data).unwrap();
        assert!(
            doc.metadata
                .contains_key(crate::chunk::RAGFLOW_POSITION_TAGS_METADATA),
            "positioned content must flip the chunker into position-aware mode"
        );
        assert!(
            doc.content.contains("positioned line one@@1\t"),
            "{}",
            doc.content
        );

        // The chunker keeps the boxes as `position_int` and drops the tags from
        // the visible text, exactly like `naive_merge`.
        use crate::chunk::{ChunkStrategy, NaiveChunker};
        let chunks = NaiveChunker::new()
            .chunk(&doc, &crate::ParserConfig::default())
            .unwrap();
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        assert_eq!(
            chunks[0].content,
            "positioned line one\npositioned line two"
        );
        let positions: Vec<Vec<i32>> = serde_json::from_str(
            chunks[0]
                .metadata
                .get("position_int")
                .expect("position_int metadata"),
        )
        .expect("positions parse");
        assert_eq!(positions.len(), 2, "{positions:?}");
        for position in &positions {
            assert_eq!(position.len(), 5, "{positions:?}");
            assert_eq!(position[0], 1, "page numbers are 1-based: {positions:?}");
        }
        // First paragraph: x0 72, top 792 - (700 + 9) = 83.
        assert_eq!(positions[0][1], 72);
        assert_eq!(positions[0][3], 83);
        // Second paragraph moved down 200 pt by `0 -200 Td`.
        assert_eq!(positions[1][3], 283);
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
