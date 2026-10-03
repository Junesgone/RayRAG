//! Keyword-driven text processing shared by the retrieval tools — RAGFlow
//! v0.27.2 `rag/advanced_rag/harness/tools/text_processing.py`.
//!
//! Sentence splitting, light stemming, and the keyword narrowing/highlighting
//! that keeps chunk payloads token-cheap: retrieval returns full chunks, and
//! narrowing cuts each one down to the sentences that actually carry the query
//! terms. Pure text work — no retrieval, no store access.

use std::collections::HashSet;

use regex::Regex;
use serde_json::Value;

/// `_compact_keywords`: dedupe and cap a keyword string (comma- or
/// space-separated) preserving first-seen order.
pub fn compact_keywords(keywords: &str, max_terms: usize) -> String {
    if keywords.is_empty() {
        return String::new();
    }
    let splitter = Regex::new(r"[,\s]+").unwrap();
    let mut seen: Vec<String> = Vec::new();
    for token in splitter.split(keywords.trim()) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if !seen.iter().any(|existing| existing == token) {
            seen.push(token.to_string());
        }
        if seen.len() >= max_terms {
            break;
        }
    }
    seen.join(" ")
}

/// Sentence terminators: Chinese `。！？；`, English `! ? ;`, and a
/// digit-guarded English period (so `3.14` / `v1.2` don't split). The
/// digit-guarded period is scanned manually because the Rust regex engine has
/// no lookaround.
pub fn split_plain(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut sentences: Vec<String> = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        let is_terminator = matches!(ch, '。' | '！' | '？' | '；' | '!' | '?' | ';');
        let period_terminator = ch == '.'
            && index > 0
            && !chars[index - 1].is_ascii_digit()
            && chars
                .get(index + 1)
                .map(|next| !next.is_ascii_digit())
                .unwrap_or(true);
        if is_terminator || period_terminator {
            let mut end = index + 1;
            while end < chars.len() {
                let next = chars[end];
                let next_is_terminator =
                    matches!(next, '。' | '！' | '？' | '；' | '!' | '?' | ';');
                let next_period = next == '.'
                    && end > 0
                    && !chars[end - 1].is_ascii_digit()
                    && chars
                        .get(end + 1)
                        .map(|after| !after.is_ascii_digit())
                        .unwrap_or(true);
                if next_is_terminator || next_period {
                    end += 1;
                } else {
                    break;
                }
            }
            let segment: String = chars[start..end].iter().collect();
            if !segment.trim().is_empty() {
                sentences.push(segment);
            }
            start = end;
            index = end;
        } else {
            index += 1;
        }
    }
    if start < chars.len() {
        let tail: String = chars[start..].iter().collect();
        if !tail.trim().is_empty() {
            sentences.push(tail);
        }
    }
    sentences
}

const HTML_BLOCK_TAGS: [&str; 39] = [
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "td",
    "th",
    "caption",
    "colgroup",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "div",
    "p",
    "pre",
    "blockquote",
    "section",
    "article",
    "aside",
    "nav",
    "main",
    "figure",
    "figcaption",
    "header",
    "footer",
    "address",
    "details",
    "summary",
    "form",
    "fieldset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
];

fn html_block_spans(text: &str) -> Vec<(usize, usize)> {
    let tag = Regex::new(r"<(/?)([a-zA-Z][a-zA-Z0-9]*)\b([^>]*)>").unwrap();
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut stack: Vec<(String, usize)> = Vec::new();
    for caps in tag.captures_iter(text) {
        let name = caps[2].to_lowercase();
        if !HTML_BLOCK_TAGS.contains(&name.as_str()) {
            continue;
        }
        let whole = caps.get(0).unwrap();
        let closing = !caps[1].is_empty();
        if closing {
            if let Some(position) = stack.iter().rposition(|(open, _)| open == &name) {
                let (_, start) = stack[position];
                stack.truncate(position);
                if stack.is_empty() {
                    spans.push((start, whole.end()));
                }
            }
        } else if !caps[3].trim_end().ends_with('/') {
            stack.push((name, whole.start()));
        }
    }
    spans
}

