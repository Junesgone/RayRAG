//! Memory utilities — search-result highlighting and aggregation.
//!
//! Rust ports of RAGFlow `memory/utils/highlight_utils.py` and
//! `memory/utils/aggregation_utils.py`. Pure functions, no I/O, so they can be
//! unit-tested without a live document store.

use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// Sentence separators used by RAGFlow's highlight splitter
/// (`re.split(r"[.?!;\n]", txt)`).
const SENTENCE_SEPARATORS: &[char] = &['.', '?', '!', ';', '\n'];

/// Word-boundary characters accepted around an English keyword match,
/// mirroring RAGFlow's character class
/// `(^|[ .?/'\"(),!:;-])(kw)([ .?/'\"(),!:;-]|$)`.
const WORD_BOUNDARY_CLASS: &str = "[ .?/'\"(),!:;-]";

fn em_pattern() -> &'static Regex {
    static EM: OnceLock<Regex> = OnceLock::new();
    EM.get_or_init(|| Regex::new(r"(?i)<em>[^<>]+</em>").expect("em regex is valid"))
}

/// Detect whether a sentence is English using an ASCII-letter heuristic.
/// RAGFlow passes a language-aware callback here; this is the portable default.
pub fn is_english_ascii(sentence: &str) -> bool {
    let letters = sentence.chars().filter(|c| c.is_alphabetic()).count();
    if letters == 0 {
        return true;
    }
    let ascii_letters = sentence.chars().filter(|c| c.is_ascii_alphabetic()).count();
    ascii_letters * 10 >= letters * 7 // >= 70% ASCII letters -> English
}

/// Wrap keyword matches in text with `<em>`, by sentence.
///
/// Mirrors RAGFlow `highlight_utils.highlight_text` exactly:
/// - `\r\n` is normalized to spaces first.
/// - The text is split on `. ? ! ; \n`; empty sentences are skipped.
/// - English sentences (or every sentence when `is_english` is `None`) use a
///   word-boundary regex so sub-word matches are not wrapped.
/// - Non-English sentences use literal replacement, longest keyword first,
///   and the replacement keeps the *keyword's* casing (RAGFlow behaviour).
/// - Only sentences that end up containing `<em>...</em>` are kept, joined
///   with `"..."`.
/// - When no sentence matches, the space-normalized original text is returned.
pub fn highlight_text(
    txt: &str,
    keywords: &[String],
    is_english: Option<fn(&str) -> bool>,
) -> String {
    if txt.is_empty() || keywords.is_empty() {
        return String::new();
    }
    let normalized = txt.replace(['\r', '\n'], " ");
    let mut kept: Vec<String> = Vec::new();
    for raw in normalized.split(SENTENCE_SEPARATORS) {
        let sentence = raw.trim();
        if sentence.is_empty() {
            continue;
        }
        let highlighted = if is_english.is_none_or(|f| f(sentence)) {
            highlight_english(sentence, keywords)
        } else {
            highlight_literal(sentence, keywords)
        };
        if em_pattern().is_match(&highlighted) {
            kept.push(highlighted);
        }
    }
    if kept.is_empty() {
        normalized
    } else {
        kept.join("...")
    }
}

/// Word-boundary highlighting: `(^|[ .?/'"(),!:;-])(kw)([ .?/'"(),!:;-]|$)`.
fn highlight_english(sentence: &str, keywords: &[String]) -> String {
    let mut out = sentence.to_string();
    for keyword in keywords {
        if keyword.is_empty() {
            continue;
        }
        let pattern = format!(
            r"(?i)(^|{class})({escaped})({class}|$)",
            class = WORD_BOUNDARY_CLASS,
            escaped = regex::escape(keyword),
        );
        let Ok(re) = Regex::new(&pattern) else {
            continue;
        };
        out = re.replace_all(&out, "$1<em>$2</em>$3").into_owned();
    }
    out
}

/// Literal highlighting with longest-keyword-first ordering; the replacement
/// keeps the keyword's own casing (RAGFlow uses `<em>{w}</em>`).
fn highlight_literal(sentence: &str, keywords: &[String]) -> String {
    let mut ordered: Vec<&String> = keywords.iter().filter(|k| !k.is_empty()).collect();
    ordered.sort_by_key(|keyword| std::cmp::Reverse(keyword.chars().count()));
    let mut out = sentence.to_string();
    for keyword in ordered {
        let Ok(re) = Regex::new(&format!("(?i){}", regex::escape(keyword))) else {
            continue;
        };
        let replacement = format!("<em>{}</em>", keyword.replace('$', "$$"));
        out = re.replace_all(&out, replacement).into_owned();
    }
    out
}

/// Build id -> highlighted text from a list of message documents.
///
/// Mirrors RAGFlow `highlight_utils.get_highlight_from_messages`: only
/// documents carrying a string `id` and a string `field_name` are considered,
/// and only results that actually contain an `<em>` match are returned.
pub fn get_highlight_from_messages(
    messages: &[Value],
    keywords: &[String],
    field_name: &str,
    is_english: Option<fn(&str) -> bool>,
) -> BTreeMap<String, String> {
    let mut ans = BTreeMap::new();
    if messages.is_empty() || keywords.is_empty() {
        return ans;
    }
    for doc in messages {
        let Some(doc_id) = doc.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(txt) = doc.get(field_name).and_then(Value::as_str) else {
            continue;
        };
        if txt.is_empty() {
            continue;
        }
        let highlighted = highlight_text(txt, keywords, is_english);
        if em_pattern().is_match(&highlighted) {
            ans.insert(doc_id.to_string(), highlighted);
        }
    }
    ans
}

