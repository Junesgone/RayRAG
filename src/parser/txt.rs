//! Plain text parser — full port of RAGFlow `deepdoc/parser/txt_parser.py`.
//!
//! `RAGFlowTxtParser::parser_txt` splits text on a delimiter expression
//! (backtick-wrapped tokens plus literal characters), re-joins the captured
//! delimiters back into the stream via a capturing-group split, and accumulates
//! chunks until each exceeds `chunk_token_num` tokens (measured with the
//! project's `estimate_tokens` proxy for RAGFlow's `num_tokens_from_string`).
//!
//! The Python side also decodes the delimiter through
//! `encode('utf-8').decode('unicode_escape')` — i.e. `\\n` in the delimiter
//! string means a real newline. We do the same unicode-escape expansion here
//! for the common escapes (`\\n`, `\\t`, `\\r`, `\\\\`), so callers can pass
//! the same default `"\n!?;。；！？"`.

use crate::chunk::estimate_tokens;
use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use regex::Regex;

/// Expand Python-unicode-escape sequences in the delimiter string:
/// `\n` → LF, `\t` → TAB, `\r` → CR, `\\` → backslash, `\uXXXX` → char.
fn unicode_escape_expand(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some('u') => {
                let mut hex = String::new();
                for _ in 0..4 {
                    if let Some(&h) = chars.peek() {
                        hex.push(h);
                        chars.next();
                    }
                }
                if let Ok(cp) = u32::from_str_radix(&hex, 16)
                    && let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                    }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Port of `RAGFlowTxtParser::parser_txt` (txt_parser.py:29-67).
/// Returns `(content, "")` chunk pairs.
pub fn parser_txt(txt: &str, chunk_token_num: usize, delimiter: &str) -> Vec<(String, String)> {
    let mut cks: Vec<String> = vec![String::new()];
    let mut tk_nums: Vec<usize> = vec![0];

    let delimiter = unicode_escape_expand(delimiter);

    let add_chunk = |t: &str, cks: &mut Vec<String>, tk_nums: &mut Vec<usize>| {
        let tnum = estimate_tokens(t);
        if tk_nums.last().copied().unwrap_or(0) > chunk_token_num {
            cks.push(t.to_string());
            tk_nums.push(tnum);
        } else {
            if !cks.last().map(String::is_empty).unwrap_or(true) {
                let last = cks.last_mut().unwrap();
                last.push('\n');
                last.push_str(t);
            } else {
                cks.last_mut().unwrap().push_str(t);
            }
            let n = tk_nums.len();
            tk_nums[n - 1] += tnum;
        }
    };

    // Split delimiter into backtick tokens and literal runs.
    let mut dels: Vec<String> = Vec::new();
    let mut s = 0usize;
    let backtick = Regex::new(r"`([^`]+)`").unwrap();
    for m in backtick.find_iter(&delimiter) {
        let f = m.start();
        let t = m.end();
        let token = &delimiter[f + 1..t - 1]; // strip backticks
        dels.push(token.to_string());
        for ch in delimiter[s..f].chars() {
            dels.push(ch.to_string());
        }
        s = t;
    }
    if s < delimiter.len() {
        for ch in delimiter[s..].chars() {
            dels.push(ch.to_string());
        }
    }
    let dels: Vec<String> = dels
        .into_iter()
        .filter(|d| !d.is_empty())
        .map(|d| regex::escape(&d))
        .collect();
    let dels = dels.join("|");

    if dels.is_empty() {
        // No delimiters — whole text is one chunk.
        add_chunk(txt, &mut cks, &mut tk_nums);
        return cks.into_iter().map(|c| (c, String::new())).collect();
    }

    // Python: re.split(r"(%s)" % dels, txt) — the capturing group keeps the
    // delimiters in the result stream. Rust's Regex::split drops captures, so
    // we re-emit each match ourselves.
    let split_re = Regex::new(&format!("({dels})")).unwrap();
    let delims_only = Regex::new(&format!("^{dels}$")).unwrap();
    let mut last = 0usize;
    let mut parts: Vec<&str> = Vec::new();
    for m in split_re.find_iter(txt) {
        if m.start() > last {
            parts.push(&txt[last..m.start()]);
        }
        parts.push(m.as_str());
        last = m.end();
    }
    if last < txt.len() {
        parts.push(&txt[last..]);
    }
    for sec in parts {
        if delims_only.is_match(sec) {
            continue;
        }
        add_chunk(sec, &mut cks, &mut tk_nums);
    }

    cks.into_iter().map(|c| (c, String::new())).collect()
}

/// Compatibility `Parse` adapter — plain passthrough plus UTF-16 BOM support.
#[derive(Default)]
pub struct TxtParser;

impl TxtParser {
    pub fn new() -> Self {
        Self
    }
}

impl Parse for TxtParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = decode_text(data)?;
        Ok(new_document(name, content, "text/plain", data.len()))
    }
}

