//! Email parser — mirrors `rag/app/email.py` chunk().
//!
//! Ported pure-algorithm core: header collection, MIME content-type
//! dispatch (text/plain, text/html, multipart recursion), charset decode
//! chain, section assembly and naive_merge reuse. A lightweight .eml MIME
//! parser (RFC 822 headers + base64/quoted-printable bodies + multipart
//! boundary tree) stands in for Python's email.parser; attachment
//! recursion into naive_chunk stays an integration gap.

use crate::book::naive_merge;
use encoding_rs::{GB18030, GBK, UTF_8};

/// A decoded MIME part.
#[derive(Debug, Clone)]
pub struct EmlPart {
    pub content_type: String,
    pub disposition: Option<String>,
    pub filename: Option<String>,
    pub body: Vec<u8>,
    pub parts: Vec<EmlPart>,
}

/// Decode quoted-printable (RFC 2045 §6.7): `=XX` hex, `=\r?\n` soft break.
pub fn decode_quoted_printable(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'=' && i + 1 < input.len() {
            let next = input[i + 1];
            if next == b'\n' {
                i += 2;
                continue;
            }
            if next == b'\r' && i + 2 < input.len() && input[i + 2] == b'\n' {
                i += 3;
                continue;
            }
            if i + 2 < input.len() {
                let hex = &input[i + 1..i + 3];
                if let Ok(s) = std::str::from_utf8(hex) {
                    if let Ok(v) = u8::from_str_radix(s, 16) {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                }
            }
            out.push(input[i]);
            i += 1;
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

fn decode_transfer_encoding(body: &[u8], cte: &str) -> Vec<u8> {
    match cte.trim().to_ascii_lowercase().as_str() {
        "base64" => {
            let compact: String = String::from_utf8_lossy(body)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, compact.as_bytes())
                .unwrap_or_default()
        }
        "quoted-printable" => decode_quoted_printable(body),
        _ => body.to_vec(),
    }
}

/// Parse `Header: value` lines at the top of a message or part; returns
/// (headers, body_start_index).
fn parse_headers(bytes: &[u8]) -> (Vec<(String, String)>, usize) {
    let mut headers = Vec::new();
    let mut idx = 0usize;
    let mut last: Option<usize> = None;
    while idx < bytes.len() {
        // line end
        let mut end = idx;
        while end < bytes.len() && bytes[end] != b'\n' {
            end += 1;
        }
        let mut line = &bytes[idx..end];
        if line.last() == Some(&b'\r') {
            line = &line[..line.len() - 1];
        }
        if line.is_empty() {
            return (headers, end + 1);
        }
        if line[0] == b' ' || line[0] == b'\t' {
            // folded header continuation
            if let Some(li) = last {
                let val = String::from_utf8_lossy(line).trim().to_string();
                headers[li].1.push(' ');
                headers[li].1.push_str(&val);
            }
            idx = end + 1;
            continue;
        }
        let line_str = String::from_utf8_lossy(line).to_string();
        if let Some(colon) = line_str.find(':') {
            let name = line_str[..colon].trim().to_string();
            let value = line_str[colon + 1..].trim().to_string();
            headers.push((name, value));
            last = Some(headers.len() - 1);
        }
        idx = end + 1;
    }
    (headers, idx)
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn boundary_of(content_type: &str) -> Option<String> {
    for part in content_type.split(';').skip(1) {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("boundary=") {
            let rest = rest.trim();
            let rest = rest.trim_matches('"');
            return Some(rest.to_string());
        }
    }
    None
}

/// Parse an .eml message into a part tree — a light stand-in for
/// Python's email.parser (policy.default).
pub fn parse_eml(bytes: &[u8]) -> EmlPart {
    let (headers, body_start) = parse_headers(bytes);
    let content_type = header_value(&headers, "Content-Type")
        .map(|s| s.to_string())
        .unwrap_or_else(|| "text/plain".to_string());
    let cte = header_value(&headers, "Content-Transfer-Encoding")
        .map(|s| s.to_string())
        .unwrap_or_default();
    let disposition = header_value(&headers, "Content-Disposition").map(|s| s.to_string());
    let filename = disposition.as_ref().and_then(|d| {
        d.split(';')
            .skip(1)
            .find_map(|p| {
                let p = p.trim();
                p.strip_prefix("filename=")
                    .map(|v| v.trim().trim_matches('"').to_string())
            })
    });

    let mut part = EmlPart {
        content_type: content_type.clone(),
        disposition,
        filename,
        body: Vec::new(),
        parts: Vec::new(),
    };

    let base_type = content_type.split(';').next().unwrap_or("").trim().to_lowercase();
    if base_type.starts_with("multipart/") {
        if let Some(boundary) = boundary_of(&content_type) {
            let body = &bytes[body_start..];
            let bmark = format!("--{boundary}");
            let body_str = String::from_utf8_lossy(body);
            // split on boundary lines
            for seg in body_str.split(&bmark) {
                let seg = seg.trim_start_matches('\r').trim_start_matches('\n');
                let seg = seg.trim();
                if seg.is_empty() || seg == "--" {
                    continue;
                }
                if seg.starts_with("--") {
                    continue;
                }
                part.parts.push(parse_eml(seg.as_bytes()));
            }
        }
    } else {
        let decoded = decode_transfer_encoding(&bytes[body_start..], &cte);
        part.body = decoded;
    }
    part
}

/// Charset decode chain — mirrors email.py:68-79: try the declared
/// charset, then utf-8/gb2312/gbk/gb18030/latin1, finally utf-8 ignore.
pub fn decode_payload(payload: &[u8], charset: &str) -> String {
    let decode_with = |enc: &str| -> Option<String> {
        match enc.to_ascii_lowercase().as_str() {
            "utf-8" | "utf8" => UTF_8.decode(payload).0.into_owned().into(),
            "gb2312" | "gbk" => GBK.decode(payload).0.into_owned().into(),
            "gb18030" => GB18030.decode(payload).0.into_owned().into(),
            "latin1" | "latin-1" | "iso-8859-1" => {
                Some(payload.iter().map(|&b| b as char).collect())
            }
            _ => None,
        }
    };
    if let Some(s) = decode_with(charset) {
        return s;
    }
    for enc in ["utf-8", "gb2312", "gbk", "gb18030", "latin1"] {
        if let Some(s) = decode_with(enc) {
            // encoding_rs is lossy by design; emulate Python's strict
            // attempt by checking for replacement chars for utf-8.
            if enc == "utf-8" {
                if std::str::from_utf8(payload).is_ok() {
                    return s;
                }
                continue;
            }
            return s;
        }
    }
    String::from_utf8_lossy(payload).into_owned()
}

/// Collect headers + body text + html text — mirrors email.py:61-99.
/// Returns (text_sections, html_lines) where text sections mirror
/// TxtParser.parser_txt (line split, non-empty) and html lines are
/// non-empty lines.
pub fn collect_email_content(
    msg: &EmlPart,
    headers: &[(String, String)],
) -> (Vec<String>, Vec<String>) {
    let mut text_txt: Vec<String> = Vec::new();
    let mut html_txt: Vec<String> = Vec::new();

    for (h, v) in headers {
        text_txt.push(format!("{h}: {v}"));
    }

    fn walk(msg: &EmlPart, text_txt: &mut Vec<String>, html_txt: &mut Vec<String>) {
        let base_type = msg
            .content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        match base_type.as_str() {
            "text/plain" => {
                let charset = msg
                    .content_type
                    .split(';')
                    .nth(1)
                    .and_then(|p| p.trim().strip_prefix("charset="))
                    .map(|v| v.trim().trim_matches('"').to_string())
                    .unwrap_or_else(|| "utf-8".to_string());
                text_txt.push(decode_payload(&msg.body, &charset));
            }
            "text/html" => {
                let charset = msg
                    .content_type
                    .split(';')
                    .nth(1)
                    .and_then(|p| p.trim().strip_prefix("charset="))
                    .map(|v| v.trim().trim_matches('"').to_string())
                    .unwrap_or_else(|| "utf-8".to_string());
                html_txt.push(decode_payload(&msg.body, &charset));
            }
            _ => {
                for p in &msg.parts {
                    walk(p, text_txt, html_txt);
                }
            }
        }
    }
    walk(msg, &mut text_txt, &mut html_txt);

    // TxtParser.parser_txt: split lines, drop empty
    let text_sections: Vec<String> = text_txt
        .iter()
        .flat_map(|t| t.lines().map(String::from))
        .filter(|l| !l.is_empty())
        .collect();
    (text_sections, html_txt)
}

/// Email chunk() core — mirrors email.py:41-108: assemble header/body
/// sections, naive_merge with parser_config, return merged chunks.
pub fn parse_email(
    msg: &EmlPart,
    headers: &[(String, String)],
    chunk_token_num: usize,
    delimiter: &str,
) -> Vec<String> {
    let (text_sections, html_txt) = collect_email_content(msg, headers);
    let mut sections: Vec<(String, String)> = text_sections
        .iter()
        .map(|s| (s.clone(), String::new()))
        .collect();
    for line in html_txt.iter().filter(|l| !l.is_empty()) {
        sections.push((line.clone(), String::new()));
    }
    let _ = delimiter;
    // email.py:102 uses naive_merge with default overlapped_percent=0
    naive_merge(&sections, chunk_token_num, "\n!?。；！？", 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eml_sample() -> String {
        let mut s = String::new();
        s.push_str("From: boss@example.com\r\n");
        s.push_str("To: team@example.com\r\n");
        s.push_str("Subject: Weekly report\r\n");
        s.push_str("Content-Type: multipart/mixed; boundary=\"bnd\"\r\n");
        s.push_str("\r\n");
        s.push_str("--bnd\r\n");
        s.push_str("Content-Type: text/plain; charset=utf-8\r\n");
        s.push_str("Content-Transfer-Encoding: base64\r\n");
        s.push_str("\r\n");
        s.push_str(&base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            "本周报告正文内容。".as_bytes(),
        ));
        s.push_str("\r\n--bnd\r\n");
        s.push_str("Content-Type: text/html; charset=utf-8\r\n");
        s.push_str("Content-Transfer-Encoding: quoted-printable\r\n");
        s.push_str("\r\n");
        s.push_str("<p>=E6=9C=AC=E5=91=A8=E6=8A=A5=E5=91=8A</p>\r\n");
        s.push_str("\r\n--bnd--\r\n");
        s
    }

    #[test]
    fn decode_quoted_printable_handles_hex_and_soft_breaks() {
        assert_eq!(decode_quoted_printable(b"=E6=9C=AC"), "本".as_bytes());
        assert_eq!(decode_quoted_printable(b"ab=\r\ncd"), b"abcd");
        assert_eq!(decode_quoted_printable(b"plain"), b"plain");
    }

    #[test]
    fn parse_eml_multipart_tree() {
        let eml = eml_sample();
        let msg = parse_eml(eml.as_bytes());
        assert!(msg.content_type.starts_with("multipart/mixed"));
        assert_eq!(msg.parts.len(), 2);
        assert!(msg.parts[0].content_type.starts_with("text/plain"));
        assert!(msg.parts[1].content_type.starts_with("text/html"));
    }

    #[test]
    fn decode_payload_charset_chain() {
        // utf-8 ok
        assert_eq!(decode_payload("中文".as_bytes(), "utf-8"), "中文");
        // gbk bytes decoded with gb2312 fallback
        let gbk_bytes = GBK.encode("水质检测").0.into_owned();
        assert_eq!(decode_payload(&gbk_bytes, "gb2312"), "水质检测");
        // invalid utf-8 with unknown charset -> chain -> gbk works
        assert_eq!(decode_payload(&gbk_bytes, "x-unknown"), "水质检测");
    }

    #[test]
    fn collect_email_content_headers_and_bodies() {
        let eml = eml_sample();
        let msg = parse_eml(eml.as_bytes());
        let headers = vec![
            ("From".to_string(), "boss@example.com".to_string()),
            ("Subject".to_string(), "Weekly report".to_string()),
        ];
        let (text_sections, html_txt) = collect_email_content(&msg, &headers);
        assert!(text_sections.iter().any(|s| s.starts_with("From: boss@example.com")));
        assert!(text_sections.iter().any(|s| s.contains("本周报告正文内容")));
        assert!(html_txt.iter().any(|s| s.contains("本周报告")));
    }

    #[test]
    fn parse_email_merges_sections() {
        let eml = eml_sample();
        let msg = parse_eml(eml.as_bytes());
        let headers = vec![
            ("From".to_string(), "boss@example.com".to_string()),
            ("Subject".to_string(), "Weekly report".to_string()),
        ];
        let chunks = parse_email(&msg, &headers, 512, "\n!?。；！？");
        assert!(!chunks.is_empty(), "{chunks:?}");
        let joined = chunks.join("\n");
        assert!(joined.contains("From: boss@example.com"));
        assert!(joined.contains("本周报告"));
    }

    #[test]
    fn attachment_recognition_marks_filename() {
        let mut eml = String::new();
        eml.push_str("Content-Type: multipart/mixed; boundary=\"bb\"\r\n\r\n");
        eml.push_str("--bb\r\n");
        eml.push_str("Content-Type: text/plain; charset=utf-8\r\n\r\nbody\r\n");
        eml.push_str("--bb\r\n");
        eml.push_str("Content-Type: application/pdf\r\n");
        eml.push_str("Content-Disposition: attachment; filename=\"report.pdf\"\r\n");
        eml.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
        eml.push_str("JVBERi0xLjQK\r\n--bb--\r\n");
        let msg = parse_eml(eml.as_bytes());
        assert_eq!(msg.parts.len(), 2);
        let att = &msg.parts[1];
        assert!(att.content_type.starts_with("application/pdf"));
        assert_eq!(att.filename.as_deref(), Some("report.pdf"));
    }
}
