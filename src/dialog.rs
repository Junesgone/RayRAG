//! Dialogue answer post-processing — ported from RAGFlow's
//! `api/db/services/dialog_service.py` (`repair_bad_citation_formats`) and
//! `common/text_utils.py` (`normalize_arabic_digits`).
//!
//! LLMs frequently cite retrieved chunks in inconsistent formats
//! (`(ID: 12)`, `[ID: 12]`, `【ID: 12】`, `ref12`). RAGFlow normalizes all
//! of them to the canonical `[ID:12]` marker and drops out-of-range indexes
//! while collecting the valid set for the response metadata. This module
//! mirrors that behaviour.

use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

/// `(ID: 12)` — parenthesised citation.
static PATTERN_PAREN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(\s*ID\s*[: ]\s*(\d+)\s*\)").unwrap());
/// `[ID: 12]` — bracketed citation.
static PATTERN_BRACKET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\s*ID\s*[: ]\s*(\d+)\s*\]").unwrap());
/// `【ID: 12】` — CJK bracketed citation.
static PATTERN_CJK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"【\s*ID\s*[: ]\s*(\d+)\s*】").unwrap());
/// `ref12` / `REF 12` — ref-prefixed citation.
static PATTERN_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)ref\s*(\d+)").unwrap());
/// Canonical `[ID:12]` marker, also used to extract cited indexes.
pub static CITATION_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:ID:)?([0-9\u{0660}-\u{0669}\u{06F0}-\u{06F9}]+)\]").unwrap()
});

/// `normalize_arabic_digits`: map Arabic-Indic digits (U+0660..U+0669,
/// U+06F0..U+06F9) to ASCII digits so citation indexes parse consistently.
pub fn normalize_arabic_digits(text: &str) -> String {
    text.chars()
        .map(|ch| {
            let code = ch as u32;
            if (0x0660..=0x0669).contains(&code) {
                char::from_u32(code - 0x0660 + 0x30).unwrap_or(ch)
            } else if (0x06F0..=0x06F9).contains(&code) {
                char::from_u32(code - 0x06F0 + 0x30).unwrap_or(ch)
            } else {
                ch
            }
        })
        .collect()
}

/// `repair_bad_citation_formats`: rewrite the four known bad citation
/// shapes to the canonical `[ID:{digits}]` marker, dropping out-of-range
/// indexes (kept verbatim) and returning the set of valid cited indexes.
///
/// The replacement preserves the *original* digits from the answer so the
/// canonical marker always carries ASCII digits (Arabic-Indic digits were
/// normalized for matching only).
pub fn repair_bad_citation_formats(answer: &str, max_index: usize) -> (String, HashSet<usize>) {
    let normalized = normalize_arabic_digits(answer);
    let mut valid: HashSet<usize> = HashSet::new();
    let mut out = String::with_capacity(answer.len());

    let mut rewrite = |out: &mut String, input: &str, pattern: &Regex| {
        let mut last = 0;
        for capture in pattern.captures_iter(input) {
            let whole = capture.get(0).unwrap();
            out.push_str(&input[last..whole.start()]);
            let digits = capture.get(1).unwrap();
            let parsed: Result<usize, _> = digits.as_str().parse();
            match parsed {
                Ok(index) if index < max_index => {
                    valid.insert(index);
                    out.push_str(&format!("[ID:{}]", &input[digits.start()..digits.end()]));
                }
                _ => out.push_str(&input[whole.start()..whole.end()]),
            }
            last = whole.end();
        }
        out.push_str(&input[last..]);
    };

    // Pass each bad pattern over the evolving output, exactly like RAGFlow.
    rewrite(&mut out, &normalized, &PATTERN_PAREN);
    let mut intermediate = std::mem::take(&mut out);
    rewrite(&mut out, &intermediate, &PATTERN_BRACKET);
    intermediate = std::mem::take(&mut out);
    rewrite(&mut out, &intermediate, &PATTERN_CJK);
    intermediate = std::mem::take(&mut out);
    rewrite(&mut out, &intermediate, &PATTERN_REF);

    (out, valid)
}

/// Extract the canonical `[ID:n]` markers (already normalized) into a set of
/// cited chunk indexes.
pub fn extract_cited_indexes(answer: &str) -> HashSet<usize> {
    CITATION_MARKER
        .captures_iter(&normalize_arabic_digits(answer))
        .filter_map(|capture| {
            capture
                .get(1)
                .and_then(|digits| digits.as_str().parse().ok())
        })
        .collect()
}

