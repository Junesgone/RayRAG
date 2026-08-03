//! HTML parser — ported from RAGFlow's `deepdoc/parser/html_parser.py`.
//!
//! Strips HTML tags to extract plain text content.
//! Captures image descriptions for contextual chunking.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};

#[derive(Default)]
pub struct HtmlParser;

impl HtmlParser {
    pub fn new() -> Self {
        Self
    }

    /// Strip HTML tags, keeping text content. Captures image alt text.
    pub(crate) fn strip_tags(html: &str) -> String {
        let mut result = String::new();
        let mut in_tag = false;
        let mut in_script = false;
        let mut in_style = false;
        let mut tag_name = String::new();
        let mut img_alt = String::new();
        let mut this_is_img = false;
        let mut reading_alt = false;
        let chars: Vec<char> = html.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let c = chars[i];

            if c == '<' {
                in_tag = true;
                tag_name.clear();
                img_alt.clear();
                this_is_img = false;
                reading_alt = false;

                // Check for script/style blocks
                let remaining: String =
                    chars[i..].iter().take(7).collect::<String>().to_lowercase();
                if remaining.starts_with("<script") {
                    in_script = true;
                } else if remaining.starts_with("<style") {
                    in_style = true;
                }
            } else if c == '>' {
                in_tag = false;
                if this_is_img && !img_alt.is_empty() {
                    result.push_str("\n<!--IMAGE_START-->\n");
                    result.push_str(&img_alt);
                    result.push_str("\n<!--IMAGE_END-->\n");
                }
                if in_script
                    && let Some(pos) = chars[i..]
                        .iter()
                        .collect::<String>()
                        .to_lowercase()
                        .find("</script>")
                {
                    i += pos + 9;
                    in_script = false;
                    continue;
                }
                if in_style
                    && let Some(pos) = chars[i..]
                        .iter()
                        .collect::<String>()
                        .to_lowercase()
                        .find("</style>")
                {
                    i += pos + 8;
                    in_style = false;
                    continue;
                }
            } else if in_tag && !in_script && !in_style {
                // Track tag name and alt attribute
                tag_name.push(c);
                let tag_lower = tag_name.to_lowercase();
                if tag_lower == "img " || tag_lower.starts_with("img ") {
                    this_is_img = true;
                }
                if this_is_img && reading_alt {
                    img_alt.push(c);
                }
                // Detect alt="
                let recent = tag_name.to_lowercase();
                if recent.ends_with("alt=\"") || recent.ends_with("alt='") {
                    reading_alt = true;
                    img_alt.clear(); // restart
                }
                if reading_alt && (c == '"' || c == '\'') && img_alt.len() > 1 {
                    reading_alt = false;
                    img_alt.pop(); // remove closing quote
                }
            } else if !in_tag && !in_script && !in_style {
                result.push(c);
            }

            i += 1;
        }

        // Collapse whitespace
        result
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Parse for HtmlParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let html = String::from_utf8(data.to_vec())
            .map_err(|e| anyhow::anyhow!("UTF-8 decode error: {}", e))?;
        let content = Self::strip_tags(&html);
        Ok(new_document(name, content, "text/html", data.len()))
    }
}
