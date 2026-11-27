//! Positioned PDF text extraction — the geometry half of RAGFlow's
//! `deepdoc/parser/pdf_parser.py`.
//!
//! Upstream reads text with pdfplumber, keeps every layout box as
//! `{"page_number", "x0", "x1", "top", "bottom"}` (points, `top` measured down
//! from the top of the page) and appends a position tag to each box's text:
//!
//! ```text
//! @@{page}\t{x0:.1f}\t{x1:.1f}\t{top:.1f}\t{bottom:.1f}##
//! ```
//!
//! (`pdf_parser.py::_line_tag`; a box that spills onto the next page carries the
//! range `page-page+1`.) `naive_merge` then strips the tags from the visible
//! content and projects them into the `position_int` / `page_num_int` /
//! `top_int` chunk metadata, which is what the chunk and retrieval APIs return
//! and what the web previewer overlays on the rendered page.
//!
//! RayRAG has no layout model, so the boxes here are **text line** boxes built
//! from the content stream itself: every `Tj`/`TJ`/`'`/`"` run is positioned
//! through the text and graphics matrices, runs sharing a baseline merge into a
//! line, and the line box is the union of its runs. Font metrics come from the
//! embedded `/Widths` (simple fonts) or `/W` + `/DW` (CID fonts), and text is
//! decoded through `/ToUnicode` when the font ships one — so subset Identity-H
//! fonts no longer come out as glyph indices.

use lopdf::{Dictionary, Document, Object};
use std::collections::HashMap;

/// One extracted text line with its page-local box in PDF points.
#[derive(Debug, Clone, PartialEq)]
pub struct PositionedLine {
    /// 1-based page number, matching the tag convention.
    pub page: usize,
    /// Left edge (points from the page's left edge).
    pub x0: f64,
    /// Right edge.
    pub x1: f64,
    /// Distance from the page's top edge to the top of the line.
    pub top: f64,
    /// Distance from the page's top edge to the bottom of the line.
    pub bottom: f64,
    /// The line's text.
    pub text: String,
    /// Height of the page the line sits on, in points. The paragraph merge
    /// needs it to rebuild upstream's cumulative page heights.
    pub page_height: f64,
}

impl PositionedLine {
    /// Upstream `_line_tag`: one tag per box, one decimal place.
    ///
    /// Coordinates are clamped at zero because the tag regex both upstream
    /// (`@@[0-9-]+\t[0-9.\t]+##`) and in [`crate::chunk::position`] refuses a
    /// minus sign inside the coordinate fields; a glyph poking out of the page
    /// box would otherwise silently lose its tag.
    pub fn tag(&self) -> String {
        format!(
            "@@{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}##",
            self.page,
            self.x0.max(0.0),
            self.x1.max(0.0),
            self.top.max(0.0),
            self.bottom.max(0.0)
        )
    }

    /// `text@@page\t...##` — the unit `naive_merge` splits and re-merges.
    pub fn tagged_text(&self) -> String {
        format!("{}{}", self.text, self.tag())
    }
}

/// Zoom the upstream parser renders page images at (`RAGFlowPdfParser.__call__`
/// passes `zoomin=3`). Page boxes are measured in points, so the same factor
/// scales them into the pixel heights `_line_tag` compares against.
const ZOOM: f64 = 3.0;