/// `normalize_sql`: strip LLM artefacts from generated SQL — think blocks
/// (`</think>...`), Chinese reasoning markers (思考...), markdown code
/// fences (```sql ... ```) and trailing semicolons that some engines reject.
/// Mirrors RAGFlow `DialogService.use_sql.normalize_sql`.
pub fn normalize_sql(sql: &str) -> String {
    let mut out = sql.to_string();
    // Remove think blocks if present (format: </think>...)
    out = regex_replace_all(r"</think>\n.*?\n\s*", &out, "");
    out = regex_replace_all(r"思考\n.*?\n", &out, "");
    // Remove markdown code blocks (```sql ... ```)
    out = regex_replace_all(r"(?i)```(?:sql)?\s*", &out, "");
    out = regex_replace_all(r"(?i)```\s*$", &out, "");
    // Remove trailing semicolon that ES SQL parser doesn't like
    out.trim_end().trim_end_matches(';').trim().to_string()
}

/// `clean_tts_text`: sanitise text before speech synthesis — strip control
/// characters, emoji, collapse whitespace and cap at 500 chars. Mirrors
/// RAGFlow `dialog_service.clean_tts_text` (MAX_LEN = 500).
pub fn clean_tts_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    // Strip control characters (C0 except tab/newline/cr, plus DEL).
    for ch in text.chars() {
        let code = ch as u32;
        if (code < 0x20 && !matches!(code, 0x09 | 0x0A | 0x0D)) || code == 0x7F {
            continue;
        }
        out.push(ch);
    }
    // Strip emoji ranges (mirrors RAGFlow emoji_pattern).
    out = EMOJI_PATTERN.replace_all(&out, "").to_string();
    // Collapse whitespace.
    let mut collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > TTS_MAX_LEN {
        collapsed = collapsed.chars().take(TTS_MAX_LEN).collect();
    }
    collapsed
}

/// TTS text length cap (RAGFlow `clean_tts_text.MAX_LEN`).
const TTS_MAX_LEN: usize = 500;

/// Emoji ranges mirroring RAGFlow `clean_tts_text.emoji_pattern`: emotions,
/// misc symbols/pictographs, transport, flags, dingbats, supplemental
/// symbols, supplemental symbols and pictographs.
static EMOJI_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"[\u{1F600}-\u{1F64F}\u{1F300}-\u{1F5FF}\u{1F680}-\u{1F6FF}\u{1F1E0}-\u{1F1FF}\u{2700}-\u{27BF}\u{1F900}-\u{1F9FF}\u{1FA70}-\u{1FAFF}\u{1FAD0}-\u{1FAFF}]+",
    )
    .unwrap()
});

fn regex_replace_all(pattern: &str, input: &str, replacement: &str) -> String {
    Regex::new(pattern)
        .map(|regex| regex.replace_all(input, replacement).to_string())
        .unwrap_or_else(|_| input.to_string())
}

// ── use_sql helpers (appended) ─────────────────────────────────────────────
//
// Ports of `DialogService.use_sql` pure helpers from RAGFlow
// `api/db/services/dialog_service.py`: aggregate-SQL detection, row-count
// question detection and the kb_id WHERE-clause injection guard.

/// `use_sql.is_aggregate_sql` — true when the SQL contains an aggregate
/// function (COUNT, SUM, AVG, MAX, MIN, DISTINCT).
pub fn is_aggregate_sql(sql_text: &str) -> bool {
    AGGREGATE_PATTERN.is_match(&sql_text.to_ascii_lowercase())
}

/// `use_sql.is_row_count_question` — true when the question asks for a total
/// row count of a dataset/table/spreadsheet/excel.
pub fn is_row_count_question(q: &str) -> bool {
    let q = q.to_ascii_lowercase();
    if !ROW_COUNT_PATTERN.is_match(&q) {
        return false;
    }
    ROW_COUNT_TARGET_PATTERN.is_match(&q)
}

