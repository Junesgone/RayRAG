//! Table chunker — mirrors RAGFlow `rag/app/table.py` (Excel / CSV / TXT).
//!
//! RAGFlow's table pipeline treats every data row as ONE chunk whose content
//! is `- field: value` lines, plus typed column metadata. Ported semantics:
//!
//! - `detect_header_rows` / `looks_like_header` / `looks_like_data`:
//!   header-vs-data row classification (complex headers when the first two
//!   rows touch a merged range, up to 5 header rows otherwise).
//! - `build_hierarchical_headers`: multi-level headers joined with `-`
//!   (`科目-名称`), `Column_N` fallback for empty parts.
//! - `merged_value`: merged-cell inheritance (`get_merged_cell_value` /
//!   `get_inherited_value` in table.py).
//! - `column_data_type`: per-column type inference (int/float/datetime/bool/
//!   text) with value normalization, mirroring `trans_datatime` /
//!   `trans_bool` / `column_data_type`.
//! - `rows_to_chunks`: one chunk per row, `- field: value` lines, doc
//!   metadata `docnm_kwd` + `title_tks`, English detection for tokenizer.
//!
//! The Excel side (merged ranges, cell grid) lives in `parser/excel.rs`;
//! this module works on the decoded `SpreadsheetSheet` grid + merged ranges,
//! and on CSV/TXT row grids, so all three table.py branches share one core.

use crate::Result;
use crate::naive::{ChunkDoc, tokenize_doc};

/// Field-type suffix map — mirrors table.py `fields_map`:
/// text→_tks, int→_long, keyword→_kwd, float→_flt, datetime→_dt, bool→_kwd.
pub const FIELD_SUFFIX: [(&str, &str); 5] = [
    ("text", "_tks"),
    ("int", "_long"),
    ("float", "_flt"),
    ("datetime", "_dt"),
    ("bool", "_kwd"),
];

/// A merged cell range, 1-based inclusive (min_row, min_col, max_row, max_col).
pub type MergeRange = (u32, u32, u32, u32);

/// True when any merged range touches row 1 or 2 — mirrors
/// `_has_complex_header_structure` (merged_ranges.min_row <= 2).
pub fn has_complex_header_structure(merged_ranges: &[MergeRange]) -> bool {
    merged_ranges.iter().any(|(min_row, ..)| *min_row <= 2)
}

/// `_looks_like_header`: CJK / ≥2 alpha chars / punctuation-heavy values
/// classify as header text.
pub fn looks_like_header(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if value.chars().any(|c| c > '\u{7f}') {
        return true;
    }
    let alpha = value.chars().filter(|c| c.is_ascii_alphabetic()).count();
    if alpha >= 2 {
        return true;
    }
    value
        .chars()
        .any(|c| matches!(c, '(' | ')' | '：' | ':' | '（' | '）' | '_' | '-'))
}

/// `_looks_like_data`: single Y/N/M/X chars, numeric, or short hex.
pub fn looks_like_data(value: &str) -> bool {
    if value.len() == 1
        && matches!(
            value.to_ascii_uppercase().as_str(),
            "Y" | "N" | "M" | "X" | "/" | "-"
        )
    {
        return true;
    }
    let digits = value.replace(['.', '-', ','], "");
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    value.starts_with("0x") && value.len() <= 10
}

/// `_row_looks_like_header`: header-like cells must outnumber data-like cells.
pub fn row_looks_like_header(row: &[Option<String>]) -> bool {
    let mut header_like = 0usize;
    let mut data_like = 0usize;
    let mut non_empty = 0usize;
    for value in row.iter().flatten() {
        if value.trim().is_empty() {
            continue;
        }
        non_empty += 1;
        if looks_like_header(value.trim()) {
            header_like += 1;
        } else if looks_like_data(value.trim()) {
            data_like += 1;
        }
    }
    non_empty > 0 && header_like >= data_like
}

/// `_detect_header_rows`: scan up to 5 rows; consecutive header-like rows
/// extend the header block, first data-like row stops it. At least 1.
pub fn detect_header_rows(rows: &[Vec<Option<String>>]) -> usize {
    if rows.len() < 2 {
        return 1;
    }
    let mut header_rows = 1usize;
    let max_check = rows.len().min(5);
    for row in rows.iter().take(max_check).skip(1) {
        if row_looks_like_header(row) {
            header_rows += 1;
        } else {
            break;
        }
    }
    header_rows
}

