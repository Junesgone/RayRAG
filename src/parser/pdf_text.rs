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
        .map(|page| median_of(&mut boxes, page, |b| b.height()))
        .collect();
    let mean_width: Vec<f64> = (1..=page_count)
        .map(|page| median_of(&mut boxes, page, |b| b.char_width()))
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

/// Extract every text line of `doc`, page by page, in reading order.
pub fn extract_positioned_lines(doc: &Document) -> Vec<PositionedLine> {
    let mut lines = Vec::new();
    let mut pages: Vec<(u32, lopdf::ObjectId)> = doc
        .get_pages()
        .iter()
        .map(|(number, id)| (*number, *id))
        .collect();
    pages.sort_by_key(|(number, _id)| *number);
    for (number, page_id) in pages {
        let Some(geometry) = PageGeometry::of(doc, page_id) else {
            continue;
        };
        let fonts = page_fonts(doc, page_id);
        let Ok(content) = doc.get_and_decode_page_content(page_id) else {
            continue;
        };
        let mut runs = Vec::new();
        let mut state = State::default();
        let mut stack = Vec::new();
        for operation in &content.operations {
            apply_operation(doc, &fonts, &mut stack, &mut state, &mut runs, operation);
        }
        lines.extend(geometry.lines(number as usize, runs));
    }
    lines
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

    /// `self × other`: the transform `other` is applied first, then `self`.
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
    /// Page height in points, for the cumulative heights `_line_tag` needs.
    height: f64,
}

impl PageGeometry {
    fn of(doc: &Document, page_id: lopdf::ObjectId) -> Option<Self> {
        let rect =
            inherited(doc, page_id, b"CropBox").or_else(|| inherited(doc, page_id, b"MediaBox"))?;
        let numbers = rect.as_array().ok()?;
        if numbers.len() != 4 {
            return None;
        }
        let value = |index: usize| numbers[index].as_float().ok().map(f64::from);
        let bottom_edge = value(1)?;
        let top_edge = value(3).or(Some(bottom_edge))?;
        Some(PageGeometry {
            origin_x: value(0)?,
            top_edge,
            height: (top_edge - bottom_edge).max(0.0),
        })
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
            if !same_line && !current.is_empty() {
                if let Some(line) = self.merge_runs(page, std::mem::take(&mut current)) {
                    lines.push(line);
                }
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
    font: String,
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

fn apply_operation(
    doc: &Document,
    fonts: &HashMap<String, FontMetrics>,
    stack: &mut Vec<State>,
    state: &mut State,
    runs: &mut Vec<Run>,
    operation: &lopdf::content::Operation,
) {
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
        "Tf" => {
            if let Ok(name) = operands[0.min(operands.len().saturating_sub(1))]
                .clone()
                .as_name_str()
                .map(str::to_owned)
                && !operands.is_empty()
            {
                state.text.font = name;
            }
            if let Some(size) = operand_number(operands, 1) {
                state.text.size = size;
            }
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
                show_text(doc, fonts, state, runs, bytes);
            }
        }
        "'" | "\"" => {
            if operation.operator.as_str() == "\"" {
                if let Some(word_spacing) = operand_number(operands, 0) {
                    state.text.word_spacing = word_spacing;
                }
                if let Some(char_spacing) = operand_number(operands, 1) {
                    state.text.char_spacing = char_spacing;
                }
            }
            let leading = state.text.leading;
            state.tlm = Matrix::translate(0.0, -leading).mul(state.tlm);
            state.tm = state.tlm;
            if let Some(Object::String(bytes, _)) = operands.get(2) {
                show_text(doc, fonts, state, runs, bytes);
            }
        }
        "TJ" => {
            if let Some(Object::Array(items)) = operands.first() {
                for item in items {
                    match item {
                        Object::String(bytes, _) => show_text(doc, fonts, state, runs, bytes),
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
}

fn show_text(
    doc: &Document,
    fonts: &HashMap<String, FontMetrics>,
    state: &mut State,
    runs: &mut Vec<Run>,
    bytes: &[u8],
) {
    let Some(font) = fonts.get(&state.text.font) else {
        return;
    };
    let size = state.text.size;
    let codes = font.codes(bytes);
    if codes.is_empty() {
        return;
    }
    let text = font.decode(&codes);
    if text.is_empty() {
        return;
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

fn page_fonts(doc: &Document, page_id: lopdf::ObjectId) -> HashMap<String, FontMetrics> {
    let mut fonts = HashMap::new();
    let Some(resources) =
        inherited(doc, page_id, b"Resources").and_then(|value| as_dict(doc, value))
    else {
        return fonts;
    };
    let Some(font_dict) = dict_entry(doc, resources, b"Font") else {
        return fonts;
    };
    for (name, value) in font_dict.iter() {
        let Some(font) = as_dict(doc, value) else {
            continue;
        };
        fonts.insert(
            String::from_utf8_lossy(name).into_owned(),
            font_metrics(doc, font),
        );
    }
    fonts
}

/// Resolve an inheritable page attribute through the page tree.
fn inherited<'a>(doc: &'a Document, page_id: lopdf::ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut current = page_id;
    for _ in 0..32 {
        let page = doc.get_dictionary(current).ok()?;
        if let Ok(value) = page.get(key) {
            return deref(doc, value);
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

fn font_metrics(doc: &Document, font: &Dictionary) -> FontMetrics {
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
    if let Some(content) = font
        .get(b"ToUnicode")
        .ok()
        .and_then(|object| deref(doc, object))
        .and_then(|object| object.as_stream().ok())
        .and_then(|stream| stream.get_plain_content().ok())
    {
        metrics.to_unicode = parse_to_unicode(&content);
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
    metrics
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
        let map = parse_to_unicode(b"1 beginbfrange\n<0020> <0022> <0041>\nendbfrange\n");
        assert_eq!(map.get(&0x20).map(String::as_str), Some("A"));
        assert_eq!(map.get(&0x21).map(String::as_str), Some("B"));
        assert_eq!(map.get(&0x22).map(String::as_str), Some("C"));
    }

    #[test]
    fn to_unicode_bfrange_accepts_an_array_target() {
        let map =
            parse_to_unicode(b"1 beginbfrange\n<0010> <0012> [<0041> <0042> <0043>]\nendbfrange\n");
        assert_eq!(map.get(&0x10).map(String::as_str), Some("A"));
        assert_eq!(map.get(&0x12).map(String::as_str), Some("C"));
    }

    #[test]
    fn win_ansi_high_range_is_not_latin1() {
        assert_eq!(win_ansi_char(0x92), '’');
        assert_eq!(win_ansi_char(0x07), ' ');
        assert_eq!(win_ansi_char(0xe9), 'é');
    }

    #[test]
    fn cmap_tokens_read_hex_and_array_forms() {
        assert_eq!(
            cmap_tokens("<0003> <0041>"),
            [CMapToken::Hex("0003".into()), CMapToken::Hex("0041".into())]
        );
        assert_eq!(
            cmap_tokens("<0010> <0012> [<0041> <0042>]"),
            [
                CMapToken::Hex("0010".into()),
                CMapToken::Hex("0012".into()),
                CMapToken::Array(vec!["0041".into(), "0042".into()])
            ]
        );
    }
}