/// Assemble the parsed document content the way `rag/flow/parser/parser.py`
/// does: merge the line boxes into paragraph boxes (`_concat_downward` sorts,
/// `_naive_vertical_merge` joins lines that belong together) and append
/// `_line_tag`'s position tag to every surviving box.
///
/// The result is the text `naive_merge` chunks: it splits on newlines, merges
/// sections up to the token budget, strips the tags from the visible text and
/// projects them into each chunk's `position_int`.
pub fn tagged_content(lines: &[PositionedLine]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let page_count = lines.iter().map(|line| line.page).max().unwrap_or(1);
    let mut page_heights = vec![0.0_f64; page_count];
    for line in lines {
        if let Some(slot) = page_heights.get_mut(line.page.saturating_sub(1)) {
            *slot = slot.max(line.page_height);
        }
    }
    // `page_cum_height` is the running total *before* each page, in points.
    let mut cumulative = vec![0.0_f64; page_count];
    let mut running = 0.0;
    for (index, height) in page_heights.iter().enumerate() {
        cumulative[index] = running;
        running += height;
    }
    let page_heights_px: Vec<f64> = page_heights.iter().map(|height| height * ZOOM).collect();

    let mut boxes: Vec<crate::parser::pdfbox::PdfBox> = lines
        .iter()
        .map(|line| crate::parser::pdfbox::PdfBox {
            top: line.top + cumulative[line.page.saturating_sub(1)],
            bottom: line.bottom + cumulative[line.page.saturating_sub(1)],
            x0: line.x0,
            x1: line.x1,
            text: line.text.clone(),
            page_number: line.page,
            ..crate::parser::pdfbox::PdfBox::default()
        })
        .collect();

    let mean_height: Vec<f64> = (1..=page_count)
        .map(|page| median_of(&boxes, page, |b| b.height()))
        .collect();
    let mean_width: Vec<f64> = (1..=page_count)
        .map(|page| median_of(&boxes, page, |b| b.char_width()))
        .collect();

    crate::parser::pdfbox::concat_downward(&mut boxes);
    let english = is_english(&boxes);
    crate::parser::pdfbox::naive_vertical_merge(&mut boxes, &mean_height, &mean_width, english);

    boxes
        .iter()
        .map(|b| {
            format!(
                "{}{}",
                b.text,
                crate::parser::pdfbox::line_tag(b, ZOOM, &cumulative, &page_heights_px)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Median of a per-page box measurement, the statistic upstream stores as
/// `mean_height` / `mean_width` (`pdf_parser.py:1618-1619`).
fn median_of(
    boxes: &[crate::parser::pdfbox::PdfBox],
    page: usize,
    measure: impl Fn(&crate::parser::pdfbox::PdfBox) -> f64,
) -> f64 {
    let mut values: Vec<f64> = boxes
        .iter()
        .filter(|b| b.page_number == page)
        .map(&measure)
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect();
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    values[values.len() / 2]
}

/// Upstream samples a page's characters and asks whether they contain a run of
/// 30+ Latin-ish characters (`pdf_parser.py:1585-1592`), then applies that flag
/// to the whole document. RayRAG reads every box instead of sampling, so the
/// answer is deterministic: a document counts as English when its text holds
/// such a run.
fn is_english(boxes: &[crate::parser::pdfbox::PdfBox]) -> bool {
    let mut run = 0_usize;
    for ch in boxes.iter().flat_map(|b| b.text.chars()) {
        if is_english_char(ch) {
            run += 1;
            if run >= 30 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

fn is_english_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            ' ' | ','
                | '/'
                | '¸'
                | ';'
                | ':'
                | '\''
                | '['
                | ']'
                | '('
                | ')'
                | '!'
                | '@'
                | '#'
                | '$'
                | '%'
                | '^'
                | '&'
                | '*'
                | '"'
                | '?'
                | '<'
                | '>'
                | '.'
                | '_'
                | '-'
                | '\n'
        )
}

/// Compatibility entry point returning no lines when extraction fails.
pub fn extract_positioned_lines(doc: &Document) -> Vec<PositionedLine> {
    try_extract_positioned_lines(doc).unwrap_or_else(|error| {
        tracing::warn!(%error, "PDF text extraction failed");
        Vec::new()
    })
}

/// Displayed page-box dimensions in PDF points, in page-number order.
///
/// RAGFlow's Mistral parser renders page images at 72 DPI (zoomin=1), so the
/// rendered pixel size equals this page box (with rotation applied). Exposing
/// the boxes lets position tags use the same coordinate space without pulling
/// a PDF renderer into the binary.
pub(crate) fn page_box_sizes(doc: &Document) -> Vec<(u32, f64, f64)> {
    let mut pages: Vec<(u32, lopdf::ObjectId)> = doc
        .get_pages()
        .iter()
        .map(|(number, id)| (*number, *id))
        .collect();
    pages.sort_by_key(|(number, _)| *number);
    let mut sizes = Vec::with_capacity(pages.len());
    for (number, page_id) in pages {
        let Some(geometry) = PageGeometry::of(doc, page_id) else {
            continue;
        };
        let rotation = match inherited(doc, page_id, b"Rotate") {
            Some(value) => value.as_i64().ok(),
            None => Some(0),
        };
        let Some(geometry) = rotation.and_then(|rotation| geometry.rotated(rotation)) else {
            continue;
        };
        sizes.push((number, geometry.width, geometry.height));
    }
    sizes
}

/// Extract every text line, rejecting unreadable page streams before indexing.
pub fn try_extract_positioned_lines(doc: &Document) -> crate::Result<Vec<PositionedLine>> {
    let mut lines = Vec::new();
    let resource_limit = super::pdf_stream::stream_limit_bytes();
    let mut remaining_text_bytes = resource_limit;
    let mut pages: Vec<(u32, lopdf::ObjectId)> = doc
        .get_pages()
        .iter()
        .map(|(number, id)| (*number, *id))
        .collect();
    pages.sort_by_key(|(number, _id)| *number);
    for (number, page_id) in pages {
        // Capped decode (see `pdf_stream`): an inflated page stream must not decide how
        // much memory this process uses.
        let content = super::pdf_stream::page_content_limited(
            doc,
            page_id,
            super::pdf_stream::stream_limit_bytes(),
            &format!("PDF page {number} content"),
        )
        .and_then(|bytes| {
            let (stripped, _) = super::pdf_inline::strip_inline_images(
                &bytes,
                &format!("PDF page {number} content"),
            )?;
            lopdf::content::Content::decode(&stripped)
                .map_err(|error| anyhow::anyhow!("PDF page {number}: {error}"))
        })
        .map_err(|error| anyhow::anyhow!(
            "PDF page {number} text extraction failed: {error}. Repair or export an unencrypted PDF and upload it again, or select a configured OCR document parser and reparse. PDF 页面提取失败，请修复并重新导出上传，或配置 OCR 解析客户端后重新解析。"
        ))?;
        let geometry = PageGeometry::of(doc, page_id).ok_or_else(|| {
            anyhow::anyhow!(
                "PDF page {number} has no valid CropBox or MediaBox geometry. Repair or export the PDF with a valid page box and upload it again. PDF 页面缺少有效的 CropBox/MediaBox，请修复页面尺寸后重新导出并上传。"
            )
        })?;
        let rotation = match inherited(doc, page_id, b"Rotate") {
            Some(value) => value.as_i64().ok(),
            None => Some(0),
        };
        let geometry = rotation.and_then(|rotation| geometry.rotated(rotation)).ok_or_else(|| {
            anyhow::anyhow!(
                "PDF page {number} has an invalid Rotate value. Use a multiple of 90 degrees, repair or export the PDF, and upload it again. PDF 页面旋转角度无效，请修复为 90 度的整数倍后重新导出上传。"
            )
        })?;
        let mut runs = Vec::new();
        let mut state = State {
            ctm: geometry.page_transform,
            ..State::default()
        };
        let resources = inherited(doc, page_id, b"Resources").and_then(|value| as_dict(doc, value));
        let mut interpreter = ContentInterpreter {
            doc,
            page_resources: resources,
            forms: Vec::new(),
            depth: 0,
            remaining_bytes: resource_limit,
            remaining_operations: 1_000_000,
            text_budget: PdfTextBudget {
                limit: resource_limit,
                font_mapping_bytes: resource_limit,
                run_bytes: resource_limit,
                output_bytes: &mut remaining_text_bytes,
            },
        };
        interpreter.walk(&content, resources, &mut state, &mut runs)
            .map_err(|error| anyhow::anyhow!(
                "PDF page {number} text extraction failed: {error}. Repair or export an unencrypted PDF with embedded fonts and upload it again, or select a configured OCR document parser and reparse. PDF 页面提取失败，请修复并嵌入字体重新导出上传，或配置 OCR 解析客户端后重新解析。"
            ))?;
        drop(interpreter);
        drop(state);
        drop(content);
        let raw_text_bytes = runs.iter().map(|run| run.text.len() as u64).sum::<u64>();
        let page_lines = geometry.lines(number as usize, runs);
        let visible_text_bytes = page_lines
            .iter()
            .map(|line| line.text.len() as u64)
            .sum::<u64>();
        let metadata_bytes = page_lines
            .iter()
            .map(|line| {
                (line.text.capacity() - line.text.len()) as u64
                    + 2 * std::mem::size_of::<PositionedLine>() as u64
            })
            .sum::<u64>();
        let cost = metadata_bytes.saturating_add(visible_text_bytes.saturating_sub(raw_text_bytes));
        remaining_text_bytes = remaining_text_bytes.checked_sub(cost).ok_or_else(|| anyhow::anyhow!("PDF page {number} position metadata exceeds the RAYRAG_PDF_STREAM_LIMIT_BYTES document budget. Split or export a smaller PDF and upload it again."))?;
        lines.extend(page_lines);
    }
    Ok(lines)
}

// ---------------------------------------------------------------------------
// geometry
// ---------------------------------------------------------------------------

/// 2-D affine transform in PDF order `[a b c d e f]`, row-vector convention:
/// `(x, y)` maps to `(a·x + c·y + e, b·x + d·y + f)`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Matrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Matrix {
    const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Matrix {
        Matrix { a, b, c, d, e, f }
    }

    fn translate(tx: f64, ty: f64) -> Matrix {
        Matrix {
            e: tx,
            f: ty,
            ..Matrix::IDENTITY
        }
    }

    /// Row-vector composition: apply `self` first, then `other`.
    fn mul(self, other: Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    fn apply(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }
}

/// The page box, plus the projection from PDF user space (origin bottom-left)
/// to the top-down page-local points the position tags carry.
#[derive(Debug, Clone, Copy)]
struct PageGeometry {
    origin_x: f64,
    top_edge: f64,
    width: f64,
    page_transform: Matrix,
    /// Page height in points, for the cumulative heights `_line_tag` needs.
    height: f64,
}

impl PageGeometry {
    fn of(doc: &Document, page_id: lopdf::ObjectId) -> Option<Self> {
        let from_box = |key: &[u8]| {
            let rect = inherited(doc, page_id, key)?;
            let numbers = rect.as_array().ok()?;
            if numbers.len() != 4 {
                return None;
            }
            let values: [f64; 4] = std::array::from_fn(|index| {
                deref(doc, &numbers[index])
                    .and_then(as_number)
                    .unwrap_or(f64::NAN)
            });
            if !values.iter().all(|value| value.is_finite()) {
                return None;
            }
            // PDF rectangles may specify either pair of diagonally opposite corners.
            let [x0, y0, x1, y1] = values;
            let (left, right) = (x0.min(x1), x0.max(x1));
            let (bottom, top) = (y0.min(y1), y0.max(y1));
            if right == left || top == bottom {
                return None;
            }
            Some(PageGeometry {
                origin_x: left,
                top_edge: top,
                width: right - left,
                page_transform: Matrix::IDENTITY,
                height: top - bottom,
            })
        };
        from_box(b"CropBox").or_else(|| from_box(b"MediaBox"))
    }

    fn rotated(mut self, rotation: i64) -> Option<Self> {
        let left = self.origin_x;
        let right = left + self.width;
        let top = self.top_edge;
        let bottom = top - self.height;
        let rotation = rotation.rem_euclid(360);
        self.page_transform = match rotation {
            0 => return Some(self),
            90 => Matrix::new(0.0, -1.0, 1.0, 0.0, -bottom, right),
            180 => Matrix::new(-1.0, 0.0, 0.0, -1.0, right, top),
            270 => Matrix::new(0.0, 1.0, -1.0, 0.0, top, -left),
            _ => return None,
        };
        if rotation != 180 {
            std::mem::swap(&mut self.width, &mut self.height);
        }
        self.origin_x = 0.0;
        self.top_edge = self.height;
        Some(self)
    }

    fn project_x(&self, x: f64) -> f64 {
        x - self.origin_x
    }

    fn project_top(&self, y: f64) -> f64 {
        self.top_edge - y
    }

    /// Group runs into lines (baseline proximity, pdfplumber's few-point
    /// `y_tolerance`) and give each line its union box, top-down.
    fn lines(&self, page: usize, runs: Vec<Run>) -> Vec<PositionedLine> {
        if runs.is_empty() {
            return Vec::new();
        }
        let mut runs = runs;
        // Reading order is top-down: higher device y first, then left to right.
        runs.sort_by(|left, right| {
            right
                .baseline
                .partial_cmp(&left.baseline)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(
                    left.x0
                        .partial_cmp(&right.x0)
                        .unwrap_or(std::cmp::Ordering::Equal),
                )
        });

        let mut lines = Vec::new();
        let mut current: Vec<Run> = Vec::new();
        for run in runs {
            let same_line = current.last().is_some_and(|last| {
                let tolerance = (last.size * 0.4).clamp(2.0, 6.0);
                (last.baseline - run.baseline).abs() <= tolerance
            });
            if !same_line
                && !current.is_empty()
                && let Some(line) = self.merge_runs(page, std::mem::take(&mut current))
            {
                lines.push(line);
            }
            current.push(run);
        }
        if let Some(line) = self.merge_runs(page, current) {
            lines.push(line);
        }
        lines.retain(|line| !line.text.trim().is_empty());
        lines
    }

    fn merge_runs(&self, page: usize, mut runs: Vec<Run>) -> Option<PositionedLine> {
        if runs.is_empty() {
            return None;
        }
        runs.sort_by(|left, right| {
            left.x0
                .partial_cmp(&right.x0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut text = String::new();
        let mut previous_end: Option<(f64, f64)> = None;
        for run in &runs {
            if let Some((end, size)) = previous_end {
                let gap = run.x0 - end;
                let trimmed = run.text.trim_start();
                if gap > (size * 0.25).max(1.0) && !text.ends_with(' ') && !trimmed.is_empty() {
                    text.push(' ');
                }
            }
            text.push_str(&run.text);
            previous_end = Some((run.x1, run.size));
        }
        let text = text.trim().to_owned();
        if text.is_empty() {
            return None;
        }
        let x0 = runs.iter().fold(f64::INFINITY, |acc, run| acc.min(run.x0));
        let x1 = runs
            .iter()
            .fold(f64::NEG_INFINITY, |acc, run| acc.max(run.x1));
        let top = runs.iter().fold(f64::INFINITY, |acc, run| {
            acc.min(self.project_top(run.y_max))
        });
        let bottom = runs.iter().fold(f64::NEG_INFINITY, |acc, run| {
            acc.max(self.project_top(run.y_min))
        });
        Some(PositionedLine {
            page,
            x0: self.project_x(x0),
            x1: self.project_x(x1),
            top,
            bottom,
            text,
            page_height: self.height,
        })
    }
}

// ---------------------------------------------------------------------------
// content stream interpretation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct TextState {
    font: Option<std::sync::Arc<FontMetrics>>,
    size: f64,
    char_spacing: f64,
    word_spacing: f64,
    horizontal_scale: f64,
    leading: f64,
    rise: f64,
}

#[derive(Debug, Clone)]
struct State {
    ctm: Matrix,
    text: TextState,
    tm: Matrix,
    tlm: Matrix,
}

impl Default for State {
    fn default() -> Self {
        State {
            ctm: Matrix::IDENTITY,
            text: TextState {
                horizontal_scale: 100.0,
                ..TextState::default()
            },
            tm: Matrix::IDENTITY,
            tlm: Matrix::IDENTITY,
        }
    }
}

/// A positioned run of text, in device space, before line grouping.
#[derive(Debug, Clone)]
struct Run {
    text: String,
    x0: f64,
    x1: f64,
    /// Lowest device y of the run's box.
    y_min: f64,
    /// Highest device y of the run's box.
    y_max: f64,
    /// Device y of the baseline, for line grouping.
    baseline: f64,
    size: f64,
}

fn as_number(object: &Object) -> Option<f64> {
    match object {
        Object::Integer(value) => Some(*value as f64),
        Object::Real(value) => Some(f64::from(*value)),
        _ => None,
    }
}

fn operand_number(operands: &[Object], index: usize) -> Option<f64> {
    operands.get(index).and_then(as_number)
}

fn operand_matrix(operands: &[Object]) -> Option<Matrix> {
    Some(Matrix::new(
        operand_number(operands, 0)?,
        operand_number(operands, 1)?,
        operand_number(operands, 2)?,
        operand_number(operands, 3)?,
        operand_number(operands, 4)?,
        operand_number(operands, 5)?,
    ))
}

struct PdfTextBudget<'a> {
    limit: u64,
    font_mapping_bytes: u64,
    run_bytes: u64,
    output_bytes: &'a mut u64,
}

struct ContentInterpreter<'a> {
    doc: &'a Document,
    page_resources: Option<&'a Dictionary>,
    forms: Vec<lopdf::ObjectId>,
    depth: usize,
    remaining_bytes: u64,
    remaining_operations: usize,
    text_budget: PdfTextBudget<'a>,
}

impl ContentInterpreter<'_> {
    fn walk(
        &mut self,
        content: &lopdf::content::Content,
        resources: Option<&Dictionary>,
        state: &mut State,
        runs: &mut Vec<Run>,
    ) -> crate::Result<()> {
        let mut fonts = HashMap::new();
        let mut stack = Vec::new();
        for operation in &content.operations {
            self.remaining_operations =
                self.remaining_operations.checked_sub(1).ok_or_else(|| {
                    anyhow::anyhow!("PDF Form expansion exceeds the page operation limit")
                })?;
            if operation.operator != "Do" {
                apply_operation(
                    self.doc,
                    resources,
                    &mut fonts,
                    &mut stack,
                    state,
                    runs,
                    operation,
                    &mut self.text_budget,
                )?;
                continue;
            }
            let name = operation
                .operands
                .first()
                .and_then(|value| value.as_name().ok())
                .ok_or_else(|| anyhow::anyhow!("PDF Do operation has no XObject name"))?;
            self.draw_form(name, resources, state, runs)
                .map_err(|error| {
                    anyhow::anyhow!("XObject {}: {error}", String::from_utf8_lossy(name))
                })?;
        }
        Ok(())
    }

    fn draw_form(
        &mut self,
        name: &[u8],
        resources: Option<&Dictionary>,
        state: &State,
        runs: &mut Vec<Run>,
    ) -> crate::Result<()> {
        let objects = resources
            .and_then(|resources| dict_entry(self.doc, resources, b"XObject"))
            .ok_or_else(|| anyhow::anyhow!("PDF XObject resources are missing"))?;
        let object = objects.get(name)?;
        let stream = deref(self.doc, object)
            .ok_or_else(|| anyhow::anyhow!("PDF XObject reference cannot be resolved"))?
            .as_stream()?;
        if stream.dict.get(b"Subtype")?.as_name()? != b"Form" {
            return Ok(());
        }
        let id = object.as_reference().ok();
        if id.is_some_and(|id| self.forms.contains(&id)) {
            anyhow::bail!("PDF Form has a cyclic XObject reference");
        }
        if self.depth >= 32 {
            anyhow::bail!("PDF Form nesting exceeds 32 levels");
        }
        let bytes = super::pdf_stream::plain_content_limited_in(
            self.doc,
            stream,
            self.remaining_bytes,
            "PDF Form content",
        )?;
        self.remaining_bytes -= bytes.len() as u64;
        let (stripped, _) = super::pdf_inline::strip_inline_images(&bytes, "PDF Form content")?;
        let content = lopdf::content::Content::decode(&stripped)?;
        drop(stripped);
        drop(bytes);
        let form_resources = match stream.dict.get(b"Resources") {
            Ok(value) => Some(
                as_dict(self.doc, value)
                    .ok_or_else(|| anyhow::anyhow!("PDF Form Resources is not a dictionary"))?,
            ),
            Err(_) => self.page_resources,
        };
        let mut form_state = state.clone();
        if let Ok(value) = stream.dict.get(b"Matrix") {
            let values = deref(self.doc, value)
                .ok_or_else(|| anyhow::anyhow!("PDF Form Matrix reference cannot be resolved"))?
                .as_array()?;
            if values.len() != 6 {
                anyhow::bail!("PDF Form Matrix must contain six numbers");
            }
            let matrix = operand_matrix(values)
                .ok_or_else(|| anyhow::anyhow!("PDF Form Matrix must contain six numbers"))?;
            form_state.ctm = matrix.mul(form_state.ctm);
        }
        // Form execution implicitly saves and restores the caller's graphics state.
        if let Some(id) = id {
            self.forms.push(id);
        }
        self.depth += 1;
        let result = self.walk(&content, form_resources, &mut form_state, runs);
        self.depth -= 1;
        if id.is_some() {
            self.forms.pop();
        }
        result
    }
}

fn apply_operation(
    doc: &Document,
    resources: Option<&Dictionary>,
    fonts: &mut HashMap<Vec<u8>, std::sync::Arc<FontMetrics>>,
    stack: &mut Vec<State>,
    state: &mut State,
    runs: &mut Vec<Run>,
    operation: &lopdf::content::Operation,
    budget: &mut PdfTextBudget<'_>,
) -> crate::Result<()> {
    let operands = &operation.operands;
    match operation.operator.as_str() {
        // `q`/`Q` save and restore the whole graphics state, text state included.
        "q" => stack.push(state.clone()),
        "Q" => {
            if let Some(previous) = stack.pop() {
                *state = previous;
            }
        }
        "cm" => {
            if let Some(matrix) = operand_matrix(operands) {
                state.ctm = matrix.mul(state.ctm);
            }
        }
        "BT" => {
            state.tm = Matrix::IDENTITY;
            state.tlm = Matrix::IDENTITY;
        }
        "ET" => {}
        "gs" => {
            let name = operands
                .first()
                .and_then(|value| value.as_name().ok())
                .ok_or_else(|| anyhow::anyhow!("PDF gs operation has no ExtGState name"))?;
            let graphics = resources
                .and_then(|resources| dict_entry(doc, resources, b"ExtGState"))
                .and_then(|states| states.get(name).ok())
                .and_then(|value| as_dict(doc, value))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "PDF ExtGState {} is missing or invalid",
                        String::from_utf8_lossy(name)
                    )
                })?;
            if let Ok(value) = graphics.get(b"Font") {
                let value = deref(doc, value).ok_or_else(|| {
                    anyhow::anyhow!("PDF ExtGState Font reference cannot be resolved")
                })?;
                if !matches!(value, Object::Null) {
                    let values = value.as_array().map_err(|_| {
                        anyhow::anyhow!("PDF ExtGState Font must be a font and size array")
                    })?;
                    anyhow::ensure!(
                        values.len() == 2,
                        "PDF ExtGState Font must contain a font and size"
                    );
                    let font = as_dict(doc, &values[0]).ok_or_else(|| {
                        anyhow::anyhow!("PDF ExtGState Font is not a font dictionary")
                    })?;
                    let size = deref(doc, &values[1])
                        .and_then(as_number)
                        .filter(|size| size.is_finite())
                        .ok_or_else(|| anyhow::anyhow!("PDF ExtGState Font size is invalid"))?;
                    state.text.font = Some(std::sync::Arc::new(
                        font_metrics(doc, font, budget).map_err(|error| {
                            anyhow::anyhow!(
                                "ExtGState {} font: {error}",
                                String::from_utf8_lossy(name)
                            )
                        })?,
                    ));
                    state.text.size = size;
                }
            }
        }
        "Tf" => {
            let name = operands
                .first()
                .and_then(|value| value.as_name().ok())
                .ok_or_else(|| {
                    anyhow::anyhow!("PDF Tf operation has no valid font resource name")
                })?;
            let font = resource_font(doc, resources, fonts, name, budget)?;
            let size = operand_number(operands, 1)
                .filter(|size| size.is_finite())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "PDF font {} has an invalid Tf size",
                        String::from_utf8_lossy(name)
                    )
                })?;
            state.text.font = Some(font);
            state.text.size = size;
        }
        "Tc" => {
            if let Some(value) = operand_number(operands, 0) {
                state.text.char_spacing = value;
            }
        }
        "Tw" => {
            if let Some(value) = operand_number(operands, 0) {
                state.text.word_spacing = value;
            }
        }
        "Tz" => {
            if let Some(value) = operand_number(operands, 0) {
                state.text.horizontal_scale = value;
            }
        }
        "TL" => {
            if let Some(value) = operand_number(operands, 0) {
                state.text.leading = value;
            }
        }
        "Ts" => {
            if let Some(value) = operand_number(operands, 0) {
                state.text.rise = value;
            }
        }
        "Td" => {
            if let (Some(tx), Some(ty)) = (operand_number(operands, 0), operand_number(operands, 1))
            {
                state.tlm = Matrix::translate(tx, ty).mul(state.tlm);
                state.tm = state.tlm;
            }
        }
        "TD" => {
            if let (Some(tx), Some(ty)) = (operand_number(operands, 0), operand_number(operands, 1))
            {
                state.text.leading = -ty;
                state.tlm = Matrix::translate(tx, ty).mul(state.tlm);
                state.tm = state.tlm;
            }
        }
        "Tm" => {
            if let Some(matrix) = operand_matrix(operands) {
                state.tm = matrix;
                state.tlm = matrix;
            }
        }
        "T*" => {
            let leading = state.text.leading;
            state.tlm = Matrix::translate(0.0, -leading).mul(state.tlm);
            state.tm = state.tlm;
        }
        "Tj" => {
            if let Some(Object::String(bytes, _)) = operands.first() {
                show_text(state, runs, bytes, budget)?;
            }
        }
        "'" | "\"" => {
            let text_index = if operation.operator.as_str() == "\"" {
                if let Some(word_spacing) = operand_number(operands, 0) {
                    state.text.word_spacing = word_spacing;
                }
                if let Some(char_spacing) = operand_number(operands, 1) {
                    state.text.char_spacing = char_spacing;
                }
                2
            } else {
                0
            };
            let leading = state.text.leading;
            state.tlm = Matrix::translate(0.0, -leading).mul(state.tlm);
            state.tm = state.tlm;
            if let Some(Object::String(bytes, _)) = operands.get(text_index) {
                show_text(state, runs, bytes, budget)?;
            }
        }
        "TJ" => {
            if let Some(Object::Array(items)) = operands.first() {
                for item in items {
                    match item {
                        Object::String(bytes, _) => show_text(state, runs, bytes, budget)?,
                        other => {
                            if let Some(adjustment) = as_number(other) {
                                let shift = -adjustment / 1000.0
                                    * state.text.size
                                    * state.text.horizontal_scale
                                    / 100.0;
                                state.tm = Matrix::translate(shift, 0.0).mul(state.tm);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn show_text(
    state: &mut State,
    runs: &mut Vec<Run>,
    bytes: &[u8],
    budget: &mut PdfTextBudget<'_>,
) -> crate::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let font = state.text.font.as_ref().ok_or_else(|| {
        anyhow::anyhow!("PDF text operation has no selected font (Tf or ExtGState Font)")
    })?;
    let size = state.text.size;
    let codes = font.codes(bytes)?;
    if codes.is_empty() {
        return Ok(());
    }
    let text = font.decode(&codes, &mut *budget.output_bytes)?;
    let advance: f64 = codes
        .iter()
        .map(|code| font.advance(*code, size, &state.text))
        .sum();
    if text.is_empty() {
        anyhow::ensure!(advance.is_finite(), "PDF font advance is not finite");
        state.tm = Matrix::translate(advance, 0.0).mul(state.tm);
        return Ok(());
    }

    // The text rendering matrix is `Tfs × Tm × CTM`; every coordinate below is
    // already in text space (multiples of the font size), so applying
    // `Tm × CTM` to them is equivalent.
    let rise = state.text.rise;
    let matrix = state.tm.mul(state.ctm);
    let (start_x, start_y) = matrix.apply(0.0, rise);
    let (end_x, end_y) = matrix.apply(advance, rise);
    let (ascent, descent) = (font.ascent * size, font.descent.abs() * size);
    let corners = [
        matrix.apply(0.0, rise + ascent),
        matrix.apply(advance, rise + ascent),
        matrix.apply(0.0, rise - descent),
        matrix.apply(advance, rise - descent),
    ];
    let min_x = corners
        .iter()
        .fold(start_x.min(end_x), |acc, (x, _)| acc.min(*x));
    let max_x = corners
        .iter()
        .fold(start_x.max(end_x), |acc, (x, _)| acc.max(*x));
    let min_y = corners
        .iter()
        .fold(start_y.min(end_y), |acc, (_, y)| acc.min(*y));
    let max_y = corners
        .iter()
        .fold(start_y.max(end_y), |acc, (_, y)| acc.max(*y));
    anyhow::ensure!(
        [min_x, max_x, min_y, max_y, start_y, size]
            .iter()
            .all(|value| value.is_finite()),
        "PDF font metrics or text transform produced non-finite coordinates"
    );
    let run_cost = text.capacity() as u64 + 2 * std::mem::size_of::<Run>() as u64;
    budget.run_bytes = budget.run_bytes.checked_sub(run_cost).ok_or_else(|| {
        anyhow::anyhow!("PDF positioned text exceeds the RAYRAG_PDF_STREAM_LIMIT_BYTES page budget")
    })?;
    runs.push(Run {
        text,
        x0: min_x,
        x1: max_x,
        y_min: min_y,
        y_max: max_y,
        baseline: start_y,
        size,
    });
    // Advance the text matrix by the run width, in text-space units.
    state.tm = Matrix::translate(advance, 0.0).mul(state.tm);
    Ok(())
}

// ---------------------------------------------------------------------------
// fonts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct FontMetrics {
    widths: HashMap<u16, f64>,
    default_width: f64,
    two_byte: bool,
    to_unicode: HashMap<u16, String>,
    explicit_simple_encoding: bool,
    fallback_encoding: HashMap<u16, String>,
    fallback_encoding_error: Option<String>,
    ascent: f64,
    descent: f64,
}

impl FontMetrics {
    /// Split a PDF string into character codes: two bytes per code for CID
    /// fonts (`Type0`), one otherwise.
    fn codes(&self, bytes: &[u8]) -> crate::Result<Vec<u16>> {
        if self.two_byte {
            anyhow::ensure!(
                bytes.len().is_multiple_of(2),
                "PDF text contains an incomplete two-byte font code"
            );
            Ok(bytes
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect())
        } else {
            Ok(bytes.iter().map(|byte| u16::from(*byte)).collect())
        }
    }

    fn decode(&self, codes: &[u16], remaining: &mut u64) -> crate::Result<String> {
        let mut text = String::new();
        if self.to_unicode.is_empty() && self.two_byte {
            for character in char::decode_utf16(codes.iter().copied()) {
                let mut buffer = [0; 4];
                let character = character.unwrap_or('\u{fffd}');
                append_pdf_text(&mut text, character.encode_utf8(&mut buffer), remaining)?;
            }
        } else {
            for code in codes {
                if let Some(mapped) = self
                    .to_unicode
                    .get(code)
                    .or_else(|| self.fallback_encoding.get(code))
                {
                    append_pdf_text(&mut text, mapped, remaining)?;
                } else if let Some(error) = &self.fallback_encoding_error {
                    anyhow::bail!(
                        "PDF font fallback Encoding is required for character code {code}: {error}"
                    );
                } else if self.explicit_simple_encoding {
                    anyhow::bail!(
                        "PDF font Encoding has no Unicode mapping for character code {code}"
                    );
                } else if self.to_unicode.is_empty() {
                    let mut buffer = [0; 4];
                    append_pdf_text(
                        &mut text,
                        win_ansi_char(*code as u8).encode_utf8(&mut buffer),
                        remaining,
                    )?;
                } else {
                    append_pdf_text(&mut text, &fallback_char(*code, self.two_byte), remaining)?;
                }
            }
        }
        Ok(text)
    }

    /// Advance in text space (the units the text matrix moves in).
    fn advance(&self, code: u16, size: f64, state: &TextState) -> f64 {
        let width = self
            .widths
            .get(&code)
            .copied()
            .unwrap_or(self.default_width)
            / 1000.0;
        let word_spacing = if code == 32 { state.word_spacing } else { 0.0 };
        (width * size + state.char_spacing + word_spacing) * state.horizontal_scale / 100.0
    }
}

fn append_pdf_text(text: &mut String, value: &str, remaining: &mut u64) -> crate::Result<()> {
    *remaining = remaining.checked_sub(value.len() as u64).ok_or_else(|| {
        anyhow::anyhow!(
            "PDF extracted text exceeds the RAYRAG_PDF_STREAM_LIMIT_BYTES document budget"
        )
    })?;
    text.push_str(value);
    Ok(())
}

fn fallback_char(code: u16, two_byte: bool) -> String {
    if two_byte {
        char::from_u32(u32::from(code))
            .filter(|value| !value.is_control())
            .map(String::from)
            .unwrap_or_default()
    } else {
        win_ansi_char(code as u8).to_string()
    }
}

/// WinAnsi's printable range — the encoding behind virtually every Latin PDF
/// font. Control codes come out as spaces so they do not glue words together.
fn win_ansi_char(byte: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match byte {
        0x00..=0x1f => ' ',
        0x20..=0x7e => byte as char,
        0x80..=0x9f => HIGH[usize::from(byte - 0x80)],
        _ => byte as char,
    }
}

fn resource_font(
    doc: &Document,
    resources: Option<&Dictionary>,
    fonts: &mut HashMap<Vec<u8>, std::sync::Arc<FontMetrics>>,
    name: &[u8],
    budget: &mut PdfTextBudget<'_>,
) -> crate::Result<std::sync::Arc<FontMetrics>> {
    if let Some(font) = fonts.get(name) {
        return Ok(font.clone());
    }
    let font = resources
        .and_then(|resources| dict_entry(doc, resources, b"Font"))
        .and_then(|fonts| fonts.get(name).ok())
        .and_then(|value| as_dict(doc, value))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "PDF font {} is missing or invalid in the active Resources dictionary",
                String::from_utf8_lossy(name)
            )
        })?;
    let metrics = std::sync::Arc::new(
        font_metrics(doc, font, budget)
            .map_err(|error| anyhow::anyhow!("font {}: {error}", String::from_utf8_lossy(name)))?,
    );
    fonts.insert(name.to_vec(), metrics.clone());
    Ok(metrics)
}

/// Resolve an inheritable page attribute through the page tree.
fn inherited<'a>(doc: &'a Document, page_id: lopdf::ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut current = page_id;
    for _ in 0..32 {
        let page = doc.get_dictionary(current).ok()?;
        if let Ok(value) = page.get(key) {
            let value = deref(doc, value)?;
            if !matches!(value, Object::Null) {
                return Some(value);
            }
        }
        let parent = page.get(b"Parent").ok().and_then(|value| match value {
            Object::Reference(id) => Some(*id),
            _ => None,
        })?;
        current = parent;
    }
    None
}

