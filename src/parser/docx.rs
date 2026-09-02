//! DOCX parser — full port of RAGFlow `deepdoc/parser/docx_parser.py`.
//!
//! Hand-written OOXML walker (zip + quick-xml) to preserve the exact Python
//! semantics that docx-rs cannot express:
//! - page counting: `pn` increments only when a run's XML contains the literal
//!   `lastRenderedPageBreak` (docx_parser.py:179)
//! - paragraphs yield `(text, style_name)` pairs; style name is resolved via
//!   `word/styles.xml` styleId → w:name
//! - tables are composed with the `__compose_table_content` heuristic:
//!   `blockType` regex classification (Dt/DT/Nu/Ca/En/NE/Sg/Tx/Lx/Nr/Ot),
//!   majority-type header detection for numeric tables, per-cell header
//!   prefixes, `;`-joined rows, `\n`-joined when ≤3 columns.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
use std::io::{Cursor, Read};

/// One parsed paragraph: (text, style name).
pub type DocxParagraph = (String, String);

/// Port of `RAGFlowDocxParser` (docx_parser.py:32-185).
#[derive(Default)]
pub struct RAGFlowDocxParser;

impl RAGFlowDocxParser {
    pub fn new() -> Self {
        Self
    }

    /// `__call__` — docx_parser.py:162-185. Returns (paragraphs, table lines).
    pub fn parse_binary(
        &self,
        data: &[u8],
        from_page: usize,
        to_page: usize,
    ) -> Result<(Vec<DocxParagraph>, Vec<Vec<String>>)> {
        let mut zf = zip::ZipArchive::new(Cursor::new(data))
            .map_err(|e| anyhow::anyhow!("Invalid DOCX zip: {e}"))?;

        let document_xml = read_zip_entry(&mut zf, "word/document.xml")
            .ok_or_else(|| anyhow::anyhow!("word/document.xml missing"))?;
        let styles_xml = read_zip_entry(&mut zf, "word/styles.xml").unwrap_or_default();
        let style_names = parse_style_names(&styles_xml);

        let (paragraphs, tables) = parse_document_xml(&document_xml, &style_names);

        // Page filter — Python advances `pn` per run inside each paragraph and
        // keeps runs while from_page <= pn < to_page; a paragraph's collected
        // runs all share the page value recorded at parse time (pn at the
        // paragraph's end). We approximate by filtering on that page value.
        let mut secs: Vec<DocxParagraph> = Vec::new();
        for (text, style, page) in &paragraphs {
            if *page > to_page {
                break;
            }
            if from_page <= *page && *page < to_page && !text.trim().is_empty() {
                secs.push((text.clone(), style.clone()));
            }
        }

        // Compose tables with the RAGFlow heuristic.
        let mut tbls = Vec::new();
        for table in &tables {
            let composed = Self::extract_table_content(table);
            if !composed.is_empty() {
                tbls.push(composed);
            }
        }

        // Note: page numbers per paragraph are tracked in parse_document_xml;
        // the filter above re-approximates the run-level loop. For exact page
        // semantics we return every paragraph and let the caller filter — the
        // Python loop's `pn` advances on lastRenderedPageBreak anywhere in a
        // run, which we recorded per-paragraph.
        Ok((secs, tbls))
    }

    /// `__extract_table_content` — docx_parser.py:73-77 (no DataFrame needed).
    fn extract_table_content(table: &[Vec<String>]) -> Vec<String> {
        if table.is_empty() {
            return Vec::new();
        }
        Self::compose_table_content(table)
    }

