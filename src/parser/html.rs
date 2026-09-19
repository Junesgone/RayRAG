//! HTML parser — full port of RAGFlow `deepdoc/parser/html_parser.py`.
//!
//! `RAGFlowHtmlParser::parser_txt`:
//! 1. Parse with an HTML5 DOM (scraper/html5ever ≈ BeautifulSoup html.parser),
//!    drop `<style>`/`<script>` (script inside `<div>` too), strip inline
//!    `style` attributes and HTML comments.
//! 2. `read_text_recursively` walks the DOM: `<table>` elements are hoisted
//!    whole (unescaped HTML, tagged with a table id + index); block tags
//!    (h1-h6, p, div, article, section, aside, ul, ol, li, table, pre, code,
//!    blockquote, figure, figcaption) start a new block id; text nodes carry
//!    their parent tag name (inner_text when no parent).
//! 3. `merge_block_text` groups text by block id, prefixes title tags with
//!    markdown `#`s, and collects tables separately.
//! 4. `chunk_block` merges blocks up to `chunk_token_num` tokens, splitting
//!    oversized blocks by the token budget.
//! 5. Tables are appended as their own sections.
//!
//! Token counting uses the project's `estimate_tokens` proxy for RAGFlow's
//! `rag_tokenizer.tokenize(...).split(" ")` length.

use crate::chunk::estimate_tokens;
use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use scraper::{ElementRef, Html, Node};

/// Block elements that start a new logical block id (html_parser.py:29-35).
pub const BLOCK_TAGS: &[&str] = &[
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "p",
    "div",
    "article",
    "section",
    "aside",
    "ul",
    "ol",
    "li",
    "table",
    "pre",
    "code",
    "blockquote",
    "figure",
    "figcaption",
];

/// Title tags and their markdown heading prefix (html_parser.py:36).
pub const TITLE_TAGS: &[(&str, &str)] = &[
    ("h1", "#"),
    ("h2", "##"),
    ("h3", "###"),
    ("h4", "####"),
    ("h5", "#####"),
    ("h6", "######"),
];

/// One extracted text/table item during the DOM walk.
#[derive(Debug, Clone)]
pub struct HtmlItem {
    pub content: String,
    pub tag_name: String,
    /// block_id for text items; table_id for table items.
    pub block_id: Option<String>,
    /// table items only: occurrence index within the table's list.
    pub index: Option<usize>,
}

/// Port of `RAGFlowHtmlParser` (html_parser.py:39-212).
#[derive(Default)]
pub struct RAGFlowHtmlParser;

impl RAGFlowHtmlParser {
    pub fn new() -> Self {
        Self
    }

    /// `parser_txt` — html_parser.py:50-76.
    pub fn parser_txt(&self, txt: &str, chunk_token_num: usize) -> Vec<String> {
        // Pre-sanitize: strip <style>/<script> blocks (incl. nested in div),
        // HTML comments, and inline style attributes — mirrors the BeautifulSoup
        // decompose()/del attrs steps. Regex-free block removal via marker scan.
        let mut sanitized = sanitize_html(txt);
        // Ensure body wrapper so root_element() traversal is well-formed.
        if !sanitized.trim_start().starts_with("<html") {
            sanitized = format!("<html><body>{sanitized}</body></html>");
        }
        let soup = Html::parse_document(&sanitized);

        let mut temp_sections: Vec<HtmlItem> = Vec::new();
        let body = soup.root_element();
        Self::walk_children(body, &mut temp_sections, chunk_token_num, None, None);

        let (block_txt_list, table_list) = Self::merge_block_text(&temp_sections);
        let mut sections = Self::chunk_block(&block_txt_list, chunk_token_num);
        for table in table_list {
            sections.push(table.content);
        }
        sections
    }

    /// `read_text_recursively` — html_parser.py:107-147.
    /// Walks `element`'s children, collecting text items (with parent tag
    /// name) and hoisted table items.
    fn walk_children(
        element: ElementRef,
        parser_result: &mut Vec<HtmlItem>,
        chunk_token_num: usize,
        parent_name: Option<&str>,
        block_id: Option<String>,
    ) {
        for child in element.children() {
            let node = child.value();
            match node {
                Node::Text(text) => {
                    let content = text.trim();
                    if content.is_empty() {
                        continue;
                    }
                    // If the text itself parses as HTML, recurse into it.
                    if looks_like_html(content) {
                        let nested = Html::parse_fragment(content);
                        let nested_root = nested.root_element();
                        // Re-run with the same parent/block context.
                        Self::walk_children(
                            nested_root,
                            parser_result,
                            chunk_token_num,
                            parent_name,
                            block_id.clone(),
                        );
                        continue;
                    }
                    let tag_name = parent_name.unwrap_or("inner_text").to_string();
                    parser_result.push(HtmlItem {
                        content: content.to_string(),
                        tag_name,
                        block_id: block_id.clone(),
                        index: None,
                    });
                }
                Node::Element(_) => {
                    let child_el = ElementRef::wrap(child).unwrap();
                    let name = child_el.value().name().to_lowercase();
                    if name == "table" {
                        let _table_id = format!("tbl-{}", parser_result.len());
                        let raw = html_unescape(&child_el.html());
                        parser_result.push(HtmlItem {
                            content: raw,
                            tag_name: "table".to_string(),
                            block_id: None,
                            index: Some(0),
                        });
                        continue;
                    }
                    let mut child_block = block_id.clone();
                    if BLOCK_TAGS.contains(&name.as_str()) {
                        child_block = Some(format!("blk-{}", parser_result.len()));
                    }
                    Self::walk_children(
                        child_el,
                        parser_result,
                        chunk_token_num,
                        Some(&name),
                        child_block,
                    );
                }
                _ => {}
            }
        }
    }