/// `_parse_simple_headers`: single header row; empty cells become `Column_N`.
pub fn parse_simple_headers(rows: &[Vec<Option<String>>]) -> (Vec<String>, usize) {
    if rows.is_empty() {
        return (Vec::new(), 0);
    }
    let header_row = &rows[0];
    let headers = (0..header_row.len())
        .map(|i| {
            header_row[i]
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Column_{}", i + 1))
        })
        .collect();
    (headers, 1)
}

/// `_is_valid_header_part`: reject lone Y/N/M/X, numeric, or symbol parts.
pub fn is_valid_header_part(value: &str) -> bool {
    if value.len() == 1 && matches!(value.to_ascii_uppercase().as_str(), "Y" | "N" | "M" | "X") {
        return false;
    }
    let digits = value.replace(['.', '-', ','], "");
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    !matches!(value, "/" | "-" | "+" | "*" | "=")
}

/// `_get_merged_cell_value` / `_get_inherited_value`: the value at the
/// top-left of the merged range covering (row, col), if any.
pub fn merged_value(
    merged_ranges: &[MergeRange],
    row: u32,
    col: u32,
    grid: &[Vec<Option<String>>],
) -> Option<String> {
    for (min_row, min_col, max_row, max_col) in merged_ranges {
        if *min_row <= row && row <= *max_row && *min_col <= col && col <= *max_col {
            // Anchor cell at (min_row, min_col), 1-based.
            let r = (*min_row as usize).saturating_sub(1);
            let c = (*min_col as usize).saturating_sub(1);
            if let Some(row_cells) = grid.get(r)
                && let Some(cell) = row_cells.get(c)
            {
                return cell.clone();
            }
            return None;
        }
    }
    None
}

/// `_build_hierarchical_headers`: per column, collect non-empty valid header
/// parts from each header row (merged-aware), join with `-`.
pub fn build_hierarchical_headers(
    grid: &[Vec<Option<String>>],
    merged_ranges: &[MergeRange],
    header_rows: usize,
) -> Vec<String> {
    let max_col = grid
        .iter()
        .take(header_rows)
        .map(|row| row.len())
        .max()
        .unwrap_or(0);
    let mut headers = Vec::with_capacity(max_col);
    for col_idx in 0..max_col {
        let mut parts: Vec<String> = Vec::new();
        for row_idx in 0..header_rows {
            let mut cell_value = grid
                .get(row_idx)
                .and_then(|row| row.get(col_idx))
                .and_then(|v| v.clone());
            if let Some(mv) =
                merged_value(merged_ranges, row_idx as u32 + 1, col_idx as u32 + 1, grid)
            {
                cell_value = Some(mv);
            }
            if let Some(value) = cell_value {
                let trimmed = value.trim().to_owned();
                if !trimmed.is_empty()
                    && !parts.contains(&trimmed)
                    && is_valid_header_part(&trimmed)
                {
                    parts.push(trimmed);
                }
            }
        }
        if parts.is_empty() {
            headers.push(format!("Column_{}", col_idx + 1));
        } else {
            headers.push(parts.join("-"));
        }
    }
    headers
        .into_iter()
        .filter(|h| !h.is_empty() && h != "-")
        .collect()
}

/// `_extract_row_data`: a data row's values, merged-cell aware; None rows
/// become empty strings (grid cells are already Option<String>).
pub fn extract_row_data(
    grid: &[Vec<Option<String>>],
    merged_ranges: &[MergeRange],
    absolute_row_idx: usize,
    expected_cols: usize,
) -> Vec<String> {
    let mut row_data = Vec::with_capacity(expected_cols);
    for col_idx in 0..expected_cols {
        let cell = grid
            .get(absolute_row_idx)
            .and_then(|row| row.get(col_idx))
            .and_then(|v| v.clone())
            .unwrap_or_default();
        let value = if cell.trim().is_empty() {
            merged_value(
                merged_ranges,
                absolute_row_idx as u32 + 1,
                col_idx as u32 + 1,
                grid,
            )
            .unwrap_or_default()
        } else {
            cell
        };
        row_data.push(value.trim().to_owned());
    }
    row_data
}

/// `_is_empty_row`: every cell blank.
pub fn is_empty_row(row_data: &[String]) -> bool {
    row_data.iter().all(|v| v.trim().is_empty())
}

