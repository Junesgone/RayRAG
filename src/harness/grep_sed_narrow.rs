//! In-memory grep+sed narrowing engine — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/grep_sed_narrow.py`.
//!
//! Mirrors the Claude Code / Codex `grep` + `sed` workflow over retrieval
//! chunks held in memory: grep terms already produced by the main-analysis LLM
//! (entities, numbers, key phrases) become word-boundary regexes for locating;
//! simple string transforms narrow the text, dropping unrelated boilerplate
//! instead of crude head-truncation. Fallback chain (never drops the answer):
//! `narrow_by_terms` → no hits/terms → keyword sentence narrowing → original
//! chunks as-is.

use std::collections::HashSet;

use regex::Regex;
use serde_json::{Value, json};

use crate::harness::chunk_utils::chunk_text;
use crate::harness::tools::text_processing::{
    is_fact_dense_sentence, narrow_by_keywords, split_sentences,
};

/// Cost / safety caps.
pub const MAX_GREP_TERMS: usize = 16;
pub const MAX_CONTEXT: usize = 2;
pub const DEFAULT_OUT_CHARS_PER_CHUNK: usize = 1200;
pub const DEFAULT_OUT_TOTAL_CHARS: usize = 16000;
/// Head length kept per chunk when there is no match.
pub const HEAD_FALLBACK_CHARS: usize = 400;
/// Absolute char budget per side during context expansion.
pub const CONTEXT_CHAR_BUDGET: usize = 600;
/// Short chunks (<= this) are not narrowed — they are already 1-2 lines.
pub const MIN_NARROW_CHARS: usize = 200;

