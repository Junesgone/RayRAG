//! Retrieval memory — a central store of every raw chunk any claim has
//! retrieved — RAGFlow v0.27.2 `rag/advanced_rag/harness/memory.py`.
//!
//! Throughout a multi-hop answer the system retrieves many chunks (search is
//! cheap) but hands the LLM only a small narrowed slice. The raw chunks must
//! not be thrown away: `add` stores every chunk as returned, de-duplicated by
//! identity, BEFORE any narrowing; `search` is the consumer-facing,
//! language-agnostic relevance lookup used to REUSE evidence already retrieved
//! (no LLM, no knowledge-base call); `grep` is the loose-keyword primitive.

use std::collections::HashSet;

use regex::Regex;
use serde_json::{Value, json};

use crate::harness::grep_sed_narrow::escape_term;
use crate::harness::tools::text_processing::split_sentences;

/// How many memory chunks a single grep query may return at most.
pub const GREP_MAX_CHUNKS: usize = 6;
/// Max sentences kept per chunk (hit + context).
pub const GREP_MAX_SENTENCES: usize = 4;
/// Absolute char budget per side of a hit when expanding context.
pub const GREP_CONTEXT_CHARS: usize = 400;
/// Short chunks are kept whole.
pub const SHORT_CHUNK_CHARS: usize = 200;

