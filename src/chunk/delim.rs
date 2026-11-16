//! Canonical `parser_config.delimiter` parser — RAGFlow v0.27.2
//! `rag/nlp/delim.py`.
//!
//! The delimiter field grammar:
//!
//! ```text
//! delimiter_field := token*
//! token           := backtick_wrapped | bare_char
//! backtick_wrapped := "`" bare_char+ "`"
//! bare_char        := any single Unicode character except "`"
//! ```
//!
//! Semantics (matching upstream):
//! 1. Characters between matching backticks form one multi-character
//!    delimiter; every character outside backticks is its own delimiter.
//! 2. Delimiters are deduplicated (insertion order) and stably sorted
//!    longest-first, so `##` matches before `#`.
//! 3. CRLF and standalone CR are normalized to LF before parsing.
//! 4. Matching is case-sensitive (no `re.I`).
//!
//! Bounded divergence: RayRAG's `ParserConfig::default().delimiter` stays
//! `"\n"`; the upstream canonical default `DEFAULT_DELIMITER` is exported for
//! call sites that want it, and switching the shared default is queued.

use regex::Regex;

/// Single source of truth for the upstream default delimiter field value
/// (txt/markdown parsers): newline, `!`, `?`, `;`, and the full-width
/// Chinese equivalents.
pub const DEFAULT_DELIMITER: &str = "\n!?;。；！？";

/// `normalize_text_newlines`: CRLF and standalone CR → LF.
pub fn normalize_text_newlines(text: &str) -> String {
    if text.is_empty() {
        return text.to_owned();
    }
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn backtick_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`([^`]+)`").expect("backtick delimiter regex"))
}

/// `has_wrapped_delimiter`: the field contains at least one backtick token.
pub fn has_wrapped_delimiter(field: &str) -> bool {
    !field.is_empty() && backtick_regex().is_match(field)
}

/// `parse_delimiter_field`: bare characters plus backtick-wrapped tokens,
/// deduplicated in insertion order and stably sorted longest-first.
pub fn parse_delimiter_field(field: &str) -> Vec<String> {
    if field.is_empty() {
        return Vec::new();
    }
    let normalized = normalize_text_newlines(field);
    let mut delimiters: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cursor = 0usize;
    for capture in backtick_regex().captures_iter(&normalized) {
        let Some(full) = capture.get(0) else {
            continue;
        };
        for ch in normalized[cursor..full.start()].chars() {
            let value = ch.to_string();
            if seen.insert(value.clone()) {
                delimiters.push(value);
            }
        }
        let token = capture
            .get(1)
            .map(|group| group.as_str())
            .unwrap_or_default();
        if !token.is_empty() && seen.insert(token.to_owned()) {
            delimiters.push(token.to_owned());
        }
        cursor = full.end();
    }
    for ch in normalized[cursor..].chars() {
        let value = ch.to_string();
        if seen.insert(value.clone()) {
            delimiters.push(value);
        }
    }
    // Stable sort by character count, longest-first (Python `len(str)`
    // semantics; `String::len()` would order by UTF-8 byte length instead).
    delimiters.sort_by(|left, right| right.chars().count().cmp(&left.chars().count()));
    delimiters
}

/// `compile_delimiter_pattern`: `re.escape`-joined alternation, empty when no
/// delimiters are present. Intended for `re.split(r"(%s)" % pattern, text)`.
pub fn compile_delimiter_pattern(delimiters: &[String]) -> String {
    let escaped: Vec<String> = delimiters
        .iter()
        .filter(|delimiter| !delimiter.is_empty())
        .map(|delimiter| regex::escape(delimiter))
        .collect();
    escaped.join("|")
}

/// Compiled `(pattern)` regex for a delimiter field, or `None` when the field
/// yields no delimiters.
#[cfg_attr(not(test), allow(dead_code))]
pub fn delimiter_regex(field: &str) -> Option<Regex> {
    let pattern = compile_delimiter_pattern(&parse_delimiter_field(field));
    if pattern.is_empty() {
        return None;
    }
    Regex::new(&format!("({pattern})")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_wrapped_tokens_longest_first() {
        assert_eq!(
            parse_delimiter_field("#`##`\n"),
            vec!["##".to_owned(), "#".to_owned(), "\n".to_owned()]
        );
        assert_eq!(
            parse_delimiter_field("ab"),
            vec!["a".to_owned(), "b".to_owned()]
        );
        assert!(parse_delimiter_field("").is_empty());
        assert_eq!(parse_delimiter_field("`||`"), vec!["||".to_owned()]);
    }

    #[test]
    fn dedupes_and_normalizes_newlines() {
        assert_eq!(
            parse_delimiter_field("aab"),
            vec!["a".to_owned(), "b".to_owned()]
        );
        assert_eq!(parse_delimiter_field("\r\n\r"), vec!["\n".to_owned()]);
        assert_eq!(normalize_text_newlines("x\r\ny\rz"), "x\ny\nz");
    }

    #[test]
    fn wrapped_detection_and_pattern_compilation() {
        assert!(has_wrapped_delimiter("a`b`"));
        assert!(!has_wrapped_delimiter("ab"));
        assert!(!has_wrapped_delimiter(""));
        // `regex::escape` also escapes `#` (verbose-mode meta), so assert
        // matching semantics rather than the exact escaped string.
        let pattern = compile_delimiter_pattern(&["##".to_owned(), "#".to_owned()]);
        assert!(!pattern.is_empty());
        let combined = Regex::new(&format!("({pattern})")).expect("combined regex");
        assert!(combined.is_match("a##b"));
        assert!(combined.is_match("a#b"));
        assert!(compile_delimiter_pattern(&[]).is_empty());
        let regex = delimiter_regex("`::`").expect("regex");
        assert!(regex.is_match("a::b"));
        assert!(delimiter_regex("").is_none());
    }

    #[test]
    fn default_delimiter_matches_upstream_constant() {
        assert_eq!(DEFAULT_DELIMITER, "\n!?;。；！？");
        let parsed = parse_delimiter_field(DEFAULT_DELIMITER);
        assert_eq!(parsed.len(), 8, "newline plus seven punctuation marks");
        assert_eq!(parsed[0], "\n");
        assert!(parsed.contains(&"。".to_owned()));
    }
}