/// `use_sql.add_kb_filter` — inject a validated `kb_id` WHERE filter for
/// ES/OpenSearch engines (Infinity encodes the KB in the table name, so it is
/// a no-op there — callers skip it). Every `kb_id` is validated as a canonical
/// UUID before interpolation to prevent SQL injection, raising RAGFlow's
/// `"Invalid kb_id format: {value!r}"` as an `ApiError` (HTTP 400).
pub fn add_kb_filter(sql: &str, kb_ids: &[String]) -> Result<String, crate::api::common::ApiError> {
    if kb_ids.is_empty() {
        return Ok(sql.to_string());
    }
    for kid in kb_ids {
        if uuid::Uuid::parse_str(kid).is_err() {
            return Err(crate::api::common::ApiError::admin(format!(
                "Invalid kb_id format: {kid:?}"
            )));
        }
    }
    let kb_filter = if kb_ids.len() == 1 {
        format!("kb_id = '{}'", kb_ids[0])
    } else {
        let alternatives = kb_ids
            .iter()
            .map(|kid| format!("kb_id = '{kid}'"))
            .collect::<Vec<_>>()
            .join(" OR ");
        format!("({alternatives})")
    };
    let lower = sql.to_ascii_lowercase();
    let mut out = sql.to_string();
    if !lower.contains("where ") {
        // RAGFlow splits the *lowercased* SQL on "order by", so the head and
        // tail keep lowercase in this branch (a faithful quirk of
        // `use_sql.add_kb_filter`).
        if let Some(order_index) = lower.find("order by") {
            let head = &lower[..order_index];
            let tail = &lower[order_index + "order by".len()..];
            out = format!("{head} WHERE {kb_filter}  order by {tail}");
        } else {
            out = format!("{out} WHERE {kb_filter}");
        }
    } else if !lower.contains("kb_id =") && !lower.contains("kb_id=") {
        // re.sub(r"\bwhere\b ", f"where {kb_filter} and ", sql, IGNORECASE):
        // the matched token is replaced by a lowercase "where" clause.
        out = WHERE_RE
            .replace(&out, format!("where {kb_filter} and "))
            .to_string();
    }
    Ok(out)
}