/// `trans_datatime`: parse common datetime shapes to `YYYY-MM-DD HH:MM:SS`.
/// Mirrors `datetime_parse(...).strftime("%Y-%m-%d %H:%M:%S")` for the
/// formats RayRAG can recognize without dateparser: ISO-8601, `YYYY-MM-DD`,
/// `YYYY/MM/DD`, `YYYY年M月D日`, plus `YYYY-MM-DD HH:MM[:SS]` variants.
pub fn trans_datatime(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // ISO with T separator and optional zone suffix.
    let mut normalized = s.to_owned();
    if let Some(t_pos) = normalized.find('T') {
        normalized.replace_range(t_pos..t_pos + 1, " ");
    }
    let normalized = normalized
        .trim_end_matches('Z')
        .trim_end_matches('z')
        .trim_end_matches("+08:00")
        .trim_end_matches("+00:00");

    let (date_part, time_part) = match normalized.split_once(' ') {
        Some((d, t)) => (d, t),
        None => (normalized, ""),
    };

    let mut ymd: Option<(i64, u32, u32)> = None;
    for sep in ['-', '/', '.'] {
        let parts: Vec<&str> = date_part.split(sep).collect();
        if parts.len() == 3 && parts[0].len() == 4 && parts[0].chars().all(|c| c.is_ascii_digit()) {
            let year = parts[0].parse::<i64>().ok();
            let month = parts[1].parse::<u32>().ok();
            let day = parts[2].parse::<u32>().ok();
            if let (Some(y), Some(m), Some(d)) = (year, month, day) {
                ymd = Some((y, m, d));
            }
            break;
        }
    }
    if ymd.is_none() {
        // 2026年8月3日
        let re = regex::Regex::new(r"^(\d{4})年(\d{1,2})月(\d{1,2})日?$").ok()?;
        if let Some(caps) = re.captures(date_part) {
            ymd = Some((
                caps[1].parse::<i64>().ok()?,
                caps[2].parse::<u32>().ok()?,
                caps[3].parse::<u32>().ok()?,
            ));
        }
    }
    let (year, month, day) = ymd?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut hms = (0u32, 0u32, 0u32);
    if !time_part.is_empty() {
        let tparts: Vec<&str> = time_part.split(':').collect();
        if tparts.is_empty() || tparts.len() > 3 {
            return None;
        }
        let h = tparts[0].parse::<u32>().ok()?;
        let m = tparts
            .get(1)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        let s = tparts
            .get(2)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        if h > 23 || m > 59 || s > 59 {
            return None;
        }
        hms = (h, m, s);
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}",
        year = year,
        month = month,
        day = day,
        h = hms.0,
        m = hms.1,
        s = hms.2
    ))
}

/// `trans_bool`: truthy → "yes", falsy → "no", else None.
pub fn trans_bool(s: &str) -> Option<&'static str> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "yes" | "是" | "*" | "✓" | "✔" | "☑" | "✅" | "√"
    ) {
        return Some("yes");
    }
    if matches!(lower.as_str(), "false" | "no" | "否" | "⍻" | "×") {
        return Some("no");
    }
    None
}

/// `column_data_type`: infer the majority type and normalize values in place.
/// Returns `(normalized_values, type)` — mirrors table.py `column_data_type`.
pub fn column_data_type(values: &[String]) -> (Vec<String>, &'static str) {
    let mut counts: [usize; 5] = [0; 5]; // int, float, datetime, bool, text
    let mut float_flag = false;
    for v in values {
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        let stripped = v.replace("%%", "");
        if !stripped.is_empty()
            && stripped
                .chars()
                .all(|c| c.is_ascii_digit() || c == '+' || c == '-')
            && !stripped.starts_with('0')
            && regex::Regex::new(r"^[+-]?[0-9]+$")
                .ok()
                .is_some_and(|re| re.is_match(&stripped))
        {
            counts[0] += 1;
            if stripped
                .parse::<i128>()
                .ok()
                .is_some_and(|n| n > (i64::MAX as i128))
            {
                float_flag = true;
                break;
            }
        } else if regex::Regex::new(r"^[+-]?[0-9.]{1,19}$")
            .ok()
            .is_some_and(|re| re.is_match(&stripped))
            && !stripped.starts_with('0')
        {
            counts[1] += 1;
        } else if trans_bool(v).is_some() {
            counts[3] += 1;
        } else if trans_datatime(v).is_some() {
            counts[2] += 1;
        } else {
            counts[4] += 1;
        }
    }
    let ty = if float_flag {
        "float"
    } else {
        // majority wins; ties prefer earlier in [int, float, datetime, bool, text]
        let mut best = 4usize;
        let mut best_count = 0usize;
        for (i, c) in counts.iter().enumerate() {
            if *c > best_count {
                best_count = *c;
                best = i;
            }
        }
        ["int", "float", "datetime", "bool", "text"][best]
    };

    let normalized: Vec<String> = values
        .iter()
        .map(|v| {
            let v = v.trim();
            if v.is_empty() {
                return String::new();
            }
            match ty {
                "int" => v
                    .parse::<i64>()
                    .ok()
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| v.to_owned()),
                "float" => v
                    .parse::<f64>()
                    .ok()
                    .map(|n| {
                        if n.fract() == 0.0 {
                            format!("{n:.1}")
                        } else {
                            n.to_string()
                        }
                    })
                    .unwrap_or_else(|| v.to_owned()),
                "datetime" => trans_datatime(v).unwrap_or_else(|| v.to_owned()),
                "bool" => trans_bool(v).unwrap_or("").to_owned(),
                _ => v.to_owned(),
            }
        })
        .collect();
    (normalized, ty)
}

