//! PDF layout box algorithms — pure-Rust port of the deterministic parts of
//! RAGFlow `deepdoc/parser/pdf_parser.py` (RAGFlowPdfParser).
//!
//! This module covers the model-free pipeline stages that operate on
//! character/box coordinates:
//! - garbled-text detection (`_is_garbled_char/_is_garbled_text`,
//!   `_has_subset_font_prefix`, `_is_garbled_by_font_encoding`)
//! - projection matches (`_match_proj`, `proj_match`)
//! - geometry helpers (`_x_dis`, `_y_dis`, `__char_width`, `__height`)
//! - `sort_X_by_page`, `_text_merge`, `_naive_vertical_merge`,
//!   `_concat_downward`, `_filter_forpages`, `_merge_with_same_bullet`,
//!   `_line_tag`, `__filterout_scraps`
//!
//! Model-dependent stages (layout recognition, table transformer, OCR
//! preprocess) stay out of scope — callers supply `layout_type` etc.

use std::collections::HashMap;

/// A text box on a PDF page (mirrors the Python dict shape used by
/// pdf_parser.py: `top/bottom/x0/x1`, `text`, `page_number`, optional
/// `layout_type`/`layoutno`/`col_id`).
#[derive(Debug, Clone, Default)]
pub struct PdfBox {
    pub top: f64,
    pub bottom: f64,
    pub x0: f64,
    pub x1: f64,
    pub text: String,
    pub page_number: usize, // 1-based
    pub layout_type: String,
    pub layoutno: String,
    pub col_id: Option<u32>,
}

impl PdfBox {
    pub fn height(&self) -> f64 {
        self.bottom - self.top
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    pub fn char_width(&self) -> f64 {
        let l = self.text.chars().count().max(1) as f64;
        (self.x1 - self.x0) / l
    }
}

/// CID placeholder pattern `(cid:123)` used by pdfminer for unmapped glyphs.
pub const CID_PATTERN: &str = r"\(cid\s*:\s*\d+\s*\)";

/// `_is_garbled_char` — pdf_parser.py:200-226.
pub fn is_garbled_char(ch: Option<char>) -> bool {
    let Some(ch) = ch else { return false };
    let cp = ch as u32;
    if (0xE000..=0xF8FF).contains(&cp) {
        return true;
    }
    if (0xF0000..=0xFFFFF).contains(&cp) {
        return true;
    }
    if (0x100000..=0x10FFFF).contains(&cp) {
        return true;
    }
    if cp == 0xFFFD {
        return true;
    }
    if cp < 0x20 && ch != '\t' && ch != '\n' && ch != '\r' {
        return true;
    }
    if (0x80..=0x9F).contains(&cp) {
        return true;
    }
    // Unicode categories Cn (unassigned) / Cs (surrogate).
    if is_unassigned_or_surrogate(ch) {
        return true;
    }
    false
}

fn is_unassigned_or_surrogate(ch: char) -> bool {
    // Surrogates cannot appear in a valid Rust char; approximate Cn via
    // private/noncharacter ranges already covered above. Rust chars are
    // always valid scalar values, so Cs is unreachable.
    matches!(ch, '\u{FFFF}')
}

/// `_is_garbled_text` — pdf_parser.py:228-250.
pub fn is_garbled_text(text: &str, threshold: f64) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    if regex::Regex::new(CID_PATTERN)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
    {
        return true;
    }
    let mut garbled = 0usize;
    let mut total = 0usize;
    for ch in text.chars() {
        if ch.is_whitespace() {
            continue;
        }
        total += 1;
        if is_garbled_char(Some(ch)) {
            garbled += 1;
        }
    }
    if total == 0 {
        return false;
    }
    (garbled as f64 / total as f64) >= threshold
}

/// `_has_subset_font_prefix` — pdf_parser.py:252-261.
pub fn has_subset_font_prefix(fontname: &str) -> bool {
    if fontname.is_empty() {
        return false;
    }
    regex::Regex::new(r"^[A-Z0-9]{2,6}\+")
        .map(|re| re.is_match(fontname))
        .unwrap_or(false)
}

