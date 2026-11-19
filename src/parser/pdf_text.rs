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
            remaining_bytes: super::pdf_stream::stream_limit_bytes(),
            remaining_operations: 1_000_000,
        };
        interpreter.walk(&content, resources, &mut state, &mut runs)
            .map_err(|error| anyhow::anyhow!(
                "PDF page {number} text extraction failed: {error}. Repair or export an unencrypted PDF with embedded fonts and upload it again, or select a configured OCR document parser and reparse. PDF 页面提取失败，请修复并嵌入字体重新导出上传，或配置 OCR 解析客户端后重新解析。"
            ))?;
        lines.extend(geometry.lines(number as usize, runs));
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

struct ContentInterpreter<'a> {
    doc: &'a Document,
    page_resources: Option<&'a Dictionary>,
    forms: Vec<lopdf::ObjectId>,
    depth: usize,
    remaining_bytes: u64,
    remaining_operations: usize,
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
                    self.doc, resources, &mut fonts, &mut stack, state, runs, operation,
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
                    state.text.font = Some(std::sync::Arc::new(font_metrics(doc, font).map_err(
                        |error| {
                            anyhow::anyhow!(
                                "ExtGState {} font: {error}",
                                String::from_utf8_lossy(name)
                            )
                        },
                    )?));
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
            let font = resource_font(doc, resources, fonts, name)?;
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
                show_text(state, runs, bytes)?;
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
                show_text(state, runs, bytes)?;
            }
        }
        "TJ" => {
            if let Some(Object::Array(items)) = operands.first() {
                for item in items {
                    match item {
                        Object::String(bytes, _) => show_text(state, runs, bytes)?,
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

fn show_text(state: &mut State, runs: &mut Vec<Run>, bytes: &[u8]) -> crate::Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let font = state.text.font.as_ref().ok_or_else(|| {
        anyhow::anyhow!("PDF text operation has no selected font (Tf or ExtGState Font)")
    })?;
    let size = state.text.size;
    let codes = font.codes(bytes);
    if codes.is_empty() {
        return Ok(());
    }
    let text = font.decode(&codes);
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
    fn codes(&self, bytes: &[u8]) -> Vec<u16> {
        if self.two_byte {
            bytes
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .collect()
        } else {
            bytes.iter().map(|byte| u16::from(*byte)).collect()
        }
    }

    fn decode(&self, codes: &[u16]) -> String {
        if !self.to_unicode.is_empty() {
            return codes
                .iter()
                .map(|code| {
                    self.to_unicode
                        .get(code)
                        .cloned()
                        .unwrap_or_else(|| fallback_char(*code, self.two_byte))
                })
                .collect();
        }
        if self.two_byte {
            return String::from_utf16_lossy(codes);
        }
        codes
            .iter()
            .map(|code| win_ansi_char(*code as u8))
            .collect::<String>()
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
        font_metrics(doc, font)
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

fn font_metrics(doc: &Document, font: &Dictionary) -> crate::Result<FontMetrics> {
    let subtype = font
        .get(b"Subtype")
        .and_then(Object::as_name_str)
        .unwrap_or_default();
    let two_byte = subtype == "Type0";
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
            let content = super::pdf_stream::plain_content_limited(
                stream,
                super::pdf_stream::stream_limit_bytes(),
                "PDF /ToUnicode CMap",
            )?;
            metrics.to_unicode = parse_to_unicode(&content);
        }
    }

    if two_byte {
        if let Some(descendant) = font
            .get(b"DescendantFonts")
            .ok()
            .and_then(|object| deref(doc, object))
            .and_then(|object| object.as_array().ok())
            .and_then(|fonts| fonts.first())
            .and_then(|first| as_dict(doc, first))
        {
            metrics.default_width = descendant
                .get(b"DW")
                .ok()
                .and_then(as_number)
                .unwrap_or(1000.0);
            if let Ok(widths) = descendant.get(b"W").and_then(Object::as_array) {
                parse_cid_widths(widths, &mut metrics.widths);
            }
            if let Some(descriptor) = dict_entry(doc, descendant, b"FontDescriptor") {
                apply_descriptor(descriptor, &mut metrics);
            }
        }
    } else {
        let first_char = font
            .get(b"FirstChar")
            .ok()
            .and_then(as_number)
            .unwrap_or(0.0) as u16;
        if let Ok(widths) = font.get(b"Widths").and_then(Object::as_array) {
            for (index, width) in widths.iter().enumerate() {
                if let Some(value) = as_number(width) {
                    metrics
                        .widths
                        .insert(first_char.wrapping_add(index as u16), value);
                }
            }
        }
        if let Some(descriptor) = dict_entry(doc, font, b"FontDescriptor") {
            apply_descriptor(descriptor, &mut metrics);
        }
    }
    Ok(metrics)
}

fn apply_descriptor(descriptor: &Dictionary, metrics: &mut FontMetrics) {
    if let Some(missing) = descriptor.get(b"MissingWidth").ok().and_then(as_number)
        && metrics.default_width == 500.0
    {
        metrics.default_width = missing;
    }
    if let Some(ascent) = descriptor.get(b"Ascent").ok().and_then(as_number) {
        metrics.ascent = (ascent / 1000.0).clamp(0.5, 1.2);
    }
    if let Some(descent) = descriptor.get(b"Descent").ok().and_then(as_number) {
        metrics.descent = (descent / 1000.0).clamp(-0.6, 0.0);
    }
}

/// `/W [c [w…] cfirst clast w …]` — widths per CID, in 1000-unit text space.
fn parse_cid_widths(widths: &[Object], out: &mut HashMap<u16, f64>) {
    let mut index = 0;
    while index < widths.len() {
        let Some(start) = as_number(&widths[index]) else {
            index += 1;
            continue;
        };
        let start = start.max(0.0) as u16;
        match widths.get(index + 1) {
            Some(Object::Array(list)) => {
                for (offset, width) in list.iter().enumerate() {
                    if let Some(value) = as_number(width) {
                        out.insert(start.wrapping_add(offset as u16), value);
                    }
                }
                index += 2;
            }
            Some(end) => {
                let Some(end) = as_number(end) else {
                    index += 1;
                    continue;
                };
                let Some(width) = widths.get(index + 2).and_then(as_number) else {
                    index += 1;
                    continue;
                };
                let end = (end.max(0.0) as u16).max(start);
                for code in start..=end {
                    out.insert(code, width);
                }
                index += 3;
            }
            None => break,
        }
    }
}

/// One token of a `/ToUnicode` CMap program.
#[derive(Debug, PartialEq)]
enum CMapToken {
    /// `<0041>`
    Hex(String),
    /// `[<0041> <0042>]`
    Array(Vec<String>),
    /// `beginbfchar` / `endbfrange` / …
    Word(String),
}

fn cmap_tokens(line: &str) -> Vec<CMapToken> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'<' => {
                if let Some(end) = line[index..].find('>') {
                    tokens.push(CMapToken::Hex(line[index + 1..index + end].to_owned()));
                    index += end + 1;
                } else {
                    break;
                }
            }
            b'[' => {
                let end = line[index..].find(']').map(|offset| index + offset);
                let Some(end) = end else { break };
                let inner: Vec<String> = cmap_tokens(&line[index + 1..end])
                    .into_iter()
                    .filter_map(|token| match token {
                        CMapToken::Hex(value) => Some(value),
                        _ => None,
                    })
                    .collect();
                tokens.push(CMapToken::Array(inner));
                index = end + 1;
            }
            _ => {
                let start = index;
                while index < bytes.len()
                    && !bytes[index].is_ascii_whitespace()
                    && bytes[index] != b'<'
                    && bytes[index] != b'['
                {
                    index += 1;
                }
                if index > start {
                    tokens.push(CMapToken::Word(line[start..index].to_owned()));
                } else {
                    index += 1;
                }
            }
        }
    }
    tokens
}