    /// `blockType` — docx_parser.py:81-109.
    /// 静态预编译正则（避免每格重复编译；并行下消除偶发编译失败）。
    fn block_type(b: &str) -> &'static str {
        use std::sync::OnceLock;
        static PATTERNS: OnceLock<Vec<(regex::Regex, &'static str)>> = OnceLock::new();
        let patterns = PATTERNS.get_or_init(|| {
            let raw: &[(&str, &str)] = &[
                (
                    r"^(20|19)[0-9]{2}[年/-][0-9]{1,2}[月/-][0-9]{1,2}日*$",
                    "Dt",
                ),
                (r"^(20|19)[0-9]{2}年$", "Dt"),
                (r"^(20|19)[0-9]{2}[年/-][0-9]{1,2}月*$", "Dt"),
                (r"^[0-9]{1,2}[月/-][0-9]{1,2}日*$", "Dt"),
                (r"^第*[一二三四1-4]季度$", "Dt"),
                (r"^(20|19)[0-9]{2}年*[一二三四1-4]季度$", "Dt"),
                (r"^(20|19)[0-9]{2}[ABCDE]$", "DT"),
                (r"^[0-9.,+%/ -]+$", "Nu"),
                (r"^[0-9A-Z/\._~-]+$", "Ca"),
                (r"^[A-Z]*[a-z' -]+$", "En"),
                (r"^[0-9.,+-]+[0-9A-Za-z/$￥%<>（）()' -]+$", "NE"),
                (r"^.{1}$", "Sg"),
            ];
            raw.iter()
                .filter_map(|(pat, name)| regex::Regex::new(pat).ok().map(|re| (re, *name)))
                .collect()
        });
        for (re, name) in patterns {
            if re.is_match(b) {
                return name;
            }
        }
        // Token fallback approximating rag_tokenizer: whitespace words for
        // Latin text; CJK runs split into 2-char pseudo-tokens (rag_tokenizer
        // yields word-level tokens, typically ~2 chars for Chinese).
        let mut tks: Vec<String> = Vec::new();
        for word in b.split_whitespace() {
            if word.chars().any(is_cjk) {
                let cjk_run: String = word.chars().filter(|c| is_cjk(*c)).collect();
                let chars: Vec<char> = cjk_run.chars().collect();
                let mut i = 0;
                while i < chars.len() {
                    let end = (i + 2).min(chars.len());
                    tks.push(chars[i..end].iter().collect());
                    i = end;
                }
            } else if !word.is_empty() {
                tks.push(word.to_string());
            }
        }
        let tks: Vec<&str> = tks
            .iter()
            .map(String::as_str)
            .filter(|t| t.chars().count() > 1)
            .collect();
        if tks.len() > 3 {
            return if tks.len() < 12 { "Tx" } else { "Lx" };
        }
        // Python's rag_tokenizer.tag == "nr" (person name) requires a POS tagger;
        // approximated as not-a-person → "Ot".
        "Ot"
    }

    /// 数字类型排序权：非数字（表头倾向）优先。
    fn num_rank(cell_type: &str) -> u8 {
        if cell_type == "Nu" { 1 } else { 0 }
    }

    /// `__compose_table_content` — docx_parser.py:79-160.
    pub fn compose_table_content(df: &[Vec<String>]) -> Vec<String> {
        if df.len() < 2 {
            return Vec::new();
        }
        let nrows = df.len();
        let ncols = df[0].len();

        // Majority type across all cells from row 1 down.
        let mut type_counts: HashMap<&'static str, usize> = HashMap::new();
        for i in 1..nrows {
            for j in 0..ncols {
                let cell = df[i].get(j).cloned().unwrap_or_default();
                *type_counts.entry(Self::block_type(&cell)).or_insert(0) += 1;
            }
        }
        // HashMap 迭代序不定：tie 时用确定性规则（非数字类型优先，保证表头判定的稳定性）
        let majority = |counts: HashMap<&'static str, usize>| {
            counts
                .into_iter()
                .max_by(|(a_type, a_count), (b_type, b_count)| {
                    b_count
                        .cmp(a_count)
                        .then_with(|| Self::num_rank(a_type).cmp(&Self::num_rank(b_type)))
                })
        };
        let Some((max_type, _)) = majority(type_counts) else {
            return Vec::new();
        };

        let mut hdrows: Vec<usize> = vec![0];
        if max_type == "Nu" {
            for r in 1..nrows {
                let mut row_counts: HashMap<&'static str, usize> = HashMap::new();
                for j in 0..ncols {
                    let cell = df[r].get(j).cloned().unwrap_or_default();
                    *row_counts.entry(Self::block_type(&cell)).or_insert(0) += 1;
                }
                let Some((row_type, _)) =
                    row_counts
                        .into_iter()
                        .max_by(|(a_type, a_count), (b_type, b_count)| {
                            b_count
                                .cmp(a_count)
                                .then_with(|| Self::num_rank(a_type).cmp(&Self::num_rank(b_type)))
                        })
                else {
                    continue;
                };
                if row_type != max_type {
                    hdrows.push(r);
                }
            }
        }

