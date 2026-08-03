//! PPT parser — extract text from PowerPoint slides.
//! PPTX is a ZIP container with slide XML files.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::io::Read;

#[derive(Default)]
pub struct PptParser;

impl PptParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        let cursor = std::io::Cursor::new(data);
        let mut archive = zip::ZipArchive::new(cursor)?;
        let mut text = String::new();

        // Collect slide filenames
        let mut slide_files: Vec<String> = Vec::new();
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i) {
                let name = file.name().to_lowercase();
                if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                    slide_files.push(file.name().to_string());
                }
            }
        }
        slide_files.sort_by_key(|name| slide_number(name).unwrap_or(usize::MAX));

        for (idx, slide_name) in slide_files.iter().enumerate() {
            let mut slide = match archive.by_name(slide_name) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let mut xml_str = String::new();
            if slide.read_to_string(&mut xml_str).is_err() {
                continue;
            }

            let slide_text = extract_text_from_xml(&xml_str)?;
            if !slide_text.trim().is_empty() {
                text.push_str(&format!("\n<!-- Slide {} -->\n", idx + 1));
                text.push_str(&slide_text);
                text.push('\n');
            }
        }

        Ok(text)
    }
}

impl Parse for PptParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(data)?;
        Ok(new_document(
            name,
            content,
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            data.len(),
        ))
    }
}

/// Extract text from a slide XML by matching `<a:t>` tags.
fn extract_text_from_xml(xml: &str) -> Result<String> {
    let mut reader = Reader::from_str(xml);
    let mut parts = Vec::new();
    let mut in_text = false;
    loop {
        match reader.read_event()? {
            Event::Start(event) if event.local_name().as_ref() == b"t" => in_text = true,
            Event::Text(event) if in_text => {
                let value = event.unescape()?;
                let value = value.trim();
                if !value.is_empty() {
                    parts.push(value.to_string());
                }
            }
            Event::CData(event) if in_text => {
                let value = String::from_utf8_lossy(event.as_ref());
                let value = value.trim();
                if !value.is_empty() {
                    parts.push(value.to_string());
                }
            }
            Event::End(event) if event.local_name().as_ref() == b"t" => in_text = false,
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(parts.join(" "))
}

fn slide_number(name: &str) -> Option<usize> {
    name.rsplit('/')
        .next()?
        .strip_prefix("slide")?
        .strip_suffix(".xml")?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppt_xml_decodes_entities_and_slide_numbers_sort_numerically() {
        assert_eq!(
            extract_text_from_xml(r#"<p:sld><a:t>A &amp; B</a:t><a:t>Second</a:t></p:sld>"#)
                .unwrap(),
            "A & B Second"
        );
        let mut slides = [
            "ppt/slides/slide10.xml",
            "ppt/slides/slide2.xml",
            "ppt/slides/slide1.xml",
        ];
        slides.sort_by_key(|name| slide_number(name).unwrap());
        assert_eq!(
            slides,
            [
                "ppt/slides/slide1.xml",
                "ppt/slides/slide2.xml",
                "ppt/slides/slide10.xml"
            ]
        );
    }
}
