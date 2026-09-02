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
                && let Some(cell) = row_cells.get(c) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&[&str]]) -> Vec<Vec<Option<String>>> {
        rows.iter()
            .map(|r| r.iter().map(|s| Some(s.to_string())).collect())
            .collect()
    }

    #[test]
    fn header_classification_distinguishes_header_from_data() {
        assert!(looks_like_header("姓名"));
        assert!(looks_like_header("Supplier/Vendor"));
        assert!(looks_like_header("size(M,L,XL)"));
        assert!(!looks_like_header("42"));
        assert!(!looks_like_header("Y"));
        assert!(looks_like_data("42"));
        assert!(looks_like_data("Y"));
        assert!(looks_like_data("0x1f"));
        assert!(!looks_like_data("Name"));
    }

    #[test]
    fn row_header_detection_requires_header_majority() {
        let header_row = vec![
            Some("姓名".into()),
            Some("电话".into()),
            Some("地址".into()),
        ];
        assert!(row_looks_like_header(&header_row));
        // CJK values are header-like in RAGFlow (`ord(c) > 127` → header);
        // a row with mostly numeric/symbol data cells is data-like.
        let data_row = vec![Some("1".into()), Some("139".into()), Some("Y".into())];
        assert!(!row_looks_like_header(&data_row));
        // Mixed: two data-like, one header-like → data row.
        let mixed = vec![Some("Y".into()), Some("N".into()), Some("备注".into())];
        assert!(!row_looks_like_header(&mixed));
    }

    #[test]
    fn detect_header_rows_stops_at_first_data_row() {
        // RAGFlow treats alpha/CJK cells as header-like; numeric rows stop it.
        let g = grid(&[
            &["name", "phone", "addr"],
            &["1", "13900000000", "N"],
            &["2", "13800000000", "Y"],
        ]);
        assert_eq!(detect_header_rows(&g), 1);
        // Two header rows (multi-level), then data. Row 3 is numeric → data.
        let g2 = grid(&[
            &["basic", "", ""],
            &["name", "phone", "addr"],
            &["1", "139", "N"],
        ]);
        assert_eq!(detect_header_rows(&g2), 2);
    }

    #[test]
    fn simple_headers_fill_blank_cells_with_column_n() {
        let g = grid(&[&["姓名", "", "地址"]]);
        let (headers, n) = parse_simple_headers(&g);
        assert_eq!(headers, vec!["姓名", "Column_2", "地址"]);
        assert_eq!(n, 1);
    }

    #[test]
    fn hierarchical_headers_join_parts_with_dash_and_skip_invalid() {
        let g = grid(&[&["基本信息", "", ""], &["姓名", "电话", "地址"]]);
        let headers = build_hierarchical_headers(&g, &[], 2);
        assert_eq!(headers, vec!["基本信息-姓名", "电话", "地址"]);
        // Header cells that are lone Y/N/M/X or numeric are filtered out;
        // the column still gets a Column_N placeholder (RAGFlow keeps it).
        let g2 = grid(&[&["状态", "Y", "N"]]);
        let headers2 = build_hierarchical_headers(&g2, &[], 1);
        assert_eq!(headers2, vec!["状态", "Column_2", "Column_3"]);
    }

    #[test]
    fn merged_value_reads_anchor_cell() {
        let g = grid(&[&["分组", "值"], &["", "1"], &["", "2"]]);
        let ranges = vec![(1, 1, 3, 1)];
        assert_eq!(merged_value(&ranges, 2, 1, &g), Some("分组".into()));
        assert_eq!(merged_value(&ranges, 3, 1, &g), Some("分组".into()));
        assert_eq!(merged_value(&ranges, 1, 2, &g), None);
    }

    #[test]
    fn extract_row_data_inherits_merged_values() {
        let g = grid(&[&["分组", "值"], &["", "1"], &["", "2"]]);
        let ranges = vec![(1, 1, 3, 1)];
        let row = extract_row_data(&g, &ranges, 1, 2);
        assert_eq!(row, vec!["分组", "1"]);
        let row2 = extract_row_data(&g, &ranges, 2, 2);
        assert_eq!(row2, vec!["分组", "2"]);
    }

    #[test]
    fn empty_row_detection() {
        assert!(is_empty_row(&["".into(), "  ".into()]));
        assert!(!is_empty_row(&["a".into(), "".into()]));
    }

    #[test]
    fn datetime_parsing_handles_common_formats() {
        assert_eq!(
            trans_datatime("2026-08-03"),
            Some("2026-08-03 00:00:00".into())
        );
        assert_eq!(
            trans_datatime("2026/08/03 14:30"),
            Some("2026-08-03 14:30:00".into())
        );
        assert_eq!(
            trans_datatime("2026-08-03T14:30:05Z"),
            Some("2026-08-03 14:30:05".into())
        );
        assert_eq!(
            trans_datatime("2026年8月3日"),
            Some("2026-08-03 00:00:00".into())
        );
        assert_eq!(trans_datatime("not a date"), None);
        assert_eq!(trans_datatime("2026-13-01"), None);
    }

    #[test]
    fn bool_translation_maps_cn_and_symbols() {
        assert_eq!(trans_bool("是"), Some("yes"));
        assert_eq!(trans_bool("✓"), Some("yes"));
        assert_eq!(trans_bool("×"), Some("no"));
        assert_eq!(trans_bool("否"), Some("no"));
        assert_eq!(trans_bool("unknown"), None);
    }

    #[test]
    fn column_type_inference_majority_wins() {
        let (values, ty) =
            column_data_type(&["1".into(), "2".into(), "3".into(), "x".into(), "4".into()]);
        assert_eq!(ty, "int");
        assert_eq!(values[0], "1");

        let (values, ty) =
            column_data_type(&["1.5".into(), "2.5".into(), "abc".into(), "3.5".into()]);
        assert_eq!(ty, "float");
        assert_eq!(values[1], "2.5");

        let (values, ty) = column_data_type(&["是".into(), "否".into(), "是".into()]);
        assert_eq!(ty, "bool");
        assert_eq!(values[0], "yes");
        assert_eq!(values[1], "no");

        let (_, ty) = column_data_type(&["2026-08-03".into(), "2026-08-04".into(), "text".into()]);
        assert_eq!(ty, "datetime");
    }

    #[test]
    fn field_names_carry_type_suffix() {
        assert_eq!(field_name("姓名", "text"), "姓名_tks");
        assert_eq!(field_name("amount", "int"), "amount_long");
        assert_eq!(field_name("score", "float"), "score_flt");
        assert_eq!(field_name("date", "datetime"), "date_dt");
        assert_eq!(field_name("ok", "bool"), "ok_kwd");
    }

    #[test]
    fn row_to_chunk_formats_field_lines() {
        let chunk = row_to_chunk(
            "data.xlsx",
            &["姓名".into(), "电话".into(), "地址".into()],
            &["张三".into(), "139".into(), "中山".into()],
            false,
        )
        .unwrap();
        assert_eq!(
            chunk.content_with_weight,
            "- 姓名: 张三\n- 电话: 139\n- 地址: 中山"
        );
        assert_eq!(chunk.docnm_kwd, "data.xlsx");
        assert!(chunk.title_tks.contains(&"data".to_string()));
    }

    #[test]
    fn row_to_chunk_skips_all_empty_rows() {
        let chunk = row_to_chunk(
            "data.xlsx",
            &["a".into(), "b".into()],
            &["".into(), "  ".into()],
            false,
        );
        assert!(chunk.is_none());
    }

    #[test]
    fn chunk_table_end_to_end_simple_grid() {
        let g = grid(&[
            &["姓名", "电话", "地址"],
            &["张三", "139", "中山"],
            &["李四", "138", "珠海"],
        ]);
        let (chunks, headers, count) =
            chunk_table("data.xlsx", &g, &[], "Chinese", 0, usize::MAX).unwrap();
        assert_eq!(headers, vec!["姓名", "电话", "地址"]);
        assert_eq!(count, 2);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content_with_weight.contains("张三"));
    }

    #[test]
    fn chunk_table_honors_page_bounds() {
        let g = grid(&[
            &["姓名", "电话"],
            &["张三", "139"],
            &["李四", "138"],
            &["王五", "137"],
        ]);
        let (chunks, _, count) = chunk_table("data.xlsx", &g, &[], "Chinese", 1, 2).unwrap();
        assert_eq!(count, 1);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content_with_weight.contains("李四"));
    }

    #[test]
    fn chunk_table_handles_merged_group_column() {
        // With a merged range touching row 1 the complex-header path runs;
        // RAGFlow's heuristic treats CJK cells (甲/乙) as header-like, so the
        // data rows extend the header block and the merged anchor "分组"
        // participates in the hierarchical header. Faithful port behavior:
        let g = grid(&[
            &["分组", "名称", "数量"],
            &["", "甲", "1"],
            &["", "乙", "2"],
        ]);
        // Merge 分组 over rows 1-3 (A1:A3).
        let ranges = vec![(1, 1, 3, 1)];
        let (chunks, headers, _) =
            chunk_table("g.xlsx", &g, &ranges, "Chinese", 0, usize::MAX).unwrap();
        // Complex path: rows 2-3 are header-like (CJK) → header block = 3,
        // hierarchical headers join every non-empty part.
        assert_eq!(headers, vec!["分组", "名称-甲-乙", "数量"]);
        // With the whole grid consumed as headers there are no data rows.
        assert!(chunks.is_empty());
    }

    #[test]
    fn chunk_table_simple_chinese_grid_without_merges() {
        // No merged ranges → simple-header path: first row is the header,
        // every subsequent row is one chunk with `- field: value` lines.
        let g = grid(&[
            &["姓名", "电话", "地址"],
            &["张三", "139", "中山"],
            &["李四", "138", "珠海"],
        ]);
        let (chunks, headers, count) =
            chunk_table("data.xlsx", &g, &[], "Chinese", 0, usize::MAX).unwrap();
        assert_eq!(headers, vec!["姓名", "电话", "地址"]);
        assert_eq!(count, 2);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content_with_weight.contains("- 姓名: 张三"));
        assert!(chunks[1].content_with_weight.contains("- 姓名: 李四"));
    }

    #[test]
    fn xlsx_to_chunks_end_to_end_with_merged_group_column() {
        // Full pipeline: xlsx bytes (writer with merges) → read_xlsx_sheets →
        // grid → chunk_table. Mirrors RAGFlow: file → table.py chunk().
        use crate::parser::excel::read_xlsx_sheets;
        use crate::parser::excel::write_xlsx_sheets_with_merges;
        use serde_json::json;

        let bytes = write_xlsx_sheets_with_merges(
            &[(
                "Sheet1".into(),
                vec![
                    // Header row with a vertical merge in column A (group).
                    vec![json!("分组"), json!("名称"), json!("数量")],
                    vec![json!(""), json!("A组"), json!("10")],
                    vec![json!(""), json!("B组"), json!("20")],
                ],
            )],
            // A1:A3 merged: "分组" spans rows 1-3.
            &[vec![(1, 1, 3, 1)]],
        )
        .unwrap();

        let sheets = read_xlsx_sheets(&bytes, 1000).unwrap();
        assert_eq!(sheets[0].merged_ranges, vec![(1, 1, 3, 1)]);

        // Build the Option<String> grid from rows.
        let grid: Vec<Vec<Option<String>>> = sheets[0]
            .rows
            .iter()
            .map(|row| row.iter().map(|v| Some(v.clone())).collect())
            .collect();
        let (chunks, headers, count) = chunk_table(
            "merged.xlsx",
            &grid,
            &sheets[0].merged_ranges,
            "Chinese",
            0,
            usize::MAX,
        )
        .unwrap();

        // Complex-header path (merge touches row 1): RAGFlow's CJK heuristic
        // treats A组/B组 as header-like, extending the header block.
        assert_eq!(headers, vec!["分组", "名称-A组-B组", "数量"]);
        // Grid fully consumed as headers → no data rows.
        assert_eq!(count, 0);
        assert!(chunks.is_empty());
    }

    #[test]
    fn xlsx_to_chunks_end_to_end_numeric_rows_stop_headers() {
        // English headers + numeric data: simple path, one chunk per row.
        use crate::parser::excel::read_xlsx_sheets;
        use crate::parser::excel::write_xlsx_sheets_with_merges;
        use serde_json::json;

        let bytes = write_xlsx_sheets_with_merges(
            &[(
                "Sheet1".into(),
                vec![
                    vec![json!("name"), json!("score")],
                    vec![json!("alice"), json!("95")],
                    vec![json!("bob"), json!("88")],
                ],
            )],
            &[],
        )
        .unwrap();

        let sheets = read_xlsx_sheets(&bytes, 1000).unwrap();
        let grid: Vec<Vec<Option<String>>> = sheets[0]
            .rows
            .iter()
            .map(|row| row.iter().map(|v| Some(v.clone())).collect())
            .collect();
        let (chunks, headers, count) =
            chunk_table("scores.xlsx", &grid, &[], "English", 0, usize::MAX).unwrap();

        assert_eq!(headers, vec!["name", "score"]);
        assert_eq!(count, 2);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].content_with_weight.contains("- name: alice"));
        assert!(chunks[1].content_with_weight.contains("- name: bob"));
    }
}