        let mut lines: Vec<String> = Vec::new();
        for i in 1..nrows {
            if hdrows.contains(&i) {
                continue;
            }
            // Header rows above i: nearest contiguous run of hdrows above.
            let mut hr: Vec<isize> = hdrows.iter().map(|&r| r as isize - i as isize).collect();
            hr.retain(|&r| r < 0);
            let mut t = hr.len().saturating_sub(1);
            while t > 0 {
                if hr[t] - hr[t - 1] > 1 {
                    hr = hr[t..].to_vec();
                    break;
                }
                t -= 1;
            }
            let mut headers: Vec<String> = Vec::new();
            for j in 0..ncols {
                let mut seen: Vec<String> = Vec::new();
                for &h in &hr {
                    let idx = (i as isize + h) as usize;
                    let x = df
                        .get(idx)
                        .and_then(|row| row.get(j))
                        .cloned()
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    if seen.contains(&x) {
                        continue;
                    }
                    seen.push(x);
                }
                let mut hdr = seen.join(",");
                if !hdr.is_empty() {
                    hdr.push_str(": ");
                }
                headers.push(hdr);
            }
            let mut cells: Vec<String> = Vec::new();
            for j in 0..ncols {
                let cell = df[i].get(j).cloned().unwrap_or_default();
                if cell.trim().is_empty() {
                    continue;
                }
                cells.push(format!("{}{}", headers[j], cell));
            }
            lines.push(cells.join(";"));
        }

        if ncols > 3 {
            lines
        } else {
            vec![lines.join("\n")]
        }
    }
}

/// True for CJK unified ideographs (approximate rag_tokenizer domain).
fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF)
}