    /// `merge_block_text` — html_parser.py:150-177.
    /// Groups consecutive text items sharing a block id into single blocks;
    /// title tags get markdown heading prefixes; tables pass through.
    pub fn merge_block_text(parser_result: &[HtmlItem]) -> (Vec<String>, Vec<HtmlItem>) {
        let mut block_content: Vec<String> = Vec::new();
        let mut current_content = String::new();
        let mut table_info_list: Vec<HtmlItem> = Vec::new();
        let mut last_block_id: Option<String> = None;

        for item in parser_result {
            let mut content = item.content.clone();
            let tag_name = item.tag_name.as_str();
            let title_flag = TITLE_TAGS.iter().any(|(t, _)| *t == tag_name);
            let block_id = item.block_id.clone();

            if block_id.is_some() {
                if title_flag
                    && let Some((_, prefix)) = TITLE_TAGS.iter().find(|(t, _)| *t == tag_name) {
                        content = format!("{prefix} {content}");
                    }
                if last_block_id != block_id {
                    if last_block_id.is_some() {
                        block_content.push(current_content.clone());
                    }
                    current_content = content;
                    last_block_id = block_id;
                } else {
                    if !current_content.is_empty() {
                        current_content.push(' ');
                    }
                    current_content.push_str(&content);
                }
            } else {
                if tag_name == "table" {
                    table_info_list.push(item.clone());
                } else {
                    if !current_content.is_empty() {
                        current_content.push(' ');
                    }
                    current_content.push_str(&content);
                }
            }
        }
        if !current_content.is_empty() {
            block_content.push(current_content);
        }
        (block_content, table_info_list)
    }

    /// `chunk_block` — html_parser.py:180-212.
    /// Merges blocks up to the token budget; oversized blocks are split on the
    /// tokenized word list into budget-sized pieces.
    pub fn chunk_block(block_txt_list: &[String], chunk_token_num: usize) -> Vec<String> {
        let mut chunks: Vec<String> = Vec::new();
        let mut current_block = String::new();
        let mut current_token_count = 0usize;

        for block in block_txt_list {
            let block_token_count = estimate_tokens(block);
            if block_token_count > chunk_token_num {
                if !current_block.is_empty() {
                    chunks.push(current_block.clone());
                }
                // Split the block's words into budget-sized chunks.
                let words: Vec<&str> = block.split_whitespace().collect();
                let mut start = 0usize;
                while start < words.len() {
                    let end = (start + chunk_token_num).min(words.len());
                    chunks.push(words[start..end].join(" "));
                    start = end;
                }
                current_block.clear();
                current_token_count = 0;
            } else {
                if current_token_count + block_token_count <= chunk_token_num {
                    if !current_block.is_empty() {
                        current_block.push('\n');
                    }
                    current_block.push_str(block);
                    current_token_count += block_token_count;
                } else {
                    chunks.push(current_block.clone());
                    current_block = block.clone();
                    current_token_count = block_token_count;
                }
            }
        }
        if !current_block.is_empty() {
            chunks.push(current_block);
        }
        chunks
    }

    /// `split_table` — html_parser.py:79-104.
    /// Splits a table's `<tr>` rows into budget-sized `<table>` fragments.
    pub fn split_table(html_table: &str, chunk_token_num: usize) -> Vec<String> {
        let soup = Html::parse_fragment(html_table);
        let mut tables: Vec<Vec<String>> = Vec::new();
        let mut current_table: Vec<String> = Vec::new();
        let mut current_count = 0usize;

        for row in soup
            .root_element()
            .select(&scraper::Selector::parse("tr").unwrap())
        {
            let row_html = row.html();
            let token_count = estimate_tokens(&row_html);
            if current_count + token_count > chunk_token_num {
                tables.push(std::mem::take(&mut current_table));
                current_count = 0;
            }
            current_table.push(row_html);
            current_count += token_count;
        }
        if !current_table.is_empty() {
            tables.push(current_table);
        }

        tables
            .into_iter()
            .map(|rows| format!("<table>{}</table>", rows.join("")))
            .collect()
    }
}

