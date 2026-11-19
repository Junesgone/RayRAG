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
                    && let Some(ch) = char::from_u32(cp)
                {
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

/// Decode document bytes using the BOM order RAGFlow's decode_text applies:
/// UTF-32 BE/LE, UTF-16 BE/LE, UTF-8 BOM, then plain UTF-8. Invalid bytes are
/// reported with an actionable re-upload message instead of being replaced.
pub fn decode_text(data: &[u8]) -> Result<String> {
    if data.starts_with(&[0x00, 0x00, 0xFE, 0xFF]) {
        return decode_utf32(&data[4..], true);
    }
    if data.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
        return decode_utf32(&data[4..], false);
    }
    if data.starts_with(&[0xFE, 0xFF]) {
        return decode_utf16(&data[2..], true);
    }
    if data.starts_with(&[0xFF, 0xFE]) {
        return decode_utf16(&data[2..], false);
    }
    if let Some(payload) = data.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return std::str::from_utf8(payload)
            .map(str::to_owned)
            .map_err(|error| anyhow::anyhow!("UTF-8 decode error: {error}. {DECODE_GUIDANCE}"));
    }
    std::str::from_utf8(data).map(str::to_owned).map_err(|error| {
        anyhow::anyhow!(
            "Text decoding failed: the file is not valid UTF-8 and has no UTF-8/UTF-16/UTF-32 BOM ({error}). {DECODE_GUIDANCE}"
        )
    })
}

const DECODE_GUIDANCE: &str = "请将文件另存为 UTF-8 或带 BOM 的 UTF-16 后重新上传 / Save the file as UTF-8, or as UTF-16/UTF-32 with a BOM, then upload again.";

fn decode_utf16(data: &[u8], big_endian: bool) -> Result<String> {
    if data.len() % 2 != 0 {
        anyhow::bail!("UTF-16 decode error: the byte length is odd. {DECODE_GUIDANCE}");
    }
    let units = data
        .chunks_exact(2)
        .map(|chunk| {
            if big_endian {
                u16::from_be_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_le_bytes([chunk[0], chunk[1]])
            }
        })
        .collect::<Vec<_>>();
    String::from_utf16(&units)
        .map_err(|error| anyhow::anyhow!("UTF-16 decode error: {error}. {DECODE_GUIDANCE}"))
}

fn decode_utf32(data: &[u8], big_endian: bool) -> Result<String> {
    if data.len() % 4 != 0 {
        anyhow::bail!(
            "UTF-32 decode error: the byte length is not a multiple of four. {DECODE_GUIDANCE}"
        );
    }
    let mut text = String::with_capacity(data.len() / 2);
    for chunk in data.chunks_exact(4) {
        let value = if big_endian {
            u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        } else {
            u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])
        };
        let Some(character) = char::from_u32(value) else {
            anyhow::bail!(
                "UTF-32 decode error: U+{value:04X} is not a Unicode scalar value. {DECODE_GUIDANCE}"
            );
        };
        text.push(character);
    }
    Ok(text)
}


