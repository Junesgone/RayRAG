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


