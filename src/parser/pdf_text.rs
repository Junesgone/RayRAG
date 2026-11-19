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
            lopdf::content::Content::decode(&bytes)
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
        let bytes = super::pdf_stream::plain_content_limited(
            stream,
            self.remaining_bytes,
            "PDF Form content",
        )?;
        self.remaining_bytes -= bytes.len() as u64;
        let content = lopdf::content::Content::decode(&bytes)?;
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
    if text.is_empty() {
        return Ok(());
    }
    let advance: f64 = codes
        .iter()
        .map(|code| font.advance(*code, size, &state.text))
        .sum();

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
                if let Some(mapped) = self.to_unicode.get(code) {
                    append_pdf_text(&mut text, mapped, remaining)?;
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
            let content =
                super::pdf_stream::plain_content_limited(stream, limit, "PDF /ToUnicode CMap")?;
            metrics.to_unicode =
                parse_to_unicode_with_budget(&content, &mut budget.font_mapping_bytes)?;
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
    budget.font_mapping_bytes = budget.font_mapping_bytes.checked_sub(256).ok_or_else(|| {
        anyhow::anyhow!(
            "PDF selected font tables exceed the RAYRAG_PDF_STREAM_LIMIT_BYTES page budget"
        )
    })?;
    Ok(metrics)
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

/// Bound expanded mapping text and per-entry work, including overwritten entries.
fn insert_cmap_mapping(
    map: &mut HashMap<u16, String>,
    code: u16,
    text: String,
    remaining: &mut u64,
) -> crate::Result<()> {
    let cost = (text.capacity() as u64).saturating_add(64);
    *remaining = remaining.checked_sub(cost).ok_or_else(|| {
        anyhow::anyhow!(
            "PDF /ToUnicode CMap expansion exceeds the RAYRAG_PDF_STREAM_LIMIT_BYTES budget"
        )
    })?;
    map.insert(code, text);
    Ok(())
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