/// `_is_garbled_by_font_encoding` — pdf_parser.py:263-316.
pub fn is_garbled_by_font_encoding(page_chars: &[GarbledChar], min_chars: usize) -> bool {
    if page_chars.is_empty() || page_chars.len() < min_chars {
        return false;
    }
    let mut subset_font_count = 0usize;
    let mut total_non_space = 0usize;
    let mut ascii_punct_sym = 0usize;
    let mut cjk_like = 0usize;

    for c in page_chars {
        let text = c.text.trim();
        if text.is_empty() {
            continue;
        }
        total_non_space += 1;
        if has_subset_font_prefix(&c.fontname) {
            subset_font_count += 1;
        }
        let Some(first) = text.chars().next() else {
            continue;
        };
        let cp = first as u32;
        if (0x2E80..=0x9FFF).contains(&cp)
            || (0xF900..=0xFAFF).contains(&cp)
            || (0x20000..=0x2FA1F).contains(&cp)
            || (0xAC00..=0xD7AF).contains(&cp)
            || (0x3040..=0x30FF).contains(&cp)
        {
            cjk_like += 1;
        } else if (0x21..=0x2F).contains(&cp)
            || (0x3A..=0x40).contains(&cp)
            || (0x5B..=0x60).contains(&cp)
            || (0x7B..=0x7E).contains(&cp)
        {
            ascii_punct_sym += 1;
        }
    }

    if total_non_space < min_chars {
        return false;
    }
    let subset_ratio = subset_font_count as f64 / total_non_space as f64;
    if subset_ratio < 0.3 {
        return false;
    }
    let cjk_ratio = cjk_like as f64 / total_non_space as f64;
    let punct_ratio = ascii_punct_sym as f64 / total_non_space as f64;
    cjk_ratio < 0.05 && punct_ratio > 0.4
}

/// Input record for `is_garbled_by_font_encoding`.
#[derive(Debug, Clone)]
pub struct GarbledChar {
    pub text: String,
    pub fontname: String,
}

/// `_match_proj` — pdf_parser.py:119-130.
pub fn match_proj(text: &str) -> bool {
    let patterns = [
        r"第[零一二三四五六七八九十百]+章",
        r"第[零一二三四五六七八九十百]+[条节]",
        r"[零一二三四五六七八九十百]+[、是 　]",
        r"[\(（][零一二三四五六七八九十百]+[）\)]",
        r"[\(（][0-9]+[）\)]",
        r"[0-9]+(、|\.[　 ]|）|\.[^0-9./a-zA-Z_%><-]{4,})",
        r"[0-9]+\.[0-9.]+(、|\.[ 　])",
        r"[⚫•➢①② ]",
    ];
    // Python re.match anchors at the start; Rust is_match searches anywhere.
    patterns.iter().any(|p| {
        let anchored = format!("^{p}");
        regex::Regex::new(&anchored)
            .map(|re| re.is_match(text))
            .unwrap_or(false)
    })
}

/// `proj_match` — pdf_parser.py:1417-1439. Returns the level index or None.
pub fn proj_match(line: &str) -> Option<u32> {
    if line.chars().count() <= 2 {
        return None;
    }
    if regex::Regex::new(r"[0-9 ().,%%+/-]+$")
        .map(|re| re.is_match(line))
        .unwrap_or(false)
    {
        return None;
    }
    let pairs: &[(&str, u32)] = &[
        (r"第[零一二三四五六七八九十百]+章", 1),
        (r"第[零一二三四五六七八九十百]+[条节]", 2),
        (r"[零一二三四五六七八九十百]+[、 　]", 3),
        (r"[\(（][零一二三四五六七八九十百]+[）\)]", 4),
        (r"[0-9]+(、|\.[　 ]|\.[^0-9])", 5),
        (r"[0-9]+\.[0-9]+(、|[. 　]|[^0-9])", 6),
        (r"[0-9]+\.[0-9]+\.[0-9]+(、|[ 　]|[^0-9])", 7),
        (r"[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+(、|[ 　]|[^0-9])", 8),
        (r".{0,48}[：:?？]$", 9),
        (r"[0-9]+）", 10),
        (r"[\(（][0-9]+[）\)]", 11),
        (r"[零一二三四五六七八九十百]+是", 12),
        (r"[⚫•➢✓]", 12),
    ];
    // Python re.match anchors at the start — Rust is_match searches anywhere,
    // so prefix each pattern with ^ to mirror re.match semantics.
    for (p, j) in pairs {
        let anchored = format!("^{p}");
        if regex::Regex::new(&anchored)
            .map(|re| re.is_match(line))
            .unwrap_or(false)
        {
            return Some(*j);
        }
    }
    None
}

/// `_x_dis` — pdf_parser.py:113-114.
pub fn x_dis(a: &PdfBox, b: &PdfBox) -> f64 {
    let d1 = (a.x1 - b.x0).abs();
    let d2 = (a.x0 - b.x1).abs();
    let d3 = (a.x0 + a.x1 - b.x0 - b.x1).abs() / 2.0;
    d1.min(d2).min(d3)
}

/// `_y_dis` — pdf_parser.py:116-117.
pub fn y_dis(a: &PdfBox, b: &PdfBox) -> f64 {
    (b.top + b.bottom - a.top - a.bottom) / 2.0
}