fn dict_entry<'a>(doc: &'a Document, dict: &'a Dictionary, key: &[u8]) -> Option<&'a Dictionary> {
    as_dict(doc, dict.get(key).ok()?)
}

fn as_dict<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    match object {
        Object::Reference(id) => doc.get_dictionary(*id).ok(),
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

fn deref<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

fn font_metrics(
    doc: &Document,
    font: &Dictionary,
    budget: &mut PdfTextBudget<'_>,
) -> crate::Result<FontMetrics> {
    let subtype = font
        .get(b"Subtype")
        .and_then(Object::as_name)
        .unwrap_or_default();
    let two_byte = subtype == b"Type0";
    let mut metrics = FontMetrics {
        widths: HashMap::new(),
        default_width: 500.0,
        two_byte,
        to_unicode: HashMap::new(),
        explicit_simple_encoding: false,
        fallback_encoding: HashMap::new(),
        fallback_encoding_error: None,
        ascent: 0.75,
        descent: -0.25,
    };
    if let Ok(object) = font.get(b"ToUnicode") {
        let object = deref(doc, object)
            .ok_or_else(|| anyhow::anyhow!("PDF /ToUnicode CMap reference cannot be resolved"))?;
        if !matches!(object, Object::Null) {
            let stream = object
                .as_stream()
                .map_err(|error| anyhow::anyhow!("PDF /ToUnicode CMap is not a stream: {error}"))?;
            let limit = budget.limit;
            let content = super::pdf_stream::plain_content_limited_in(
                doc,
                stream,
                limit,
                "PDF /ToUnicode CMap",
            )?;
            metrics.to_unicode =
                parse_to_unicode_with_budget(&content, &mut budget.font_mapping_bytes)?;
        }
    }

    if !two_byte {
        if metrics.to_unicode.is_empty() {
            metrics.explicit_simple_encoding =
                parse_simple_font_encoding(doc, font, &mut metrics.to_unicode, budget)?;
        } else {
            let mut fallback = HashMap::new();
            match parse_simple_font_encoding(doc, font, &mut fallback, budget) {
                Ok(explicit) => {
                    metrics.explicit_simple_encoding = explicit;
                    metrics.fallback_encoding = fallback;
                }
                Err(error) if error.downcast_ref::<FontMappingLimitExceeded>().is_some() => {
                    return Err(error);
                }
                Err(error) => {
                    let message = error.to_string();
                    budget.font_mapping_bytes = budget
                        .font_mapping_bytes
                        .checked_sub(message.capacity() as u64)
                        .ok_or(FontMappingLimitExceeded)?;
                    metrics.fallback_encoding_error = Some(message);
                }
            }
        }
    }
    if two_byte {
        let descendants = optional_font_value(doc, font, b"DescendantFonts")?
            .ok_or_else(|| anyhow::anyhow!("PDF Type0 font has no DescendantFonts"))?
            .as_array()
            .map_err(|_| anyhow::anyhow!("PDF DescendantFonts must be an array"))?;
        anyhow::ensure!(
            descendants.len() == 1,
            "PDF DescendantFonts must contain one CID font"
        );
        let descendant = as_dict(doc, &descendants[0])
            .ok_or_else(|| anyhow::anyhow!("PDF DescendantFonts entry is not a font dictionary"))?;
        metrics.default_width = match optional_font_value(doc, descendant, b"DW")? {
            Some(value) => font_number(doc, value, "/DW")?,
            None => 1000.0,
        };
        if let Some(value) = optional_font_value(doc, descendant, b"W")? {
            let widths = value
                .as_array()
                .map_err(|_| anyhow::anyhow!("PDF /W must be an array"))?;
            parse_cid_widths(doc, widths, &mut metrics.widths, budget)?;
        }
        if let Some(value) = optional_font_value(doc, descendant, b"FontDescriptor")? {
            let descriptor = as_dict(doc, value)
                .ok_or_else(|| anyhow::anyhow!("PDF FontDescriptor must be a dictionary"))?;
            apply_descriptor(doc, descriptor, &mut metrics)?;
        }
    } else {
        if let Some(value) = optional_font_value(doc, font, b"Widths")? {
            let widths = value
                .as_array()
                .map_err(|_| anyhow::anyhow!("PDF /Widths must be an array"))?;
            let first = match optional_font_value(doc, font, b"FirstChar")? {
                Some(value) => font_code(doc, value, 255, "/FirstChar")?,
                None => 0,
            };
            anyhow::ensure!(
                widths.len() <= 256 - usize::from(first),
                "PDF /Widths exceeds the single-byte character range"
            );
            for (offset, value) in widths.iter().enumerate() {
                let width = font_number(doc, value, "/Widths value")?;
                insert_font_width(&mut metrics.widths, first + offset as u16, width, budget)?;
            }
        }
        if let Some(value) = optional_font_value(doc, font, b"FontDescriptor")? {
            let descriptor = as_dict(doc, value)
                .ok_or_else(|| anyhow::anyhow!("PDF FontDescriptor must be a dictionary"))?;
            apply_descriptor(doc, descriptor, &mut metrics)?;
        }
    }
    let font_cost = (std::mem::size_of::<FontMetrics>() as u64).saturating_add(256);
    budget.font_mapping_bytes = budget
        .font_mapping_bytes
        .checked_sub(font_cost)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "PDF selected font tables exceed the RAYRAG_PDF_STREAM_LIMIT_BYTES page budget"
            )
        })?;
    Ok(metrics)
}

fn base_font_encoding(name: &[u8]) -> crate::Result<[Option<u16>; 256]> {
    anyhow::ensure!(
        matches!(
            name,
            b"StandardEncoding" | b"WinAnsiEncoding" | b"MacRomanEncoding" | b"MacExpertEncoding"
        ),
        "PDF font Encoding {} is unsupported without a usable ToUnicode map",
        String::from_utf8_lossy(name)
    );
    let mut font = Dictionary::new();
    font.set("Type", "Font");
    font.set("Encoding", Object::Name(name.to_vec()));
    let document = Document::new();
    let lopdf::Encoding::OneByteEncoding(table) = font.get_font_encoding(&document)? else {
        anyhow::bail!("PDF font Encoding did not resolve to a single-byte table");
    };
    Ok(std::array::from_fn(|index| {
        table[index].map(|glyph| glyph.utf16_code_unit())
    }))
}

fn adobe_glyph_character(name: &str) -> crate::Result<Option<char>> {
    let mut encoding = Dictionary::new();
    encoding.set(
        "Differences",
        vec![Object::Integer(0), Object::Name(name.as_bytes().to_vec())],
    );
    let mut font = Dictionary::new();
    font.set("Type", "Font");
    font.set("Encoding", encoding);
    let document = Document::new();
    match font.get_font_encoding(&document)? {
        lopdf::Encoding::Differences(differences) => Ok(differences
            .map
            .get(&0)
            .and_then(|glyph| char::from_u32(u32::from(glyph.utf16_code_unit())))),
        _ => Ok(None),
    }
}

fn glyph_name_unicode(name: &[u8]) -> crate::Result<String> {
    if name == b".notdef" {
        return Ok(String::new());
    }
    let name = std::str::from_utf8(name)
        .map_err(|_| anyhow::anyhow!("PDF font Differences glyph name is not ASCII"))?;
    let name = name.split('.').next().unwrap_or_default();
    let mut text = String::new();
    for component in name.split('_') {
        anyhow::ensure!(
            !component.is_empty(),
            "PDF font Differences contains an empty glyph component"
        );
        if let Some(hex) = component
            .strip_prefix("uni")
            .filter(|hex| !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            anyhow::ensure!(
                hex.len().is_multiple_of(4),
                "PDF font Differences glyph {component} has invalid Unicode groups"
            );
            let units = hex.as_bytes().chunks_exact(4).map(|group| {
                u16::from_str_radix(std::str::from_utf8(group).expect("hexadecimal ASCII"), 16)
                    .expect("four hexadecimal digits")
            });
            for character in char::decode_utf16(units) {
                text.push(character.map_err(|_| {
                    anyhow::anyhow!("PDF font Differences glyph {component} has invalid Unicode")
                })?);
            }
        } else if let Some(hex) = component.strip_prefix('u').filter(|hex| {
            (4..=6).contains(&hex.len()) && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            let value = u32::from_str_radix(hex, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| {
                    anyhow::anyhow!("PDF font Differences glyph {component} has invalid Unicode")
                })?;
            text.push(value);
        } else if let Some(character) = adobe_glyph_character(component)? {
            text.push(character);
        } else {
            anyhow::bail!(
                "PDF font Differences glyph {component} is unsupported; export the PDF with a ToUnicode map"
            );
        }
    }
    Ok(text)
}

fn parse_simple_font_encoding(
    doc: &Document,
    font: &Dictionary,
    map: &mut HashMap<u16, String>,
    budget: &mut PdfTextBudget<'_>,
) -> crate::Result<bool> {
    let Some(value) = optional_font_value(doc, font, b"Encoding")? else {
        return Ok(false);
    };
    let (base, differences) = match value {
        Object::Name(name) => (base_font_encoding(name)?, None),
        Object::Dictionary(encoding) => {
            if let Some(value) = optional_font_value(doc, encoding, b"Type")? {
                anyhow::ensure!(
                    value.as_name().ok() == Some(b"Encoding"),
                    "PDF font Encoding dictionary has an invalid Type"
                );
            }
            let name = optional_font_value(doc, encoding, b"BaseEncoding")?
                .map(|value| {
                    value
                        .as_name()
                        .map_err(|_| anyhow::anyhow!("PDF font BaseEncoding must be a name"))
                })
                .transpose()?
                .unwrap_or(b"StandardEncoding");
            (
                base_font_encoding(name)?,
                optional_font_value(doc, encoding, b"Differences")?,
            )
        }
        _ => anyhow::bail!("PDF font Encoding must be a name or dictionary"),
    };
    for (code, glyph) in base.iter().enumerate() {
        if let Some(glyph) = glyph {
            let character = char::from_u32(u32::from(*glyph)).ok_or_else(|| {
                anyhow::anyhow!("PDF font base Encoding contains invalid Unicode")
            })?;
            insert_cmap_mapping(
                map,
                code as u16,
                character.to_string(),
                &mut budget.font_mapping_bytes,
            )?;
        }
    }
    if let Some(value) = differences {
        let values = value
            .as_array()
            .map_err(|_| anyhow::anyhow!("PDF font Differences must be an array"))?;
        let mut code = None;
        for value in values {
            let value = deref(doc, value).ok_or_else(|| {
                anyhow::anyhow!("PDF font Differences reference cannot be resolved")
            })?;
            match value {
                Object::Integer(value) => {
                    anyhow::ensure!(
                        (0..=255).contains(value),
                        "PDF font Differences code must be from 0 through 255"
                    );
                    code = Some(*value as u16);
                }
                Object::Name(name) => {
                    let current = code.filter(|code| *code <= 255).ok_or_else(|| {
                        anyhow::anyhow!("PDF font Differences glyph has no valid character code")
                    })?;
                    insert_cmap_mapping(
                        map,
                        current,
                        glyph_name_unicode(name)?,
                        &mut budget.font_mapping_bytes,
                    )?;
                    code = Some(current + 1);
                }
                _ => anyhow::bail!(
                    "PDF font Differences entries must be integer codes or glyph names"
                ),
            }
        }
    }
    Ok(true)
}

fn optional_font_value<'a>(
    doc: &'a Document,
    font: &'a Dictionary,
    key: &[u8],
) -> crate::Result<Option<&'a Object>> {
    let Ok(value) = font.get(key) else {
        return Ok(None);
    };
    let value = deref(doc, value).ok_or_else(|| {
        anyhow::anyhow!(
            "PDF font /{} reference cannot be resolved",
            String::from_utf8_lossy(key)
        )
    })?;
    Ok(if matches!(value, Object::Null) {
        None
    } else {
        Some(value)
    })
}

fn font_number(doc: &Document, object: &Object, field: &str) -> crate::Result<f64> {
    deref(doc, object)
        .and_then(as_number)
        .filter(|value| value.is_finite())
        .ok_or_else(|| anyhow::anyhow!("PDF font {field} must be a finite number"))
}

fn font_code(doc: &Document, object: &Object, maximum: u16, field: &str) -> crate::Result<u16> {
    let number = font_number(doc, object, field)?;
    anyhow::ensure!(
        number.fract() == 0.0 && number >= 0.0 && number <= f64::from(maximum),
        "PDF font {field} must be an integer from 0 through {maximum}"
    );
    Ok(number as u16)
}

