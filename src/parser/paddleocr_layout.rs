//! PaddleOCR Layout parser — RAGFlow `deepdoc/parser/paddleocr_parser.py`
//! layout-recognition flow, in Rust.
//!
//! Pipeline (mirrors `PaddleOCRParser.parse_pdf` → `_transfer_to_sections`):
//!   1. Submit the file to the RAGFlow PaddleOCR async job endpoint and poll
//!      for the JSONL result (transport lives in `crate::ocr`).
//!   2. Parse `layoutParsingResults[].prunedResult.parsing_res_list[]` into
//!      structured blocks — page index, `block_label`, image-stripped
//!      `block_content`, and a normalized bbox (`_normalize_bbox`).
//!   3. Render each block as a section carrying RAGFlow's position tag
//!      `@@{page}\t{left/2}\t{right/2}\t{top/2}\t{bottom/2}##`
//!      (ZOOMIN = 2, integer division), honoring `parse_method`:
//!      - `raw`      → `(block_content, tag)`       tuples upstream
//!      - `paper`    → `(block_content + tag, label)`
//!      - `manual` / `pipeline` → `(block_content, label, tag)`
//!   4. `extract_positions` recovers tagged boxes from serialized text —
//!      the input contract of upstream `crop()`.
//!
//! The `Parse` impl degrades gracefully: with a configured PaddleOCR backend
//! it runs the full flow; otherwise it falls back to the legacy OCR proxy
//! text (layout markers only); without any OCR it reports the file bytes.

use crate::ocr::{OcrClient, PaddleLayoutBlock, PaddleOcrConfig};
use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// Mirrors `paddleocr_parser.py _ZOOMIN` — position tags are divided by 2.
pub const ZOOMIN: i64 = 2;

/// `parse_method` values accepted by `PaddleOCRParser.parse_pdf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseMethod {
    /// `(block_content, tag)` — default.
    Raw,
    /// `(block_content + tag, label)` — tag appended to the content text.
    Paper,
    /// `(block_content, label, tag)`.
    Manual,
    /// `(block_content, label, tag)`.
    Pipeline,
}

impl ParseMethod {
    /// Map a RAGFlow `parse_method` string; anything unknown → `Raw`.
    pub fn from_str(value: &str) -> Self {
        match value {
            "manual" => Self::Manual,
            "pipeline" => Self::Pipeline,
            "paper" => Self::Paper,
            _ => Self::Raw,
        }
    }
}

/// A rendered layout section — the Rust counterpart of upstream
/// `_transfer_to_sections` output tuples `(content[, label], tag)`.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutSection {
    pub content: String,
    pub label: String,
    pub tag: String,
}

/// PaddleOCR layout parser. `ocr` carries the configured OCR backend.
#[derive(Default)]
pub struct PaddleOcrLayoutParser {
    ocr: Option<OcrClient>,
}

impl PaddleOcrLayoutParser {
    pub fn new() -> Self {
        Self { ocr: None }
    }

    /// Attach an OCR client (legacy proxy or PaddleOCR backend).
    pub fn with_ocr(ocr: OcrClient) -> Self {
        Self { ocr: Some(ocr) }
    }

    /// Resolve the OCR backend from `RAYRAG_OCR_PROVIDER` (see
    /// [`OcrClient::from_env`]); `Ok(None)` when OCR is disabled.
    pub fn from_env() -> Result<Option<Self>> {
        Ok(OcrClient::from_env()?.map(|ocr| Self { ocr: Some(ocr) }))
    }

    /// Full asynchronous layout flow against an explicitly supplied
    /// PaddleOCR configuration (mirrors upstream `PaddleOCRParser`).
    pub async fn parse_with_config(
        &self,
        name: &str,
        data: &[u8],
        config: &PaddleOcrConfig,
        method: ParseMethod,
    ) -> Result<Document> {
        let client = OcrClient::paddleocr(config.clone())?;
        let blocks = client.paddleocr_layout_file(name, data).await?;
        Ok(assemble_document(
            name,
            data,
            &blocks,
            method,
            LayoutKind::PaddleOcr,
        ))
    }

