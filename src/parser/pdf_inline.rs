//! Inline image removal for PDF content streams.
//!
//! lopdf's content parser has no special case for BI ... ID <bytes> EI: the
//! image payload is tokenised as operators and operands, so binary data that
//! happens to contain '(' ')' or '<' can make a whole page fail to decode, and
//! bytes that happen to look like "(...) Tj" can inject text. pdfminer, which
//! RAGFlow uses for PDF text, treats the span as an opaque object and skips it.
//!
//! This module strips those spans before Content::decode. It mirrors the
//! pdfminer rules for the end marker: /L when it agrees with the stream,
//! "~>" for ASCII85 images, otherwise "EI" followed by whitespace.

use crate::Result;
use std::borrow::Cow;

/// Remove inline image objects from a decoded content stream.
///
/// Returns the sanitized bytes plus whether an inline image was found, so a
/// caller that reports "page contains images but no text" keeps that signal.
pub fn strip_inline_images<'a>(content: &'a [u8], what: &str) -> Result<(Cow<'a, [u8]>, bool)> {
    let mut out: Option<Vec<u8>> = None;
    let mut copied = 0usize;
    let mut i = 0usize;
    let mut found = false;
    while i < content.len() {
        match content[i] {
            b'(' => {
                i = skip_literal_string(content, i, what)?;
                continue;
            }
            b'%' => {
                i = skip_comment(content, i);
                continue;
            }
            b'<' if content.get(i + 1) != Some(&b'<') => {
                i = skip_hex_string(content, i, what)?;
                continue;
            }
            _ => {}
        }
        if !token_at(content, i, b"BI") {
            i += 1;
            continue;
        }

        found = true;
        let dict_start = i + 2;
        let mut cursor = dict_start;
        let dict_end;
        loop {
            while cursor < content.len() && (pdf_space(content[cursor]) || content[cursor] == b'%')
            {
                cursor = if content[cursor] == b'%' {
                    skip_comment(content, cursor)
                } else {
                    cursor + 1
                };
            }
            if cursor >= content.len() {
                anyhow::bail!("{what} inline image dictionary has no ID operator");
            }
            if token_at(content, cursor, b"ID") {
                dict_end = cursor;
                cursor += 2;
                break;
            }
            if token_at(content, cursor, b"BI") {
                anyhow::bail!("{what} has a nested inline image before its ID operator");
            }
            let next = skip_content_token(content, cursor, what)?;
            anyhow::ensure!(next > cursor, "{what} inline image dictionary is truncated");
            cursor = next;
        }
        if cursor >= content.len() || !pdf_space(content[cursor]) {
            anyhow::bail!("{what} inline image ID is not followed by whitespace");
        }
        let data_start = cursor + 1;
        let (length, ascii85) = inline_image_settings(&content[dict_start..dict_end]);
        let end = skip_inline_image_data(content, data_start, length, ascii85, what);

        let buffer = out.get_or_insert_with(|| Vec::with_capacity(content.len()));
        buffer.extend_from_slice(&content[copied..i]);
        // Keep the surrounding operators separated by one whitespace token.
        buffer.push(b' ');
        copied = end;
        i = end;
    }
    match out {
        Some(mut buffer) => {
            buffer.extend_from_slice(&content[copied..]);
            Ok((Cow::Owned(buffer), found))
        }
        None => Ok((Cow::Borrowed(content), false)),
    }
}

fn pdf_space(byte: u8) -> bool {
    matches!(byte, 0 | 9 | 10 | 12 | 13 | 32)
}

fn pdf_delimiter(byte: u8) -> bool {
    pdf_space(byte)
        || matches!(
            byte,
            b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
        )
}

/// True when token occurs at at as an operator, not as a name suffix.
fn token_at(content: &[u8], at: usize, token: &[u8]) -> bool {
    let end = at.saturating_add(token.len());
    if end > content.len() || &content[at..end] != token {
        return false;
    }
    // /BI and /ID are names, not operators. Without this check a dictionary
    // key such as /ID would be mistaken for the inline image data marker.
    if at > 0 && content[at - 1] == b'/' {
        return false;
    }
    let before = at == 0 || pdf_delimiter(content[at - 1]);
    let after = end == content.len() || pdf_delimiter(content[end]);
    before && after
}

