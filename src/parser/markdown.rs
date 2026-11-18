//! Markdown parser — full port of RAGFlow `deepdoc/parser/markdown_parser.py`.
//!
//! Two components:
//! 1. `RAGFlowMarkdownParser::extract_tables_and_remainder` — pulls standard
//!    (bordered), borderless and raw-HTML tables out of a markdown document.
//!    With `separate_tables = true` the tables are removed from the body text
//!    (returned separately); with `false` they are kept in place (Python
//!    renders them through the `markdown` lib; we keep the raw table source as
//!    the closest content-equivalent since rendering is cosmetic).
//! 2. `MarkdownElementExtractor` — walks markdown line by line and slices it
//!    into block elements: headers (`#`), fenced code blocks, list blocks,
//!    blockquotes and plain text blocks. An explicit `delimiter` (regex of
//!    backtick-wrapped tokens) splits the text instead of line walking, with
//!    optional `start_line`/`end_line` metadata.

use regex::Regex;

/// One extracted markdown block element.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownElement {
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    /// "header" | "code_block" | "list_block" | "blockquote" | "text_block"
    /// — `None` when produced by a delimiter split.
    pub element_type: Option<String>,
}

impl MarkdownElement {
    fn new(
        content: String,
        start_line: usize,
        end_line: usize,
        element_type: Option<String>,
    ) -> Self {
        Self {
            content,
            start_line,
            end_line,
            element_type,
        }
    }
}

/// Port of `RAGFlowMarkdownParser` (markdown_parser.py:23-122).
#[derive(Default)]
pub struct RAGFlowMarkdownParser;

impl RAGFlowMarkdownParser {
    pub fn new() -> Self {
        Self
    }

    /// `extract_tables_and_remainder` — markdown_parser.py:27-122.
    /// Returns `(working_text, tables)`. `separate_tables` controls whether the
    /// matched table source is removed from the text or left inline.
    pub fn extract_tables_and_remainder(
        &self,
        markdown_text: &str,
        separate_tables: bool,
    ) -> (String, Vec<String>) {
        let mut tables: Vec<String> = Vec::new();
        let mut working_text = markdown_text.to_string();

        let replace_tables =
            |pattern: &Regex, working_text: &mut String, tables: &mut Vec<String>| {
                let mut new_text = String::new();
                let mut last_end = 0usize;
                for m in pattern.find_iter(working_text) {
                    let raw_table = m.as_str().to_string();
                    tables.push(raw_table.clone());
                    if separate_tables {
                        new_text.push_str(&working_text[last_end..m.start()]);
                        new_text.push_str("\n\n");
                    } else {
                        // Python renders the table to HTML here; the raw table
                        // source is content-equivalent, so keep it inline.
                        new_text.push_str(&working_text[last_end..m.start()]);
                        new_text.push_str(&raw_table);
                        new_text.push_str("\n\n");
                    }
                    last_end = m.end();
                }
                new_text.push_str(&working_text[last_end..]);
                *working_text = new_text;
            };

        if working_text.contains('|') {
            // Standard (bordered) markdown table: header row, separator row,
            // then >= 1 body rows — every row starts and ends with `|`.
            let border_table = Regex::new(
                r"(?:\n|^)(?:\|.*?\|.*?\|.*?\n)(?:\|(?:\s*[:-]+[-| :]*\s*)\|.*?\n)(?:\|.*?\|.*?\|.*?\n)+",
            )
            .unwrap();
            replace_tables(&border_table, &mut working_text, &mut tables);

            // Borderless markdown table: rows do not start with `|`, but still
            // contain `|` and are separated by the `---` delimiter row.
            let no_border_table = Regex::new(
                r"(?:\n|^)(?:\S.*?\|.*?\n)(?:(?:\s*[:-]+[-| :]*\s*).*?\n)(?:\S.*?\|.*?\n)+",
            )
            .unwrap();
            replace_tables(&no_border_table, &mut working_text, &mut tables);
        }

        // Normalize HTML tags with attributes (`<table class="x">` → `<table>`).
        let tags = ["table", "td", "tr", "th", "tbody", "thead", "div"];
        let tag_pattern = Regex::new(&format!(r"(?i)<(?:{})[^>]*>", tags.join("|"))).unwrap();
        working_text = tag_pattern
            .replace_all(&working_text, |caps: &regex::Captures| {
                let raw = &caps[0];
                let tag_name = raw
                    .trim_start_matches('<')
                    .split(|c: char| c.is_whitespace() || c == '>')
                    .next()
                    .unwrap_or("")
                    .to_string();
                format!("<{tag_name}>")
            })
            .into_owned();

        if working_text.to_lowercase().contains("<table>") {
            // Raw HTML table with optional <html>/<body> wrappers.
            let html_table = Regex::new(
                r"(?is)(?:\n|^)\s*(?:(?:<html[^>]*>\s*<body[^>]*>\s*<table[^>]*>.*?</table>\s*</body>\s*</html>)|(?:<body[^>]*>\s*<table[^>]*>.*?</table>\s*</body>)|(?:<table[^>]*>.*?</table>))(?:\n|$)",
            )
            .unwrap();
            replace_tables(&html_table, &mut working_text, &mut tables);
        }

        (working_text, tables)
    }
}