fn markdown_table_spans(text: &str) -> Vec<(usize, usize)> {
    let table = Regex::new(
        r"(?m)^[ \t]*\|?[^\n]*\|[^\n]*\r?\n[ \t]*\|?[ \t]*:?-{1,}:?[ \t]*(?:\|[ \t]*:?-{1,}:?[ \t]*)+\|?[ \t]*\r?\n(?:[ \t]*\|?[^\n]*\|[^\n]*\r?\n?)*",
    )
    .unwrap();
    table
        .find_iter(text)
        .map(|m| (m.start(), m.end()))
        .collect()
}

fn protected_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = html_block_spans(text);
    spans.extend(markdown_table_spans(text));
    spans.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    let mut last_end: Option<usize> = None;
    for (start, end) in spans {
        match last_end {
            Some(previous) if start < previous => {
                if end > previous {
                    if let Some(last) = merged.last_mut() {
                        last.1 = end;
                    }
                    last_end = Some(end);
                }
            }
            _ => {
                merged.push((start, end));
                last_end = Some(end);
            }
        }
    }
    merged
}

/// `_split_sentences`: split into sentences keeping each terminator attached;
/// block-level HTML elements and markdown tables stay atomic.
pub fn split_sentences(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let spans = protected_spans(text);
    if spans.is_empty() {
        return split_plain(text);
    }
    let mut sentences: Vec<String> = Vec::new();
    let mut position = 0usize;
    for (start, end) in spans {
        if start > position {
            sentences.extend(split_plain(&text[position..start]));
        }
        let block = &text[start..end];
        if !block.trim().is_empty() {
            sentences.push(block.to_string());
        }
        position = end;
    }
    if position < text.len() {
        sentences.extend(split_plain(&text[position..]));
    }
    sentences
}

// ---------------------------------------------------------------------------
// Stem-tolerant keyword matching (ported from agentic_search4 v8). The upstream
// module prefers nltk's PorterStemmer with a suffix-stripping fallback; the
// Rust port uses the fallback directly (nltk is a Python dependency).
// ---------------------------------------------------------------------------

const STEM_SUFFIXES: [(&str, &str); 12] = [
    ("ations", ""),
    ("ation", ""),
    ("ated", ""),
    ("ates", ""),
    ("ate", ""),
    ("ings", ""),
    ("ing", ""),
    ("ies", "i"),
    ("ied", "i"),
    ("ed", ""),
    ("es", ""),
    ("s", ""),
];

/// `_fallback_stem`: suffix stripper approximating Porter.
pub fn fallback_stem(word: &str) -> String {
    let mut stem = word.to_string();
    for (suffix, replacement) in STEM_SUFFIXES {
        if stem.ends_with(suffix) && stem.chars().count() - suffix.chars().count() >= 3 {
            stem = format!("{}{}", &stem[..stem.len() - suffix.len()], replacement);
            break;
        }
    }
    if stem.len() > 3 && stem.ends_with('y') {
        stem = format!("{}i", &stem[..stem.len() - 1]);
    }
    if stem.len() > 3 && stem.ends_with('e') {
        stem = stem[..stem.len() - 1].to_string();
    }
    let bytes = stem.as_bytes();
    if stem.len() > 3
        && bytes[bytes.len() - 1] == bytes[bytes.len() - 2]
        && !matches!(bytes[bytes.len() - 1], b'a' | b'e' | b'i' | b'o' | b'u')
    {
        stem = stem[..stem.len() - 1].to_string();
    }
    stem
}

/// `_stem` (the fallback path; see the module note).
pub fn stem(word: &str) -> String {
    fallback_stem(word)
}