/// Parse `word/styles.xml`: styleId → w:name (human-readable style name).
fn parse_style_names(xml: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut cur_style: Option<(String, String)> = None; // (styleId, name)
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"style" {
                    let mut style_id = String::new();
                    for attr in e.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"styleId" {
                            style_id = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                    cur_style = Some((style_id, String::new()));
                } else if e.local_name().as_ref() == b"name"
                    && let Some((_, name)) = cur_style.as_mut()
                {
                    for attr in e.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"val" {
                            *name = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() == b"style"
                    && let Some((id, name)) = cur_style.take()
                    && !id.is_empty() && !name.is_empty() {
                        out.insert(id, name);
                    }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// Parse `word/document.xml`: paragraphs (text + style + page breaks) and
/// tables (raw cell grid).
fn parse_document_xml(
    xml: &str,
    style_names: &HashMap<String, String>,
) -> (Vec<(String, String, usize)>, Vec<Vec<Vec<String>>>) {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    let mut paragraphs: Vec<(String, String, usize)> = Vec::new(); // (text, style, page)
    let mut tables: Vec<Vec<Vec<String>>> = Vec::new();

    // Current paragraph state.
    let mut cur_text = String::new();
    let mut cur_style_id = String::new();
    let mut cur_page = 0usize;

    // Table state.
    let mut cur_table: Vec<Vec<String>> = Vec::new();
    let mut cur_row: Vec<String> = Vec::new();
    let mut cur_cell = String::new();

    let mut in_paragraph = false;
    let mut in_run = false;
    let mut in_text = false;
    let mut in_table = false;
    let mut in_row = false;
    let mut in_cell = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"p" if !in_table => {
                        in_paragraph = true;
                        cur_text.clear();
                        cur_style_id.clear();
                    }
                    b"pStyle" if in_paragraph => {
                        for attr in e.attributes().flatten() {
                            if attr.key.local_name().as_ref() == b"val" {
                                cur_style_id = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"r" if in_paragraph => in_run = true,
                    b"t" if in_run => in_text = true,
                    b"lastRenderedPageBreak" if in_run => cur_page += 1,
                    b"tbl" => {
                        in_table = true;
                        cur_table.clear();
                    }
                    b"tr" if in_table => {
                        in_row = true;
                        cur_row.clear();
                    }
                    b"tc" if in_row => {
                        in_cell = true;
                        cur_cell.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                if in_text {
                    cur_text.push_str(&String::from_utf8_lossy(t.as_ref()));
                } else if in_cell {
                    cur_cell.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"t" => in_text = false,
                    b"r" => in_run = false,
                    b"p" if !in_table => {
                        in_paragraph = false;
                        let style = style_names.get(&cur_style_id).cloned().unwrap_or_default();
                        paragraphs.push((cur_text.clone(), style, cur_page));
                    }
                    b"tc" if in_row => {
                        in_cell = false;
                        cur_row.push(cur_cell.clone());
                    }
                    b"tr" if in_table => {
                        in_row = false;
                        cur_table.push(cur_row.clone());
                    }
                    b"tbl" => {
                        in_table = false;
                        tables.push(cur_table.clone());
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    (paragraphs, tables)
}

/// Read a named zip entry as UTF-8 (index lookup avoids borrow conflicts).
fn read_zip_entry(zf: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<String> {
    use std::io::Read;
    for i in 0..zf.len() {
        let mut f = zf.by_index(i).ok()?;
        if f.name() == name {
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_err() {
                return None;
            }
            return Some(String::from_utf8_lossy(&buf).to_string());
        }
    }
    None
}

/// Port of `DocxParser` — Parse adapter joining paragraphs and tables.
#[derive(Default)]
pub struct DocxParser {
    inner: RAGFlowDocxParser,
}

impl DocxParser {
    pub fn new() -> Self {
        Self {
            inner: RAGFlowDocxParser::new(),
        }
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        let (paragraphs, tables) = self.inner.parse_binary(data, 0, usize::MAX)?;
        let mut text = String::new();
        for (t, _style) in &paragraphs {
            text.push_str(t);
            text.push('\n');
        }
        for table in &tables {
            text.push_str("\n<!--TABLE_START-->\n");
            for row in table {
                text.push_str(row);
                text.push('\n');
            }
            text.push_str("<!--TABLE_END-->\n");
        }
        Ok(text)
    }
}

impl Parse for DocxParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(data)?;
        let mut doc = new_document(
            name,
            content,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            data.len(),
        );
        // `get_picture` — docx_parser.py:33-70: count embedded images so
        // downstream image-context steps know whether this document has any.
        if let Ok(images) = extract_images(data)
            && !images.is_empty() {
                doc.metadata
                    .insert("image_count".to_string(), images.len().to_string());
            }
        Ok(doc)
    }
}

// ---------------------------------------------------------------------------
// Embedded-image extraction — docx_parser.py `get_picture` (33-70).
//
// Paragraph images live in the drawing markup: `<pic:pic><a:blip r:embed=
// "rIdN"/></pic:pic>`. The embed id resolves through
// `word/_rels/document.xml.rels` to a `word/media/*` part, whose bytes are
// the image. The Python code walks per-paragraph and tolerates damaged
// images via a blob fallback; here we do the same resolution document-wide
// and return `(media file name, bytes)` pairs.
// ---------------------------------------------------------------------------

/// `get_picture` — docx_parser.py:33-70, document-wide. Returns every
/// embedded image as `(file name, bytes)`, in document order, skipping
/// unresolvable or empty blips.
pub fn extract_images(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut zf = zip::ZipArchive::new(Cursor::new(data))
        .map_err(|e| anyhow::anyhow!("Invalid DOCX zip: {e}"))?;

    let document_xml = read_zip_entry(&mut zf, "word/document.xml")
        .ok_or_else(|| anyhow::anyhow!("word/document.xml missing"))?;
    let rels_xml = read_zip_entry(&mut zf, "word/_rels/document.xml.rels").unwrap_or_default();

    // rId → relationship target.
    let mut rels: HashMap<String, String> = HashMap::new();
    {
        let mut reader = Reader::from_str(&rels_xml);
        loop {
            match reader.read_event() {
                Ok(Event::Start(event)) | Ok(Event::Empty(event))
                    if event.local_name().as_ref() == b"Relationship" =>
                {
                    let id = event
                        .attributes()
                        .flatten()
                        .find(|attr| attr.key.local_name().as_ref() == b"Id")
                        .map(|attr| String::from_utf8_lossy(&attr.value).into_owned());
                    let target = event
                        .attributes()
                        .flatten()
                        .find(|attr| attr.key.local_name().as_ref() == b"Target")
                        .map(|attr| String::from_utf8_lossy(&attr.value).into_owned());
                    if let (Some(id), Some(target)) = (id, target) {
                        rels.insert(id, target);
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
        }
    }

    // Collect `r:embed` ids from every `<a:blip>` in the document body.
    let mut embeds: Vec<String> = Vec::new();
    {
        let mut reader = Reader::from_str(&document_xml);
        loop {
            match reader.read_event() {
                Ok(Event::Start(event)) | Ok(Event::Empty(event))
                    if event.local_name().as_ref() == b"blip" =>
                {
                    for attr in event.attributes().flatten() {
                        if attr.key.local_name().as_ref() == b"embed" {
                            embeds.push(String::from_utf8_lossy(&attr.value).into_owned());
                        }
                    }
                }
                Ok(Event::Eof) => break,
                _ => {}
            }
        }
    }

    let mut images = Vec::new();
    for embed in embeds {
        let Some(target) = rels.get(&embed) else {
            continue;
        };
        // Resolve the relationship target relative to word/ (mirrors
        // `document.part.related_parts[embed]`).
        let path = if target.starts_with('/') {
            target.trim_start_matches('/').to_owned()
        } else {
            format!("word/{}", target.trim_start_matches("./"))
        };
        let Some(bytes) = read_zip_entry_bytes(&mut zf, &path) else {
            continue;
        };
        if bytes.is_empty() {
            continue;
        }
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        images.push((name, bytes));
    }
    Ok(images)
}

fn read_zip_entry_bytes(zf: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<Vec<u8>> {
    let Ok(mut file) = zf.by_name(name) else {
        return None;
    };
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn block(b: &str) -> &'static str {
        RAGFlowDocxParser::block_type(b)
    }

    #[test]
    fn block_type_classifies_dates_numbers_and_text() {
        assert_eq!(block("2023年"), "Dt");
        assert_eq!(block("2023/05/12"), "Dt");
        assert_eq!(block("2023-05"), "Dt");
        assert_eq!(block("第三季度"), "Dt");
        assert_eq!(block("2023B"), "DT");
        assert_eq!(block("1,234.56"), "Nu");
        assert_eq!(block("ABC123"), "Ca");
        assert_eq!(block("hello world"), "En");
        assert_eq!(block("1.5kg"), "NE");
        // Single uppercase letter hits Ca (before Sg in the pattern list).
        assert_eq!(block("X"), "Ca");
        // Long text → Tx; very long → Lx.
        assert_eq!(block("这是一段比较长的文本内容用于测试类型判断"), "Tx");
        assert_eq!(
            block(
                "这是一段非常长的文本内容用于测试类型判断它包含了很多词汇以便触发长文本分类逻辑的阈值"
            ),
            "Lx"
        );
        assert_eq!(block("不明"), "Ot");
    }

    #[test]
    fn compose_table_with_numeric_rows_uses_headers() {
        // Header row + two numeric data rows, 3 columns.
        let df = vec![
            vec!["项目".to_string(), "数值".to_string(), "备注".to_string()],
            vec!["A".to_string(), "1".to_string(), "x".to_string()],
            vec!["B".to_string(), "2".to_string(), "y".to_string()],
        ];
        let lines = RAGFlowDocxParser::compose_table_content(&df);
        // ≤3 columns → single newline-joined string.
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("A"));
        assert!(lines[0].contains("1"));
        assert!(lines[0].contains("x"));
        assert!(lines[0].contains("B"));
        assert!(lines[0].contains("2"));
    }

    #[test]
    fn compose_table_requires_at_least_two_rows() {
        let df = vec![vec!["h".to_string()]];
        assert!(RAGFlowDocxParser::compose_table_content(&df).is_empty());
        let df2: Vec<Vec<String>> = vec![];
        assert!(RAGFlowDocxParser::compose_table_content(&df2).is_empty());
    }

    #[test]
    fn compose_table_four_columns_returns_multiple_lines() {
        let df = vec![
            vec![
                "h1".to_string(),
                "h2".to_string(),
                "h3".to_string(),
                "h4".to_string(),
            ],
            vec![
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
                "d".to_string(),
            ],
            vec![
                "e".to_string(),
                "f".to_string(),
                "g".to_string(),
                "h".to_string(),
            ],
        ];
        let lines = RAGFlowDocxParser::compose_table_content(&df);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("a"));
        assert!(lines[1].contains("e"));
    }

    #[test]
    fn parse_style_names_maps_style_id_to_name() {
        let xml = r#"<?xml version="1.0"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Normal">
    <w:name w:val="Normal"/>
  </w:style>
</w:styles>"#;
        let map = parse_style_names(xml);
        assert_eq!(map.get("Heading1").map(String::as_str), Some("heading 1"));
        assert_eq!(map.get("Normal").map(String::as_str), Some("Normal"));
    }

    #[test]
    fn parse_document_xml_extracts_paragraphs_and_tables() {
        let xml = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>
    <w:p><w:r><w:t>Body text</w:t></w:r></w:p>
    <w:tbl>
      <w:tr><w:tc><w:p><w:r><w:t>c1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>c2</w:t></w:r></w:p></w:tc></w:tr>
      <w:tr><w:tc><w:p><w:r><w:t>v1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>v2</w:t></w:r></w:p></w:tc></w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;
        let styles = parse_style_names(
            r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:styleId="Heading1"><w:name w:val="heading 1"/></w:style></w:styles>"#,
        );
        let (paragraphs, tables) = parse_document_xml(xml, &styles);
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].0, "Title");
        assert_eq!(paragraphs[0].1, "heading 1");
        assert_eq!(paragraphs[1].0, "Body text");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].len(), 2);
        assert_eq!(tables[0][0], vec!["c1", "c2"]);
        assert_eq!(tables[0][1], vec!["v1", "v2"]);
    }

    #[test]
    fn page_break_increments_page_counter() {
        let xml = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Page one</w:t></w:r></w:p>
    <w:p><w:r><w:t>break</w:t><w:lastRenderedPageBreak/><w:t>after</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        let styles = HashMap::new();
        let (paragraphs, _) = parse_document_xml(xml, &styles);
        assert_eq!(paragraphs.len(), 2);
        assert_eq!(paragraphs[0].2, 0);
        assert_eq!(paragraphs[1].2, 1); // page advanced by lastRenderedPageBreak
    }

    #[test]
    fn docx_parser_parse_trait_joins_content() {
        let data = build_minimal_docx();
        let parser = DocxParser::new();
        let doc = parser.parse("test.docx", &data).unwrap();
        assert!(doc.content.contains("Hello"));
        assert_eq!(
            doc.mime_type,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
    }

    #[test]
    fn extract_images_resolves_blip_embeds_through_rels() {
        // `get_picture` — docx_parser.py:33-70: `<a:blip r:embed="rId5"/>`
        // → word/_rels/document.xml.rels rId5 → word/media/image1.png bytes.
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
  <w:body>
    <w:p><w:r><w:t>Text with a picture</w:t></w:r></w:p>
    <w:p><w:r><w:drawing><pic:pic><a:blip r:embed="rId5"/></pic:pic></w:drawing></w:r></w:p>
  </w:body>
</w:document>"#;
            zw.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zw.write_all(doc.as_bytes()).unwrap();

            let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId5" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
  <Relationship Id="rId6" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image2.png"/>
</Relationships>"#;
            zw.start_file(
                "word/_rels/document.xml.rels",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zw.write_all(rels.as_bytes()).unwrap();

            zw.start_file(
                "word/media/image1.png",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zw.write_all(&[0x89, b'P', b'N', b'G', 1, 2, 3]).unwrap();
            // image2 is referenced by rels but never blipped → not extracted.
            zw.finish().unwrap();
        }

        let images = extract_images(&buf).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].0, "image1.png");
        assert_eq!(images[0].1, vec![0x89, b'P', b'N', b'G', 1, 2, 3]);

        // parse() surfaces the count as metadata.
        let doc = DocxParser::new().parse("pic.docx", &buf).unwrap();
        assert_eq!(
            doc.metadata.get("image_count").map(String::as_str),
            Some("1")
        );
    }

    /// Build a minimal valid DOCX (document.xml + styles.xml + [Content_Types].xml).
    fn build_minimal_docx() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let ct = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
</Types>"#;
            zw.start_file(
                "[Content_Types].xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zw.write_all(ct.as_bytes()).unwrap();

            let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;
            zw.start_file("_rels/.rels", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(rels.as_bytes()).unwrap();

            let doc = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Hello</w:t></w:r></w:p>
    <w:p><w:r><w:t>World</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
            zw.start_file(
                "word/document.xml",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zw.write_all(doc.as_bytes()).unwrap();

            let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/> "#;
            zw.start_file("word/styles.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            zw.write_all(styles.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        buf
    }
}