/// Infer the database-style field name for a column:
/// pinyin-like slug + type suffix. Mirrors table.py:
/// `PY.get_pinyins(...)[0].lower() + fields_map[ty]`.
/// RayRAG uses the original column name (stripped of `_`) as the slug since
/// it does not bundle pinyin; non-ASCII names keep their characters.
pub fn field_name(column: &str, ty: &str) -> String {
    let slug = column.replace('_', " ");
    let suffix = FIELD_SUFFIX
        .iter()
        .find(|(t, _)| *t == ty)
        .map(|(_, s)| *s)
        .unwrap_or("_tks");
    format!("{}{}", slug, suffix)
}

/// One data row → one chunk (`- field: value` lines) — mirrors the loop in
/// table.py `chunk()` building `d` per `df.iterrows()`.
pub fn row_to_chunk(
    docnm: &str,
    headers: &[String],
    row: &[String],
    english: bool,
) -> Option<ChunkDoc> {
    let mut lines: Vec<String> = Vec::new();
    for (field, value) in headers.iter().zip(row.iter()) {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        lines.push(format!("- {}: {}", field, value));
    }
    if lines.is_empty() {
        return None;
    }
    let text = lines.join("\n");
    let mut d = ChunkDoc {
        docnm_kwd: docnm.to_owned(),
        title_tks: crate::chunk::token_count(docnm)
            .to_string()
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        ..Default::default()
    };
    // title_tks: RAGFlow tokenizes the filename without extension.
    let stem = docnm
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(docnm)
        .to_owned();
    d.title_tks = tokenize_title(&stem);
    tokenize_doc(&mut d, &text);
    let _ = english;
    Some(d)
}

/// Lightweight tokenizer for the title (whitespace/word split).
fn tokenize_title(stem: &str) -> Vec<String> {
    stem.split_whitespace().map(str::to_owned).collect()
}

/// Table rows → chunks. `grid` excludes header rows. Mirrors the
/// per-row chunk loop in table.py `chunk()`.
pub fn rows_to_chunks(
    docnm: &str,
    headers: &[String],
    data_rows: &[Vec<String>],
    english: bool,
) -> Vec<ChunkDoc> {
    data_rows
        .iter()
        .filter_map(|row| row_to_chunk(docnm, headers, row, english))
        .collect()
}

/// English detection — mirrors `lang.lower() == "english"`.
pub fn is_english(lang: &str) -> bool {
    lang.trim().eq_ignore_ascii_case("english")
}

/// Full table chunking entry point mirroring `table.py chunk()`:
/// parse headers (simple or hierarchical), extract data rows, then one chunk
/// per row. `from_page`/`to_page` bound the data rows (0-based, exclusive).
pub fn chunk_table(
    filename: &str,
    grid: &[Vec<Option<String>>],
    merged_ranges: &[MergeRange],
    lang: &str,
    from_page: usize,
    to_page: usize,
) -> Result<(Vec<ChunkDoc>, Vec<String>, usize)> {
    if grid.is_empty() {
        return Ok((Vec::new(), Vec::new(), 0));
    }
    let complex = has_complex_header_structure(merged_ranges);
    let (headers, header_rows) = if complex {
        let n = detect_header_rows(grid);
        if n == 1 {
            parse_simple_headers(grid)
        } else {
            (build_hierarchical_headers(grid, merged_ranges, n), n)
        }
    } else {
        parse_simple_headers(grid)
    };
    if headers.is_empty() {
        return Ok((Vec::new(), Vec::new(), 0));
    }

    let mut fails: Vec<String> = Vec::new();
    let mut data_rows: Vec<Vec<String>> = Vec::new();
    for i in header_rows..grid.len() {
        let row_index = i - header_rows;
        if row_index < from_page {
            continue;
        }
        if row_index >= to_page {
            break;
        }
        let row = extract_row_data(grid, merged_ranges, i, headers.len());
        if is_empty_row(&row) {
            continue;
        }
        data_rows.push(row);
    }

    let english = is_english(lang);
    let chunks = rows_to_chunks(filename, &headers, &data_rows, english);
    let _ = &mut fails;
    Ok((chunks, headers, data_rows.len()))
}


