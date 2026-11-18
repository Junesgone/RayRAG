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