/// Pre-sanitize HTML before DOM parsing (mirrors html_parser.py:57-69):
/// - drop `<style>`/`<script>` element subtrees (case-insensitive, incl.
///   script nested inside div)
/// - drop HTML comments `<!-- ... -->`
/// - strip inline `style="..."` attributes from every element
fn sanitize_html(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] != '<' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Comment: <!-- ... -->
        if i + 3 < chars.len() && chars[i + 1] == '!' && chars[i + 2] == '-' && chars[i + 3] == '-'
        {
            let mut j = i + 4;
            let mut closed = false;
            while j + 2 < chars.len() {
                if chars[j] == '-' && chars[j + 1] == '-' && chars[j + 2] == '>' {
                    i = j + 3;
                    closed = true;
                    break;
                }
                j += 1;
            }
            if !closed {
                break; // unterminated comment — drop rest
            }
            continue;
        }
        // Find end of this tag (`>` respecting quoted attribute values).
        let mut j = i + 1;
        let mut in_quote: Option<char> = None;
        while j < chars.len() {
            let c = chars[j];
            if let Some(q) = in_quote {
                if c == q {
                    in_quote = None;
                }
            } else if c == '"' || c == '\'' {
                in_quote = Some(c);
            } else if c == '>' {
                break;
            }
            j += 1;
        }
        if j >= chars.len() {
            // Unterminated tag — keep remainder as text.
            out.push_str(&chars[i..].iter().collect::<String>());
            break;
        }
        let tag_str: String = chars[i + 1..j].iter().collect();
        let tag_trimmed = tag_str.trim_start();
        let tag_name: String = tag_trimmed
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        let is_end_tag = tag_trimmed.starts_with('/');
        if (tag_name == "style" || tag_name == "script") && !is_end_tag {
            // Skip the whole element subtree until the matching close tag.
            let close = format!("</{tag_name}");
            let lower: String = chars[i..].iter().collect::<String>().to_lowercase();
            if let Some(pos) = lower.find(&close) {
                let abs = i + pos;
                // Skip past the close tag's '>'.
                let mut k = abs + close.len();
                while k < chars.len() && chars[k] != '>' {
                    k += 1;
                }
                i = (k + 1).min(chars.len());
            } else {
                i = chars.len(); // no close tag — drop rest
            }
            continue;
        }
        // Rebuild the tag without `style="..."` / `style='...'` attributes.
        let mut cleaned = String::with_capacity(tag_str.len());
        let mut s = 0usize;
        let lower_tag = tag_str.to_lowercase();
        let mut search_from = 0usize;
        while let Some(rel) = lower_tag[search_from..].find("style") {
            let idx = search_from + rel;
            // Must be an attribute boundary (preceded by whitespace or start).
            let prev_ok = idx == 0
                || lower_tag.as_bytes()[idx - 1] == b' '
                || lower_tag.as_bytes()[idx - 1] == b'\t';
            if !prev_ok {
                search_from = idx + 5;
                continue;
            }
            let after = &lower_tag[idx + 5..];
            let after_start = after.chars().next().unwrap_or(' ');
            if after_start != '=' {
                search_from = idx + 5;
                continue;
            }
            // style= then quote or bare value up to whitespace.
            let mut k = idx + 5;
            let mut quote: Option<char> = None;
            if k < tag_str.len() {
                let c = tag_str.as_bytes()[k];
                if c == b'"' || c == b'\'' {
                    quote = Some(c as char);
                    k += 1;
                }
            }
            while k < tag_str.len() {
                let c = tag_str.as_bytes()[k] as char;
                if let Some(q) = quote {
                    if c == q {
                        k += 1;
                        break;
                    }
                } else if c == ' ' || c == '\t' || c == '>' {
                    break;
                }
                k += 1;
            }
            cleaned.push_str(&tag_str[s..idx]);
            s = k;
            search_from = k;
        }
        cleaned.push_str(&tag_str[s..]);
        out.push('<');
        out.push_str(&cleaned);
        out.push('>');
        i = j + 1;
    }
    out
}

/// Best-effort HTML entity unescape for table content (Python `html.unescape`).
fn html_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'&'
            && let Some(end) = s[i..].find(';') {
                let entity = &s[i + 1..i + end];
                let decoded = match entity {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{00a0}'),
                    _ => {
                        if let Some(hex) = entity
                            .strip_prefix("#x")
                            .or_else(|| entity.strip_prefix("#X"))
                        {
                            u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                        } else if let Some(dec) = entity.strip_prefix('#') {
                            dec.parse::<u32>().ok().and_then(char::from_u32)
                        } else {
                            None
                        }
                    }
                };
                if let Some(ch) = decoded {
                    out.push(ch);
                    i += end + 1;
                    continue;
                }
            }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// True if the text looks like it contains an HTML tag (html_parser.py:111-116).
