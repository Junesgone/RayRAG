//! Shared sentence-boundary definition for the title and token chunkers —
//! RAGFlow v0.27.2 `rag/flow/chunker/_sentence_boundary.py`.
//!
//! Boundary re-split for an oversized chunk: Chinese/English period,
//! exclamation mark, question mark, the Chinese variants and the newline.
//! The capturing group keeps the boundary attached to the preceding sentence
//! so re-merged text preserves the original boundaries (matching
//! `re.split(r"(%s)" % pattern, text)`).

use regex::Regex;

/// Upstream `SENTENCE_BOUNDARY_PATTERN`.
pub const SENTENCE_BOUNDARY_PATTERN: &str = r"([。!?？；！\n]|\. )";

/// Upstream `SENTENCE_BOUNDARY_RE` (compiled with `re.DOTALL`, i.e. Rust's
/// `(?s)` flag, for exact parity even though the pattern has no `.` meta).
pub fn sentence_boundary_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(&format!("(?s){SENTENCE_BOUNDARY_PATTERN}")).expect("sentence boundary regex")
    })
}

/// Split `text` into sentences with every boundary **kept attached to its
/// preceding sentence** (the capture-group contract of the upstream split).
/// Text after the last boundary becomes the final sentence.
pub fn split_sentences_keep_boundaries(text: &str) -> Vec<String> {
    let regex = sentence_boundary_regex();
    let mut out = Vec::new();
    let mut cursor = 0usize;
    for boundary in regex.find_iter(text) {
        let end = boundary.end();
        if end > cursor {
            out.push(text[cursor..end].to_owned());
        }
        cursor = end;
    }
    if cursor < text.len() {
        out.push(text[cursor..].to_owned());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_matches_upstream_constant() {
        assert_eq!(SENTENCE_BOUNDARY_PATTERN, r"([。!?？；！\n]|\. )");
        let regex = sentence_boundary_regex();
        assert!(regex.is_match("你好。"));
        assert!(regex.is_match("Done!"));
        assert!(regex.is_match("End. Next"));
        assert!(regex.is_match("line\nnext"));
        assert!(!regex.is_match("no boundary here"));
    }

    #[test]
    fn split_keeps_boundaries_attached() {
        assert_eq!(
            split_sentences_keep_boundaries("第一句。第二句！\n第三句"),
            vec!["第一句。", "第二句！", "\n", "第三句"]
        );
        assert_eq!(
            split_sentences_keep_boundaries("End. Next"),
            vec!["End. ", "Next"]
        );
        assert_eq!(
            split_sentences_keep_boundaries("尾部边界。"),
            vec!["尾部边界。"]
        );
        assert!(split_sentences_keep_boundaries("").is_empty());
    }
}
