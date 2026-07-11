//! HTML parser — ported from RAGFlow's `deepdoc/parser/html_parser.py`.
//!
//! Strips HTML tags to extract plain text content.

use crate::parser::{new_document, Parse};
use crate::{Document, Result};

pub struct HtmlParser;

impl HtmlParser {
    pub fn new() -> Self {
        Self
    }

    /// Strip HTML tags, keeping text content.
    fn strip_tags(html: &str) -> String {
        let mut result = String::new();
        let mut in_tag = false;
        let mut in_script = false;
        let mut in_style = false;
        let chars: Vec<char> = html.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            let c = chars[i];

            if c == '<' {
                in_tag = true;
                // Check for script/style blocks
                let remaining: String = chars[i..]
                    .iter()
                    .take(7)
                    .collect::<String>()
                    .to_lowercase();
                if remaining.starts_with("<script") {
                    in_script = true;
                } else if remaining.starts_with("<style") {
                    in_style = true;
                }
            } else if c == '>' {
                in_tag = false;
                if in_script {
                    // Skip until </script>
                    if let Some(pos) = chars[i..]
                        .iter()
                        .collect::<String>()
                        .to_lowercase()
                        .find("</script>")
                    {
                        i += pos + 9; // skip past </script>
                        in_script = false;
                        continue;
                    }
                }
                if in_style {
                    if let Some(pos) = chars[i..]
                        .iter()
                        .collect::<String>()
                        .to_lowercase()
                        .find("</style>")
                    {
                        i += pos + 8;
                        in_style = false;
                        continue;
                    }
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