fn insert_font_width(
    out: &mut HashMap<u16, f64>,
    code: u16,
    width: f64,
    budget: &mut PdfTextBudget<'_>,
) -> crate::Result<()> {
    budget.font_mapping_bytes = budget.font_mapping_bytes.checked_sub(64).ok_or_else(|| {
        anyhow::anyhow!("PDF width expansion exceeds the RAYRAG_PDF_STREAM_LIMIT_BYTES page budget")
    })?;
    out.insert(code, width);
    Ok(())
}

fn apply_descriptor(
    doc: &Document,
    descriptor: &Dictionary,
    metrics: &mut FontMetrics,
) -> crate::Result<()> {
    if !metrics.two_byte {
        if let Some(value) = optional_font_value(doc, descriptor, b"MissingWidth")? {
            metrics.default_width = font_number(doc, value, "/MissingWidth")?;
        }
    }
    if let Some(value) = optional_font_value(doc, descriptor, b"Ascent")? {
        metrics.ascent = (font_number(doc, value, "/Ascent")? / 1000.0).clamp(0.5, 1.2);
    }
    if let Some(value) = optional_font_value(doc, descriptor, b"Descent")? {
        metrics.descent = (font_number(doc, value, "/Descent")? / 1000.0).clamp(-0.6, 0.0);
    }
    Ok(())
}

/// Width assignments, including repeated ranges, consume the page font budget.
fn parse_cid_widths(
    doc: &Document,
    widths: &[Object],
    out: &mut HashMap<u16, f64>,
    budget: &mut PdfTextBudget<'_>,
) -> crate::Result<()> {
    let mut index = 0;
    while index < widths.len() {
        let start = font_code(doc, &widths[index], u16::MAX, "/W start CID")?;
        let next = widths
            .get(index + 1)
            .and_then(|value| deref(doc, value))
            .ok_or_else(|| anyhow::anyhow!("PDF /W has an incomplete width entry"))?;
        match next {
            Object::Array(list) => {
                anyhow::ensure!(
                    list.len() <= 65536 - usize::from(start),
                    "PDF /W array exceeds the CID range"
                );
                for (offset, value) in list.iter().enumerate() {
                    let width = font_number(doc, value, "/W width")?;
                    insert_font_width(out, start + offset as u16, width, budget)?;
                }
                index += 2;
            }
            end => {
                let end = font_code(doc, end, u16::MAX, "/W end CID")?;
                anyhow::ensure!(end >= start, "PDF /W has a descending CID range");
                let value = widths
                    .get(index + 2)
                    .ok_or_else(|| anyhow::anyhow!("PDF /W range has no width"))?;
                let width = font_number(doc, value, "/W width")?;
                for code in start..=end {
                    insert_font_width(out, code, width, budget)?;
                }
                index += 3;
            }
        }
    }
    Ok(())
}

/// Borrowed tokens keep CMap parsing independent of line breaks and token counts.
#[derive(Debug, PartialEq)]
enum CMapToken<'a> {
    Hex(&'a [u8]),
    Literal(&'a [u8]),
    Word(&'a [u8]),
    ArrayStart,
    ArrayEnd,
}

fn cmap_whitespace(byte: u8) -> bool {
    byte == 0 || byte.is_ascii_whitespace()
}

struct CMapReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> CMapReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn next(&mut self) -> crate::Result<Option<CMapToken<'a>>> {
        while self.offset < self.bytes.len() {
            let start = self.offset;
            let byte = self.bytes[self.offset];
            self.offset += 1;
            match byte {
                byte if cmap_whitespace(byte) => continue,
                b'%' => {
                    while self.offset < self.bytes.len()
                        && !matches!(self.bytes[self.offset], b'\n' | b'\r')
                    {
                        self.offset += 1;
                    }
                }
                b'(' => {
                    let mut depth = 1;
                    while self.offset < self.bytes.len() && depth > 0 {
                        match self.bytes[self.offset] {
                            b'\\' => self.offset += 1,
                            b'(' => depth += 1,
                            b')' => depth -= 1,
                            _ => {}
                        }
                        self.offset += 1;
                    }
                    anyhow::ensure!(
                        depth == 0,
                        "PDF /ToUnicode has an unterminated literal string"
                    );
                    return Ok(Some(CMapToken::Literal(
                        &self.bytes[start + 1..self.offset - 1],
                    )));
                }
                b'<' if self.bytes.get(self.offset) == Some(&b'<') => {
                    self.offset += 1;
                    return Ok(Some(CMapToken::Word(&self.bytes[start..self.offset])));
                }
                b'<' => {
                    let end = self.bytes[self.offset..]
                        .iter()
                        .position(|byte| *byte == b'>')
                        .map(|length| self.offset + length)
                        .ok_or_else(|| {
                            anyhow::anyhow!("PDF /ToUnicode has an unterminated hex string")
                        })?;
                    let value = &self.bytes[self.offset..end];
                    self.offset = end + 1;
                    return Ok(Some(CMapToken::Hex(value)));
                }
                b'[' => return Ok(Some(CMapToken::ArrayStart)),
                b']' => return Ok(Some(CMapToken::ArrayEnd)),
                b'>' => {
                    if self.bytes.get(self.offset) == Some(&b'>') {
                        self.offset += 1;
                    }
                    return Ok(Some(CMapToken::Word(&self.bytes[start..self.offset])));
                }
                b')' => anyhow::bail!("PDF /ToUnicode has an unmatched literal string delimiter"),
                _ => {
                    while self.offset < self.bytes.len()
                        && !cmap_whitespace(self.bytes[self.offset])
                        && !b"<>[]()%/".contains(&self.bytes[self.offset])
                    {
                        self.offset += 1;
                    }
                    return Ok(Some(CMapToken::Word(&self.bytes[start..self.offset])));
                }
            }
        }
        Ok(None)
    }

    fn string_bytes(&mut self) -> crate::Result<Vec<u8>> {
        match self.next()? {
            Some(token) => cmap_string_bytes(token),
            None => anyhow::bail!("PDF /ToUnicode mapping requires a byte string"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("PDF font Unicode mapping expansion exceeds the RAYRAG_PDF_STREAM_LIMIT_BYTES page budget")]
struct FontMappingLimitExceeded;

/// Bound expanded mapping text and per-entry work, including overwritten entries.
fn insert_cmap_mapping(
    map: &mut HashMap<u16, String>,
    code: u16,
    text: String,
    remaining: &mut u64,
) -> crate::Result<()> {
    let cost = (text.capacity() as u64).saturating_add(64);
    *remaining = remaining
        .checked_sub(cost)
        .ok_or(FontMappingLimitExceeded)?;
    map.insert(code, text);
    Ok(())
}

#[cfg(test)]
fn parse_to_unicode(content: &[u8], limit: u64) -> crate::Result<HashMap<u16, String>> {
    let mut remaining = limit;
    parse_to_unicode_with_budget(content, &mut remaining)
}

fn parse_to_unicode_with_budget(
    content: &[u8],
    remaining: &mut u64,
) -> crate::Result<HashMap<u16, String>> {
    let mut reader = CMapReader::new(content);
    let mut map = HashMap::new();
    let mut section: Option<&[u8]> = None;
    let mut code_space_width = None;
    while let Some(token) = reader.next()? {
        match token {
            CMapToken::Word(b"begincodespacerange") => {
                anyhow::ensure!(
                    section.is_none(),
                    "PDF /ToUnicode has nested mapping sections"
                );
                section = Some(b"codespace");
            }
            CMapToken::Word(b"endcodespacerange") => {
                anyhow::ensure!(
                    section == Some(b"codespace"),
                    "PDF /ToUnicode has an unexpected endcodespacerange"
                );
                section = None;
            }
            CMapToken::Word(b"usecmap") => anyhow::bail!(
                "PDF /ToUnicode usecmap requires an external CMap unsupported by the native parser"
            ),
            CMapToken::Word(b"beginbfchar") => {
                anyhow::ensure!(
                    section.is_none(),
                    "PDF /ToUnicode has nested mapping sections"
                );
                section = Some(b"bfchar");
            }
            CMapToken::Word(b"beginbfrange") => {
                anyhow::ensure!(
                    section.is_none(),
                    "PDF /ToUnicode has nested mapping sections"
                );
                section = Some(b"bfrange");
            }
            CMapToken::Word(b"endbfchar") => {
                anyhow::ensure!(
                    section == Some(b"bfchar"),
                    "PDF /ToUnicode has an unexpected endbfchar"
                );
                section = None;
            }
            CMapToken::Word(b"endbfrange") => {
                anyhow::ensure!(
                    section == Some(b"bfrange"),
                    "PDF /ToUnicode has an unexpected endbfrange"
                );
                section = None;
            }
            CMapToken::Word(b"endcmap") => break,
            source @ (CMapToken::Hex(_) | CMapToken::Literal(_))
                if section == Some(b"codespace") =>
            {
                let (start, width) = cmap_source_code(&cmap_string_bytes(source)?)?;
                let (end, end_width) = cmap_source_code(&reader.string_bytes()?)?;
                anyhow::ensure!(
                    width == end_width && end >= start,
                    "PDF /ToUnicode codespace range is invalid"
                );
                anyhow::ensure!(
                    code_space_width.is_none_or(|previous| previous == width),
                    "PDF /ToUnicode mixed-width codespaces are unsupported by the native parser"
                );
                code_space_width = Some(width);
            }
            source @ (CMapToken::Hex(_) | CMapToken::Literal(_)) if section == Some(b"bfchar") => {
                let (code, _) = cmap_source_code(&cmap_string_bytes(source)?)?;
                let target = reader.string_bytes()?;
                insert_cmap_mapping(&mut map, code, cmap_unicode(&target)?, remaining)?;
            }
            source @ (CMapToken::Hex(_) | CMapToken::Literal(_)) if section == Some(b"bfrange") => {
                let (start, start_width) = cmap_source_code(&cmap_string_bytes(source)?)?;
                let (end, end_width) = cmap_source_code(&reader.string_bytes()?)?;
                anyhow::ensure!(
                    start_width == end_width,
                    "PDF /ToUnicode source range lengths do not match"
                );
                anyhow::ensure!(end >= start, "PDF /ToUnicode has a descending source range");
                match reader.next()? {
                    Some(target @ (CMapToken::Hex(_) | CMapToken::Literal(_))) => {
                        let mut target = cmap_string_bytes(target)?;
                        for code in start..=end {
                            insert_cmap_mapping(&mut map, code, cmap_unicode(&target)?, remaining)?;
                            if code != end {
                                anyhow::ensure!(
                                    increment_cmap_bytes(&mut target),
                                    "PDF /ToUnicode destination range overflows"
                                );
                            }
                        }
                    }
                    Some(CMapToken::ArrayStart) => {
                        for code in start..=end {
                            let target = reader.string_bytes()?;
                            insert_cmap_mapping(&mut map, code, cmap_unicode(&target)?, remaining)?;
                        }
                        anyhow::ensure!(
                            reader.next()? == Some(CMapToken::ArrayEnd),
                            "PDF /ToUnicode range array length does not match its source range"
                        );
                    }
                    _ => anyhow::bail!("PDF /ToUnicode range requires a hex string or array"),
                }
            }
            _ if section.is_some() => anyhow::bail!("PDF /ToUnicode has an invalid mapping entry"),
            _ => {}
        }
    }
    anyhow::ensure!(
        section.is_none(),
        "PDF /ToUnicode has an unterminated mapping section"
    );
    Ok(map)
}

fn cmap_string_bytes(token: CMapToken<'_>) -> crate::Result<Vec<u8>> {
    match token {
        CMapToken::Hex(value) => decode_cmap_hex(value),
        CMapToken::Literal(value) => {
            let mut bytes = Vec::new();
            let mut offset = 0;
            while offset < value.len() {
                let byte = value[offset];
                offset += 1;
                if byte == b'\\' {
                    let escaped = *value.get(offset).ok_or_else(|| {
                        anyhow::anyhow!("PDF /ToUnicode has an incomplete literal escape")
                    })?;
                    offset += 1;
                    match escaped {
                        b'n' => bytes.push(b'\n'),
                        b'r' => bytes.push(b'\r'),
                        b't' => bytes.push(b'\t'),
                        b'b' => bytes.push(8),
                        b'f' => bytes.push(12),
                        b'\n' => {}
                        b'\r' => {
                            if value.get(offset) == Some(&b'\n') {
                                offset += 1;
                            }
                        }
                        b'0'..=b'7' => {
                            let mut octal = u16::from(escaped - b'0');
                            let mut digits = 1;
                            while digits < 3
                                && value
                                    .get(offset)
                                    .is_some_and(|byte| matches!(*byte, b'0'..=b'7'))
                            {
                                octal = octal * 8 + u16::from(value[offset] - b'0');
                                offset += 1;
                                digits += 1;
                            }
                            bytes.push(octal as u8);
                        }
                        other => bytes.push(other),
                    }
                } else if byte == b'\r' {
                    bytes.push(b'\n');
                    if value.get(offset) == Some(&b'\n') {
                        offset += 1;
                    }
                } else {
                    bytes.push(byte);
                }
            }
            Ok(bytes)
        }
        _ => anyhow::bail!("PDF /ToUnicode mapping requires a byte string"),
    }
}

fn decode_cmap_hex(value: &[u8]) -> crate::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut high = None;
    for byte in value.iter().copied().filter(|byte| !cmap_whitespace(*byte)) {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => anyhow::bail!("PDF /ToUnicode contains an invalid hex digit"),
        };
        if let Some(high) = high.take() {
            bytes.push(high * 16 + digit);
        } else {
            high = Some(digit);
        }
    }
    if let Some(high) = high {
        bytes.push(high * 16);
    }
    Ok(bytes)
}

fn cmap_source_code(bytes: &[u8]) -> crate::Result<(u16, usize)> {
    let code = match bytes {
        [low] => u16::from(*low),
        [high, low] => u16::from_be_bytes([*high, *low]),
        _ => anyhow::bail!("PDF /ToUnicode source codes must contain one or two bytes"),
    };
    Ok((code, bytes.len()))
}

fn cmap_unicode(bytes: &[u8]) -> crate::Result<String> {
    anyhow::ensure!(
        bytes.len().is_multiple_of(2),
        "PDF /ToUnicode destination is not UTF-16BE"
    );
    char::decode_utf16(
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]])),
    )
    .collect::<std::result::Result<String, _>>()
    .map_err(|error| anyhow::anyhow!("PDF /ToUnicode destination contains invalid UTF-16: {error}"))
}