/// `_stemmable`: only plain ASCII words are stemmed.
pub fn stemmable(token: &str) -> bool {
    token.len() >= 4 && token.is_ascii() && token.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn word_re() -> Regex {
    Regex::new(r"[a-z0-9]+").unwrap()
}

/// `_keyword_forms`: split keywords into verbatim substrings and stem sequences.
pub fn keyword_forms(keywords: &[String]) -> (Vec<String>, Vec<Vec<String>>) {
    let words = word_re();
    let mut verbatim: Vec<String> = Vec::new();
    let mut stemmed: Vec<Vec<String>> = Vec::new();
    for keyword in keywords {
        let lowered = keyword.trim().to_lowercase();
        if lowered.is_empty() {
            continue;
        }
        let tokens: Vec<String> = words
            .find_iter(&lowered)
            .map(|m| m.as_str().to_string())
            .collect();
        if !tokens.is_empty() && tokens.iter().all(|token| stemmable(token)) {
            stemmed.push(tokens.iter().map(|token| stem(token)).collect());
        } else {
            verbatim.push(lowered);
        }
    }
    (verbatim, stemmed)
}

/// `_sentence_stems`.
pub fn sentence_stems(sentence: &str) -> Vec<String> {
    word_re()
        .find_iter(&sentence.to_lowercase())
        .map(|m| {
            let token = m.as_str();
            if stemmable(token) {
                stem(token)
            } else {
                token.to_string()
            }
        })
        .collect()
}

/// `_sentence_matches`: verbatim substring or a contiguous stem run.
pub fn sentence_matches(
    low: &str,
    stems: &[String],
    verbatim: &[String],
    stemmed: &[Vec<String>],
) -> bool {
    if verbatim
        .iter()
        .any(|keyword| low.contains(keyword.as_str()))
    {
        return true;
    }
    for sequence in stemmed {
        let width = sequence.len();
        if width == 0 || stems.len() < width {
            continue;
        }
        for start in 0..=(stems.len() - width) {
            if stems[start..start + width] == sequence[..] {
                return true;
            }
        }
    }
    false
}

fn fact_re() -> Regex {
    Regex::new(
        r"(?i)(\d[\d,\.]*(?:st|nd|rd|th)?%?)|(19|20)\d{2}|\b(percent|percentage|million|billion|thousand|km|km2|sq\s*km|m\s*above|m)\b",
    )
    .unwrap()
}

fn proper_noun_re() -> Regex {
    Regex::new(r"\b[A-Z][a-z]{2,}\b").unwrap()
}

/// `_is_fact_dense_sentence`: number / year / percentage / proper noun.
pub fn is_fact_dense_sentence(sentence: &str) -> bool {
    let facts = fact_re();
    if facts.is_match(sentence) || facts.is_match(&sentence.to_lowercase()) {
        return true;
    }
    // The upstream lookbehind `(?<![.!?]\.)` is scanned manually: a proper
    // noun directly following an abbreviation dot (`U.S. Army`) is skipped.
    let chars: Vec<char> = sentence.chars().collect();
    for m in proper_noun_re().find_iter(sentence) {
        let start = sentence[..m.start()].chars().count();
        if start >= 2 {
            let previous = chars[start - 1];
            let before = chars[start - 2];
            if previous == '.' && matches!(before, '.' | '!' | '?') {
                continue;
            }
        }
        return true;
    }
    false
}

/// `_highlight_keywords`: star verbatim phrases (as one span) and stem-matched
/// words not already inside a phrase.
pub fn highlight_keywords(text: &str, keywords: &[String]) -> String {
    let mut phrases: Vec<String> = keywords
        .iter()
        .map(|keyword| keyword.trim().to_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect();
    phrases.sort();
    phrases.dedup();
    phrases.sort_by_key(|phrase| std::cmp::Reverse(phrase.len()));

    let mut terms: Vec<String> = phrases.clone();
    let (_, stemmed) = keyword_forms(keywords);
    let stem_set: HashSet<String> = stemmed.iter().flatten().cloned().collect();
    if !stem_set.is_empty() {
        for word in Regex::new(r"[A-Za-z]+").unwrap().find_iter(text) {
            let low = word.as_str().to_lowercase();
            if stemmable(&low)
                && stem_set.contains(&stem(&low))
                && !phrases.iter().any(|phrase| phrase.contains(&low))
            {
                terms.push(low);
            }
        }
    }
    if terms.is_empty() {
        return text.to_string();
    }
    terms.sort_by_key(|term| std::cmp::Reverse(term.len()));
    terms.dedup();
    let pattern = Regex::new(
        &terms
            .iter()
            .map(|term| regex::escape(term))
            .collect::<Vec<_>>()
            .join("|"),
    )
    .unwrap();
    pattern
        .replace_all(text, |caps: &regex::Captures| format!("*{}*", &caps[0]))
        .to_string()
}

/// `_narrow_content`: keep keyword sentences +/-2 neighbours (stem-tolerant),
/// always keep fact-dense sentences, return structured tables whole; `None`
/// when no keyword occurs anywhere.
pub fn narrow_content(content: &str, keywords: &[String]) -> Option<String> {
    let low_content = content.to_lowercase();
    if low_content.contains("<table") || low_content.contains("<tr") || low_content.contains("<td")
    {
        return Some(format!("...{}...", highlight_keywords(content, keywords)));
    }
    let pipe_rows = content
        .lines()
        .filter(|line| line.matches('|').count() >= 2)
        .count();
    if pipe_rows >= 3 {
        return Some(format!("...{}...", highlight_keywords(content, keywords)));
    }

    let sentences = split_sentences(content);
    if sentences.is_empty() {
        return None;
    }
    let (verbatim, stemmed) = keyword_forms(keywords);
    if verbatim.is_empty() && stemmed.is_empty() {
        return None;
    }
    let mut keep: HashSet<usize> = HashSet::new();
    let mut matched = false;
    for (index, sentence) in sentences.iter().enumerate() {
        let low = sentence.to_lowercase();
        if sentence_matches(&low, &sentence_stems(sentence), &verbatim, &stemmed) {
            matched = true;
            let start = index.saturating_sub(2);
            let end = (index + 3).min(sentences.len());
            keep.extend(start..end);
        } else if is_fact_dense_sentence(sentence) {
            let start = index.saturating_sub(1);
            let end = (index + 2).min(sentences.len());
            keep.extend(start..end);
        }
    }
    if !matched {
        return None;
    }
    let mut ordered: Vec<usize> = keep.into_iter().collect();
    ordered.sort();
    let narrowed: String = ordered
        .iter()
        .map(|index| sentences[*index].as_str())
        .collect::<Vec<_>>()
        .join("")
        .trim()
        .to_string();
    Some(format!("...{}...", highlight_keywords(&narrowed, keywords)))
}

/// `_narrow_by_keywords`: narrow each chunk and drop keyword-less chunks.
/// Mutates the kept chunks' content fields (upstream mutates in place).
pub fn narrow_by_keywords(chunks: &[Value], keywords: &str) -> Vec<Value> {
    if chunks.is_empty() {
        return Vec::new();
    }
    let mut kwds: Vec<String> = keywords
        .split(',')
        .map(|keyword| keyword.trim().to_lowercase())
        .filter(|keyword| !keyword.is_empty())
        .collect();
    if kwds.is_empty() {
        return chunks.to_vec();
    }
    if kwds.len() < 3 {
        let words: Vec<String> = keywords
            .split(' ')
            .map(|keyword| keyword.trim().to_lowercase())
            .filter(|keyword| !keyword.is_empty())
            .collect();
        let mut bigrams: Vec<String> = Vec::new();
        for index in 0..words.len().saturating_sub(1) {
            bigrams.push(format!("{} {}", words[index], words[index + 1]));
        }
        kwds = bigrams;
    }

    let mut out: Vec<Value> = Vec::new();
    let mut dedup: HashSet<String> = HashSet::new();
    for chunk in chunks {
        let content = chunk
            .get("content_with_weight")
            .or_else(|| chunk.get("content"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let Some(narrowed) = narrow_content(content, &kwds) else {
            continue;
        };
        let hash = format!("{:016x}", xxhash_rust::xxh3::xxh3_64(narrowed.as_bytes()));
        if !dedup.insert(hash) {
            continue;
        }
        let mut kept = chunk.clone();
        if let Some(object) = kept.as_object_mut() {
            object.insert(
                "content_with_weight".to_string(),
                Value::String(narrowed.clone()),
            );
            if object.contains_key("content") {
                object.insert("content".to_string(), Value::String(narrowed));
            }
            object.remove("highlight");
        }
        out.push(kept);
    }
    out
}

/// `_narrow_or_keep`: keyword narrowing, but keep the originals when narrowing
/// would drop everything.
pub fn narrow_or_keep(chunks: &[Value], keywords: &str, _label: &str) -> Vec<Value> {
    if keywords.is_empty() || chunks.is_empty() {
        return chunks.to_vec();
    }
    let narrowed = narrow_by_keywords(chunks, keywords);
    if !narrowed.is_empty() {
        return narrowed;
    }
    chunks.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn splits_sentences_with_guards_and_atomic_blocks() {
        let sentences = split_sentences("First one. Second 3.14 stays. Third!");
        assert_eq!(sentences.len(), 3);
        assert_eq!(sentences[1], " Second 3.14 stays.");

        let table = "intro. <table><tr><td>a</td></tr><tr><td>b</td></tr></table> outro.";
        let sentences = split_sentences(table);
        assert_eq!(sentences.len(), 3);
        assert!(sentences[1].starts_with("<table>"));

        let markdown = "before\n| a | b |\n| --- | --- |\n| 1 | 2 |\nafter";
        let sentences = split_sentences(markdown);
        assert!(
            sentences
                .iter()
                .any(|sentence| sentence.contains("| 1 | 2 |"))
        );
        assert_eq!(sentences.len(), 3);
    }

    #[test]
    fn stems_and_matches_inflections() {
        assert_eq!(stem("nominations"), "nomin");
        assert_eq!(stem("nominated"), "nomin");
        assert_eq!(stem("company"), "compani");
        let (verbatim, stemmed) = keyword_forms(&["nominations".to_string()]);
        assert!(verbatim.is_empty());
        assert_eq!(stemmed, vec![vec!["nomin".to_string()]]);
        assert!(sentence_matches(
            "was nominated three times",
            &sentence_stems("was nominated three times"),
            &verbatim,
            &stemmed
        ));
        // Identifiers stay verbatim.
        let (verbatim, stemmed) = keyword_forms(&["1344259".to_string()]);
        assert_eq!(verbatim, vec!["1344259".to_string()]);
        assert!(stemmed.is_empty());
    }

    #[test]
    fn narrows_and_highlights() {
        let content = "Unrelated opening sentence. The film was nominated three times. It won two awards. Random closing noise.";
        let narrowed = narrow_content(content, &["nominations".to_string()]).expect("matched");
        assert!(narrowed.starts_with("..."));
        assert!(narrowed.contains("*nominated*"));
        assert!(!narrow_content("nothing relevant here", &["zzz".to_string()]).is_some());

        let table = "| rank | city |\n| --- | --- |\n| 1 | Tokyo |\n| 2 | Lima |";
        let whole = narrow_content(table, &["tokyo".to_string()]).unwrap();
        assert!(whole.contains("Lima"), "tables stay whole");

        let chunks = vec![
            json!({"content_with_weight": content}),
            json!({"content_with_weight": "no keywords"}),
        ];
        let kept = narrow_by_keywords(&chunks, "nominations, awards, ceremony");
        assert_eq!(kept.len(), 1);
        assert!(
            kept[0]["content_with_weight"]
                .as_str()
                .unwrap()
                .contains('*')
        );

        // Keep-original fallback when nothing matches.
        let kept = narrow_or_keep(&chunks, "zzz", "test");
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn compacts_and_flags_fact_dense() {
        assert_eq!(compact_keywords("a, b a c", 10), "a b c");
        assert!(is_fact_dense_sentence("The population was 4,523 in 1999."));
        assert!(is_fact_dense_sentence("Atlanta Braves won."));
        assert!(!is_fact_dense_sentence("they went home quietly"));
    }
}