    /// Full asynchronous layout flow against the configured client.
    /// Falls back to plain OCR text when the backend is the legacy proxy.
    pub async fn parse_async(
        &self,
        name: &str,
        data: &[u8],
        method: ParseMethod,
    ) -> Result<Document> {
        let client = self.ocr.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "[PaddleOCR] no OCR client configured; set RAYRAG_OCR_PROVIDER=paddleocr"
            )
        })?;
        if !client.is_paddleocr() {
            // Legacy GPU proxy: no layout metadata, keep the historical
            // layout-marker wrapper over plain OCR text.
            let text = client.ocr_file(name, data).await?;
            let content = if text.trim().is_empty() {
                format!(
                    "<!--IMAGE_START-->\n[No text detected in image: {name}]\n<!--IMAGE_END-->\n"
                )
            } else {
                format!(
                    "<!--IMAGE_START-->\n[DOCUMENT_LAYOUT: {}]\n{}\n<!--IMAGE_END-->\n",
                    detect_layout_type(&text),
                    text.trim(),
                )
            };
            return Ok(new_document(
                name,
                content,
                mime_from_name(name),
                data.len(),
            ));
        }
        let blocks = client.paddleocr_layout_file(name, data).await?;
        Ok(assemble_document(
            name,
            data,
            &blocks,
            method,
            LayoutKind::PaddleOcr,
        ))
    }
}

impl Parse for PaddleOcrLayoutParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(self.parse_async(name, data, ParseMethod::Raw))
    }
}

/// Which OCR source produced the document (affects the layout header).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LayoutKind {
    PaddleOcr,
}

fn mime_from_name(name: &str) -> &'static str {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        _ => "application/octet-stream",
    }
}

fn assemble_document(
    name: &str,
    data: &[u8],
    blocks: &[PaddleLayoutBlock],
    method: ParseMethod,
    _kind: LayoutKind,
) -> Document {
    let sections = render_sections(blocks, method);
    let mut content = String::from("<!--IMAGE_START-->\n[DOCUMENT_LAYOUT: paddleocr]\n");
    content.push_str(&format!("[LAYOUT_SUMMARY: {}]\n", layout_summary(blocks)));
    for section in sections {
        match method {
            // upstream raw: (content, tag) — serialize tag on its own line so
            // extract_positions can recover it.
            ParseMethod::Raw => {
                content.push_str(&section.content);
                content.push('\n');
                content.push_str(&section.tag);
                content.push('\n');
            }
            ParseMethod::Paper => {
                // upstream paper: content + tag concatenated.
                content.push_str(&section.content);
                content.push_str(&section.tag);
                content.push('\n');
            }
            ParseMethod::Manual | ParseMethod::Pipeline => {
                if !section.label.is_empty() {
                    content.push_str(&format!("[{}] ", section.label));
                }
                content.push_str(&section.content);
                content.push('\n');
                content.push_str(&section.tag);
                content.push('\n');
            }
        }
    }
    content.push_str("<!--IMAGE_END-->\n");
    new_document(name, content, mime_from_name(name), data.len())
}

/// Render layout blocks into sections with RAGFlow position tags — mirrors
/// `paddleocr_parser.py _transfer_to_sections` (ZOOMIN division included).
pub fn render_sections(blocks: &[PaddleLayoutBlock], method: ParseMethod) -> Vec<LayoutSection> {
    let mut sections = Vec::new();
    for block in blocks {
        let content = block.content.trim();
        if content.is_empty() {
            continue;
        }
        let tag = position_tag(block);
        let label = block.label.clone();
        match method {
            ParseMethod::Raw => sections.push(LayoutSection {
                content: content.to_owned(),
                label: String::new(),
                tag,
            }),
            ParseMethod::Paper => sections.push(LayoutSection {
                content: format!("{content}{tag}"),
                label,
                tag,
            }),
            ParseMethod::Manual | ParseMethod::Pipeline => sections.push(LayoutSection {
                content: content.to_owned(),
                label,
                tag,
            }),
        }
    }
    sections
}

