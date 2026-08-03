//! EPUB parser — extract text from e-book files.
//! EPUB is a ZIP container with HTML chapters.
//! Strips HTML tags, preserves structure.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use std::io::Read;

#[derive(Default)]
pub struct EpubParser;

impl EpubParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        let cursor = std::io::Cursor::new(data);
        let mut archive = zip::ZipArchive::new(cursor)?;
        let mut text = String::new();

        // EPUB content is in .html/.xhtml files (skip metadata/nav)
        for i in 0..archive.len() {
            let mut file = match archive.by_index(i) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let name = file.name().to_lowercase();
            if !name.ends_with(".html") && !name.ends_with(".xhtml") && !name.ends_with(".htm") {
                continue;
            }

            // Skip navigation/toc files
            if name.contains("nav") || name.contains("toc") {
                continue;
            }

            let mut content = String::new();
            if file.read_to_string(&mut content).is_ok() {
                // Strip HTML tags (simple version)
                let stripped = strip_html(&content);
                if !stripped.trim().is_empty() {
                    text.push_str(&stripped);
                    text.push_str("\n\n");
                }
            }
        }

        Ok(text)
    }
}

impl Parse for EpubParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(data)?;
        Ok(new_document(
            name,
            content,
            "application/epub+zip",
            data.len(),
        ))
    }
}

/// Simple HTML tag stripper.
fn strip_html(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    let mut in_script = false;
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            in_tag = true;
            let remaining: String = chars[i..]
                .iter()
                .take(10)
                .collect::<String>()
                .to_lowercase();
            if remaining.starts_with("<script") || remaining.starts_with("<style") {
                in_script = true;
            }
        } else if c == '>' {
            in_tag = false;
            if in_script {
                let rest: String = chars[i..].iter().collect::<String>().to_lowercase();
                if let Some(end) = rest.find("</script>").or_else(|| rest.find("</style>")) {
                    i += end + 9;
                    in_script = false;
                    continue;
                }
            }
        } else if !in_tag && !in_script {
            result.push(c);
        }
        i += 1;
    }

    result
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