fn increment_cmap_bytes(bytes: &mut [u8]) -> bool {
    for byte in bytes.iter_mut().rev() {
        if *byte != u8::MAX {
            *byte += 1;
            return true;
        }
        *byte = 0;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    /// A one-page PDF whose content stream is `content`, with a Helvetica font
    /// carrying explicit `/Widths` (space 250, every other glyph 500).
    fn single_page_pdf(content: &str) -> Document {
        let mut doc = Document::with_version("1.7");
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
        let content_id = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            content.as_bytes().to_vec(),
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
        doc
    }

    #[test]
    fn quote_text_operators_match_explicit_newline_and_spacing() {
        for leading in [0, 20] {
            for (quoted, explicit) in [
                ("(single quote marker) '", "T* (single quote marker) Tj"),
                (
                    "4 2 (double quote marker) \"",
                    "4 Tw 2 Tc T* (double quote marker) Tj",
                ),
            ] {
                let prefix = format!("BT /F1 10 Tf {leading} TL 3 Tc 7 Tw 72 700 Td (before) Tj");
                let quoted_doc = single_page_pdf(&format!("{prefix} {quoted} (tail) Tj ET"));
                let explicit_doc = single_page_pdf(&format!("{prefix} {explicit} (tail) Tj ET"));
                assert_eq!(
                    try_extract_positioned_lines(&quoted_doc).unwrap(),
                    try_extract_positioned_lines(&explicit_doc).unwrap(),
                    "operator {quoted}, leading {leading}"
                );
            }
        }
    }

    #[test]
    fn inline_image_payload_is_opaque_and_following_text_survives() {
        let content = "BT /F1 12 Tf 72 720 Td (Before) Tj ET \
            q BI /W 1 /H 1 /BPC 8 /CS /G ID \u{0}(Tj) Tj EI Q \
            BT /F1 12 Tf 72 700 Td (After) Tj ET";
        let doc = single_page_pdf(content);
        let lines = try_extract_positioned_lines(&doc).unwrap();
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert!(texts.contains(&"Before"), "{texts:?}");
        assert!(texts.contains(&"After"), "{texts:?}");
        assert!(
            !texts
                .iter()
                .any(|text| text.contains("Tj") || text.contains('\u{0}')),
            "{texts:?}"
        );
    }

    fn attach_form(doc: &mut Document, dict: Dictionary, bytes: &[u8]) -> lopdf::ObjectId {
        let form_id = doc.add_object(lopdf::Stream::new(dict, bytes.to_vec()));
        let page_id = doc.get_pages()[&1];
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .get_mut(b"Resources")
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("XObject", dictionary! { "Form" => form_id });
        form_id
    }

    fn simple_page_font(doc: &Document) -> lopdf::ObjectId {
        let page = doc.get_pages()[&1];
        doc.get_dictionary(page)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference()
            .unwrap()
    }

    #[test]
    fn explicit_simple_font_encodings_preserve_declared_character_tables() {
        for (encoding, code, expected) in [
            ("WinAnsiEncoding", "80", "€"),
            ("MacRomanEncoding", "80", "Ä"),
            ("StandardEncoding", "27", "’"),
            ("MacExpertEncoding", "21", "\u{f721}"),
        ] {
            let mut doc = single_page_pdf(&format!("BT /F1 12 Tf 72 700 Td <{code}> Tj ET"));
            let font = simple_page_font(&doc);
            doc.get_dictionary_mut(font)
                .unwrap()
                .set("Encoding", encoding);
            let lines = try_extract_positioned_lines(&doc).unwrap();
            assert_eq!(lines[0].text, expected, "encoding={encoding}");
        }
    }

    #[test]
    fn indirect_differences_decode_glyph_names_sequences_and_unicode_through_chunks() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (ABCDEF) Tj ET");
        let base = doc.add_object(Object::Name(b"WinAnsiEncoding".to_vec()));
        let first = doc.add_object(Object::Integer(65));
        let glyph = doc.add_object(Object::Name(b"Aacute".to_vec()));
        let differences = doc.add_object(Object::Array(vec![
            first.into(),
            glyph.into(),
            Object::Name(b"f_f_i".to_vec()),
            Object::Name(b"uni4E2D6587".to_vec()),
            Object::Name(b"u1F600".to_vec()),
            Object::Name(b"Euro.alt".to_vec()),
            Object::Name(b".notdef".to_vec()),
        ]));
        let encoding = doc.add_object(dictionary! { "Type" => "Encoding", "BaseEncoding" => base, "Differences" => differences });
        let font = simple_page_font(&doc);
        doc.get_dictionary_mut(font)
            .unwrap()
            .set("Encoding", encoding);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let parsed = crate::parser::Parse::parse(
            &crate::parser::pdf::PdfParser::new(),
            "differences.pdf",
            &bytes,
        )
        .unwrap();
        assert!(parsed.content.contains("Áffi中文😀€"), "{}", parsed.content);
        let config = crate::ParserConfig::default();
        let chunks = crate::chunk::chunker_for(&config)
            .chunk(&parsed, &config)
            .unwrap();
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.content.contains("Áffi中文😀€"))
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.metadata.contains_key("position_int"))
        );
    }

    #[test]
    fn invalid_font_encodings_fail_with_context_and_can_be_repaired() {
        for encoding in [
            Object::Name(b"UnsupportedEncoding".to_vec()),
            Object::Integer(17),
            Object::Dictionary(dictionary! { "Differences" => vec![Object::Name(b"A".to_vec())] }),
            Object::Dictionary(
                dictionary! { "Differences" => vec![Object::Integer(-1), Object::Name(b"A".to_vec())] },
            ),
            Object::Dictionary(
                dictionary! { "Differences" => vec![Object::Integer(255), Object::Name(b"A".to_vec()), Object::Name(b"B".to_vec())] },
            ),
            Object::Dictionary(
                dictionary! { "Differences" => vec![Object::Integer(65), Object::Name(b"uniD800".to_vec())] },
            ),
            Object::Dictionary(
                dictionary! { "Differences" => vec![Object::Integer(65), Object::Name(b"unrecognizedGlyph".to_vec())] },
            ),
        ] {
            let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (AB) Tj ET");
            let font = simple_page_font(&doc);
            doc.get_dictionary_mut(font)
                .unwrap()
                .set("Encoding", encoding);
            let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
            for detail in ["PDF page 1", "font F1", "upload it again"] {
                assert!(error.contains(detail), "{error}");
            }
            doc.get_dictionary_mut(font)
                .unwrap()
                .set("Encoding", "WinAnsiEncoding");
            assert_eq!(try_extract_positioned_lines(&doc).unwrap()[0].text, "AB");
        }
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td <00> Tj ET");
        let font = simple_page_font(&doc);
        doc.get_dictionary_mut(font)
            .unwrap()
            .set("Encoding", "StandardEncoding");
        assert!(
            try_extract_positioned_lines(&doc)
                .unwrap_err()
                .to_string()
                .contains("character code 0")
        );
    }

    #[test]
    fn empty_glyph_mapping_advances_cursor_and_cannot_trigger_raw_fallback() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (A) Tj (B) Tj ET");
        let font = simple_page_font(&doc);
        doc.get_dictionary_mut(font).unwrap().set("Encoding", dictionary! { "Differences" => vec![Object::Integer(65), Object::Name(b".notdef".to_vec()), Object::Name(b"B".to_vec())] });
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert_eq!(lines[0].text, "B");
        assert!((lines[0].x0 - 78.0).abs() < 0.01, "{lines:?}");
        let contents = doc
            .get_dictionary(doc.get_pages()[&1])
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_object_mut(contents)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .content = b"BT /F1 12 Tf 72 700 Td (A) Tj ET".to_vec();
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let error = crate::parser::Parse::parse(
            &crate::parser::pdf::PdfParser::new(),
            "empty-mapping.pdf",
            &bytes,
        )
        .unwrap_err()
        .to_string();
        for detail in ["PDF page 1", "no usable Unicode", "upload it again"] {
            assert!(error.contains(detail), "{error}");
        }
    }

    #[test]
    fn composed_glyph_names_validate_components_and_utf16_pairs() {
        assert_eq!(glyph_name_unicode(b"uniD83DDE00").unwrap(), "😀");
        assert_eq!(glyph_name_unicode(b"A_B.alt").unwrap(), "AB");
        for name in [
            b".notdef.alt".as_slice(),
            b"A__B",
            b"_A",
            b"uniD800",
            b"uniDC00",
        ] {
            assert!(glyph_name_unicode(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn partial_to_unicode_uses_declared_fallback_and_keeps_primary_precedence() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td <418042> Tj ET");
        let cmap = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"1 beginbfchar <41> <4e2d> endbfchar".to_vec(),
        ));
        let base = doc.add_object(Object::Name(b"MacRomanEncoding".to_vec()));
        let differences = doc.add_object(Object::Array(vec![
            Object::Integer(65),
            Object::Name(b"Z".to_vec()),
            Object::Name(b"uni6587".to_vec()),
        ]));
        let encoding =
            doc.add_object(dictionary! { "BaseEncoding" => base, "Differences" => differences });
        let font = simple_page_font(&doc);
        let dictionary = doc.get_dictionary_mut(font).unwrap();
        dictionary.set("ToUnicode", cmap);
        dictionary.set("Encoding", encoding);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let parsed = crate::parser::Parse::parse(
            &crate::parser::pdf::PdfParser::new(),
            "partial-unicode.pdf",
            &bytes,
        )
        .unwrap();
        assert!(parsed.content.contains("中Ä文"), "{}", parsed.content);
        let config = crate::ParserConfig::default();
        let chunks = crate::chunk::chunker_for(&config)
            .chunk(&parsed, &config)
            .unwrap();
        assert!(chunks.iter().any(|chunk| chunk.content.contains("中Ä文")));
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.metadata.contains_key("position_int"))
        );
    }

    #[test]
    fn invalid_fallback_is_deferred_until_a_missing_unicode_code_is_used() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (AB) Tj ET");
        let cmap = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"2 beginbfchar <41> <4e2d> <42> <6587> endbfchar".to_vec(),
        ));
        let font = simple_page_font(&doc);
        let dictionary = doc.get_dictionary_mut(font).unwrap();
        dictionary.set("ToUnicode", cmap);
        dictionary.set("Encoding", "UnsupportedEncoding");
        assert_eq!(try_extract_positioned_lines(&doc).unwrap()[0].text, "中文");
        doc.get_object_mut(cmap)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .content = b"1 beginbfchar <41> <4e2d> endbfchar".to_vec();
        let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
        for detail in [
            "PDF page 1",
            "character code 66",
            "UnsupportedEncoding",
            "upload it again",
        ] {
            assert!(error.contains(detail), "{error}");
        }
        doc.get_dictionary_mut(font)
            .unwrap()
            .set("Encoding", "MacRomanEncoding");
        assert_eq!(try_extract_positioned_lines(&doc).unwrap()[0].text, "中B");
        doc.get_dictionary_mut(font).unwrap().set("Encoding", dictionary! { "Differences" => vec![Object::Integer(66), Object::Name(b"unknownGlyph".to_vec())] });
        assert!(
            try_extract_positioned_lines(&doc)
                .unwrap_err()
                .to_string()
                .contains("unknownGlyph")
        );
    }

    #[test]
    fn font_metadata_storage_is_counted_alongside_width_assignments() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (A) Tj ET");
        let font = simple_page_font(&doc);
        doc.get_dictionary_mut(font)
            .unwrap()
            .set("Encoding", Object::Null);
        let dictionary = doc.get_dictionary(font).unwrap();
        let widths = dictionary.get(b"Widths").unwrap().as_array().unwrap().len();
        let mut output = 1 << 20;
        let mut budget = PdfTextBudget {
            limit: 1 << 20,
            font_mapping_bytes: 1 << 20,
            run_bytes: 1 << 20,
            output_bytes: &mut output,
        };
        font_metrics(&doc, dictionary, &mut budget).unwrap();
        let expected = widths as u64 * 64 + std::mem::size_of::<FontMetrics>() as u64 + 256;
        assert_eq!((1 << 20) - budget.font_mapping_bytes, expected);
    }

    #[test]
    fn fallback_mapping_budget_failure_is_never_deferred_or_swallowed() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (A) Tj ET");
        let cmap = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"1 beginbfchar <41> <0041> endbfchar".to_vec(),
        ));
        let font = simple_page_font(&doc);
        let dictionary = doc.get_dictionary_mut(font).unwrap();
        dictionary.set("ToUnicode", cmap);
        dictionary.set("Encoding", "WinAnsiEncoding");
        let mut output = 4096;
        let mut budget = PdfTextBudget {
            limit: 4096,
            font_mapping_bytes: 500,
            run_bytes: 4096,
            output_bytes: &mut output,
        };
        let error = font_metrics(&doc, doc.get_dictionary(font).unwrap(), &mut budget).unwrap_err();
        assert!(
            error.downcast_ref::<FontMappingLimitExceeded>().is_some(),
            "{error}"
        );
    }

    #[test]
    fn usable_to_unicode_remains_preferred_over_font_encoding() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (AB) Tj ET");
        let cmap = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"2 beginbfchar <41> <4e2d> <42> <6587> endbfchar".to_vec(),
        ));
        let font = simple_page_font(&doc);
        let font = doc.get_dictionary_mut(font).unwrap();
        font.set("ToUnicode", cmap);
        font.set("Encoding", "UnsupportedEncoding");
        assert_eq!(try_extract_positioned_lines(&doc).unwrap()[0].text, "中文");
    }

    #[test]
    fn font_differences_redefinitions_consume_page_mapping_budget() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (A) Tj ET");
        let font = simple_page_font(&doc);
        let differences = (0..2000)
            .flat_map(|_| [Object::Integer(65), Object::Name(b"A".to_vec())])
            .collect::<Vec<_>>();
        doc.get_dictionary_mut(font).unwrap().set(
            "Encoding",
            dictionary! { "BaseEncoding" => "WinAnsiEncoding", "Differences" => differences },
        );
        let mut output = 1 << 20;
        let mut budget = PdfTextBudget {
            limit: 1 << 20,
            font_mapping_bytes: 32_768,
            run_bytes: 1 << 20,
            output_bytes: &mut output,
        };
        let error = font_metrics(&doc, doc.get_dictionary(font).unwrap(), &mut budget)
            .unwrap_err()
            .to_string();
        assert!(error.contains("budget"), "{error}");
    }

    #[test]
    fn missing_font_references_fail_even_when_other_text_is_readable() {
        for content in [
            "BT /F1 12 Tf 72 700 Td (Kept) Tj /Missing 12 Tf (Dropped) Tj ET",
            "BT (No selected font) Tj ET BT /F1 12 Tf (Kept) Tj ET",
            "BT [(No selected font)] TJ ET BT /F1 12 Tf (Kept) Tj ET",
            "BT (No selected font) ' ET BT /F1 12 Tf (Kept) Tj ET",
            "BT 0 0 (No selected font) \" ET BT /F1 12 Tf (Kept) Tj ET",
        ] {
            let mut doc = single_page_pdf(content);
            let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
            assert!(error.contains("PDF page 1"), "{error}");
            assert!(error.contains("font"), "{error}");
            assert!(error.contains("embedded fonts"), "{error}");
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let error = crate::parser::Parse::parse(
                &crate::parser::pdf::PdfParser::new(),
                "missing-font.pdf",
                &bytes,
            )
            .unwrap_err()
            .to_string();
            assert!(error.contains("PDF page 1"), "{error}");
        }
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Kept) Tj ET /Form Do");
        attach_form(
            &mut doc,
            dictionary! { "Subtype" => "Form", "Resources" => dictionary! {} },
            b"BT /Missing 12 Tf (Dropped) Tj ET",
        );
        let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
        for detail in ["PDF page 1", "XObject Form", "font Missing", "Resources"] {
            assert!(error.contains(detail), "{error}");
        }
    }

    #[test]
    fn malformed_font_selection_fails_and_empty_text_remains_valid() {
        for selection in ["17 12 Tf", "/F1 Tf", "/F1 (size) Tf"] {
            let doc = single_page_pdf(&format!(
                "BT /F1 12 Tf (Kept) Tj {selection} (Dropped) Tj ET"
            ));
            let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
            assert!(error.contains("Tf"), "{error}");
            assert!(error.contains("PDF page 1"), "{error}");
        }
        let doc =
            single_page_pdf("BT () Tj [()] TJ () ' 0 0 () \" /F1 12 Tf 72 700 Td (Body) Tj ET");
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "Body");
    }

    #[test]
    fn graphics_state_can_select_override_and_restore_text_fonts() {
        for content in [
            "/GS1 gs BT 72 700 Td (Wide) Tj ET",
            "BT /F1 12 Tf 72 700 Td (Page) Tj ET q /GS1 gs BT 72 680 Td (Wide) Tj ET Q BT 72 660 Td (Back) Tj ET",
        ] {
            let mut doc = single_page_pdf(content);
            let wide = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "FirstChar" => 32, "LastChar" => 126, "Widths" => vec![Object::Integer(1000); 95] });
            let page_id = doc.get_pages()[&1];
            doc.get_dictionary_mut(page_id).unwrap().get_mut(b"Resources").unwrap().as_dict_mut().unwrap().set(
                "ExtGState", dictionary! { "GS1" => dictionary! { "Font" => vec![Object::Reference(wide), Object::Integer(10)] } }
            );
            let lines = try_extract_positioned_lines(&doc).unwrap();
            let wide = lines.iter().find(|line| line.text == "Wide").unwrap();
            assert!((wide.x1 - wide.x0 - 40.0).abs() < 0.01, "{lines:?}");
            if lines.len() == 3 {
                assert_eq!(lines[0].text, "Page");
                assert_eq!(lines[2].text, "Back");
                assert!((lines[2].x1 - lines[2].x0 - 24.0).abs() < 0.01, "{lines:?}");
            } else {
                assert_eq!(lines.len(), 1, "{lines:?}");
            }
        }
    }

    #[test]
    fn unused_font_mappings_are_not_decoded_and_null_mapping_is_optional() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
        let broken_cmap = doc.add_object(lopdf::Stream::new(
            dictionary! { "Filter" => "UnknownFilter" },
            b"invalid cmap".to_vec(),
        ));
        let bad = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "ToUnicode" => broken_cmap });
        let page_id = doc.get_pages()[&1];
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .get_mut(b"Resources")
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .get_mut(b"Font")
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Bad", bad);
        assert_eq!(try_extract_positioned_lines(&doc).unwrap()[0].text, "Body");
        let font_id = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference()
            .unwrap();
        for indirect in [false, true] {
            let mut nullable = doc.clone();
            let value = if indirect {
                Object::Reference(nullable.add_object(Object::Null))
            } else {
                Object::Null
            };
            nullable
                .get_dictionary_mut(font_id)
                .unwrap()
                .set("ToUnicode", value);
            assert_eq!(
                try_extract_positioned_lines(&nullable).unwrap()[0].text,
                "Body"
            );
        }
        let stream_id = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_object_mut(stream_id)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .content = b"BT /Bad 12 Tf 72 700 Td (Body) Tj ET".to_vec();
        let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
        assert!(
            error.contains("font Bad") && error.contains("ToUnicode"),
            "{error}"
        );
    }

    #[test]
    fn font_resource_names_preserve_non_utf8_bytes_without_collisions() {
        let mut doc =
            single_page_pdf("BT /F#80 12 Tf 72 700 Td (A) Tj 0 -20 Td /F#81 12 Tf (B) Tj ET");
        let narrow = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "FirstChar" => 32, "LastChar" => 126, "Widths" => vec![Object::Integer(500); 95] });
        let wide = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica", "FirstChar" => 32, "LastChar" => 126, "Widths" => vec![Object::Integer(1000); 95] });
        let page_id = doc.get_pages()[&1];
        let fonts = doc
            .get_dictionary_mut(page_id)
            .unwrap()
            .get_mut(b"Resources")
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .get_mut(b"Font")
            .unwrap()
            .as_dict_mut()
            .unwrap();
        fonts.set(vec![b'F', 0x80], narrow);
        fonts.set(vec![b'F', 0x81], wide);
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert_eq!(lines[0].text, "A");
        assert_eq!(lines[1].text, "B");
        assert!((lines[0].x1 - lines[0].x0 - 6.0).abs() < 0.01, "{lines:?}");
        assert!((lines[1].x1 - lines[1].x0 - 12.0).abs() < 0.01, "{lines:?}");
    }

    fn indirect_predictor_stream(doc: &mut Document, body: &[u8]) -> lopdf::Stream {
        use std::io::Write;
        let mut rows = vec![0];
        rows.extend_from_slice(body);
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&rows).unwrap();
        let encoded = encoder
            .finish()
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            + ">";
        let ascii = doc.add_object(Object::Name(b"ASCIIHexDecode".to_vec()));
        let flate = doc.add_object(Object::Name(b"FlateDecode".to_vec()));
        let filters = doc.add_object(Object::Array(vec![ascii.into(), flate.into()]));
        let filters = doc.add_object(Object::Reference(filters));
        let predictor = doc.add_object(Object::Integer(15));
        let columns = doc.add_object(Object::Integer(body.len() as i64));
        let params = doc.add_object(dictionary! { "Predictor" => predictor, "Columns" => columns });
        let null = doc.add_object(Object::Null);
        let parameters = doc.add_object(Object::Array(vec![null.into(), params.into()]));
        lopdf::Stream::new(
            dictionary! { "Filter" => filters, "DecodeParms" => parameters },
            encoded.into_bytes(),
        )
    }

    #[test]
    fn tiff_predictor_depths_preserve_pdf_text_and_chunk_positions() {
        fn tiff_stream(body: &[u8], bits: usize) -> lopdf::Stream {
            use std::io::Write;
            let mut body = body.to_vec();
            if bits == 16 && !body.len().is_multiple_of(2) {
                body.push(b' ');
            }
            let mut samples = Vec::new();
            if bits == 16 {
                samples.extend(
                    body.chunks_exact(2)
                        .map(|pair| u16::from_be_bytes([pair[0], pair[1]])),
                );
            } else {
                let mask = ((1u16 << bits) - 1) as u8;
                for byte in &body {
                    for shift in (0..8).step_by(bits).rev() {
                        samples.push(u16::from((byte >> shift) & mask));
                    }
                }
            }
            let mask = if bits == 16 {
                u16::MAX
            } else {
                (1u16 << bits) - 1
            };
            let mut deltas = samples.clone();
            for index in 1..samples.len() {
                deltas[index] = samples[index].wrapping_sub(samples[index - 1]) & mask;
            }
            let mut packed = Vec::new();
            if bits == 16 {
                for sample in deltas {
                    packed.extend_from_slice(&sample.to_be_bytes());
                }
            } else {
                for chunk in deltas.chunks(8 / bits) {
                    packed.push(
                        chunk
                            .iter()
                            .fold(0u16, |byte, value| (byte << bits) | *value)
                            as u8,
                    );
                }
            }
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&packed).unwrap();
            lopdf::Stream::new(
                dictionary! { "Filter" => "FlateDecode", "DecodeParms" => dictionary! { "Predictor" => 2, "BitsPerComponent" => bits as i64, "Columns" => samples.len() as i64 } },
                encoder.finish().unwrap(),
            )
        }
        for bits in [1, 2, 4, 8, 16] {
            let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
            let page = doc.get_pages()[&1];
            let content = doc
                .get_dictionary(page)
                .unwrap()
                .get(b"Contents")
                .unwrap()
                .as_reference()
                .unwrap();
            doc.objects.insert(
                content,
                Object::Stream(tiff_stream(b"BT /F1 12 Tf 72 700 Td (Body) Tj ET", bits)),
            );
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let parsed = crate::parser::Parse::parse(
                &crate::parser::pdf::PdfParser::new(),
                "tiff-prediction.pdf",
                &bytes,
            )
            .unwrap();
            assert!(
                parsed.content.contains("Body"),
                "bits={bits}: {}",
                parsed.content
            );
            let config = crate::ParserConfig::default();
            let chunks = crate::chunk::chunker_for(&config)
                .chunk(&parsed, &config)
                .unwrap();
            assert!(chunks.iter().any(|chunk| chunk.content.contains("Body")));
            assert!(
                chunks
                    .iter()
                    .all(|chunk| chunk.metadata.contains_key("position_int"))
            );
        }
    }

    #[test]
    fn lzw_and_run_length_pdf_body_and_cmap_reach_positioned_chunks() {
        fn encoded_stream(bytes: &[u8], mode: usize) -> lopdf::Stream {
            let mut dictionary = dictionary! {};
            let content = if mode < 2 {
                let mut encoder = if mode == 1 {
                    weezl::encode::Encoder::with_tiff_size_switch(weezl::BitOrder::Msb, 8)
                } else {
                    weezl::encode::Encoder::new(weezl::BitOrder::Msb, 8)
                };
                dictionary.set("Filter", "LZWDecode");
                dictionary.set("DecodeParms", dictionary! { "EarlyChange" => mode as i64 });
                encoder.encode(bytes).unwrap()
            } else {
                dictionary.set("Filter", "RunLengthDecode");
                let mut content = Vec::new();
                for chunk in bytes.chunks(128) {
                    content.push(chunk.len() as u8 - 1);
                    content.extend_from_slice(chunk);
                }
                content.push(128);
                content
            };
            lopdf::Stream::new(dictionary, content)
        }
        for mode in 0..3 {
            let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (AB) Tj ET");
            let page = doc.get_pages()[&1];
            let contents = doc
                .get_dictionary(page)
                .unwrap()
                .get(b"Contents")
                .unwrap()
                .as_reference()
                .unwrap();
            doc.objects.insert(
                contents,
                Object::Stream(encoded_stream(b"BT /F1 12 Tf 72 700 Td (AB) Tj ET", mode)),
            );
            let font = doc
                .get_dictionary(page)
                .unwrap()
                .get(b"Resources")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"Font")
                .unwrap()
                .as_dict()
                .unwrap()
                .get(b"F1")
                .unwrap()
                .as_reference()
                .unwrap();
            let cmap = doc.add_object(encoded_stream(
                b"2 beginbfchar <41> <4e2d> <42> <6587> endbfchar",
                mode,
            ));
            doc.get_dictionary_mut(font).unwrap().set("ToUnicode", cmap);
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let parsed = crate::parser::Parse::parse(
                &crate::parser::pdf::PdfParser::new(),
                "additional-filters.pdf",
                &bytes,
            )
            .unwrap();
            assert!(
                parsed.content.contains("中文"),
                "mode={mode}: {}",
                parsed.content
            );
            let config = crate::ParserConfig::default();
            let chunks = crate::chunk::chunker_for(&config)
                .chunk(&parsed, &config)
                .unwrap();
            assert!(chunks.iter().any(|chunk| chunk.content.contains("中文")));
            assert!(
                chunks
                    .iter()
                    .all(|chunk| chunk.metadata.contains_key("position_int"))
            );
            let stream = doc
                .get_object_mut(contents)
                .unwrap()
                .as_stream_mut()
                .unwrap();
            stream.content.pop();
            assert!(try_extract_positioned_lines(&doc).is_err(), "mode={mode}");
        }
    }

    #[test]
    fn indirect_filter_settings_reach_page_cmap_and_nested_form_text() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (AB) Tj ET /Form Do");
        let page = doc.get_pages()[&1];
        let contents = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        let stream =
            indirect_predictor_stream(&mut doc, b"BT /F1 12 Tf 72 700 Td (AB) Tj ET /Form Do");
        doc.objects.insert(contents, Object::Stream(stream));
        let array = doc.add_object(Object::Array(vec![contents.into()]));
        doc.get_dictionary_mut(page).unwrap().set("Contents", array);
        let font = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference()
            .unwrap();
        let cmap =
            indirect_predictor_stream(&mut doc, b"1 beginbfrange <41> <42> <4e2d> endbfrange");
        let cmap = doc.add_object(cmap);
        doc.get_dictionary_mut(font).unwrap().set("ToUnicode", cmap);
        let form_stream = indirect_predictor_stream(&mut doc, b"BT /F1 12 Tf 72 680 Td (AB) Tj ET");
        let form = attach_form(&mut doc, dictionary! { "Subtype" => "Form" }, b"");
        let mut form_stream = form_stream;
        form_stream.dict.set("Subtype", "Form");
        doc.objects.insert(form, Object::Stream(form_stream));
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let parsed = crate::parser::Parse::parse(
            &crate::parser::pdf::PdfParser::new(),
            "indirect.pdf",
            &bytes,
        )
        .unwrap();
        assert_eq!(
            parsed.content.matches("中丮").count(),
            2,
            "{}",
            parsed.content
        );
        let config = crate::ParserConfig::default();
        let chunks = crate::chunk::chunker_for(&config)
            .chunk(&parsed, &config)
            .unwrap();
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.metadata.contains_key("position_int"))
        );
    }

    #[test]
    fn broken_page_content_references_fail_instead_of_indexing_partial_body() {
        for failure in ["missing", "nonstream", "cycle"] {
            let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Good) Tj ET");
            let page = doc.get_pages()[&1];
            let good = doc
                .get_dictionary(page)
                .unwrap()
                .get(b"Contents")
                .unwrap()
                .as_reference()
                .unwrap();
            let broken = match failure {
                "missing" => doc.new_object_id(),
                "nonstream" => doc.add_object(Object::Integer(17)),
                _ => {
                    let id = doc.new_object_id();
                    doc.objects.insert(id, Object::Reference(id));
                    id
                }
            };
            let array = doc.add_object(Object::Array(vec![good.into(), broken.into()]));
            doc.get_dictionary_mut(page).unwrap().set("Contents", array);
            let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
            for detail in ["PDF page 1", "upload it again"] {
                assert!(error.contains(detail), "{failure}: {error}");
            }
            doc.get_dictionary_mut(page).unwrap().set("Contents", good);
            assert_eq!(try_extract_positioned_lines(&doc).unwrap()[0].text, "Good");
        }
    }

    #[test]
    fn nested_forms_keep_local_fonts_transforms_and_caller_state() {
        let mut doc =
            single_page_pdf("q 2 0 0 2 100 200 cm /Form Do Q BT /F1 10 Tf 30 500 Td (Page) Tj ET");
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
            "FirstChar" => 32, "LastChar" => 126,
            "Widths" => vec![Object::Integer(1000); 95],
        });
        let inner_id = doc.add_object(lopdf::Stream::new(
            dictionary! {
                "Subtype" => "Form",
                "Matrix" => vec![1.into(), 0.into(), 0.into(), 1.into(), 3.into(), 4.into()],
                "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
            },
            b"BT /F1 10 Tf 0 0 Td (Form marker) Tj ET".to_vec(),
        ));
        attach_form(
            &mut doc,
            dictionary! {
                "Subtype" => "Form",
                "Matrix" => vec![1.into(), 0.into(), 0.into(), 1.into(), 10.into(), 20.into()],
                "Resources" => dictionary! { "XObject" => dictionary! { "Inner" => inner_id } },
            },
            b"/Inner Do",
        );
        let lines = try_extract_positioned_lines(&doc).unwrap();
        let form = lines
            .iter()
            .find(|line| line.text == "Form marker")
            .unwrap();
        assert!((form.x0 - 126.0).abs() < 0.01, "{form:?}");
        assert!((form.x1 - 346.0).abs() < 0.01, "{form:?}");
        let page = lines.iter().find(|line| line.text == "Page").unwrap();
        assert!((page.x0 - 30.0).abs() < 0.01, "{page:?}");
        assert!((page.x1 - 50.0).abs() < 0.01, "{page:?}");
    }

    #[test]
    fn rotated_nested_form_without_resources_uses_page_font_dictionary() {
        let mut doc = single_page_pdf("q 0 1 -1 0 210 60 cm /Form Do Q");
        let inner = doc.add_object(lopdf::Stream::new(
            dictionary! { "Subtype" => "Form" },
            b"BT /F1 12 Tf 0 0 Td (Nested fallback) Tj ET".to_vec(),
        ));
        attach_form(
            &mut doc,
            dictionary! {
                "Subtype" => "Form",
                "Resources" => dictionary! { "XObject" => dictionary! { "Inner" => inner } },
            },
            b"/Inner Do",
        );
        let page_id = doc.get_pages()[&1];
        let page = doc.get_dictionary_mut(page_id).unwrap();
        page.set("Rotate", 90);
        page.set(
            "CropBox",
            vec![10.into(), 20.into(), 310.into(), 520.into()],
        );
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].text, "Nested fallback");
        assert!((lines[0].x0 - 40.0).abs() < 0.01, "{lines:?}");
        assert!((lines[0].top - 191.0).abs() < 0.01, "{lines:?}");
        assert_eq!(lines[0].page_height, 300.0);
    }

    #[test]
    fn null_page_rotation_uses_parent_or_default_rotation() {
        for angle in [0, 90] {
            let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
            let page_id = doc.get_pages()[&1];
            doc.get_dictionary_mut(page_id)
                .unwrap()
                .set("Rotate", angle);
            let expected = try_extract_positioned_lines(&doc).unwrap();
            let parent_id = doc
                .get_dictionary(page_id)
                .unwrap()
                .get(b"Parent")
                .unwrap()
                .as_reference()
                .unwrap();
            if angle != 0 {
                doc.get_dictionary_mut(parent_id)
                    .unwrap()
                    .set("Rotate", angle);
            }
            doc.get_dictionary_mut(page_id)
                .unwrap()
                .set("Rotate", Object::Null);
            assert_eq!(try_extract_positioned_lines(&doc).unwrap(), expected);
        }
    }

    #[test]
    fn forms_inherit_selected_font_and_restore_the_caller() {
        let mut doc = single_page_pdf("BT /F1 10 Tf ET /Form Do BT 30 500 Td (Page) Tj ET");
        attach_form(
            &mut doc,
            dictionary! { "Subtype" => "Form", "Resources" => dictionary! {} },
            b"BT 72 700 Td (Inherited font) Tj ET",
        );
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert!(lines.iter().any(|line| line.text == "Inherited font"));
        assert!(lines.iter().any(|line| line.text == "Page"));
    }

    #[test]
    fn forms_reject_cycles_corrupt_streams_and_invalid_matrices_with_page_guidance() {
        for failure in ["cycle", "filter", "matrix", "resources"] {
            let mut doc = single_page_pdf("/Form Do BT /F1 12 Tf 72 700 Td (Page) Tj ET");
            let form_id = attach_form(&mut doc, dictionary! { "Subtype" => "Form" }, b"/Form Do");
            let stream = doc
                .get_object_mut(form_id)
                .unwrap()
                .as_stream_mut()
                .unwrap();
            let expected = match failure {
                "cycle" => "cyclic",
                "filter" => {
                    stream.dict.set("Filter", "UnsupportedDecode");
                    "UnsupportedDecode"
                }
                "matrix" => {
                    stream.dict.set("Matrix", vec![Object::Integer(1)]);
                    "six numbers"
                }
                _ => {
                    stream.dict.set("Resources", 1);
                    "Resources"
                }
            };
            let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
            for message in ["PDF page 1", "XObject Form", expected, "OCR", "重新解析"] {
                assert!(error.contains(message), "{failure}: {error}");
            }
        }
    }

    #[test]
    fn forms_obey_shared_decode_and_operation_budgets() {
        let mut doc = single_page_pdf("/Form Do /Form Do");
        attach_form(&mut doc, dictionary! { "Subtype" => "Form" }, b"q Q");
        let page_id = doc.get_pages()[&1];
        let resources =
            inherited(&doc, page_id, b"Resources").and_then(|value| as_dict(&doc, value));
        let content = lopdf::content::Content::decode(b"/Form Do /Form Do").unwrap();
        for (bytes, operations, expected) in [(4, 20, "safety limit"), (20, 3, "operation limit")] {
            let mut output_bytes = 1 << 20;
            let mut interpreter = ContentInterpreter {
                doc: &doc,
                page_resources: resources,
                forms: Vec::new(),
                depth: 0,
                remaining_bytes: bytes,
                remaining_operations: operations,
                text_budget: PdfTextBudget {
                    limit: 1 << 20,
                    font_mapping_bytes: 1 << 20,
                    run_bytes: 1 << 20,
                    output_bytes: &mut output_bytes,
                },
            };
            let error = interpreter
                .walk(&content, resources, &mut State::default(), &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
        }
    }

    #[test]
    fn text_and_font_budgets_are_shared_with_nested_forms() {
        let mut doc = single_page_pdf("q BT /F1 12 Tf 72 700 Td (Hello) Tj ET Q /Form Do");
        attach_form(
            &mut doc,
            dictionary! { "Subtype" => "Form" },
            b"BT /F1 12 Tf 72 680 Td (World) Tj ET",
        );
        let page_id = doc.get_pages()[&1];
        let resources =
            inherited(&doc, page_id, b"Resources").and_then(|value| as_dict(&doc, value));
        let content =
            lopdf::content::Content::decode(b"q BT /F1 12 Tf 72 700 Td (Hello) Tj ET Q /Form Do")
                .unwrap();
        for (fonts, mut output, expected) in
            [(40_000, 1 << 20, "budget"), (1 << 20, 8, "document budget")]
        {
            let mut interpreter = ContentInterpreter {
                doc: &doc,
                page_resources: resources,
                forms: Vec::new(),
                depth: 0,
                remaining_bytes: 1 << 20,
                remaining_operations: 100,
                text_budget: PdfTextBudget {
                    limit: 1 << 20,
                    font_mapping_bytes: fonts,
                    run_bytes: 1 << 20,
                    output_bytes: &mut output,
                },
            };
            let error = interpreter
                .walk(&content, resources, &mut State::default(), &mut Vec::new())
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected), "{error}");
            assert!(error.contains("XObject Form"), "{error}");
        }
    }

    #[test]
    fn document_text_budget_survives_page_interpreter_recreation() {
        let doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Hello) Tj ET");
        let page_id = doc.get_pages()[&1];
        let resources =
            inherited(&doc, page_id, b"Resources").and_then(|value| as_dict(&doc, value));
        let content =
            lopdf::content::Content::decode(b"BT /F1 12 Tf 72 700 Td (Hello) Tj ET").unwrap();
        let mut output = 8;
        for page in 0..2 {
            let mut interpreter = ContentInterpreter {
                doc: &doc,
                page_resources: resources,
                forms: Vec::new(),
                depth: 0,
                remaining_bytes: 1 << 20,
                remaining_operations: 100,
                text_budget: PdfTextBudget {
                    limit: 1 << 20,
                    font_mapping_bytes: 1 << 20,
                    run_bytes: 1 << 20,
                    output_bytes: &mut output,
                },
            };
            let result =
                interpreter.walk(&content, resources, &mut State::default(), &mut Vec::new());
            if page == 0 {
                assert!(result.is_ok());
            } else {
                assert!(result.unwrap_err().to_string().contains("document budget"));
            }
        }
    }

    #[test]
    fn repeated_mapped_text_fails_before_exceeding_document_budget() {
        let mut doc = single_page_pdf(&format!(
            "BT /F1 12 Tf 72 700 Td ({}) Tj ET",
            "A".repeat(1 << 20)
        ));
        let cmap = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            format!("1 beginbfchar <41> <{}> endbfchar", "0041".repeat(128)).into_bytes(),
        ));
        let page_id = doc.get_pages()[&1];
        let font_id = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_dictionary_mut(font_id)
            .unwrap()
            .set("ToUnicode", cmap);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let error = crate::parser::Parse::parse(
            &crate::parser::pdf::PdfParser::new(),
            "expanded.pdf",
            &bytes,
        )
        .unwrap_err()
        .to_string();
        for detail in [
            "PDF page 1",
            "document budget",
            "RAYRAG_PDF_STREAM_LIMIT_BYTES",
            "upload it again",
        ] {
            assert!(error.contains(detail), "{error}");
        }
    }

    #[test]
    fn unreadable_stream_is_rejected_even_without_page_geometry() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
        let page_id = doc.get_pages()[&1];
        let stream_id = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Contents")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_object_mut(stream_id)
            .unwrap()
            .as_stream_mut()
            .unwrap()
            .dict
            .set("Filter", "UnsupportedDecode");
        doc.get_dictionary_mut(page_id).unwrap().remove(b"MediaBox");
        let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
        assert!(error.contains("PDF page 1"), "{error}");
        assert!(error.contains("UnsupportedDecode"), "{error}");
        assert!(error.contains("重新解析"), "{error}");
        assert!(extract_positioned_lines(&doc).is_empty());
    }

    #[test]
    fn missing_page_geometry_is_reported_instead_of_skipping_text() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
        let page_id = doc.get_pages()[&1];
        doc.get_dictionary_mut(page_id).unwrap().remove(b"MediaBox");

        let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
        for detail in [
            "PDF page 1",
            "CropBox",
            "MediaBox",
            "upload it again",
            "重新导出",
        ] {
            assert!(error.contains(detail), "{error}");
        }
        assert!(extract_positioned_lines(&doc).is_empty());
    }

    #[test]
    fn malformed_crop_box_falls_back_to_valid_media_box() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
        let page_id = doc.get_pages()[&1];
        doc.get_dictionary_mut(page_id)
            .unwrap()
            .set("CropBox", vec![Object::Integer(0)]);

        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].text, "Body");
    }

    #[test]
    fn rotated_pages_keep_display_reading_order_and_position_tags() {
        for angle in [0_i64, 90, 180, 270, -90, 450] {
            let text_matrix = match angle.rem_euclid(360) {
                0 => "1 0 0 1 50 120",
                90 => "0 1 -1 0 210 60",
                180 => "-1 0 0 -1 270 420",
                270 => "0 -1 1 0 110 480",
                _ => unreachable!(),
            };
            let mut doc = single_page_pdf(&format!(
                "BT /F1 12 Tf {text_matrix} Tm (Alpha) Tj ( tail) Tj 0 -20 Td (Beta) Tj ET"
            ));
            let page_id = doc.get_pages()[&1];
            doc.get_dictionary_mut(page_id).unwrap().set(
                "MediaBox",
                vec![10.into(), 20.into(), 310.into(), 520.into()],
            );
            if angle == 90 {
                let parent_id = doc
                    .get_dictionary(page_id)
                    .unwrap()
                    .get(b"Parent")
                    .unwrap()
                    .as_reference()
                    .unwrap();
                let rotate_id = doc.add_object(Object::Integer(angle));
                doc.get_dictionary_mut(parent_id)
                    .unwrap()
                    .set("Rotate", rotate_id);
            } else {
                doc.get_dictionary_mut(page_id)
                    .unwrap()
                    .set("Rotate", angle);
            }
            let lines = try_extract_positioned_lines(&doc).unwrap();
            assert_eq!(lines.len(), 2, "angle={angle}: {lines:?}");
            assert_eq!(lines[0].text, "Alpha tail", "angle={angle}");
            assert_eq!(lines[1].text, "Beta", "angle={angle}");
            let height = if angle.rem_euclid(180) == 0 {
                500.0
            } else {
                300.0
            };
            for (index, line) in lines.iter().enumerate() {
                assert!((line.x0 - 40.0).abs() < 0.01, "angle={angle}: {line:?}");
                assert!(
                    (line.top - (height - 109.0 + 20.0 * index as f64)).abs() < 0.01,
                    "angle={angle}: {line:?}"
                );
                assert_eq!(line.page_height, height);
                assert!(line.tag().starts_with("@@1\t40.0\t"));
            }
            let mut bytes = Vec::new();
            doc.save_to(&mut bytes).unwrap();
            let parsed = crate::parser::Parse::parse(
                &crate::parser::pdf::PdfParser::new(),
                "rotated.pdf",
                &bytes,
            )
            .unwrap();
            let config = crate::ParserConfig::default();
            let chunks = crate::chunk::chunker_for(&config)
                .chunk(&parsed, &config)
                .unwrap();
            assert!(!chunks.is_empty(), "angle={angle}");
            let content = chunks
                .iter()
                .map(|chunk| chunk.content.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                content.contains("Alpha tail") && content.contains("Beta"),
                "angle={angle}: {content}"
            );
            assert!(
                chunks
                    .iter()
                    .all(|chunk| chunk.metadata.contains_key("position_int"))
            );
        }
    }

    #[test]
    fn invalid_page_rotation_is_an_actionable_parser_error() {
        for rotation in [Object::Integer(45), Object::Name(b"ninety".to_vec())] {
            let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
            let page_id = doc.get_pages()[&1];
            doc.get_dictionary_mut(page_id)
                .unwrap()
                .set("Rotate", rotation);
            let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
            for detail in ["PDF page 1", "Rotate", "90 degrees", "upload it again"] {
                assert!(error.contains(detail), "{error}");
            }
        }
    }

    #[test]
    fn page_boxes_accept_all_diagonal_corner_orders() {
        let content = "BT /F1 12 Tf 72 700 Td (Body) Tj ET";
        for key in ["MediaBox", "CropBox"] {
            let mut expected_doc = single_page_pdf(content);
            let page_id = expected_doc.get_pages()[&1];
            expected_doc
                .get_dictionary_mut(page_id)
                .unwrap()
                .set(key, vec![10.into(), 20.into(), 612.into(), 792.into()]);
            let expected = try_extract_positioned_lines(&expected_doc).unwrap();
            for corners in [[612, 20, 10, 792], [10, 792, 612, 20], [612, 792, 10, 20]] {
                let mut doc = expected_doc.clone();
                doc.get_dictionary_mut(page_id).unwrap().set(
                    key,
                    corners.into_iter().map(Object::Integer).collect::<Vec<_>>(),
                );
                assert_eq!(
                    try_extract_positioned_lines(&doc).unwrap(),
                    expected,
                    "{key} {corners:?}"
                );
            }
        }
    }

    #[test]
    fn inherited_page_boxes_resolve_indirect_numeric_coordinates() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
        let expected = try_extract_positioned_lines(&doc).unwrap();
        let page_id = doc.get_pages()[&1];
        let parent_id = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Parent")
            .unwrap()
            .as_reference()
            .unwrap();
        let coordinates =
            [0, 0, 612, 792].map(|value| Object::Reference(doc.add_object(Object::Integer(value))));
        let box_id = doc.add_object(Object::Array(coordinates.to_vec()));
        doc.get_dictionary_mut(page_id).unwrap().remove(b"MediaBox");
        doc.get_dictionary_mut(parent_id)
            .unwrap()
            .set("MediaBox", box_id);
        assert_eq!(try_extract_positioned_lines(&doc).unwrap(), expected);
    }

    #[test]
    fn invalid_page_box_coordinates_fail_with_page_repair_guidance() {
        for invalid in [
            Object::Null,
            Object::Real(f32::NAN),
            Object::Real(f32::INFINITY),
            Object::Integer(0),
            Object::Reference((9999, 0)),
        ] {
            let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Body) Tj ET");
            let page_id = doc.get_pages()[&1];
            doc.get_dictionary_mut(page_id)
                .unwrap()
                .set("MediaBox", vec![0.into(), 0.into(), 612.into(), invalid]);
            let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
            assert!(error.contains("PDF page 1"), "{error}");
            assert!(
                error.contains("repair") || error.contains("Repair"),
                "{error}"
            );
        }
    }

    #[test]
    fn text_runs_are_boxed_and_tagged_like_upstream() {
        // 12 pt at baseline y = 700 on a 792 pt page: eight 500/1000 em glyphs
        // plus a 250/1000 space, so the run is 51 pt wide from x = 72.
        let doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (Hello wor) Tj ET");
        let lines = extract_positioned_lines(&doc);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(line.page, 1);
        assert_eq!(line.text, "Hello wor");
        assert!((line.x0 - 72.0).abs() < 0.01, "{line:?}");
        assert!((line.x1 - 123.0).abs() < 0.01, "{line:?}");
        // Ascent 0.75 em above the baseline, descent 0.25 em below it.
        assert!((line.top - 83.0).abs() < 0.01, "{line:?}");
        assert!((line.bottom - 95.0).abs() < 0.01, "{line:?}");
        assert_eq!(line.tag(), "@@1\t72.0\t123.0\t83.0\t95.0##");
    }

    #[test]
    fn tj_kerning_advances_the_pen_between_runs() {
        // -500 units pulls the pen forward by half an em (5 pt at 10 pt), so the
        // second run starts 15 + 5 = 20 pt after the first and leaves a gap.
        let doc = single_page_pdf("BT /F1 10 Tf 1 0 0 1 50 500 Tm [(Hel) -500 (lo)] TJ ET");
        let lines = extract_positioned_lines(&doc);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].text, "Hel lo");
        assert!((lines[0].x0 - 50.0).abs() < 0.01, "{lines:?}");
        assert!((lines[0].x1 - 80.0).abs() < 0.01, "{lines:?}");
    }

    #[test]
    fn adjacent_runs_join_without_a_space() {
        let doc = single_page_pdf("BT /F1 10 Tf 1 0 0 1 50 500 Tm (Hel) Tj (lo) Tj ET");
        let lines = extract_positioned_lines(&doc);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].text, "Hello");
        assert!((lines[0].x1 - 75.0).abs() < 0.01, "{lines:?}");
    }

    #[test]
    fn separate_baselines_become_separate_lines_in_reading_order() {
        let content = "BT /F1 12 Tf 1 0 0 1 72 700 Tm (second) Tj 0 -20 Td (third) Tj ET \
                       BT /F1 12 Tf 72 740 Td (first) Tj ET";
        let doc = single_page_pdf(content);
        let lines = extract_positioned_lines(&doc);
        let texts: Vec<&str> = lines.iter().map(|line| line.text.as_str()).collect();
        assert_eq!(texts, ["first", "second", "third"], "{lines:?}");
        assert!(lines[0].top < lines[1].top && lines[1].top < lines[2].top);
        // `0 -20 Td` moves the pen down 20 pt, so the tops step by 20:
        // 792 - (700 + 9) = 83, then 103.
        assert!((lines[1].top - 83.0).abs() < 0.01, "{lines:?}");
        assert!((lines[2].top - 103.0).abs() < 0.01, "{lines:?}");
    }

    #[test]
    fn cropped_pages_are_projected_onto_their_own_box() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 20 780 Td (Cropped) Tj ET");
        let page_id = *doc.get_pages().values().next().unwrap();
        doc.get_dictionary_mut(page_id).unwrap().set(
            "CropBox",
            vec![10.into(), 20.into(), 500.into(), 800.into()],
        );
        let lines = extract_positioned_lines(&doc);
        assert_eq!(lines.len(), 1);
        // x is relative to the crop box's left edge, top to its top edge.
        assert!((lines[0].x0 - 10.0).abs() < 0.01, "{lines:?}");
        assert!((lines[0].top - 11.0).abs() < 0.01, "{lines:?}");
    }

    fn cid_width_document(widths: Object) -> (Document, lopdf::ObjectId, lopdf::ObjectId) {
        let mut doc = single_page_pdf("BT /F1 10 Tf 100 700 Td <000300040005> Tj ET");
        let descendant = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "CIDFontType2", "DW" => 1000, "W" => widths });
        let cmap = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"1 beginbfrange <0003> <0005> <0041> endbfrange".to_vec(),
        ));
        let page_id = doc.get_pages()[&1];
        let font = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.objects.insert(font, Object::Dictionary(dictionary! { "Type" => "Font", "Subtype" => "Type0", "Encoding" => "Identity-H", "ToUnicode" => cmap, "DescendantFonts" => vec![Object::Reference(descendant)] }));
        (doc, font, descendant)
    }

    #[test]
    fn cid_width_indirect_arrays_values_and_default_widths_are_resolved() {
        let (mut doc, font, descendant) = cid_width_document(Object::Null);
        let start = doc.add_object(Object::Integer(3));
        let width = doc.add_object(Object::Integer(600));
        let list = doc.add_object(Object::Array(vec![
            Object::Reference(width),
            700.into(),
            800.into(),
        ]));
        let table = doc.add_object(Object::Array(vec![
            Object::Reference(start),
            Object::Reference(list),
        ]));
        doc.get_dictionary_mut(descendant).unwrap().set("W", table);
        let children = doc.add_object(Object::Array(vec![Object::Reference(descendant)]));
        doc.get_dictionary_mut(font)
            .unwrap()
            .set("DescendantFonts", children);
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert_eq!(lines[0].text, "ABC");
        assert!((lines[0].x1 - lines[0].x0 - 21.0).abs() < 0.01, "{lines:?}");
        let default = doc.add_object(Object::Integer(500));
        let descriptor = doc
            .add_object(dictionary! { "MissingWidth" => 900, "Ascent" => 750, "Descent" => -250 });
        let child = doc.get_dictionary_mut(descendant).unwrap();
        child.set("W", Object::Null);
        child.set("DW", default);
        child.set("FontDescriptor", descriptor);
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert!((lines[0].x1 - lines[0].x0 - 15.0).abs() < 0.01, "{lines:?}");
        doc.get_dictionary_mut(descendant)
            .unwrap()
            .set("DW", Object::Null);
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert!((lines[0].x1 - lines[0].x0 - 30.0).abs() < 0.01, "{lines:?}");
    }

    #[test]
    fn malformed_cid_widths_fail_with_page_font_and_repair_context() {
        for widths in [
            vec![65536.into(), Object::Array(vec![500.into()])],
            vec![(-1).into(), Object::Array(vec![500.into()])],
            vec![Object::Real(3.5), Object::Array(vec![500.into()])],
            vec![65535.into(), Object::Array(vec![500.into(), 600.into()])],
            vec![7.into(), 3.into(), 500.into()],
            vec![3.into()],
            vec![3.into(), 5.into()],
            vec![3.into(), Object::Array(vec![Object::Name(b"bad".to_vec())])],
            vec![3.into(), 5.into(), Object::Real(f32::INFINITY)],
        ] {
            let (doc, _, _) = cid_width_document(Object::Array(widths));
            let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
            for detail in ["PDF page 1", "font F1", "/W", "upload it again"] {
                assert!(error.contains(detail), "{error}");
            }
        }
        let (mut doc, font, descendant) = cid_width_document(Object::Array(vec![
            65535.into(),
            Object::Array(vec![500.into(), 600.into()]),
        ]));
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        assert!(
            crate::parser::Parse::parse(
                &crate::parser::pdf::PdfParser::new(),
                "widths.pdf",
                &bytes
            )
            .is_err()
        );
        doc.get_dictionary_mut(descendant)
            .unwrap()
            .set("W", vec![3.into(), 5.into(), 600.into()]);
        assert_eq!(try_extract_positioned_lines(&doc).unwrap()[0].text, "ABC");
        doc.get_dictionary_mut(font)
            .unwrap()
            .set("DescendantFonts", Object::Null);
        assert!(
            try_extract_positioned_lines(&doc)
                .unwrap_err()
                .to_string()
                .contains("DescendantFonts")
        );
    }

    #[test]
    fn width_expansion_redefinitions_consume_budget_and_maximum_cid_does_not_wrap() {
        let doc = Document::with_version("1.7");
        let mut output = 4096;
        let mut budget = PdfTextBudget {
            limit: 4096,
            font_mapping_bytes: 128,
            run_bytes: 4096,
            output_bytes: &mut output,
        };
        let mut widths = HashMap::new();
        let error = parse_cid_widths(
            &doc,
            &[
                1.into(),
                2.into(),
                500.into(),
                1.into(),
                2.into(),
                600.into(),
            ],
            &mut widths,
            &mut budget,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("page budget"), "{error}");
        assert_eq!(widths[&1], 500.0);
        budget.font_mapping_bytes = 128;
        widths.clear();
        parse_cid_widths(
            &doc,
            &[65535.into(), Object::Array(vec![700.into()])],
            &mut widths,
            &mut budget,
        )
        .unwrap();
        assert_eq!(widths[&65535], 700.0);
        assert!(!widths.contains_key(&0));
    }

    #[test]
    fn simple_widths_and_descriptor_references_preserve_positions() {
        let mut doc = single_page_pdf("BT /F1 10 Tf 72 700 Td (ABC) Tj ET");
        let page = doc.get_pages()[&1];
        let font = doc
            .get_dictionary(page)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference()
            .unwrap();
        let first = doc.add_object(Object::Integer(65));
        let width = doc.add_object(Object::Integer(500));
        let widths = doc.add_object(Object::Array(vec![
            Object::Reference(width),
            Object::Real(750.0),
        ]));
        let missing = doc.add_object(Object::Integer(900));
        let ascent = doc.add_object(Object::Integer(800));
        let descriptor = doc.add_object(
            dictionary! { "MissingWidth" => missing, "Ascent" => ascent, "Descent" => -200 },
        );
        let dictionary = doc.get_dictionary_mut(font).unwrap();
        dictionary.set("FirstChar", first);
        dictionary.set("Widths", widths);
        dictionary.set("FontDescriptor", descriptor);
        let lines = try_extract_positioned_lines(&doc).unwrap();
        assert_eq!(lines[0].text, "ABC");
        assert!((lines[0].x1 - lines[0].x0 - 21.5).abs() < 0.01, "{lines:?}");
        assert!((lines[0].top - 84.0).abs() < 0.01, "{lines:?}");
        doc.get_dictionary_mut(font).unwrap().set("FirstChar", 255);
        assert!(
            try_extract_positioned_lines(&doc)
                .unwrap_err()
                .to_string()
                .contains("/Widths")
        );
        doc.get_dictionary_mut(font).unwrap().set("FirstChar", -1);
        assert!(
            try_extract_positioned_lines(&doc)
                .unwrap_err()
                .to_string()
                .contains("/FirstChar")
        );
        doc.get_dictionary_mut(font).unwrap().set("FirstChar", 65);
        doc.get_dictionary_mut(descriptor)
            .unwrap()
            .set("Ascent", Object::Real(f32::NAN));
        assert!(
            try_extract_positioned_lines(&doc)
                .unwrap_err()
                .to_string()
                .contains("/Ascent")
        );
    }

    #[test]
    fn overflowing_text_transforms_fail_before_invalid_position_tags() {
        let scale =
            "1000000000000000000000000000000.0 0 0 1000000000000000000000000000000.0 0 0 cm ";
        let doc = single_page_pdf(&format!(
            "{} BT /F1 12 Tf 72 700 Td (Body) Tj ET",
            scale.repeat(12)
        ));
        let error = try_extract_positioned_lines(&doc).unwrap_err().to_string();
        assert!(error.contains("non-finite coordinates"), "{error}");
        assert!(error.contains("PDF page 1"), "{error}");
    }

    #[test]
    fn identity_h_fonts_decode_through_to_unicode_and_use_cid_widths() {
        // A subset CID font: <0003>/<0004> map to "AB", widths come from /W.
        let cmap = "/CIDInit /ProcSet findresource begin\n\
                    2 beginbfchar\n\
                    <0003> <0041>\n\
                    <0004> <0042>\n\
                    endbfchar\n";
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let cmap_id = doc.add_object(lopdf::Stream::new(dictionary! {}, cmap.as_bytes().to_vec()));
        let descendant_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "CIDFontType2",
            "BaseFont" => "Subset",
            "DW" => 1000,
            "W" => vec![
                3.into(),
                Object::Array(vec![Object::Real(600.0), Object::Real(700.0)]),
            ],
        });
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type0",
            "BaseFont" => "Subset",
            "Encoding" => "Identity-H",
            "ToUnicode" => cmap_id,
            "DescendantFonts" => vec![descendant_id.into()],
        });
        let content_id = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"BT /F1 10 Tf 100 700 Td <00030004> Tj ET".to_vec(),
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

        let lines = extract_positioned_lines(&doc);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].text, "AB");
        // (600 + 700) / 1000 * 10 pt = 13 pt wide.
        assert!((lines[0].x1 - lines[0].x0 - 13.0).abs() < 0.01, "{lines:?}");
    }

    #[test]
    fn adjacent_lines_merge_into_one_tagged_paragraph() {
        // `_naive_vertical_merge` joins two lines of the same column when the
        // gap is under 1.5 mean line heights, so one box carries one tag.
        let doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (a) Tj 0 -20 Td (b) Tj ET");
        let tagged = tagged_content(&extract_positioned_lines(&doc));
        assert_eq!(tagged.lines().count(), 1, "{tagged}");
        assert_eq!(tagged, "a b@@1\t72.0\t78.0\t83.0\t115.0##");
    }

    #[test]
    fn distant_lines_keep_their_own_tags() {
        // 200 pt apart: the vertical gap breaks the paragraph, and `_line_tag`
        // emits one tag per box exactly like upstream.
        let doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (one) Tj 0 -200 Td (two) Tj ET");
        let tagged = tagged_content(&extract_positioned_lines(&doc));
        let lines: Vec<&str> = tagged.lines().collect();
        assert_eq!(lines.len(), 2, "{tagged}");
        assert_eq!(lines[0], "one@@1\t72.0\t90.0\t83.0\t95.0##");
        assert_eq!(lines[1], "two@@1\t72.0\t90.0\t283.0\t295.0##");
    }

    #[test]
    fn multi_page_documents_keep_page_local_coordinates() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (one) Tj ET");
        let pages_id = doc
            .get_pages()
            .values()
            .next()
            .map(|id| {
                doc.get_dictionary(*id)
                    .unwrap()
                    .get(b"Parent")
                    .unwrap()
                    .as_reference()
                    .unwrap()
            })
            .unwrap();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "FirstChar" => 32,
            "LastChar" => 126,
            "Widths" => (32..=126)
                .map(|_code| Object::Real(500.0))
                .collect::<Vec<Object>>(),
            "Encoding" => "WinAnsiEncoding",
        });
        let content_id = doc.add_object(lopdf::Stream::new(
            dictionary! {},
            b"BT /F1 12 Tf 72 700 Td (two) Tj ET".to_vec(),
        ));
        let second_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
        });
        let kids = doc
            .get_dictionary(pages_id)
            .unwrap()
            .get(b"Kids")
            .unwrap()
            .as_array()
            .unwrap()
            .clone();
        let mut updated = kids.clone();
        updated.push(second_id.into());
        doc.get_dictionary_mut(pages_id)
            .unwrap()
            .set("Kids", updated);
        doc.get_dictionary_mut(pages_id).unwrap().set("Count", 2);

        let lines = extract_positioned_lines(&doc);
        let pages: Vec<usize> = lines.iter().map(|line| line.page).collect();
        assert_eq!(pages, [1, 2], "{lines:?}");
        assert_eq!(lines[0].text, "one");
        assert_eq!(lines[1].text, "two");
        // Both pages report their own coordinates.
        assert!((lines[0].top - lines[1].top).abs() < 0.01, "{lines:?}");
    }

    #[test]
    fn to_unicode_bfrange_expands_sequences() {
        let map =
            parse_to_unicode(b"1 beginbfrange\n<0020> <0022> <0041>\nendbfrange\n", 4096).unwrap();
        assert_eq!(map.get(&0x20).map(String::as_str), Some("A"));
        assert_eq!(map.get(&0x21).map(String::as_str), Some("B"));
        assert_eq!(map.get(&0x22).map(String::as_str), Some("C"));
    }

    #[test]
    fn to_unicode_bfrange_accepts_an_array_target() {
        let map = parse_to_unicode(
            b"1 beginbfrange\n<0010> <0012> [<0041> <0042> <0043>]\nendbfrange\n",
            4096,
        )
        .unwrap();
        assert_eq!(map.get(&0x10).map(String::as_str), Some("A"));
        assert_eq!(map.get(&0x12).map(String::as_str), Some("C"));
    }

    #[test]
    fn cmap_streaming_supports_inline_multiline_comments_and_hex_whitespace() {
        let content = b"/CIDSystemInfo << /Registry (beginbfchar) /Ordering (UCS) >> def
% beginbfchar
1 beginbfchar <01> <4e2d> endbfchar
1 beginbfrange <02> <04> [
<00 41> % endbfrange
<0042>
<0043> ] endbfrange
1 beginbfchar <1> <004> endbfchar";
        let map = parse_to_unicode(content, 4096).unwrap();
        assert_eq!(map[&1], "中");
        assert_eq!(map[&2], "A");
        assert_eq!(map[&4], "C");
        assert_eq!(map[&0x10], "@");
    }

    #[test]
    fn cmap_pdf_null_whitespace_and_adjacent_names_are_accepted() {
        let map = parse_to_unicode(
            b"1\0beginbfchar <01> <00\0 41> endbfchar/CMapName /Example def",
            4096,
        )
        .unwrap();
        assert_eq!(map[&1], "A");
        let error = parse_to_unicode(
            b"2 begincodespacerange <00> <7f> <8100> <81ff> endcodespacerange",
            4096,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("mixed-width"), "{error}");
        assert!(
            parse_to_unicode(b"/Adobe-Japan1-UCS2 usecmap", 4096)
                .unwrap_err()
                .to_string()
                .contains("usecmap")
        );
    }

    #[test]
    fn cmap_literal_strings_and_cr_comments_preserve_byte_semantics() {
        let map = parse_to_unicode(br"2 beginbfchar (A) (\000A) (B) (\000B) endbfchar 1 beginbfrange (C) (E) [(\000C) <0044> (\000E)] endbfrange", 4096).unwrap();
        assert_eq!(map[&65], "A");
        assert_eq!(map[&66], "B");
        assert_eq!(map[&67], "C");
        assert_eq!(map[&69], "E");
        let map =
            parse_to_unicode(b"% beginbfchar\r1 beginbfchar <01> <0041> endbfchar", 4096).unwrap();
        assert_eq!(map[&1], "A");
    }

    #[test]
    fn cmap_ranges_increment_big_endian_strings_and_decode_surrogate_pairs() {
        let map = parse_to_unicode(b"1 beginbfrange <01> <02> <0041ffff> endbfrange 1 beginbfrange <03> <05> <d83dde00> endbfrange", 4096).unwrap();
        assert_eq!(map[&1], "A\u{ffff}");
        assert_eq!(map[&2], "B\0");
        assert_eq!(map[&3], "😀");
        assert_eq!(map[&5], "😂");
    }

    #[test]
    fn invalid_cmap_ranges_fail_instead_of_wrapping_or_returning_partial_maps() {
        for content in [
            "1 beginbfchar <010001> <0041> endbfchar",
            "1 beginbfrange <01> <0002> <0041> endbfrange",
            "1 beginbfrange <ff> <01> <0041> endbfrange",
            "1 beginbfrange <01> <02> [<0041>] endbfrange",
            "1 beginbfrange <ffff> <ffff> [<0041> <0042>] endbfrange",
            "1 beginbfrange <01> <02> <ffff> endbfrange",
            "1 beginbfchar <01> <d800> endbfchar",
            "1 beginbfchar <01> <004g> endbfchar",
            "1 beginbfchar <01> <0041",
            "1 beginbfchar <01> <0041>",
        ] {
            let error = parse_to_unicode(content.as_bytes(), 4096)
                .unwrap_err()
                .to_string();
            assert!(error.contains("/ToUnicode"), "{content}: {error}");
        }
    }

    #[test]
    fn cmap_expansion_budget_counts_text_entries_and_overwrites() {
        let single = b"1 beginbfchar <01> <0041> endbfchar";
        let cost = 64 + cmap_unicode(&[0, 65]).unwrap().capacity() as u64;
        assert_eq!(parse_to_unicode(single, cost).unwrap()[&1], "A");
        assert!(
            parse_to_unicode(single, cost - 1)
                .unwrap_err()
                .to_string()
                .contains("budget")
        );
        let repeated = b"2 beginbfchar <01> <0041> <01> <0042> endbfchar";
        assert!(
            parse_to_unicode(repeated, cost)
                .unwrap_err()
                .to_string()
                .contains("budget")
        );
        let expanded = format!(
            "1 beginbfrange <0000> <ffff> <{}> endbfrange",
            "0041".repeat(1024)
        );
        let error = parse_to_unicode(expanded.as_bytes(), 1024)
            .unwrap_err()
            .to_string();
        assert!(error.contains("RAYRAG_PDF_STREAM_LIMIT_BYTES"), "{error}");
    }

    #[test]
    fn cmap_default_budget_stops_large_range_expansion() {
        let expanded = format!(
            "1 beginbfrange <0000> <ffff> <{}> endbfrange",
            "0041".repeat(2048)
        );
        let error = parse_to_unicode(
            expanded.as_bytes(),
            super::super::pdf_stream::DEFAULT_PDF_STREAM_BYTES,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("budget"), "{error}");
    }

    #[test]
    fn inline_and_multiline_cmaps_reach_normal_pdf_parser_and_chunks() {
        let mut doc = single_page_pdf("BT /F1 12 Tf 72 700 Td (ABCD) Tj ET");
        let cmap = doc.add_object(lopdf::Stream::new(dictionary! {}, b"2 beginbfchar <41> <4e2d> <42> <6587> endbfchar 1 beginbfrange <43> <44> [\n<0020>\n<6d4b8bd5>\n] endbfrange".to_vec()));
        let page_id = doc.get_pages()[&1];
        let font_id = doc
            .get_dictionary(page_id)
            .unwrap()
            .get(b"Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .get(b"F1")
            .unwrap()
            .as_reference()
            .unwrap();
        doc.get_dictionary_mut(font_id)
            .unwrap()
            .set("ToUnicode", cmap);
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        let parsed =
            crate::parser::Parse::parse(&crate::parser::pdf::PdfParser::new(), "cmap.pdf", &bytes)
                .unwrap();
        assert!(parsed.content.contains("中文 测试"), "{}", parsed.content);
        let config = crate::ParserConfig::default();
        let chunks = crate::chunk::chunker_for(&config)
            .chunk(&parsed, &config)
            .unwrap();
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk.content.contains("中文 测试"))
        );
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.metadata.contains_key("position_int"))
        );
    }

    #[test]
    fn win_ansi_high_range_is_not_latin1() {
        assert_eq!(win_ansi_char(0x92), '’');
        assert_eq!(win_ansi_char(0x07), ' ');
        assert_eq!(win_ansi_char(0xe9), 'é');
    }

    #[test]
    fn cmap_tokens_read_hex_and_array_forms() {
        let mut reader = CMapReader::new(b"<0010> <0012> [<0041> <0042>]");
        for expected in [
            CMapToken::Hex(b"0010"),
            CMapToken::Hex(b"0012"),
            CMapToken::ArrayStart,
            CMapToken::Hex(b"0041"),
            CMapToken::Hex(b"0042"),
            CMapToken::ArrayEnd,
        ] {
            assert_eq!(reader.next().unwrap(), Some(expected));
        }
        assert_eq!(reader.next().unwrap(), None);
    }
}