/// Aggregate message documents by a field; returns [(value, count), ...].
///
/// Mirrors RAGFlow `aggregation_utils.aggregate_by_field`:
/// - Documents carrying both `value` and `count` are treated as
///   pre-aggregated rows and appended as-is, in encounter order.
/// - Otherwise the field value (String or list of String) is counted with
///   whitespace stripped; entries follow first-seen insertion order.
pub fn aggregate_by_field(messages: &[Value], field_name: &str) -> Vec<(String, usize)> {
    if messages.is_empty() {
        return Vec::new();
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<(String, usize)> = Vec::new();
    for doc in messages {
        if doc.get("value").is_some() && doc.get("count").is_some() {
            let value = doc
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let count = doc.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
            result.push((value, count));
            continue;
        }
        let Some(field) = doc.get(field_name) else {
            continue;
        };
        match field {
            Value::Array(items) => {
                for item in items {
                    if let Some(value) = item.as_str() {
                        push_count(&mut counts, &mut index, value.trim());
                    }
                }
            }
            Value::String(value) => push_count(&mut counts, &mut index, value.trim()),
            _ => {}
        }
    }
    if !counts.is_empty() {
        result.extend(counts);
    }
    result
}

fn push_count(counts: &mut Vec<(String, usize)>, index: &mut HashMap<String, usize>, key: &str) {
    if key.is_empty() {
        return;
    }
    if let Some(&position) = index.get(key) {
        counts[position].1 += 1;
    } else {
        index.insert(key.to_string(), counts.len());
        counts.push((key.to_string(), 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn highlight_english_uses_word_boundaries() {
        let keywords = vec!["rag".to_string()];
        // Match at a word boundary; sub-word occurrence ("storage") untouched.
        let out = highlight_text("RAG storage is great.", &keywords, None);
        assert!(out.contains("<em>RAG</em>"));
        assert!(!out.contains("<em>storage</em>"));
        // Punctuation-boundary match.
        let out = highlight_text("(rag)", &keywords, None);
        assert_eq!(out, "(<em>rag</em>)");
    }

    #[test]
    fn highlight_literal_is_longest_first_and_keeps_keyword_casing() {
        let keywords = vec!["数据".to_string(), "数据库".to_string()];
        let out = highlight_text("数据库是存储数据的地方", &keywords, Some(is_english_ascii));
        // Faithful to RAGFlow: longest keyword first, replacement keeps keyword
        // casing, and the second pass wraps the substring inside the first
        // <em> (nested tags — verified against highlight_utils.py).
        assert_eq!(out, "<em><em>数据</em>库</em>是存储<em>数据</em>的地方");
        // With the English word-boundary path (is_english = None), Chinese
        // keywords never touch a boundary char, so the text is returned
        // space-normalized but unwrapped (verified against highlight_utils.py).
        assert_eq!(
            highlight_text("数据库是存储数据的地方", &keywords, None),
            "数据库是存储数据的地方"
        );
    }

    #[test]
    fn highlight_keeps_only_matching_sentences() {
        let keywords = vec!["rust".to_string()];
        let txt = "Rust is fast. The weather is nice today. I love rust.";
        // Verified against highlight_utils.py: sentences are split on
        // `.?!;`, so the trailing period is dropped before joining with "...".
        let out = highlight_text(txt, &keywords, None);
        assert_eq!(out, "<em>Rust</em> is fast...I love <em>rust</em>");
        // No match at all -> space-normalized original.
        let out = highlight_text("Nothing to see here.", &keywords, None);
        assert_eq!(out, "Nothing to see here.");
        // Empty inputs -> empty string.
        assert_eq!(highlight_text("", &keywords, None), "");
        assert_eq!(highlight_text("abc", &[], None), "");
    }

    #[test]
    fn get_highlight_from_messages_filters_by_field_and_em() {
        let messages = vec![
            json!({"id": "c1", "content": "Rust powers RayRAG."}),
            json!({"id": "c2", "content": "No keywords inside."}),
            json!({"id": "c3"}), // missing content
        ];
        let out = get_highlight_from_messages(&messages, &["rust".to_string()], "content", None);
        assert_eq!(out.len(), 1);
        assert!(out["c1"].contains("<em>Rust</em>"));
        assert!(!out.contains_key("c2"));
        assert!(!out.contains_key("c3"));
    }

    #[test]
    fn aggregate_by_field_handles_rows_lists_and_preaggregated() {
        let messages = vec![
            json!({"value": "total", "count": 3}),
            json!({"tags": ["a", " b ", "a"]}),
            json!({"tags": "b"}),
            json!({"tags": ""}),
        ];
        let out = aggregate_by_field(&messages, "tags");
        assert_eq!(
            out,
            vec![
                ("total".to_string(), 3),
                ("a".to_string(), 2),
                ("b".to_string(), 2),
            ]
        );
        assert!(aggregate_by_field(&[], "tags").is_empty());
    }
}