/// `sort_X_by_page` — pdf_parser.py:177-188. Sort by (page, x0, top), then
/// bubble-adjust adjacent boxes whose x0 differs < threshold but whose top
/// order is inverted.
pub fn sort_x_by_page(arr: &mut [PdfBox], threshold: f64) {
    arr.sort_by(|a, b| {
        a.page_number
            .cmp(&b.page_number)
            .then_with(|| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal))
            .then_with(|| {
                a.top
                    .partial_cmp(&b.top)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    let len = arr.len();
    for i in 0..len.saturating_sub(1) {
        let mut j = i as isize;
        while j >= 0 {
            let jj = j as usize;
            if (arr[jj + 1].x0 - arr[jj].x0).abs() < threshold
                && arr[jj + 1].top < arr[jj].top
                && arr[jj + 1].page_number == arr[jj].page_number
            {
                arr.swap(jj, jj + 1);
                j -= 1;
            } else {
                break;
            }
        }
    }
}

/// `_text_merge` — pdf_parser.py:888-924. Horizontally merge adjacent boxes
/// with the same page, column, layout number and type (not table/figure/
/// equation) when vertically close.
pub fn text_merge(bxs: &mut Vec<PdfBox>, mean_height: &[f64]) {
    let mut i = 0;
    while i + 1 < bxs.len() {
        let same_page = bxs[i].page_number == bxs[i + 1].page_number;
        let same_col = bxs[i].col_id == bxs[i + 1].col_id;
        let same_layoutno = bxs[i].layoutno == bxs[i + 1].layoutno;
        let mergeable_type =
            !matches!(bxs[i].layout_type.as_str(), "table" | "figure" | "equation");
        if !same_page || !same_col || !same_layoutno || !mergeable_type {
            i += 1;
            continue;
        }
        let mh = mean_height
            .get(bxs[i].page_number.saturating_sub(1))
            .copied()
            .unwrap_or(0.0);
        if (y_dis(&bxs[i], &bxs[i + 1])).abs() < mh / 3.0 {
            let b = bxs[i].clone();
            let b_ = bxs[i + 1].clone();
            bxs[i].x1 = b_.x1;
            bxs[i].top = (b.top + b_.top) / 2.0;
            bxs[i].bottom = (b.bottom + b_.bottom) / 2.0;
            bxs[i].text.push_str(&b_.text);
            bxs.remove(i + 1);
            continue;
        }
        i += 1;
    }
}

/// `_naive_vertical_merge` — pdf_parser.py:926-1005. Group by page, sort by
/// (top, x0), merge vertically adjacent boxes with overlap ≥ 0.3 and
/// compatible layout unless split features say otherwise.
pub fn naive_vertical_merge(
    bxs: &mut Vec<PdfBox>,
    mean_height: &[f64],
    mean_width: &[f64],
    is_english: bool,
) {
    // Group by (page, "x") — Python uses a single "x" column group.
    let mut groups: HashMap<usize, Vec<PdfBox>> = HashMap::new();
    let mut order: Vec<usize> = Vec::new();
    for b in bxs.drain(..) {
        if !groups.contains_key(&b.page_number) {
            order.push(b.page_number);
        }
        groups.entry(b.page_number).or_default().push(b);
    }

    let mut merged_boxes: Vec<PdfBox> = Vec::new();
    for pg in order {
        let mut group = groups.remove(&pg).unwrap_or_default();
        group.sort_by(|a, b| {
            a.top
                .partial_cmp(&b.top)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal))
        });
        if group.is_empty() {
            continue;
        }
        let mh = mean_height
            .get(pg.saturating_sub(1))
            .copied()
            .unwrap_or_else(|| {
                group
                    .iter()
                    .map(|b| b.height())
                    .fold(0.0, f64::max)
                    .max(10.0)
            });
        let mw = mean_width.get(pg.saturating_sub(1)).copied().unwrap_or(8.0);

        let mut i = 0;
        while i + 1 < group.len() {
            // Cross-page page-number footer scraplines.
            if group[i].page_number < group[i + 1].page_number
                && regex::Regex::new(r"[0-9  •一—-]+$")
                    .map(|re| re.is_match(&group[i].text))
                    .unwrap_or(false)
            {
                group.remove(i);
                continue;
            }
            if group[i].text.trim().is_empty() {
                group.remove(i);
                continue;
            }
            if group[i].layoutno != group[i + 1].layoutno {
                i += 1;
                continue;
            }
            if group[i + 1].top - group[i].bottom > mh * 1.5 {
                i += 1;
                continue;
            }
            let overlap =
                (group[i].x1.min(group[i + 1].x1) - group[i].x0.max(group[i + 1].x0)).max(0.0);
            let min_w = group[i].width().min(group[i + 1].width()).max(1.0);
            if overlap / min_w < 0.3 {
                i += 1;
                continue;
            }

            let b = group[i].clone();
            let b_ = group[i + 1].clone();
            let b_text = b.text.trim();
            let b_text2 = b_.text.trim();
            let concatting = [
                b_text
                    .chars()
                    .last()
                    .is_some_and(|c| ",;:'\"，、‘“；：-".contains(c)),
                b_text.chars().count() > 1
                    && b_text
                        .chars()
                        .nth(b_text.chars().count() - 2)
                        .is_some_and(|c| ",;:'\"，‘“、；：".contains(c)),
                !b_text2.is_empty()
                    && b_text2
                        .chars()
                        .next()
                        .is_some_and(|c| "。；？！?”）),，、：".contains(c)),
            ];
            let last_char = b_text.chars().last().unwrap_or('\0');
            let feats = [
                b.layoutno != b_.layoutno,
                "。？！?".contains(last_char),
                is_english && ".!?".contains(last_char),
                b.page_number == b_.page_number && b_.top - b.bottom > mh * 1.5,
                b.page_number < b_.page_number && (b.x0 - b_.x0).abs() > mw * 4.0,
            ];
            let detach = [b.x1 < b_.x0, b.x0 > b_.x1];
            if (feats.iter().any(|&x| x) && !concatting.iter().any(|&x| x))
                || detach.iter().any(|&x| x)
            {
                i += 1;
                continue;
            }
            group[i].text = format!("{} {}", b.text.trim_end(), b_.text.trim_start())
                .trim()
                .to_string();
            group[i].bottom = b_.bottom;
            group[i].x0 = b.x0.min(b_.x0);
            group[i].x1 = b.x1.max(b_.x1);
            group.remove(i + 1);
        }
        merged_boxes.extend(group);
    }
    *bxs = merged_boxes;
}

/// `_concat_downward` — pdf_parser.py:1030-1032. Sorts by Y firstly
/// (page, x0, top) — the extended body of the Python method is unreachable
/// dead code (early return).
pub fn concat_downward(bxs: &mut [PdfBox]) {
    sort_y_firstly(bxs, 0.0);
}

/// `Recognizer.sort_Y_firstly` — sort by (page, top, x0).
pub fn sort_y_firstly(bxs: &mut [PdfBox], _threshold: f64) {
    bxs.sort_by(|a, b| {
        a.page_number
            .cmp(&b.page_number)
            .then_with(|| {
                a.top
                    .partial_cmp(&b.top)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.x0.partial_cmp(&b.x0).unwrap_or(std::cmp::Ordering::Equal))
    });
}

/// `_filter_forpages` — pdf_parser.py:1134-1178.
pub fn filter_forpages(bxs: &mut Vec<PdfBox>, page_image_count: usize) {
    if bxs.is_empty() {
        return;
    }
    // First pass: strip contents/acknowledgement sections.
    let mut findit = false;
    let mut i = 0;
    while i < bxs.len() {
        let normalized: String = bxs[i]
            .text
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let re =
            regex::Regex::new(r"(contents|目录|目次|table of contents|致谢|acknowledge)$").unwrap();
        if !re.is_match(&normalized) {
            i += 1;
            continue;
        }
        findit = true;
        let eng = regex::Regex::new(r"[0-9a-zA-Z :'.-]{5,}")
            .unwrap()
            .is_match(bxs[i].text.trim());
        bxs.remove(i);
        if i >= bxs.len() {
            break;
        }
        let mut prefix = if eng {
            bxs[i]
                .text
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            bxs[i].text.trim().chars().take(3).collect()
        };
        while prefix.is_empty() {
            bxs.remove(i);
            if i >= bxs.len() {
                break;
            }
            prefix = if eng {
                bxs[i]
                    .text
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                bxs[i].text.trim().chars().take(3).collect()
            };
        }
        bxs.remove(i);
        if i >= bxs.len() || prefix.is_empty() {
            break;
        }
        let re_prefix = regex::Regex::new(&format!("^{}", regex::escape(&prefix))).unwrap();
        for j in i..(i + 128).min(bxs.len()) {
            if !re_prefix.is_match(&bxs[j].text) {
                continue;
            }
            for _ in i..j {
                bxs.remove(i);
            }
            break;
        }
    }
    if findit {
        return;
    }

    // Second pass: dotted-leader pages.
    let mut page_dirty = vec![0usize; page_image_count];
    for b in bxs.iter() {
        if regex::Regex::new(r"(··|··|··)")
            .map(|re| re.is_match(&b.text))
            .unwrap_or(false)
            && let Some(slot) = page_dirty.get_mut(b.page_number.saturating_sub(1)) {
                *slot += 1;
            }
    }
    let dirty: std::collections::HashSet<usize> = page_dirty
        .iter()
        .enumerate()
        .filter(|(_, t)| **t > 3)
        .map(|(i, _)| i + 1)
        .collect();
    if dirty.is_empty() {
        return;
    }
    bxs.retain(|b| !dirty.contains(&b.page_number));
}

/// `_merge_with_same_bullet` — pdf_parser.py:1180-1204.
pub fn merge_with_same_bullet(bxs: &mut Vec<PdfBox>) {
    let mut i = 0;
    while i + 1 < bxs.len() {
        if bxs[i].text.trim().is_empty() {
            bxs.remove(i);
            continue;
        }
        if bxs[i + 1].text.trim().is_empty() {
            bxs.remove(i + 1);
            continue;
        }
        let b0 = bxs[i].text.trim().chars().next().unwrap_or('\0');
        let b1 = bxs[i + 1].text.trim().chars().next().unwrap_or('\0');
        let lowercase_letters = "qwertyuopasdfghjklzxcvbnm";
        if b0 != b1
            || lowercase_letters.contains(b0.to_ascii_lowercase())
            || is_chinese(b0)
            || bxs[i].top > bxs[i + 1].bottom
        {
            i += 1;
            continue;
        }
        bxs[i + 1].text = format!("{}\n{}", bxs[i].text, bxs[i + 1].text);
        bxs[i + 1].x0 = bxs[i].x0.min(bxs[i + 1].x0);
        bxs[i + 1].x1 = bxs[i].x1.max(bxs[i + 1].x1);
        bxs[i + 1].top = bxs[i].top;
        bxs.remove(i);
    }
}

/// Approximate `rag_tokenizer.is_chinese`.
pub fn is_chinese(c: char) -> bool {
    let cp = c as u32;
    (0x4E00..=0x9FFF).contains(&cp) || (0x3400..=0x4DBF).contains(&cp)
}

/// `_line_tag` — pdf_parser.py:1441-1454. Builds the `@@pages\tx0\tx1\ttop\tbottom##`
/// marker for a box. `page_cum_height` is cumulative page heights (len = pages+1).
pub fn line_tag(
    bx: &PdfBox,
    zoom: f64,
    page_cum_height: &[f64],
    page_image_heights_px: &[f64],
) -> String {
    let mut pns = vec![bx.page_number];
    let top = bx.top
        - page_cum_height
            .get(bx.page_number.saturating_sub(1))
            .copied()
            .unwrap_or(0.0);
    let mut bott = bx.bottom
        - page_cum_height
            .get(bx.page_number.saturating_sub(1))
            .copied()
            .unwrap_or(0.0);
    let page_images_cnt = page_image_heights_px.len();
    if pns[pns.len() - 1].saturating_sub(1) >= page_images_cnt {
        return String::new();
    }
    loop {
        let idx = pns[pns.len() - 1].saturating_sub(1);
        let Some(&ph) = page_image_heights_px.get(idx) else {
            return String::new();
        };
        if bott * zoom <= ph {
            break;
        }
        bott -= ph / zoom;
        pns.push(pns[pns.len() - 1] + 1);
        if pns[pns.len() - 1].saturating_sub(1) >= page_images_cnt {
            return String::new();
        }
    }
    format!(
        "@@{}\t{:.1}\t{:.1}\t{:.1}\t{:.1}##",
        pns.iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("-"),
        bx.x0,
        bx.x1,
        top,
        bott
    )
}

/// `__filterout_scraps` — pdf_parser.py:1456-1514. DFS-concat lines on a page
/// into paragraphs, drop useless scraps, tag kept lines. Returns the joined
/// text (with `\n\n` between paragraphs).
pub fn filterout_scraps(
    boxes: &mut Vec<PdfBox>,
    zoom: f64,
    mean_height: &[f64],
    page_image_widths_px: &[f64],
    page_cum_height: &[f64],
    page_image_heights_px: &[f64],
) -> String {
    fn width(b: &PdfBox) -> f64 {
        b.width()
    }
    fn height(b: &PdfBox) -> f64 {
        b.height()
    }
    fn usefull(b: &PdfBox, zoom: f64, page_image_widths_px: &[f64], mean_height: &[f64]) -> bool {
        if !b.layout_type.is_empty() {
            return true;
        }
        let pw = page_image_widths_px
            .get(b.page_number.saturating_sub(1))
            .copied()
            .unwrap_or(0.0);
        if width(b) > pw / zoom / 3.0 {
            return true;
        }
        let mh = mean_height
            .get(b.page_number.saturating_sub(1))
            .copied()
            .unwrap_or(0.0);
        if b.bottom - b.top > mh {
            return true;
        }
        false
    }

    let mut res: Vec<String> = Vec::new();
    let mut bx_list = std::mem::take(boxes);
    while !bx_list.is_empty() {
        let mut lines: Vec<PdfBox> = Vec::new();
        let mut widths: Vec<f64> = Vec::new();
        let pw = page_image_widths_px
            .get(bx_list[0].page_number.saturating_sub(1))
            .copied()
            .unwrap_or(0.0)
            / zoom;
        let mh = mean_height
            .get(bx_list[0].page_number.saturating_sub(1))
            .copied()
            .unwrap_or(0.0);
        let mj = proj_match(&bx_list[0].text).is_some() || bx_list[0].layout_type == "title";

        // DFS: collect a paragraph of consecutive lines. Python passes the
        // line object (not an index); recursion pops by index after the
        // recursive call returns (only indices > idx are touched inside).
        fn dfs(
            line: &PdfBox,
            st: usize,
            bx_list: &mut Vec<PdfBox>,
            lines: &mut Vec<PdfBox>,
            widths: &mut Vec<f64>,
            mh: f64,
            pw: f64,
            zoom: f64,
            page_image_widths_px: &[f64],
            mean_height: &[f64],
        ) {
            lines.push(line.clone());
            widths.push(width(line));
            let mmj = proj_match(&line.text).is_some() || line.layout_type == "title";
            let n = bx_list.len();
            let mut chosen: Option<usize> = None;
            for i in (st + 1)..(st + 20).min(n) {
                if bx_list[i].page_number as i64 - line.page_number as i64 > 0 {
                    break;
                }
                if !mmj && (y_dis(line, &bx_list[i])).abs() >= 3.0 * mh && height(line) < 1.5 * mh {
                    break;
                }
                if !usefull(&bx_list[i], zoom, page_image_widths_px, mean_height) {
                    continue;
                }
                if mmj || x_dis(&bx_list[i], line) < pw / 10.0 {
                    chosen = Some(i);
                    break;
                }
            }
            if let Some(idx) = chosen {
                let next = bx_list[idx].clone();
                dfs(
                    &next,
                    idx,
                    bx_list,
                    lines,
                    widths,
                    mh,
                    pw,
                    zoom,
                    page_image_widths_px,
                    mean_height,
                );
                // Python pops boxes[i] after recursion; recursion only
                // touches indices > idx, so idx still points at the element.
                if idx < bx_list.len() {
                    bx_list.remove(idx);
                }
            }
        }

        let head = bx_list[0].clone();
        if usefull(&head, zoom, page_image_widths_px, mean_height) {
            dfs(
                &head,
                0,
                &mut bx_list,
                &mut lines,
                &mut widths,
                mh,
                pw,
                zoom,
                page_image_widths_px,
                mean_height,
            );
        }
        bx_list.remove(0);
        let mw = if widths.is_empty() {
            0.0
        } else {
            widths.iter().sum::<f64>() / widths.len() as f64
        };
        if mj || mw / pw.max(1e-9) >= 0.35 || mw > 200.0 {
            let joined = lines
                .iter()
                .map(|c| {
                    format!(
                        "{}{}",
                        c.text,
                        line_tag(c, zoom, page_cum_height, page_image_heights_px)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            res.push(joined);
        }
    }
    *boxes = bx_list;
    res.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_at(top: f64, bottom: f64, x0: f64, x1: f64, text: &str, page: usize) -> PdfBox {
        PdfBox {
            top,
            bottom,
            x0,
            x1,
            text: text.to_string(),
            page_number: page,
            ..Default::default()
        }
    }

    #[test]
    fn garbled_char_detection() {
        assert!(is_garbled_char(Some('\u{E000}'))); // PUA
        assert!(is_garbled_char(Some('\u{F0000}'))); // PUA supplementary
        assert!(is_garbled_char(Some('\u{FFFD}'))); // replacement char
        assert!(is_garbled_char(Some('\u{01}'))); // control
        assert!(is_garbled_char(Some('\u{0085}'))); // C1 control
        assert!(!is_garbled_char(Some('你')));
        assert!(!is_garbled_char(Some('a')));
        assert!(!is_garbled_char(Some(' ')));
    }

    #[test]
    fn garbled_text_detection() {
        assert!(is_garbled_text("\u{E000}\u{E001}\u{E002}", 0.5));
        assert!(is_garbled_text("(cid:123) hello", 0.5));
        assert!(!is_garbled_text("你好世界 normal text", 0.5));
        assert!(!is_garbled_text("", 0.5));
        assert!(!is_garbled_text("   ", 0.5));
    }

    #[test]
    fn subset_font_prefix() {
        assert!(has_subset_font_prefix("DY1+ZLQDm1-1"));
        assert!(has_subset_font_prefix("ABC+Helvetica"));
        assert!(!has_subset_font_prefix("Helvetica"));
        assert!(!has_subset_font_prefix(""));
    }

    #[test]
    fn garbled_by_font_encoding() {
        // Mostly subset-embedded ASCII punctuation → garbled.
        let chars: Vec<GarbledChar> = (0..40)
            .map(|_| GarbledChar {
                text: "&*#@".to_string(),
                fontname: "ABCD+Embedded".to_string(),
            })
            .collect();
        assert!(is_garbled_by_font_encoding(&chars, 20));

        // Real CJK text with subset fonts → not garbled.
        let chars2: Vec<GarbledChar> = (0..40)
            .map(|_| GarbledChar {
                text: "中".to_string(),
                fontname: "ABCD+Embedded".to_string(),
            })
            .collect();
        assert!(!is_garbled_by_font_encoding(&chars2, 20));

        // Too few chars → false.
        let chars3: Vec<GarbledChar> = (0..5)
            .map(|_| GarbledChar {
                text: "&*#@".to_string(),
                fontname: "ABCD+Embedded".to_string(),
            })
            .collect();
        assert!(!is_garbled_by_font_encoding(&chars3, 20));
    }

    #[test]
    fn proj_matches_chapter_patterns() {
        assert!(match_proj("第一章 总则"));
        assert!(match_proj("第三条 定义"));
        assert!(match_proj("（一）范围"));
        assert!(match_proj("1、总则"));
        assert!(match_proj("①"));
        assert!(!match_proj("普通段落文本"));
        // "第3条" uses an ASCII digit — the pattern only accepts CJK numerals.
        assert!(!match_proj("第3条 定义"));
    }

    #[test]
    fn proj_match_levels() {
        assert_eq!(proj_match("第一章 总则"), Some(1));
        assert_eq!(proj_match("第一条 定义"), Some(2));
        assert_eq!(proj_match("一、概述"), Some(3));
        assert_eq!(proj_match("（一）范围"), Some(4));
        assert_eq!(proj_match("1、总则"), Some(5));
        assert_eq!(proj_match("1.1 小节"), Some(6));
        assert_eq!(proj_match("结论："), Some(9));
        assert_eq!(proj_match("12）"), Some(10));
        assert_eq!(proj_match("（12）"), Some(11));
        // "三、是" hits pattern 3 (`[零一二三四五六七八九十百]+[、 　]`)
        // before pattern 12 (`[零一二三四五六七八九十百]+是`).
        assert_eq!(proj_match("三、是"), Some(3));
        // Short line → None.
        assert_eq!(proj_match("ab"), None);
        // Numeric-only line → None.
        assert_eq!(proj_match("1.5"), None);
    }

    #[test]
    fn sort_x_by_page_stabilizes_columns() {
        let mut arr = vec![
            box_at(30.0, 40.0, 100.0, 200.0, "b", 1),
            box_at(10.0, 20.0, 100.0, 200.0, "a", 1),
        ];
        sort_x_by_page(&mut arr, 15.0);
        assert_eq!(arr[0].text, "a");
        assert_eq!(arr[1].text, "b");
    }

    #[test]
    fn x_y_dis_geometry() {
        let a = box_at(0.0, 10.0, 0.0, 100.0, "aa", 1);
        let b = box_at(20.0, 30.0, 100.0, 200.0, "bb", 1);
        assert_eq!(x_dis(&a, &b), 0.0); // adjacent edges
        assert_eq!(y_dis(&a, &b), 20.0); // (50 - 10) / 2
    }

    #[test]
    fn text_merge_horizontal() {
        let mut bxs = vec![
            box_at(0.0, 10.0, 0.0, 100.0, "Hello", 1),
            box_at(0.5, 10.5, 100.0, 200.0, "World", 1),
        ];
        bxs[0].layoutno = "1".to_string();
        bxs[1].layoutno = "1".to_string();
        bxs[0].col_id = Some(0);
        bxs[1].col_id = Some(0);
        let mh = vec![12.0];
        text_merge(&mut bxs, &mh);
        assert_eq!(bxs.len(), 1);
        assert_eq!(bxs[0].text, "HelloWorld");
        assert!((bxs[0].x1 - 200.0).abs() < 1e-6);
    }

    #[test]
    fn text_merge_skips_different_layouts() {
        let mut bxs = vec![
            box_at(0.0, 10.0, 0.0, 100.0, "Hello", 1),
            box_at(0.5, 10.5, 100.0, 200.0, "World", 1),
        ];
        bxs[0].layoutno = "1".to_string();
        bxs[1].layoutno = "2".to_string();
        let mh = vec![12.0];
        text_merge(&mut bxs, &mh);
        assert_eq!(bxs.len(), 2);
    }

    #[test]
    fn naive_vertical_merge_joins_lines() {
        let mut bxs = vec![
            box_at(0.0, 10.0, 0.0, 200.0, "第一行", 1),
            box_at(11.0, 21.0, 5.0, 200.0, "第二行", 1),
        ];
        bxs[0].layoutno = "1".to_string();
        bxs[1].layoutno = "1".to_string();
        let mh = vec![12.0];
        let mw = vec![8.0];
        naive_vertical_merge(&mut bxs, &mh, &mw, false);
        assert_eq!(bxs.len(), 1);
        assert!(bxs[0].text.contains("第一行"));
        assert!(bxs[0].text.contains("第二行"));
    }

    #[test]
    fn naive_vertical_merge_keeps_detached_boxes() {
        let mut bxs = vec![
            box_at(0.0, 10.0, 0.0, 100.0, "左列", 1),
            box_at(11.0, 21.0, 300.0, 400.0, "右列", 1),
        ];
        bxs[0].layoutno = "1".to_string();
        bxs[1].layoutno = "1".to_string();
        let mh = vec![12.0];
        let mw = vec![8.0];
        naive_vertical_merge(&mut bxs, &mh, &mw, false);
        // Detached (x1 < x0 of next) → not merged.
        assert_eq!(bxs.len(), 2);
    }

    #[test]
    fn filter_forpages_removes_contents_section() {
        let mut bxs = vec![
            box_at(0.0, 10.0, 0.0, 100.0, "目录", 1),
            box_at(20.0, 30.0, 0.0, 100.0, "第一章 总则", 1),
            box_at(40.0, 50.0, 0.0, 100.0, "第二条", 1),
        ];
        filter_forpages(&mut bxs, 1);
        // "目录" + prefix-matched "第一章 总则" removed; "第二条" kept.
        assert_eq!(bxs.len(), 1);
        assert!(bxs[0].text.contains("第二条"));
    }

    #[test]
    fn filter_forpages_removes_dotted_pages() {
        let mut bxs = vec![
            box_at(0.0, 10.0, 0.0, 100.0, "····", 1),
            box_at(20.0, 30.0, 0.0, 100.0, "····", 1),
            box_at(40.0, 50.0, 0.0, 100.0, "····", 1),
            box_at(60.0, 70.0, 0.0, 100.0, "····", 1),
            box_at(80.0, 90.0, 0.0, 100.0, "正文", 2),
        ];
        filter_forpages(&mut bxs, 2);
        assert_eq!(bxs.len(), 1);
        assert_eq!(bxs[0].page_number, 2);
    }

    #[test]
    fn merge_with_same_bullet_joins() {
        let mut bxs = vec![
            box_at(0.0, 10.0, 0.0, 100.0, "•第一项", 1),
            box_at(11.0, 21.0, 0.0, 100.0, "•第二项", 1),
        ];
        merge_with_same_bullet(&mut bxs);
        assert_eq!(bxs.len(), 1);
        assert!(bxs[0].text.contains('\n'));
        // Lowercase letters never merge.
        let mut bxs2 = vec![
            box_at(0.0, 10.0, 0.0, 100.0, "a first", 1),
            box_at(11.0, 21.0, 0.0, 100.0, "a second", 1),
        ];
        merge_with_same_bullet(&mut bxs2);
        assert_eq!(bxs2.len(), 2);
    }

    #[test]
    fn line_tag_formats_marker() {
        let b = box_at(10.0, 20.0, 5.0, 95.0, "text", 1);
        let cum = vec![0.0, 800.0];
        let heights = vec![2400.0];
        let tag = line_tag(&b, 3.0, &cum, &heights);
        assert!(tag.starts_with("@@1\t"));
        assert!(tag.ends_with("##"));
        assert!(tag.contains("5.0\t95.0\t10.0\t20.0"));
    }

    #[test]
    fn filterout_scraps_keeps_useful_lines() {
        let mut bxs = vec![box_at(0.0, 10.0, 0.0, 300.0, "第一章 标题", 1)];
        bxs[0].layout_type = "title".to_string();
        let mh = vec![12.0];
        let pw = vec![1800.0];
        let cum = vec![0.0, 800.0];
        let ph = vec![2400.0];
        let out = filterout_scraps(&mut bxs, 3.0, &mh, &pw, &cum, &ph);
        assert!(out.contains("第一章 标题"));
    }
}
