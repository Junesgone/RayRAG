//! "One" parser — mirrors `rag/app/one.py` chunk().
//!
//! One file forms a single chunk that keeps the original text order.
//! Supported formats: docx/pdf/excel/txt/md/html/doc. RayRAG ports the
//! pure-algorithm core: section normalization (empty-line filtering, pdf
//! table flattening) and the single-chunk document assembly; the actual
//! per-format extractors (Docx sections, Pdf layout, Excel html, Html,
//! tika .doc) remain in the parser modules.

use crate::parser_config::layout_recognizer_name;

/// Strip empty lines and join with '\n' — mirrors one.py txt/md branch
/// (sections = [s for s in sections if s]) and the final
/// `"\n".join(sections)`.
pub fn join_nonempty_sections(sections: &[String]) -> String {
    let filtered: Vec<&str> = sections
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    filtered.join("\n")
}

/// A single-chunk document — mirrors the `doc` dict + tokenize() in
/// one.py:164-167.
#[derive(Debug, Clone, PartialEq)]
pub struct OneChunk {
    pub docnm_kwd: String,
    pub title_tks: Vec<String>,
    pub content_with_weight: String,
    pub content_ltks: Vec<String>,
}

/// Assemble one chunk from the joined section text — mirrors one.py:164-167.
/// title_tks strips the filename extension and tokenizes by whitespace
/// (rag_tokenizer.tokenize approximation); content_ltks are the joined
/// text's whitespace tokens.
pub fn one_chunk(filename: &str, content: &str) -> OneChunk {
    let title = filename
        .rfind('.')
        .map(|idx| filename[..idx].to_string())
        .unwrap_or_else(|| filename.to_string());
    let title_tks: Vec<String> = title
        .split_whitespace()
        .map(String::from)
        .filter(|t| !t.is_empty())
        .collect();
    let content_ltks: Vec<String> = content
        .split_whitespace()
        .map(String::from)
        .filter(|t| !t.is_empty())
        .collect();
    OneChunk {
        docnm_kwd: filename.to_string(),
        title_tks,
        content_with_weight: content.to_string(),
        content_ltks,
    }
}

/// Normalize pdf table sections — mirrors one.py:121-125: skip tables
/// without rows, unwrap multi-row tables to their first row, append as
/// text sections.
pub fn flatten_pdf_tables(
    sections: Vec<String>,
    tables: Vec<(Option<String>, Vec<(String, String)>)>,
) -> Vec<String> {
    let mut out = sections;
    for (rows, poss) in tables {
        let _ = poss;
        let Some(rows) = rows else { continue };
        if rows.trim().is_empty() {
            continue;
        }
        out.push(rows);
    }
    out
}

/// Dispatch — mirrors one.py chunk() for the txt/md family: split lines,
/// drop empties, join. Other formats (docx/pdf/xlsx/html/doc) route to
/// their parser modules then call one_chunk.
pub fn parse_one_text(filename: &str, text: &str) -> Result<OneChunk, String> {
    let lower = filename.to_lowercase();
    if !(lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".markdown")
        || lower.ends_with(".mdx"))
    {
        return Err(format!(
            "file type not supported yet(doc, docx, pdf, txt supported) got {filename}"
        ));
    }
    let lines: Vec<String> = text.lines().map(String::from).collect();
    let joined = join_nonempty_sections(&lines);
    Ok(one_chunk(filename, &joined))
}

/// Resolve the layout recognizer name for pdf parsing (one.py:90-95).
pub fn resolve_pdf_layout(parser_config_layout: Option<&str>) -> String {
    layout_recognizer_name(parser_config_layout.map(|s| s.to_string()))
}