fn is_cjk_char(ch: char) -> bool {
    matches!(ch as u32,
        0x4E00..=0x9FFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

fn is_cjk(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_cjk_char)
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `_chunk_key`: `chunk_id` or `id` (truthiness chain). The upstream fallback
/// is object identity; the Rust port keys anonymous chunks by their text hash
/// (values are cloned, so identity is unavailable), which only ever dedups
/// textually identical anonymous chunks.
pub fn chunk_key(chunk: &Value) -> String {
    for key in ["chunk_id", "id"] {
        if let Some(value) = chunk.get(key)
            && truthy(value)
        {
            return value_text(value);
        }
    }
    let text = chunk_text(chunk);
    format!("anon:{:016x}", xxhash_rust::xxh3::xxh3_64(text.as_bytes()))
}

/// `_chunk_text`: the searchable text of a chunk, preferring the raw original
/// (`content` first here — unlike the retrieval accessors).
pub fn chunk_text(chunk: &Value) -> String {
    for key in ["content", "content_with_weight"] {
        if let Some(value) = chunk.get(key)
            && truthy(value)
        {
            return value_text(value);
        }
    }
    String::new()
}

/// `_sentence_span_window`: the hit sentence plus up to one neighbour each side,
/// clamped by the context char budget.
fn sentence_span_window(sentences: &[String], index: usize) -> Vec<String> {
    let lo = index.saturating_sub(1);
    let hi = (sentences.len()).min(index + 2);
    let mut kept: Vec<String> = Vec::new();
    let mut total = 0usize;
    for sentence in &sentences[lo..hi] {
        total += sentence.chars().count();
        if total > GREP_CONTEXT_CHARS * 2 {
            break;
        }
        kept.push(sentence.clone());
    }
    if kept.is_empty() {
        kept.push(sentences[index].clone());
    }
    kept
}

/// `add`: merge raw retrieved chunks into the central memory store (lossless).
/// Returns how many new chunks were stored.
pub fn add(memory: &mut Vec<Value>, chunks: &[Value]) -> usize {
    if chunks.is_empty() {
        return 0;
    }
    let mut seen: HashSet<String> = memory.iter().map(chunk_key).collect();
    let mut added = 0usize;
    for chunk in chunks {
        if !chunk.is_object() || chunk_text(chunk).is_empty() {
            continue;
        }
        let key = chunk_key(chunk);
        if !seen.insert(key) {
            continue;
        }
        memory.push(chunk.clone());
        added += 1;
    }
    added
}

fn build_patterns(terms: &[String]) -> (Vec<Regex>, Vec<Regex>) {
    let mut patterns: Vec<Regex> = Vec::new();
    let mut prefix_patterns: Vec<Regex> = Vec::new();
    for term in terms {
        let fragment = escape_term(term);
        if !fragment.is_empty()
            && let Ok(pattern) = Regex::new(&format!("(?i){fragment}"))
        {
            patterns.push(pattern);
        }
        // Prefix fallback: morphological tolerance (gap "abbreviation" vs
        // chunk "abbreviated"). CJK terms are matched verbatim.
        let stripped = term.trim();
        let prefix: String = stripped.chars().take(5).collect();
        if stripped.chars().count() >= 6
            && !prefix.is_empty()
            && !is_cjk(&prefix)
            && let Ok(pattern) = Regex::new(&format!("(?i)\\b{}", regex::escape(&prefix)))
        {
            prefix_patterns.push(pattern);
        }
    }
    (patterns, prefix_patterns)
}

/// `grep`: memory chunks that contain any of `terms` (word-boundary grep with a
/// prefix fallback), narrowed to the matching sentence + small context.
pub fn grep(memory: &[Value], terms: &[String], limit: usize) -> Vec<Value> {
    if memory.is_empty() || terms.is_empty() {
        return Vec::new();
    }
    let (patterns, prefix_patterns) = build_patterns(terms);
    if patterns.is_empty() && prefix_patterns.is_empty() {
        return Vec::new();
    }
    let matches = |text: &str| -> bool {
        patterns.iter().any(|pattern| pattern.is_match(text))
            || prefix_patterns.iter().any(|pattern| pattern.is_match(text))
    };

    let mut hits: Vec<Value> = Vec::new();
    for chunk in memory {
        let text = chunk_text(chunk);
        if text.chars().count() <= SHORT_CHUNK_CHARS {
            if matches(&text) {
                hits.push(json!({
                    "content": text,
                    "doc_id": chunk.get("doc_id").cloned().unwrap_or(Value::Null),
                    "chunk_id": chunk.get("chunk_id").cloned().unwrap_or(Value::Null),
                }));
            }
            continue;
        }
        let sentences = split_sentences(&text);
        let mut kept: Vec<String> = Vec::new();
        for (index, sentence) in sentences.iter().enumerate() {
            if matches(sentence) {
                for window in sentence_span_window(&sentences, index) {
                    if !kept.contains(&window) {
                        kept.push(window);
                    }
                }
            }
            if kept.len() >= GREP_MAX_SENTENCES {
                break;
            }
        }
        if !kept.is_empty() {
            hits.push(json!({
                "content": kept.join("\n"),
                "doc_id": chunk.get("doc_id").cloned().unwrap_or(Value::Null),
                "chunk_id": chunk.get("chunk_id").cloned().unwrap_or(Value::Null),
            }));
        }
        if hits.len() >= limit {
            break;
        }
    }
    hits
}

/// `size`.
pub fn size(memory: &[Value]) -> usize {
    memory.len()
}

/// `clear`.
pub fn clear(memory: &mut Vec<Value>) {
    memory.clear();
}

// ─────────────────────────────────────────────────────────────────────────────
// Relevance-ranked retrieval over memory (retrieval-reuse cache, not noise).
// ─────────────────────────────────────────────────────────────────────────────

const STOPWORDS: [&str; 53] = [
    "what", "which", "how", "many", "much", "does", "did", "do", "the", "a", "an", "is", "are",
    "was", "were", "be", "been", "being", "of", "for", "to", "in", "on", "with", "and", "or", "by",
    "from", "at", "it", "its", "this", "that", "these", "those", "who", "when", "where", "why",
    "than", "then", "there", "their", "they", "them", "his", "her", "him", "she", "he", "we",
    "you", "your",
];

/// `_significant_terms`: language-agnostic significant-term extraction.
pub fn significant_terms(text: &str, max_terms: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let push = |token: String, out: &mut Vec<String>, seen: &mut HashSet<String>| {
        if !token.is_empty() && seen.insert(token.clone()) {
            out.push(token);
        }
    };

    // Numbers anywhere.
    for found in Regex::new(r"\d+").unwrap().find_iter(text) {
        push(found.as_str().to_string(), &mut out, &mut seen);
        if out.len() >= max_terms {
            return out;
        }
    }
    // CJK runs -> 3-grams (and the whole run if shorter than 3).
    let cjk_re = Regex::new(r"[\u{4E00}-\u{9FFF}\u{3040}-\u{30FF}\u{AC00}-\u{D7AF}]+").unwrap();
    for found in cjk_re.find_iter(text) {
        let run: Vec<char> = found.as_str().chars().collect();
        if run.len() < 3 {
            push(run.iter().collect(), &mut out, &mut seen);
        } else {
            for index in 0..=(run.len() - 3) {
                push(run[index..index + 3].iter().collect(), &mut out, &mut seen);
            }
        }
        if out.len() >= max_terms {
            return out;
        }
    }
    // Latin words (stopword-filtered).
    for found in Regex::new(r"[A-Za-z0-9]+").unwrap().find_iter(text) {
        let raw = found.as_str();
        if raw.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let low = raw.to_lowercase();
        if low.chars().count() >= 3 && !STOPWORDS.contains(&low.as_str()) {
            push(low, &mut out, &mut seen);
        }
        if out.len() >= max_terms {
            return out;
        }
    }
    out
}

/// `_term_hits`: how many of `terms` occur in `text`, language-agnostically.
fn term_hits(text: &str, terms: &[String]) -> usize {
    if terms.is_empty() {
        return 0;
    }
    let mut hits = 0usize;
    for term in terms {
        if is_cjk(term) {
            if text.contains(term.as_str()) {
                hits += 1;
            }
        } else if term.chars().all(|ch| ch.is_ascii_digit()) {
            if text.contains(term.as_str()) {
                hits += 1;
            }
        } else if Regex::new(&format!("(?i)\\b{}\\b", regex::escape(term)))
            .map(|pattern| pattern.is_match(text))
            .unwrap_or(false)
        {
            hits += 1;
        } else if term.chars().count() >= 6 {
            let prefix: String = term.chars().take(5).collect();
            if Regex::new(&format!("(?i)\\b{}", regex::escape(&prefix)))
                .map(|pattern| pattern.is_match(text))
                .unwrap_or(false)
            {
                hits += 1;
            }
        }
    }
    hits
}

/// `search`: relevance-ranked retrieval over memory for a query string.
pub fn search(
    memory: &[Value],
    query: &str,
    top_n: usize,
    _min_overlap: usize,
    min_ratio: f64,
) -> Vec<Value> {
    let terms = significant_terms(query, 18);
    if memory.is_empty() || terms.is_empty() {
        return Vec::new();
    }
    let denominator = terms.len() as f64;
    let mut scored: Vec<(usize, String, &Value)> = Vec::new();
    for chunk in memory {
        let text = chunk_text(chunk);
        if text.is_empty() {
            continue;
        }
        let hits = term_hits(&text, &terms);
        if hits >= 1 && (hits as f64 / denominator) >= min_ratio {
            scored.push((hits, text, chunk));
        }
    }
    if scored.is_empty() {
        return Vec::new();
    }
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| right.1.chars().count().cmp(&left.1.chars().count()))
    });
    scored
        .into_iter()
        .take(top_n)
        .map(|(hits, text, chunk)| {
            json!({
                "content": text,
                "doc_id": chunk.get("doc_id").cloned().unwrap_or(Value::Null),
                "chunk_id": chunk.get("chunk_id").cloned().unwrap_or(Value::Null),
                "similarity": hits as f64,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_dedups_and_grep_narrows() {
        let mut memory: Vec<Value> = Vec::new();
        let chunks = vec![
            json!({"chunk_id": "c1", "content": "Short one mentioning Rifampicin clearly."}),
            json!({"chunk_id": "c1", "content": "duplicate id"}),
            json!({"id": "c2", "content": "Sentence A about unrelated topics. The abbreviation RIF appears here for treatment. Another sentence follows with details. Yet more context sentences are present in this chunk to exceed the short threshold by a wide margin so that sentence-level narrowing applies to it. Final trailing sentence."}),
        ];
        assert_eq!(add(&mut memory, &chunks), 2);
        assert_eq!(size(&memory), 2);

        // `\bRIF\b` matches the abbreviation, not "Rifampicin".
        let hits = grep(&memory, &["RIF".to_string()], GREP_MAX_CHUNKS);
        assert_eq!(hits.len(), 1);
        assert!(
            hits[0]["content"]
                .as_str()
                .unwrap()
                .contains("abbreviation")
        );

        // A full word hits the short chunk that keeps its whole text.
        let hits = grep(&memory, &["Rifampicin".to_string()], GREP_MAX_CHUNKS);
        assert_eq!(hits.len(), 1);
        assert!(hits[0]["content"].as_str().unwrap().contains("Rifampicin"));

        // Prefix tolerance: "abbreviation" matches "abbreviated".
        let hits = grep(&memory, &["abbreviation".to_string()], GREP_MAX_CHUNKS);
        assert_eq!(hits.len(), 1);

        clear(&mut memory);
        assert_eq!(size(&memory), 0);
    }

    #[test]
    fn significant_terms_cover_numbers_cjk_and_words() {
        let terms = significant_terms("What is the population of 利福平 in 1999?", 18);
        assert!(terms.contains(&"1999".to_string()));
        assert!(terms.contains(&"population".to_string()));
        assert!(terms.contains(&"利福平".to_string()));
        assert!(!terms.contains(&"what".to_string()));
    }

    #[test]
    fn search_ranks_by_overlap_and_ratio() {
        let memory = vec![
            json!({"chunk_id": "a", "content": "Rifampicin abbreviated RIF treats tuberculosis infections."}),
            json!({"chunk_id": "b", "content": "Rifampicin only."}),
            json!({"chunk_id": "c", "content": "Nothing relevant at all here."}),
        ];
        let results = search(&memory, "Rifampicin abbreviated", 6, 2, 0.12);
        assert!(!results.is_empty());
        assert_eq!(results[0]["chunk_id"], json!("a"));
        assert!(results.iter().all(|row| row["chunk_id"] != json!("c")));

        // A query with no shared terms clears nothing.
        assert!(search(&memory, "zzz qqq", 6, 2, 0.12).is_empty());
    }
}
