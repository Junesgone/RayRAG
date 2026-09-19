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
                push_section(&mut sections, el, include_meta);
                i = sections.last().map(|s| s.end_line + 1).unwrap_or(i + 1);
            } else if line.trim_start().starts_with("```") {
                let el = extract_code_block(&self.lines, i);
                push_section(&mut sections, el, include_meta);
                i = sections.last().map(|s| s.end_line + 1).unwrap_or(i + 1);
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
                push_section(&mut sections, el, include_meta);
                i = sections.last().map(|s| s.end_line + 1).unwrap_or(i + 1);
            } else if line.trim_start().starts_with('>') {
                let el = extract_blockquote(&self.lines, i);
                push_section(&mut sections, el, include_meta);
                i = sections.last().map(|s| s.end_line + 1).unwrap_or(i + 1);
            } else if !line.trim().is_empty() {
                let el = extract_text_block(&self.lines, i, &header_re, &bullet_re, &ordered_re);
                push_section(&mut sections, el, include_meta);
                i = sections.last().map(|s| s.end_line + 1).unwrap_or(i + 1);
            } else {
                i += 1;
            }
        }

        sections
    }
}

fn push_section(sections: &mut Vec<MarkdownElement>, el: MarkdownElement, include_meta: bool) {
    if include_meta {
        if !el.content.trim().is_empty() {
            sections.push(el);
        }
    } else if !el.content.trim().is_empty() {
        sections.push(MarkdownElement::new(el.content, 0, 0, el.element_type));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bordered_table_and_remainder() {
        let md = "Intro text\n\n| Name | Age |\n| --- | --- |\n| Alice | 30 |\n| Bob | 25 |\n\nOutro text";
        let (text, tables) = RAGFlowMarkdownParser::new().extract_tables_and_remainder(md, true);
        assert_eq!(tables.len(), 1);
        assert!(tables[0].contains("| Name | Age |"));
        assert!(tables[0].contains("| Alice | 30 |"));
        // Table removed from body.
        assert!(!text.contains("| Name | Age |"));
        assert!(text.contains("Intro text"));
        assert!(text.contains("Outro text"));
    }

    #[test]
    fn extract_borderless_table() {
        let md = "Before\n\nName | Age\n--- | ---\nAlice | 30\n\nAfter";
        let (text, tables) = RAGFlowMarkdownParser::new().extract_tables_and_remainder(md, true);
        assert_eq!(tables.len(), 1);
        assert!(tables[0].contains("Alice | 30"));
        assert!(!text.contains("Alice | 30"));
        assert!(text.contains("Before"));
        assert!(text.contains("After"));
    }

    #[test]
    fn extract_html_table_with_wrappers() {
        let md = "Text\n\n<html><body><table><tr><td>A</td></tr></table></body></html>\n\nMore";
        let (text, tables) = RAGFlowMarkdownParser::new().extract_tables_and_remainder(md, true);
        assert_eq!(tables.len(), 1);
        assert!(tables[0].contains("<table>"));
        assert!(!text.contains("<table>"));
    }

    #[test]
    fn normalize_table_tag_attributes() {
        let md = "x <table class=\"t\"><tr><td>1</td></tr></table> y";
        let (text, _) = RAGFlowMarkdownParser::new().extract_tables_and_remainder(md, false);
        assert!(text.contains("<table>"));
        assert!(!text.contains("class=\"t\""));
    }

    #[test]
    fn keep_tables_inline_when_not_separate() {
        // Border table regex requires a trailing newline (last row `\n`).
        let md = "| A | B |\n| - | - |\n| 1 | 2 |\n";
        let (text, tables) = RAGFlowMarkdownParser::new().extract_tables_and_remainder(md, false);
        assert_eq!(tables.len(), 1);
        // Table left inline.
        assert!(text.contains("| A | B |"));
    }

    #[test]
    fn get_delimiters_sorts_by_length_and_escapes() {
        let dels = "use `[start]` and `code-block` tokens `[end]`".to_string();
        let out = MarkdownElementExtractor::get_delimiters(&dels);
        // Longest first, all escaped and joined by |.
        let parts: Vec<&str> = out.split('|').collect();
        assert!(parts[0].len() >= parts[1].len());
        assert!(out.contains("code\\-block"));
        assert!(out.contains("\\[start\\]"));
        assert!(out.contains("\\[end\\]"));
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn extract_elements_with_delimiter_no_meta() {
        let md = "before [sep] middle [sep] after";
        // Python: delimiter is a backtick-wrapped token list; the matched
        // text uses the bare token.
        let els = MarkdownElementExtractor::new(md).extract_elements(Some("`[sep]`"), false);
        assert_eq!(els.len(), 3);
        assert_eq!(els[0].content, "before");
        assert_eq!(els[1].content, "middle");
        assert_eq!(els[2].content, "after");
    }

    #[test]
    fn extract_elements_with_delimiter_meta() {
        let md = "one\n[sep]\ntwo\n[sep]\nthree";
        let els = MarkdownElementExtractor::new(md).extract_elements(Some("`[sep]`"), true);
        assert_eq!(els.len(), 3);
        assert_eq!(els[0].content, "one");
        assert_eq!(els[0].start_line, 0);
        assert_eq!(els[0].end_line, 1); // "one\n" — newline counts toward end.
        assert_eq!(els[1].content, "two");
        assert_eq!(els[1].start_line, 1); // after first "[sep]" (text[..9] has 1 \n).
        assert_eq!(els[1].end_line, 3); // text[..14] = "one\n[sep]\ntwo" has 3 \n.
        assert_eq!(els[2].content, "three");
        assert_eq!(els[2].start_line, 3);
        assert_eq!(els[2].end_line, 4); // full text has 4 \n.
    }

    #[test]
    fn extract_headers_code_lists_quotes_and_text() {
        let md = "# Title\n\nSome paragraph\nwith a second line.\n\n- item 1\n- item 2\n\n> quoted\n> more\n\n```rust\nfn main() {}\n```\n\n### Sub\n\nTail text";
        let els = MarkdownElementExtractor::new(md).extract_elements(None, true);
        let types: Vec<&str> = els
            .iter()
            .map(|e| e.element_type.as_deref().unwrap_or(""))
            .collect();
        assert_eq!(types[0], "header");
        assert_eq!(els[0].content, "# Title");
        assert_eq!(types[1], "text_block");
        assert!(els[1].content.contains("Some paragraph"));
        assert!(els[1].content.contains("second line"));
        assert_eq!(types[2], "list_block");
        // Python absorbs the trailing blank line into the list block.
        assert!(els[2].content.starts_with("- item 1"));
        assert!(els[2].content.contains("- item 2"));
        assert_eq!(types[3], "blockquote");
        // Python absorbs the trailing blank line into the blockquote too.
        assert!(els[3].content.starts_with("> quoted"));
        assert!(els[3].content.contains("> more"));
        assert_eq!(types[4], "code_block");
        assert!(els[4].content.contains("fn main()"));
        assert_eq!(types[5], "header");
        assert_eq!(els[5].content, "### Sub");
        assert_eq!(types[6], "text_block");
        assert_eq!(els[6].content, "Tail text");
    }

    #[test]
    fn list_block_absorbs_continuation_lines() {
        let md = "- a\n- b\n  nested\n  text\n\nnext para";
        let els = MarkdownElementExtractor::new(md).extract_elements(None, true);
        assert_eq!(els.len(), 2);
        assert_eq!(els[0].element_type.as_deref(), Some("list_block"));
        assert!(els[0].content.contains("  nested"));
        assert!(els[0].content.contains("  text"));
        assert_eq!(els[1].content, "next para");
    }

    #[test]
    fn text_block_stops_before_next_block_element() {
        let md = "para one\n\n- list item\n\npara two";
        let els = MarkdownElementExtractor::new(md).extract_elements(None, true);
        assert_eq!(els.len(), 3);
        assert_eq!(els[0].element_type.as_deref(), Some("text_block"));
        assert_eq!(els[0].content, "para one");
        assert_eq!(els[1].element_type.as_deref(), Some("list_block"));
        assert_eq!(els[2].element_type.as_deref(), Some("text_block"));
        assert_eq!(els[2].content, "para two");
    }
}
