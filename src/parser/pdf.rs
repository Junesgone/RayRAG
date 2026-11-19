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
        let doc = super::pdf_stream::load_document(data)?;
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
                            .ok()
                            .and_then(|t| t.as_str().ok())
                            .map(String::from_utf8_lossy)
                            .unwrap_or_default()
                            .into_owned();
                        let page = dest
                            .page()
                            .ok()
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
        let doc = super::pdf_stream::load_document(data)?;
        let lines = pdf_text::try_extract_positioned_lines(&doc)?;
        let text_pages: std::collections::HashSet<usize> =
            lines.iter().map(|line| line.page).collect();
        for (number, page_id) in doc.get_pages() {
            if text_pages.contains(&(number as usize)) {
                continue;
            }
            let bytes = super::pdf_stream::page_content_limited(
                &doc,
                page_id,
                super::pdf_stream::stream_limit_bytes(),
                &format!("PDF page {number} content"),
            )?;
            let content = lopdf::content::Content::decode(&bytes)
                .map_err(|error| anyhow::anyhow!("PDF page {number}: {error}"))?;
            if content_to_text(&content).trim().is_empty()
                && content
                    .operations
                    .iter()
                    .any(|operation| matches!(operation.operator.as_str(), "Do" | "BI" | "ID"))
            {
                anyhow::bail!(
                    "No text extracted from PDF page {number} containing image or Form content. The built-in parser cannot render this page for OCR. Select an OCR document layout backend in the dataset parser settings (layout_recognize), configure its API URL, credentials and model, then reparse the whole document. A generic image OCR client does not provide PDF rendering. PDF 第 {number} 页含图像或 Form 内容但未提取到文字，请选择支持 PDF 的 OCR 文档解析后端并配置服务地址、凭据和模型后重新解析全文。"
                );
            }
        }
        let tagged = pdf_text::tagged_content(&lines);
        if !tagged.trim().is_empty() {
            return Ok(tagged);
        }
        let mut text = String::new();

        // Try OCR first if client is available. A probe that fails is reported instead of
        // being folded into `false`: without the log line a scanned PDF becomes
        // "DONE, 0 chunks" and nothing says the OCR backend was unreachable.
        let use_ocr = match self.ocr.as_ref() {
            Some(ocr) => match ocr.health().await {
                Ok(true) => true,
                Ok(false) => {
                    tracing::warn!("OCR backend reports unhealthy; parsing this PDF without OCR");
                    false
                }
                Err(error) => {
                    tracing::warn!(%error, "OCR health check failed; parsing this PDF without OCR");
                    false
                }
            },
            None => false,
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
                    let page_text = content_to_text(&content);
                    let next_size = (text.len() as u64)
                        .saturating_add(page_text.len() as u64)
                        .saturating_add(1);
                    anyhow::ensure!(
                        next_size <= super::pdf_stream::stream_limit_bytes(),
                        "PDF page {page_num} fallback text exceeds the RAYRAG_PDF_STREAM_LIMIT_BYTES document budget. Repair or export the PDF with embedded fonts and upload it again, or select a configured OCR parser."
                    );
                    text.push_str(&page_text);
                    text.push('\n');
                }
                // A refusal must be visible: silence here would look like an empty page.
                Err(error) => return Err(error),
            }
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
fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
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