static AGGREGATE_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(count|sum|avg|max|min|distinct)\s*\(").unwrap());
static ROW_COUNT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bhow many rows\b|\bnumber of rows\b|\brow count\b").unwrap());
static ROW_COUNT_TARGET_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bdataset\b|\btable\b|\bspreadsheet\b|\bexcel\b").unwrap());
static WHERE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bwhere\b ").unwrap());

#[cfg(test)]
mod use_sql_tests {
    use super::*;

    #[test]
    fn aggregate_sql_detection() {
        assert!(is_aggregate_sql("SELECT COUNT(*) FROM t"));
        assert!(is_aggregate_sql("select count(*) from t"));
        assert!(is_aggregate_sql("SELECT SUM(amount) FROM t"));
        assert!(is_aggregate_sql("SELECT DISTINCT(name) FROM t"));
        assert!(!is_aggregate_sql("SELECT name FROM t WHERE id = 1"));
        assert!(!is_aggregate_sql(""));
    }

    #[test]
    fn row_count_question_detection() {
        assert!(is_row_count_question("How many rows are in the dataset?"));
        assert!(is_row_count_question("number of rows in the excel file"));
        assert!(!is_row_count_question("how many rows in the chart"));
        assert!(!is_row_count_question("row count of my query"));
    }

    #[test]
    fn add_kb_filter_injects_validated_where_clause() {
        let sql = add_kb_filter(
            "SELECT * FROM ragflow_t1",
            &["3fa85f64-5717-4562-b3fc-2c963f66afa6".into()],
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT * FROM ragflow_t1 WHERE kb_id = '3fa85f64-5717-4562-b3fc-2c963f66afa6'"
        );
        // Multiple KBs become an OR group.
        let sql = add_kb_filter(
            "SELECT * FROM ragflow_t1",
            &[
                "3fa85f64-5717-4562-b3fc-2c963f66afa6".into(),
                "4fa85f64-5717-4562-b3fc-2c963f66afa6".into(),
            ],
        )
        .unwrap();
        assert!(sql.contains("WHERE (kb_id = '3fa85f64-5717-4562-b3fc-2c963f66afa6' OR kb_id = '4fa85f64-5717-4562-b3fc-2c963f66afa6')"));
        // Existing WHERE is preserved and ANDed (RAGFlow's re.sub replaces the
        // token with a lowercase "where" clause).
        let sql = add_kb_filter(
            "SELECT * FROM ragflow_t1 WHERE doc_id = 'x'",
            &["3fa85f64-5717-4562-b3fc-2c963f66afa6".into()],
        )
        .unwrap();
        assert!(
            sql.contains("where kb_id = '3fa85f64-5717-4562-b3fc-2c963f66afa6' and doc_id = 'x'")
        );
        // Non-UUID kb_id is rejected with the RAGFlow guard message.
        let err = add_kb_filter("SELECT 1", &["not-a-uuid".into()]).unwrap_err();
        assert_eq!(err.code, 400);
        assert!(err.message.contains("Invalid kb_id format"));
        // Empty kb list is a no-op.
        assert_eq!(add_kb_filter("SELECT 1", &[]).unwrap(), "SELECT 1");
    }

    #[test]
    fn add_kb_filter_respects_order_by_position() {
        // RAGFlow lowercases the SQL in this branch (it splits the lowercased
        // string), including the doubled spaces around the injected clause.
        let sql = add_kb_filter(
            "SELECT * FROM ragflow_t1 ORDER BY created_at DESC",
            &["3fa85f64-5717-4562-b3fc-2c963f66afa6".into()],
        )
        .unwrap();
        assert_eq!(
            sql,
            "select * from ragflow_t1  WHERE kb_id = '3fa85f64-5717-4562-b3fc-2c963f66afa6'  order by  created_at desc"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arabic_digits_normalize_to_ascii() {
        assert_eq!(normalize_arabic_digits("٠١٢٣٤٥٦٧٨٩"), "0123456789");
        assert_eq!(normalize_arabic_digits("۰۱۲۳۴۵۶۷۸۹"), "0123456789");
        assert_eq!(normalize_arabic_digits("abc123"), "abc123");
        assert_eq!(normalize_arabic_digits(""), "");
    }

    #[test]
    fn bad_citation_formats_are_canonicalized() {
        let (out, idx) =
            repair_bad_citation_formats("答案见 (ID: 12)、[ID: 3]、【ID: 7】与 ref9。", 20);
        assert_eq!(out, "答案见 [ID:12]、[ID:3]、[ID:7]与 [ID:9]。");
        assert_eq!(idx, HashSet::from([12, 3, 7, 9]));
    }

    #[test]
    fn out_of_range_citations_are_kept_verbatim_and_not_cited() {
        let (out, idx) = repair_bad_citation_formats("见 (ID: 99) 与 [ID: 5]", 10);
        assert_eq!(out, "见 (ID: 99) 与 [ID:5]");
        assert_eq!(idx, HashSet::from([5]));
    }

    #[test]
    fn ref_marker_without_index_is_left_alone() {
        let (out, idx) = repair_bad_citation_formats("参考 ref12 与 ref 文本", 20);
        assert_eq!(out, "参考 [ID:12] 与 ref 文本");
        assert_eq!(idx, HashSet::from([12]));
    }

    #[test]
    fn arabic_indian_digits_in_citations_are_canonicalized() {
        // U+0663 = 3, U+0661 = 1: 【ID: ٣】 → [ID:3].
        let (out, idx) = repair_bad_citation_formats("见【ID: ٣】", 10);
        assert_eq!(out, "见[ID:3]");
        assert_eq!(idx, HashSet::from([3]));
    }

    #[test]
    fn extract_cited_indexes_parses_canonical_markers() {
        let cited = extract_cited_indexes("参考 [ID:1] 与 [ID:2]，以及 [ID:١]");
        assert_eq!(cited, HashSet::from([1, 2, 1]));
    }

    #[test]
    fn normalize_sql_strips_think_blocks_fences_and_trailing_semicolon() {
        let cleaned = normalize_sql(
            "```sql\n</think>\n思考过程略\nSELECT name FROM t WHERE kb_id = 'x';\n```",
        );
        assert_eq!(cleaned, "SELECT name FROM t WHERE kb_id = 'x'");
    }

    #[test]
    fn normalize_sql_preserves_inner_semicolons_and_plain_sql() {
        assert_eq!(normalize_sql("SELECT a; b FROM t;"), "SELECT a; b FROM t");
        assert_eq!(normalize_sql("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn clean_tts_text_strips_control_chars_emoji_and_collapses_space() {
        let cleaned = clean_tts_text("你好😀，\u{0007}世界\u{0000}！  \n 下一句  ");
        assert_eq!(cleaned, "你好，世界！ 下一句");
    }

    #[test]
    fn clean_tts_text_caps_at_500_chars() {
        let long = "鱼".repeat(600);
        let cleaned = clean_tts_text(&long);
        assert_eq!(cleaned.chars().count(), 500);
    }
}
