//! Tokenizer — Unicode-aware token counting.
//!
//! Ported from RAGFlow's `rag/nlp/rag_tokenizer.py`.
//! Uses simple heuristic: ~4 chars/token for Latin, ~1.5 for CJK.

/// Count approximate tokens in text.
/// English/Latin: ~4 characters per token.
/// CJK: ~1.5 characters per token.
pub fn token_count(text: &str) -> usize {
    let cjk_chars = text.chars().filter(|c| is_cjk(*c)).count();
    let latin_chars = text
        .chars()
        .filter(|c| !is_cjk(*c) && !c.is_whitespace() && !c.is_ascii_punctuation())
        .count();
    let cjk_tokens = (cjk_chars as f64 / 1.5).ceil() as usize;
    let latin_tokens = (latin_chars as f64 / 4.0).ceil() as usize;
    let mut count = cjk_tokens + latin_tokens;
    // Ensure minimum of 1 for non-empty strings
    if count == 0 && !text.trim().is_empty() {
        count = 1;
    }
    count
}

/// Check if a character is CJK (Chinese/Japanese/Korean).
fn is_cjk(c: char) -> bool {
    matches!(
        c,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}'  // CJK Unified Ideographs Extension A
        | '\u{20000}'..='\u{2A6DF}' // CJK Unified Ideographs Extension B
        | '\u{3040}'..='\u{309F}'   // Hiragana
        | '\u{30A0}'..='\u{30FF}'   // Katakana
        | '\u{AC00}'..='\u{D7AF}'   // Hangul Syllables
    )
}