/// `@@{page}\t{left/2}\t{right/2}\t{top/2}\t{bottom/2}##` — mirrors the
/// upstream tag template (`left // _ZOOMIN`, integer division).
pub fn position_tag(block: &PaddleLayoutBlock) -> String {
    let div = |value: f64| (value / ZOOMIN as f64).floor() as i64;
    format!(
        "@@{}\t{}\t{}\t{}\t{}##",
        block.page,
        div(block.left),
        div(block.right),
        div(block.top),
        div(block.bottom),
    )
}

/// Recover tagged positions from serialized text — mirrors
/// `paddleocr_parser.py extract_positions`.
///
/// Returns `(pages, left, right, top, bottom)` where `pages` holds the
/// **0-based** page indices (upstream subtracts 1 from the 1-based tags);
/// a `1-3` page range expands to `[0, 1, 2]`.
pub fn extract_positions(text: &str) -> Vec<(Vec<u32>, f64, f64, f64, f64)> {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag =
        TAG.get_or_init(|| Regex::new(r"@@[0-9-]+\t[0-9.\t]+##").expect("position tag regex"));
    let mut poss = Vec::new();
    for found in tag.find_iter(text) {
        let raw = found.as_str().trim_matches(['#', '@']);
        let mut fields = raw.split('\t');
        let page_field = fields.next().unwrap_or("");
        let number = |field: Option<&str>| {
            field
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let (left, right, top, bottom) = (
            number(fields.next()),
            number(fields.next()),
            number(fields.next()),
            number(fields.next()),
        );
        let pages: Vec<u32> = page_field
            .split('-')
            .filter_map(|part| part.parse::<i64>().ok())
            .map(|page| page.saturating_sub(1) as u32)
            .collect();
        poss.push((pages, left, right, top, bottom));
    }
    poss
}

/// Dominant layout kind derived from block labels (e.g. `text`, `table`,
/// `figure`, `title`). Empty/unknown labels count as `text`; no blocks at
/// all yields `unknown`.
pub fn layout_summary(blocks: &[PaddleLayoutBlock]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for block in blocks {
        let label = if block.label.is_empty() {
            "text"
        } else {
            block.label.as_str()
        };
        *counts.entry(label).or_insert(0) += 1;
    }
    if counts.is_empty() {
        return "unknown".to_owned();
    }
    counts
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(label, _)| label.to_string())
        .unwrap_or_else(|| "mixed".to_owned())
}

/// Heuristic layout classification for plain OCR text (legacy-proxy path;
/// superseded by `layout_summary` when block labels are available).
fn detect_layout_type(text: &str) -> &'static str {
    if text.contains("Chapter") || text.contains("Section") {
        "book_page"
    } else if text.contains("Table") || text.contains('|') {
        "table"
    } else if text.lines().count() > 10 && text.len() > 500 {
        "full_page"
    } else if text.len() < 200 {
        "caption_or_label"
    } else {
        "mixed_content"
    }
}

// ---------------------------------------------------------------------------
// Table / figure extraction — pdf_parser.py `_extract_table_figure`
// (1206-1258), content-only subset.
//
// The upstream layout pass crops boxes whose label is `table` (and, when
// images are wanted, `figure`/`image`) out of the merged reading order so
// table-recognition and image-captioning stages can process them separately.
// The Rust equivalent keeps the already-parsed layout blocks but returns the
// two groups independently, each section still carrying its RAGFlow position
// tag (`@@page\t...##`) so downstream code can crop the exact region.
// ---------------------------------------------------------------------------

/// `_extract_table_figure` content split: returns `(tables, figures)` where
/// each `LayoutSection` keeps the original label and position tag. Blocks
/// with empty content and blocks of any other label are dropped.
pub fn table_figure_sections(
    blocks: &[PaddleLayoutBlock],
) -> (Vec<LayoutSection>, Vec<LayoutSection>) {
    let mut tables = Vec::new();
    let mut figures = Vec::new();
    for block in blocks {
        let content = block.content.trim();
        if content.is_empty() {
            continue;
        }
        let section = LayoutSection {
            content: content.to_owned(),
            label: block.label.clone(),
            tag: position_tag(block),
        };
        match block.label.to_lowercase().as_str() {
            "table" => tables.push(section),
            "figure" | "image" => figures.push(section),
            _ => {}
        }
    }
    (tables, figures)
}