/// Compatibility `Parse` adapter — parses markdown through the element
/// extractor and joins the blocks back into the document text.
#[derive(Default)]
pub struct MarkdownParser;

impl MarkdownParser {
    pub fn new() -> Self {
        Self
    }
}

impl crate::parser::Parse for MarkdownParser {
    fn parse(&self, name: &str, data: &[u8]) -> crate::Result<crate::Document> {
        let content = String::from_utf8(data.to_vec())
            .map_err(|e| anyhow::anyhow!("UTF-8 decode error: {e}"))?;
        let els = MarkdownElementExtractor::new(&content).extract_elements(None, false);
        let body = els
            .iter()
            .map(|e| e.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        Ok(crate::parser::new_document(
            name,
            body,
            "text/markdown",
            data.len(),
        ))
    }
}

/// Port of `MarkdownElementExtractor` (markdown_parser.py:125-321).
pub struct MarkdownElementExtractor<'a> {
    lines: Vec<&'a str>,
}

impl<'a> MarkdownElementExtractor<'a> {
    pub fn new(markdown_content: &'a str) -> Self {
        Self {
            lines: markdown_content.split('\n').collect(),
        }
    }

    /// `get_delimiters` — markdown_parser.py:130-133.
    /// Extracts backtick-wrapped tokens, dedups, sorts by length (descending)
    /// and joins with `|` (each regex-escaped).
    pub fn get_delimiters(delimiters: &str) -> String {
        let re = Regex::new(r"`([^`]+)`").unwrap();
        let mut toks: Vec<String> = re
            .captures_iter(delimiters)
            .filter_map(|c| c.get(1).map(|m| m.as_str().to_string()))
            .collect();
        toks.sort_by(|a, b| b.len().cmp(&a.len()));
        toks.dedup();
        toks.into_iter()
            .filter(|t| !t.is_empty())
            .map(|t| regex::escape(&t))
            .collect::<Vec<_>>()
            .join("|")
    }

    /// `extract_elements` — markdown_parser.py:135-208.
    /// With an explicit delimiter, splits the whole text on it; otherwise
    /// walks lines and slices block elements. `include_meta` attaches
    /// start/end line numbers.
    pub fn extract_elements(
        &self,
        delimiter: Option<&str>,
        include_meta: bool,
    ) -> Vec<MarkdownElement> {
        let mut sections: Vec<MarkdownElement> = Vec::new();

        let dels = delimiter.map(Self::get_delimiters).unwrap_or_default();
        if !dels.is_empty() {
            let text = self.lines.join("\n");
            let pattern = Regex::new(&dels).unwrap();
            if include_meta {
                let mut last_end = 0usize;
                for m in pattern.find_iter(&text) {
                    let part = &text[last_end..m.start()];
                    if !part.trim().is_empty() {
                        sections.push(MarkdownElement::new(
                            part.trim().to_string(),
                            count_newlines(&text[..last_end]),
                            count_newlines(&text[..m.start()]),
                            None,
                        ));
                    }
                    last_end = m.end();
                }
                let part = &text[last_end..];
                if !part.trim().is_empty() {
                    sections.push(MarkdownElement::new(
                        part.trim().to_string(),
                        count_newlines(&text[..last_end]),
                        count_newlines(&text[..text.len()]),
                        None,
                    ));
                }
            } else {
                for part in pattern.split(&text) {
                    if !part.trim().is_empty() {
                        sections.push(MarkdownElement::new(part.trim().to_string(), 0, 0, None));
                    }
                }
            }
            return sections;
        }

        let header_re = Regex::new(r"^#{1,6}\s+.*$").unwrap();
        let bullet_re = Regex::new(r"^\s*[-*+]\s+.*$").unwrap();
        let ordered_re = Regex::new(r"^\s*\d+\.\s+.*$").unwrap();
        let indent_bullet_re = Regex::new(r"^\s{2,}[-*+]\s+.*$").unwrap();
        let indent_ordered_re = Regex::new(r"^\s{2,}\d+\.\s+.*$").unwrap();
        let indent_text_re = Regex::new(r"^\s+\w+.*$").unwrap();

        let mut i = 0usize;
        while i < self.lines.len() {
            let line = self.lines[i];

            if header_re.is_match(line) {
                let el = extract_header(&self.lines, i);
                // Advance from the element that was just extracted. Reading the
                // position back out of `sections` broke as soon as an element was
                // dropped (empty content) or its meta was cleared: `i` fell back to
                // the previous element's end and a multi-line markdown document
                // looped forever, appending sections until the process died. Upstream
                // uses `element["end_line"] + 1` (markdown_parser.py:385).
                i = el.end_line + 1;
                push_section(&mut sections, el, include_meta);
            } else if line.trim_start().starts_with("```") {
                let el = extract_code_block(&self.lines, i);
                i = el.end_line + 1;
                push_section(&mut sections, el, include_meta);
            } else if bullet_re.is_match(line) || ordered_re.is_match(line) {
                let el = extract_list_block(
                    &self.lines,
                    i,
                    &bullet_re,
                    &ordered_re,
                    &indent_bullet_re,
                    &indent_ordered_re,
                    &indent_text_re,
                );
                i = el.end_line + 1;
                push_section(&mut sections, el, include_meta);
            } else if line.trim_start().starts_with('>') {
                let el = extract_blockquote(&self.lines, i);
                i = el.end_line + 1;
                push_section(&mut sections, el, include_meta);
            } else if !line.trim().is_empty() {
                let el = extract_text_block(&self.lines, i, &header_re, &bullet_re, &ordered_re);
                i = el.end_line + 1;
                push_section(&mut sections, el, include_meta);
            } else {
                i += 1;
            }
        }

        sections
    }
}