/// Decode bytes to UTF-8, falling back to UTF-16 LE when a BOM is present.
pub fn decode_text(data: &[u8]) -> Result<String> {
    String::from_utf8(data.to_vec()).or_else(|_| {
        if data.len() >= 2 {
            let bom = u16::from_le_bytes([data[0], data[1]]);
            if bom == 0xFEFF {
                let utf16: Vec<u16> = data[2..]
                    .chunks(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                return String::from_utf16(&utf16)
                    .map_err(|e| anyhow::anyhow!("UTF-16 decode error: {e}"));
            }
        }
        Err(anyhow::anyhow!("Failed to decode text"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_default_delimiters_and_accumulates() {
        // Default delimiter "\n!?;。；！？" → newline plus punctuation chars.
        let txt = "第一句。第二句！Third? Fourth; Fifth\n第六行";
        let chunks = parser_txt(txt, 128, "\n!?;。；！？");
        assert_eq!(chunks.len(), 1);
        // Capturing split keeps delimiters; add_chunk joins segments with \n.
        // Pure-delimiter segments are skipped by the `^dels$` guard — so the
        // punctuation itself is dropped (Python re.match alternation).
        assert!(chunks[0].0.contains("第一句"));
        assert!(!chunks[0].0.contains("。"));
        assert!(chunks[0].0.contains("第二句"));
        assert!(chunks[0].0.contains("Third"));
        assert!(chunks[0].0.contains("第六行"));
        assert_eq!(chunks[0].1, "");
    }

    #[test]
    fn exceeds_token_budget_starts_new_chunk() {
        // Tiny budget forces multiple chunks.
        let txt = "aaaa bbbb cccc dddd";
        let chunks = parser_txt(txt, 1, " ");
        assert!(chunks.len() >= 2);
        // First chunk begins with the first word.
        assert!(chunks[0].0.starts_with("aaaa"));
    }

    #[test]
    fn backtick_delimiters_are_skipped_as_pure_delimiter_segments() {
        // Backtick-wrapped tokens are treated as delimiters; the token text
        // is skipped by the `^dels$` guard (Python re.match semantics), while
        // surrounding segments are joined with \n.
        let txt = "left `sep` right";
        let chunks = parser_txt(txt, 128, "`sep`");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].0.contains("left"));
        assert!(chunks[0].0.contains("right"));
        assert!(!chunks[0].0.contains("sep"));
    }

    #[test]
    fn unicode_escape_expand_handles_common_escapes() {
        assert_eq!(unicode_escape_expand(r"\n"), "\n");
        assert_eq!(unicode_escape_expand(r"\t"), "\t");
        assert_eq!(unicode_escape_expand(r"\\"), "\\");
        assert_eq!(unicode_escape_expand(r"\u4e2d"), "中");
        assert_eq!(unicode_escape_expand("a\\nb"), "a\nb");
    }

    #[test]
    fn decode_utf16_bom_fallback() {
        let mut bytes = vec![0xFF, 0xFE];
        let text = "中文内容";
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        let decoded = decode_text(&bytes).unwrap();
        assert_eq!(decoded, "中文内容");
    }

    #[test]
    fn empty_delimiter_keeps_single_chunk() {
        let txt = "no delimiters here";
        let chunks = parser_txt(txt, 128, "");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, "no delimiters here");
    }
}