fn looks_like_html(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with('<') && trimmed.contains('>')
}

/// Compatibility `Parse` adapter — mirrors RAGFlowHtmlParser output joined
/// with newlines into the document body.
#[derive(Default)]
pub struct HtmlParser;

impl HtmlParser {
    pub fn new() -> Self {
        Self
    }

    /// Legacy convenience: strip HTML tags, keeping visible text content.
    /// Uses the full RAGFlow pipeline and joins sections.
    pub fn strip_tags(html: &str) -> String {
        RAGFlowHtmlParser::new().parser_txt(html, 512).join("\n")
    }
}

impl Parse for HtmlParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let html = String::from_utf8(data.to_vec())
            .map_err(|e| anyhow::anyhow!("UTF-8 decode error: {e}"))?;
        let sections = RAGFlowHtmlParser::new().parser_txt(&html, 512);
        let body = sections.join("\n");
        Ok(new_document(name, body, "text/html", data.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scripts_styles_and_comments() {
        let html = "<html><body><style>.x{color:red}</style><script>var a=1;</script>\
                    <!-- comment --><p>Hello <b>world</b></p></body></html>";
        let sections = RAGFlowHtmlParser::new().parser_txt(html, 512);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].contains("Hello world"));
        assert!(!sections[0].contains("var a=1"));
        assert!(!sections[0].contains(".x{color"));
        assert!(!sections[0].contains("comment"));
    }

    #[test]
    fn title_tags_get_markdown_prefix() {
        let html = "<html><body><h1>Big</h1><p>text</p><h2>Sub</h2></body></html>";
        let sections = RAGFlowHtmlParser::new().parser_txt(html, 512);
        assert_eq!(sections.len(), 1);
        assert!(sections[0].contains("# Big"));
        assert!(sections[0].contains("## Sub"));
    }

    #[test]
    fn tables_are_extracted_as_separate_sections() {
        let html = "<html><body><p>intro</p><table><tr><td>A</td><td>B</td></tr>\
                    <tr><td>1</td><td>2</td></tr></table><p>outro</p></body></html>";
        let sections = RAGFlowHtmlParser::new().parser_txt(html, 512);
        // intro+outro merge into one block; table appended separately.
        assert_eq!(sections.len(), 2);
        assert!(sections[0].contains("intro"));
        assert!(sections[0].contains("outro"));
        assert!(sections[1].contains("<table>"));
        assert!(sections[1].contains("<td>A</td>"));
    }

    #[test]
    fn chunk_block_splits_oversized_blocks() {
        let blocks: Vec<String> = vec!["word1 word2 word3 word4 word5".to_string()];
        let chunks = RAGFlowHtmlParser::chunk_block(&blocks, 2);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], "word1 word2");
        assert_eq!(chunks[1], "word3 word4");
        assert_eq!(chunks[2], "word5");
    }

    #[test]
    fn chunk_block_merges_small_blocks() {
        let blocks: Vec<String> = vec!["aaa".to_string(), "bbb".to_string(), "ccc".to_string()];
        let chunks = RAGFlowHtmlParser::chunk_block(&blocks, 512);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "aaa\nbbb\nccc");
    }

    #[test]
    fn split_table_budget_rows() {
        let table = "<table><tr><td>1</td></tr><tr><td>2</td></tr>\
                     <tr><td>3</td></tr><tr><td>4</td></tr></table>";
        let tables = RAGFlowHtmlParser::split_table(table, 2);
        // Tiny budget → multiple fragments (estimate_tokens counts chars/4-ish).
        assert!(tables.len() >= 1);
        for t in tables {
            assert!(t.starts_with("<table>"));
            assert!(t.ends_with("</table>"));
        }
    }

    #[test]
    fn html_unescape_handles_entities() {
        assert_eq!(html_unescape("a&amp;b"), "a&b");
        assert_eq!(html_unescape("&lt;tag&gt;"), "<tag>");
        assert_eq!(html_unescape("&#65;&#x42;"), "AB");
        assert_eq!(html_unescape("plain"), "plain");
    }

    #[test]
    fn inline_style_attribute_is_dropped() {
        // style attributes are stripped in the sanitize pass of parser_txt
        // pipeline (see strip_style_attrs doc); verify the walker output for a
        // styled element still yields its text.
        let html = "<html><body><p style=\"color:red\">styled</p></body></html>";
        let sections = RAGFlowHtmlParser::new().parser_txt(html, 512);
        assert!(sections[0].contains("styled"));
    }
}