/// Minimal `/ToUnicode` CMap reader: `beginbfchar` and `beginbfrange`
/// sections mapping codes to UTF-16BE strings.
fn parse_to_unicode(content: &[u8]) -> HashMap<u16, String> {
    let mut map = HashMap::new();
    let text = String::from_utf8_lossy(content);
    let mut section = "";
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.ends_with("beginbfchar") {
            section = "bfchar";
            continue;
        }
        if trimmed.ends_with("beginbfrange") {
            section = "bfrange";
            continue;
        }
        if trimmed == "endbfchar" || trimmed == "endbfrange" || trimmed == "endcmap" {
            section = "";
            continue;
        }
        let tokens = cmap_tokens(trimmed);
        match section {
            "bfchar" => {
                let hexes: Vec<&String> = tokens
                    .iter()
                    .filter_map(|token| match token {
                        CMapToken::Hex(value) => Some(value),
                        _ => None,
                    })
                    .collect();
                for pair in hexes.chunks(2) {
                    if let [code, target] = pair
                        && let (Some(code), Some(text)) = (to_u16(code), utf16_string(target))
                    {
                        map.insert(code, text);
                    }
                }
            }
            "bfrange" => {
                let mut index = 0;
                while index + 2 < tokens.len() {
                    let (start, end) = match (&tokens[index], &tokens[index + 1]) {
                        (CMapToken::Hex(start), CMapToken::Hex(end)) => (start, end),
                        _ => {
                            index += 1;
                            continue;
                        }
                    };
                    let target = &tokens[index + 2];
                    index += 3;
                    let (Some(start), Some(end)) = (to_u16(start), to_u16(end)) else {
                        continue;
                    };
                    let end = end.max(start);
                    match target {
                        CMapToken::Array(values) => {
                            for (offset, value) in values.iter().enumerate() {
                                if let Some(text) = utf16_string(value) {
                                    map.insert(start.wrapping_add(offset as u16), text);
                                }
                            }
                        }
                        CMapToken::Hex(value) => {
                            let Some(mut points) = utf16_codepoints(value) else {
                                continue;
                            };
                            if points.is_empty() {
                                continue;
                            }
                            for code in start..=end {
                                map.insert(code, encode_utf16(&points));
                                let last = points.len() - 1;
                                points[last] += 1;
                            }
                        }
                        CMapToken::Word(_) => {}
                    }
                }
            }
            _ => {}
        }
    }
    map
}

fn to_u16(value: &str) -> Option<u16> {
    u32::from_str_radix(value.trim(), 16)
        .ok()
        .map(|value| value as u16)
}

fn utf16_codepoints(value: &str) -> Option<Vec<u32>> {
    Some(
        hex_bytes(value)?
            .chunks_exact(2)
            .map(|pair| u32::from(u16::from_be_bytes([pair[0], pair[1]])))
            .collect(),
    )
}

fn utf16_string(value: &str) -> Option<String> {
    Some(encode_utf16(&utf16_codepoints(value)?))
}

fn encode_utf16(points: &[u32]) -> String {
    let units: Vec<u16> = points.iter().map(|value| *value as u16).collect();
    String::from_utf16_lossy(&units)
}

fn hex_bytes(value: &str) -> Option<Vec<u8>> {
    let trimmed = value.trim();
    if !trimmed.len().is_multiple_of(2) {
        return None;
    }
    (0..trimmed.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&trimmed[index..index + 2], 16).ok())
        .collect()
}