fn is_cjk_char(ch: char) -> bool {
    matches!(ch as u32,
        0x4E00..=0x9FFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

fn contains_cjk(text: &str) -> bool {
    text.chars().any(is_cjk_char)
}

/// `_escape_term`: escape a plain grep term into a safe, word-boundary regex
/// fragment. CJK terms never get `\b` (the upstream Python `\b` is ASCII-only);
/// terms shorter than 3 chars are matched without boundaries.
pub fn escape_term(term: &str) -> String {
    const TRIM: &str = " \t\r\n.,:;!?'\"()[]{}";
    let trimmed = term.trim_matches(|ch: char| TRIM.contains(ch));
    if trimmed.is_empty() {
        return String::new();
    }
    let escaped = regex::escape(trimmed);
    if contains_cjk(trimmed) {
        return escaped;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() >= 3 && chars[0].is_alphanumeric() && chars[chars.len() - 1].is_alphanumeric() {
        return format!(r"\b{escaped}\b");
    }
    escaped
}

/// `_terms_to_patterns`: one compiled regex per term (cap
/// [`MAX_GREP_TERMS`], case-insensitive).
pub fn terms_to_patterns(terms: &[String]) -> Vec<Regex> {
    let mut out: Vec<Regex> = Vec::new();
    for term in terms.iter().take(MAX_GREP_TERMS) {
        let fragment = escape_term(term);
        if fragment.is_empty() {
            continue;
        }
        if let Ok(pattern) = Regex::new(&format!("(?i){fragment}")) {
            out.push(pattern);
        }
    }
    out
}

/// `_line_spans`: line `(start, end)` byte spans, boundaries at `\n`.
pub fn line_spans(content: &str) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    for found in Regex::new("\n").unwrap().find_iter(content) {
        spans.push((start, found.start()));
        start = found.end();
    }
    if start <= content.len() {
        spans.push((start, content.len()));
    }
    if spans.is_empty() {
        spans.push((0, content.len()));
    }
    spans
}

fn char_prefix(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

/// `_exec_on_text`: term-grep + line-context expansion against one chunk's
/// text (mirrors `grep -n -C N`). Returns `(narrowed, matched)`.
pub fn exec_on_text(
    content: &str,
    patterns: &[Regex],
    before: usize,
    after: usize,
    out_chars_per_chunk: usize,
) -> (String, bool) {
    if content.is_empty() {
        return (String::new(), false);
    }
    // Step 1: locate matches (exact positions from the regex engine).
    let mut hit_ranges: Vec<(usize, usize)> = Vec::new();
    for pattern in patterns {
        for found in pattern.find_iter(content) {
            hit_ranges.push((found.start(), found.end()));
        }
    }
    if hit_ranges.is_empty() {
        // Keep fact-dense sentences to avoid dropping numbers/entities.
        let kept: Vec<String> = split_sentences(content)
            .into_iter()
            .filter(|sentence| is_fact_dense_sentence(sentence))
            .collect();
        let narrowed = kept.join("").trim().to_string();
        if !narrowed.is_empty() {
            return (char_prefix(&narrowed, HEAD_FALLBACK_CHARS * 4), false);
        }
        return (char_prefix(content, HEAD_FALLBACK_CHARS), false);
    }

    // Step 2: merge overlapping/adjacent matches, expand to line range + context.
    hit_ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in hit_ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => {
                last.1 = last.1.max(end);
            }
            _ => merged.push((start, end)),
        }
    }
    let lines = line_spans(content);
    let mut expanded: Vec<(usize, usize)> = Vec::new();
    for (start, end) in merged {
        let mut lo = 0usize;
        let mut hi = 0usize;
        for (index, (line_start, line_end)) in lines.iter().enumerate() {
            if start >= *line_start && start < *line_end {
                lo = index;
            }
            if end > *line_start && end <= *line_end {
                hi = index;
            }
        }
        lo = lo.saturating_sub(before);
        hi = (hi + after).min(lines.len().saturating_sub(1));
        let mut frag_start = lines[lo].0;
        let mut frag_end = lines[hi].1;
        // Per-side character budget fallback.
        if frag_end - frag_start > CONTEXT_CHAR_BUDGET * 2
            && (frag_end - frag_start) > (end - start)
        {
            frag_start = start.saturating_sub(CONTEXT_CHAR_BUDGET);
            frag_end = (end + CONTEXT_CHAR_BUDGET).min(content.len());
        }
        expanded.push((frag_start, frag_end));
    }

    // Step 3: dedupe, join, truncate.
    let mut seen: HashSet<String> = HashSet::new();
    let mut out_parts: Vec<String> = Vec::new();
    for (start, end) in expanded {
        let part = content[start..end].trim().to_string();
        if part.is_empty() {
            continue;
        }
        let key = char_prefix(&part, 200);
        if !seen.insert(key) {
            continue;
        }
        out_parts.push(part);
    }
    let mut narrowed = out_parts.join("\n\n").trim().to_string();
    if narrowed.chars().count() > out_chars_per_chunk {
        narrowed = char_prefix(&narrowed, out_chars_per_chunk);
    }
    if narrowed.is_empty() {
        narrowed = char_prefix(content, HEAD_FALLBACK_CHARS);
    }
    (narrowed, true)
}

/// `_apply_narrow`: write the narrowed text back on matched chunks.
fn apply_narrow(chunks: &[Value], kept_texts: &[String], matched: &[bool]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for ((chunk, text), ok) in chunks.iter().zip(kept_texts.iter()).zip(matched.iter()) {
        let mut item = chunk.clone();
        if *ok && let Some(object) = item.as_object_mut() {
            object.insert(
                "content_with_weight".to_string(),
                Value::String(text.clone()),
            );
            if object.contains_key("content") {
                object.insert("content".to_string(), Value::String(text.clone()));
            }
            object.remove("highlight");
        }
        out.push(item);
    }
    out
}

/// `_fallback_narrow_by_keywords`.
fn fallback_narrow_by_keywords(chunks: &[Value], keywords: &str) -> Vec<Value> {
    let narrowed = narrow_by_keywords(chunks, keywords);
    if narrowed.is_empty() {
        chunks.to_vec()
    } else {
        narrowed
    }
}

/// `narrow_by_terms`: narrow retrieval chunks by locating grep terms. Returns
/// `(kept, stats)`; `stats.matched` is false when narrowing was abandoned (the
/// chunks are returned untouched). Never fails.
pub fn narrow_by_terms(
    chunks: &[Value],
    terms: &[String],
    fallback_terms: Option<&[String]>,
    before: Option<i64>,
    after: Option<i64>,
    keywords: &str,
    max_out_chars_per_chunk: usize,
    max_out_total_chars: usize,
) -> (Vec<Value>, Value) {
    let clamp = |value: Option<i64>| -> usize {
        value
            .map(|value| value.clamp(0, MAX_CONTEXT as i64) as usize)
            .unwrap_or(0)
    };
    let before = clamp(before);
    let after = clamp(after);

    let patterns = terms_to_patterns(terms);
    let mut stats = json!({
        "chunks_in": chunks.len(),
        "chunks_kept": 0,
        "chars_in": chunks.iter().map(|chunk| chunk_text(chunk).chars().count()).sum::<usize>(),
        "chars_out": 0,
        "matched": false,
        "used_terms": patterns.len(),
    });
    if chunks.is_empty() {
        return (Vec::new(), stats);
    }
    if patterns.is_empty() {
        let narrowed = fallback_narrow_by_keywords(chunks, keywords);
        stats["chunks_kept"] = json!(narrowed.len());
        stats["chars_out"] = json!(
            narrowed
                .iter()
                .map(|chunk| chunk_text(chunk).chars().count())
                .sum::<usize>()
        );
        return (narrowed, stats);
    }

    let run = |active: &[Regex]| -> (Vec<String>, Vec<bool>) {
        let mut texts: Vec<String> = Vec::new();
        let mut flags: Vec<bool> = Vec::new();
        for chunk in chunks {
            let raw = chunk_text(chunk);
            if raw.chars().count() <= MIN_NARROW_CHARS {
                texts.push(raw);
                flags.push(true);
                continue;
            }
            let (text, ok) = exec_on_text(&raw, active, before, after, max_out_chars_per_chunk);
            texts.push(text);
            flags.push(ok);
        }
        (texts, flags)
    };

    let (mut kept_texts, mut matched_flags) = run(&patterns);
    if let Some(fallback) = fallback_terms
        && !matched_flags.iter().any(|flag| *flag)
    {
        let fallback_patterns = terms_to_patterns(fallback);
        if !fallback_patterns.is_empty() {
            let (texts, flags) = run(&fallback_patterns);
            kept_texts = texts;
            matched_flags = flags;
            stats["used_terms"] = json!(
                stats["used_terms"]
                    .as_u64()
                    .unwrap_or(0)
                    .max(fallback_patterns.len() as u64)
            );
        }
    }

    let mut kept = apply_narrow(chunks, &kept_texts, &matched_flags);
    // Only apply the total-length cap when the grep actually matched.
    if matched_flags.iter().any(|flag| *flag) {
        let total_out: usize = kept
            .iter()
            .map(|chunk| chunk_text(chunk).chars().count())
            .sum();
        if total_out > max_out_total_chars {
            let per_chunk_cap =
                200usize.max(max_out_chars_per_chunk.min(max_out_total_chars / kept.len().max(1)));
            let mut acc = 0usize;
            let mut trimmed: Vec<Value> = Vec::new();
            for chunk in &kept {
                let text = chunk_text(chunk);
                let room = max_out_total_chars.saturating_sub(acc);
                if room == 0 {
                    break;
                }
                let take = text.chars().count().min(per_chunk_cap).min(room);
                if take == 0 {
                    break;
                }
                if take < text.chars().count() {
                    let mut item = chunk.clone();
                    let truncated = char_prefix(&text, take);
                    if let Some(object) = item.as_object_mut() {
                        object.insert(
                            "content_with_weight".to_string(),
                            Value::String(truncated.clone()),
                        );
                        if object.contains_key("content") {
                            object.insert("content".to_string(), Value::String(truncated));
                        }
                    }
                    trimmed.push(item);
                } else {
                    trimmed.push(chunk.clone());
                }
                acc += take;
            }
            kept = trimmed;
        }
    }

    stats["chunks_kept"] = json!(kept.len());
    stats["chars_out"] = json!(
        kept.iter()
            .map(|chunk| chunk_text(chunk).chars().count())
            .sum::<usize>()
    );
    stats["matched"] = json!(matched_flags.iter().any(|flag| *flag));
    (kept, stats)
}

const FALLBACK_STOPWORDS: [&str; 44] = [
    "what",
    "which",
    "who",
    "where",
    "when",
    "how",
    "the",
    "a",
    "an",
    "of",
    "in",
    "on",
    "for",
    "to",
    "and",
    "or",
    "with",
    "is",
    "are",
    "was",
    "were",
    "list",
    "name",
    "give",
    "find",
    "tell",
    "me",
    "about",
    "from",
    "that",
    "this",
    "it",
    "its",
    "their",
    "they",
    "have",
    "has",
    "do",
    "does",
    "did",
    "based",
    "per",
    "according",
    "not",
];

/// `split_fallback_terms`: split free text into fallback grep terms (zero LLM).
pub fn split_fallback_terms(texts: &[String]) -> Vec<String> {
    let splitter = Regex::new(r"[\n。；;,.?!?]+").unwrap();
    let mut terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for text in texts {
        for part in splitter.split(text) {
            let trimmed = part
                .trim()
                .trim_matches(|ch: char| "'\"()[]{}".contains(ch));
            if trimmed.is_empty() || trimmed.chars().count() < 3 {
                continue;
            }
            if FALLBACK_STOPWORDS.contains(&trimmed.to_lowercase().as_str()) {
                continue;
            }
            if !seen.insert(trimmed.to_string()) {
                continue;
            }
            terms.push(trimmed.to_string());
        }
    }
    terms.truncate(MAX_GREP_TERMS);
    terms
}

/// `grep_sed_narrow`: narrow chunks by grepping terms extracted directly from
/// the claim (zero LLM).
pub fn grep_sed_narrow(
    chunks: &[Value],
    claim_sources: &[String],
    max_out_chars_per_chunk: usize,
    max_out_total_chars: usize,
) -> (Vec<Value>, Value) {
    let mut stats = json!({
        "chunks_in": chunks.len(),
        "chunks_kept": 0,
        "chars_in": chunks.iter().map(|chunk| chunk_text(chunk).chars().count()).sum::<usize>(),
        "chars_out": 0,
        "matched": false,
        "used_terms": 0,
    });
    if chunks.is_empty() {
        return (chunks.to_vec(), stats);
    }
    let terms = split_fallback_terms(claim_sources);
    stats["used_terms"] = json!(terms.len());
    narrow_by_terms(
        chunks,
        &terms,
        None,
        None,
        None,
        &claim_sources.join(" "),
        max_out_chars_per_chunk,
        max_out_total_chars,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_terms_respect_cjk_and_short_tokens() {
        assert_eq!(escape_term("  \"Alpha\"  "), r"\bAlpha\b");
        assert_eq!(escape_term("ab"), "ab");
        assert_eq!(escape_term("天津"), "天津");
        assert_eq!(escape_term("  "), "");
    }

    #[test]
    fn exec_expands_lines_with_context() {
        let content = "line one filler filler filler\nline two has Target here\nline three filler filler\nline four filler filler";
        let patterns = terms_to_patterns(&["Target".to_string()]);
        let (narrowed, matched) = exec_on_text(content, &patterns, 1, 1, 1200);
        assert!(matched);
        assert!(narrowed.contains("line one"));
        assert!(narrowed.contains("line three"));
        assert!(!narrowed.contains("line four"));
    }

    #[test]
    fn no_hit_keeps_fact_dense_sentences() {
        let content = "Nothing here matters at all. The population was 4,523 in 1999. More filler without signals.";
        let patterns = terms_to_patterns(&["zzz".to_string()]);
        let (narrowed, matched) = exec_on_text(content, &patterns, 0, 0, 1200);
        assert!(!matched);
        assert!(narrowed.contains("4,523"));
    }

    #[test]
    fn narrow_by_terms_matches_and_keeps_original_on_miss() {
        let chunks = vec![
            json!({"content_with_weight": "The film was nominated for awards in 1999 by critics everywhere. It also won other prizes at several ceremonies that year."}),
            json!({"content_with_weight": "unrelated filler text about something else entirely that keeps going and going with no numbers or names at all, just plain words repeated to exceed the minimum narrow threshold, and then some more words to be safe about the length requirement. Plus one final clause to close it out."}),
        ];
        let (kept, stats) = narrow_by_terms(
            &chunks,
            &["nominated".to_string()],
            None,
            None,
            None,
            "nominated",
            1200,
            16000,
        );
        assert_eq!(kept.len(), 2);
        assert_eq!(stats["matched"], json!(true));

        // Primary terms miss -> fallback terms are tried mechanically.
        let (kept, stats) = narrow_by_terms(
            &chunks,
            &["zzz".to_string()],
            Some(&["critics".to_string()]),
            None,
            None,
            "critics",
            1200,
            16000,
        );
        assert_eq!(stats["matched"], json!(true));
        assert_eq!(kept.len(), 2);

        // Everything misses, no fallback -> originals untouched, matched=false.
        let (kept, stats) = narrow_by_terms(
            &chunks[1..2].to_vec(),
            &["zzz".to_string()],
            None,
            None,
            None,
            "",
            1200,
            16000,
        );
        assert_eq!(stats["matched"], json!(false));
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn fallback_terms_split_and_filter() {
        let terms = split_fallback_terms(&["What is the capital of France, Paris?".to_string()]);
        // Terms are whole comma/sentence parts (multi-word phrases stay whole).
        assert!(terms.iter().any(|term| term.contains("capital")));
        assert!(terms.contains(&"Paris".to_string()));
        assert!(!terms.iter().any(|term| term.eq_ignore_ascii_case("what")));
    }
}