/// Append one extracted element unless it is blank.
///
/// `include_meta` only decides whether the line numbers are kept; it must never
/// change where the extractor resumes, so the element is pushed with its own
/// positions either way (upstream filters blank sections after the walk,
/// markdown_parser.py:407-411).
fn push_section(sections: &mut Vec<MarkdownElement>, el: MarkdownElement, include_meta: bool) {
    if el.content.trim().is_empty() {
        return;
    }
    if include_meta {
        sections.push(el);
    } else {
        sections.push(MarkdownElement::new(
            el.content,
            el.start_line,
            el.end_line,
            el.element_type,
        ));
    }
}

fn count_newlines(s: &str) -> usize {
    s.bytes().filter(|&b| b == b'\n').count()
}

/// `_extract_header` — markdown_parser.py:210-216.
fn extract_header(lines: &[&str], start: usize) -> MarkdownElement {
    MarkdownElement::new(
        lines[start].to_string(),
        start,
        start,
        Some("header".into()),
    )
}

/// `_extract_code_block` — markdown_parser.py:218-234.
fn extract_code_block(lines: &[&str], start: usize) -> MarkdownElement {
    let mut end = start;
    let mut content = vec![lines[start]];
    for i in (start + 1)..lines.len() {
        content.push(lines[i]);
        end = i;
        if lines[i].trim_start().starts_with("```") {
            break;
        }
    }
    MarkdownElement::new(content.join("\n"), start, end, Some("code_block".into()))
}

/// `_extract_list_block` — markdown_parser.py:236-263.
fn extract_list_block(
    lines: &[&str],
    start: usize,
    bullet_re: &Regex,
    ordered_re: &Regex,
    indent_bullet_re: &Regex,
    indent_ordered_re: &Regex,
    indent_text_re: &Regex,
) -> MarkdownElement {
    let mut end = start;
    let mut content = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if bullet_re.is_match(line)
            || ordered_re.is_match(line)
            || (i > start && line.trim().is_empty())
            || (i > start && indent_bullet_re.is_match(line))
            || (i > start && indent_ordered_re.is_match(line))
            || (i > start && indent_text_re.is_match(line))
        {
            content.push(line);
            end = i;
            i += 1;
        } else {
            break;
        }
    }
    MarkdownElement::new(content.join("\n"), start, end, Some("list_block".into()))
}

/// `_extract_blockquote` — markdown_parser.py:265-284.
fn extract_blockquote(lines: &[&str], start: usize) -> MarkdownElement {
    let mut end = start;
    let mut content = Vec::new();
    let mut i = start;
    while i < lines.len() {
        let line = lines[i];
        if line.trim_start().starts_with('>') || (i > start && line.trim().is_empty()) {
            content.push(line);
            end = i;
            i += 1;
        } else {
            break;
        }
    }
    MarkdownElement::new(content.join("\n"), start, end, Some("blockquote".into()))
}

/// `_extract_text_block` — markdown_parser.py:286-321.
fn extract_text_block(
    lines: &[&str],
    start: usize,
    header_re: &Regex,
    bullet_re: &Regex,
    ordered_re: &Regex,
) -> MarkdownElement {
    let mut end = start;
    let mut content = vec![lines[start]];
    let mut i = start + 1;
    while i < lines.len() {
        let line = lines[i];
        let is_block = header_re.is_match(line)
            || line.trim_start().starts_with("```")
            || bullet_re.is_match(line)
            || ordered_re.is_match(line)
            || line.trim_start().starts_with('>');
        if is_block {
            break;
        } else if line.trim().is_empty() {
            // Blank line: only stop if the NEXT line is a block element.
            if i + 1 < lines.len()
                && (header_re.is_match(lines[i + 1])
                    || lines[i + 1].trim_start().starts_with("```")
                    || bullet_re.is_match(lines[i + 1])
                    || ordered_re.is_match(lines[i + 1])
                    || lines[i + 1].trim_start().starts_with('>'))
            {
                break;
            } else {
                content.push(line);
                end = i;
                i += 1;
            }
        } else {
            content.push(line);
            end = i;
            i += 1;
        }
    }
    MarkdownElement::new(content.join("\n"), start, end, Some("text_block".into()))
}