fn skip_literal_string(content: &[u8], at: usize, what: &str) -> Result<usize> {
    let mut depth = 0usize;
    let mut i = at;
    while i < content.len() {
        match content[i] {
            b'\\' => {
                i = i.saturating_add(2);
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => i += 1,
        }
    }
    anyhow::bail!("{what} has an unterminated literal string")
}

fn skip_hex_string(content: &[u8], at: usize, what: &str) -> Result<usize> {
    let mut i = at + 1;
    while i < content.len() {
        if content[i] == b'>' {
            return Ok(i + 1);
        }
        i += 1;
    }
    anyhow::bail!("{what} has an unterminated hexadecimal string")
}

fn skip_comment(content: &[u8], at: usize) -> usize {
    let mut i = at + 1;
    while i < content.len() && content[i] != b'\r' && content[i] != b'\n' {
        i += 1;
    }
    i
}

fn skip_name(content: &[u8], at: usize) -> usize {
    let mut i = at + 1;
    while i < content.len() && !pdf_delimiter(content[i]) {
        i += 1;
    }
    i
}

/// Advance over one operand/dictionary token without leaving the stream.
fn skip_content_token(content: &[u8], at: usize, what: &str) -> Result<usize> {
    match content.get(at).copied() {
        None => Ok(at),
        Some(b'(') => skip_literal_string(content, at, what),
        Some(b'<') if content.get(at + 1) == Some(&b'<') => Ok(at + 2),
        Some(b'<') => skip_hex_string(content, at, what),
        Some(b'%') => Ok(skip_comment(content, at)),
        Some(b'/') => Ok(skip_name(content, at)),
        Some(b'>') if content.get(at + 1) == Some(&b'>') => Ok(at + 2),
        Some(_) => {
            let mut i = at;
            while i < content.len() && !pdf_delimiter(content[i]) {
                i += 1;
            }
            Ok(i.max(at + 1))
        }
    }
}

/// Read the optional inline-image /L length and whether ASCII85 terminates the
/// payload with "~>" instead of plain "EI".
fn inline_image_settings(dict: &[u8]) -> (Option<usize>, bool) {
    #[derive(Debug)]
    enum Token {
        Name(Vec<u8>),
        Number(u64),
        ArrayStart,
        Other,
    }

    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < dict.len() {
        match dict[i] {
            byte if pdf_space(byte) => i += 1,
            b'%' => i = skip_comment(dict, i),
            b'/' => {
                let start = i + 1;
                let end = skip_name(dict, i);
                tokens.push(Token::Name(dict[start..end].to_vec()));
                i = end;
            }
            b'0'..=b'9' => {
                let mut value = 0u64;
                while i < dict.len() && dict[i].is_ascii_digit() {
                    value = value
                        .saturating_mul(10)
                        .saturating_add(u64::from(dict[i] - b'0'));
                    i += 1;
                }
                tokens.push(Token::Number(value));
            }
            b'[' => {
                tokens.push(Token::ArrayStart);
                i += 1;
            }
            b']' => {
                tokens.push(Token::Other);
                i += 1;
            }
            b'(' => {
                i = skip_literal_string(dict, i, "inline image dictionary").unwrap_or(dict.len());
                tokens.push(Token::Other);
            }
            b'<' if dict.get(i + 1) != Some(&b'<') => {
                i = skip_hex_string(dict, i, "inline image dictionary").unwrap_or(dict.len());
                tokens.push(Token::Other);
            }
            _ => {
                i += 1;
                tokens.push(Token::Other);
            }
        }
    }

    let mut length = None;
    let mut ascii85 = false;
    for (index, token) in tokens.iter().enumerate() {
        let Token::Name(name) = token else {
            continue;
        };
        if name.as_slice() == b"F" || name.as_slice() == b"Filter" {
            let mut next = index + 1;
            if matches!(tokens.get(next), Some(Token::ArrayStart)) {
                next += 1;
            }
            if let Some(Token::Name(filter)) = tokens.get(next) {
                ascii85 = filter.as_slice() == b"A85" || filter.as_slice() == b"ASCII85Decode";
            }
        } else if name == b"L" || name == b"Length" {
            if let Some(Token::Number(value)) = tokens.get(index + 1) {
                length = usize::try_from(*value).ok();
            }
        }
    }
    (length, ascii85)
}

/// Find the byte offset of the EI keyword that ends inline image data.
fn find_inline_image_end(content: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < content.len() {
        if content[i] == b'E'
            && content[i + 1] == b'I'
            && i > 0
            && pdf_space(content[i - 1])
            && (i + 2 == content.len() || pdf_space(content[i + 2]))
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn skip_inline_image_data(
    content: &[u8],
    start: usize,
    length: Option<usize>,
    ascii85: bool,
    what: &str,
) -> usize {
    if let Some(length) = length {
        if let Some(end) = start.checked_add(length) {
            if end <= content.len() {
                let mut probe = end;
                while probe < content.len() && pdf_space(content[probe]) {
                    probe += 1;
                }
                if token_at(content, probe, b"EI") {
                    return probe + 2;
                }
            }
        }
    }
    if ascii85 {
        let mut i = start;
        while i + 1 < content.len() {
            if content[i] == b'~' && content[i + 1] == b'>' {
                if let Some(ei) = find_inline_image_end(content, i + 2) {
                    return ei + 2;
                }
                tracing::warn!(
                    stream = what,
                    "PDF ASCII85 inline image has no EI terminator; skipping the rest of the stream"
                );
                return content.len();
            }
            i += 1;
        }
    }
    if let Some(ei) = find_inline_image_end(content, start) {
        return ei + 2;
    }
    tracing::warn!(
        stream = what,
        "PDF inline image has no EI terminator; skipping the rest of the stream"
    );
    content.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::content::Content;

    #[test]
    fn binary_inline_image_does_not_break_or_inject_text() {
        let content =
            b"q BI /W 1 /H 1 /BPC 8 /CS /G ID \x00(Tj) Tj EI Q BT /F1 12 Tf (After) Tj ET";
        let (clean, found) = strip_inline_images(content, "PDF page 1 content").unwrap();
        assert!(found);
        let text = String::from_utf8_lossy(&clean);
        assert!(text.contains("After"), "{text:?}");
        assert!(!text.contains("(Tj)"), "{text:?}");
        assert!(!text.contains('\u{0}'), "{text:?}");
        assert!(Content::decode(&clean).is_ok());
    }

    #[test]
    fn explicit_length_skips_fake_ei_inside_the_payload() {
        let content = b"BI /L 11 /W 1 ID abc EI xyz! EI Q";
        let (clean, found) = strip_inline_images(content, "PDF page 1 content").unwrap();
        assert!(found);
        let text = String::from_utf8_lossy(&clean);
        assert!(text.contains("Q"), "{text:?}");
        assert!(!text.contains("xyz"), "{text:?}");
    }

    #[test]
    fn ascii85_payload_ends_at_tilde_marker() {
        let content = b"BI /F /A85 ID abc~> EI Q";
        let (clean, found) = strip_inline_images(content, "PDF page 1 content").unwrap();
        assert!(found);
        let text = String::from_utf8_lossy(&clean);
        assert!(text.contains("Q"), "{text:?}");
        assert!(!text.contains("abc"), "{text:?}");
    }

    #[test]
    fn unterminated_payload_is_dropped_after_a_warning() {
        let content = b"BI /W 1 /H 1 ID abc";
        let (clean, found) = strip_inline_images(content, "PDF page 1 content").unwrap();
        assert!(found);
        assert!(!String::from_utf8_lossy(&clean).contains("abc"));
    }

    #[test]
    fn missing_id_is_an_explicit_error() {
        let error = strip_inline_images(b"BI /W 1 /H 1", "PDF page 1 content")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no ID operator"), "{error}");
    }

    #[test]
    fn names_that_end_with_bi_or_id_are_not_operators() {
        let content = b"q /BI Do /ID 1 /BI2 Q";
        let (clean, found) = strip_inline_images(content, "PDF page 1 content").unwrap();
        assert!(!found);
        assert_eq!(clean.as_ref(), &content[..]);
    }

    #[test]
    fn strings_and_comments_that_mention_bi_are_not_images() {
        let content = b"BT (BI /W 1 ID) Tj % BI /W 1 ID\n ET";
        let (clean, found) = strip_inline_images(content, "PDF page 1 content").unwrap();
        assert!(!found);
        assert_eq!(clean.as_ref(), &content[..]);
    }
}
