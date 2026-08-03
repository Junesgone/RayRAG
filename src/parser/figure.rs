//! Figure / Image parser — handles PNG, JPEG, GIF, WebP, BMP, SVG, TIFF.
//!
//! Strategy:
//! - SVG: extract text from XML tags
//! - Raster images: call paddleocr-gpu-proxy for OCR, fallback to filename + metadata
//! - Embeds result as image context marker for pipeline

use crate::ocr::OcrClient;
use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Default)]
pub struct FigureParser {
    ocr: Option<OcrClient>,
}

impl FigureParser {
    pub fn new() -> Self {
        Self { ocr: None }
    }

    /// Enable OCR for raster images.
    pub fn with_ocr(ocr: OcrClient) -> Self {
        Self { ocr: Some(ocr) }
    }

    fn extract_text(&self, name: &str, data: &[u8]) -> Result<String> {
        let ext = std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "svg" => parse_svg(data),
            _ => parse_raster(name, data, self.ocr.as_ref()),
        }
    }
}

impl Parse for FigureParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(name, data)?;
        let mime = match std::path::Path::new(name)
            .extension()
            .and_then(|e| e.to_str())
        {
            Some("png") => "image/png",
            Some("jpg") | Some("jpeg") => "image/jpeg",
            Some("gif") => "image/gif",
            Some("webp") => "image/webp",
            Some("bmp") => "image/bmp",
            Some("svg") => "image/svg+xml",
            Some("tiff") | Some("tif") => "image/tiff",
            _ => "application/octet-stream",
        };
        Ok(new_document(name, content, mime, data.len()))
    }
}

/// Parse SVG: extract text from <text> and <tspan> elements.
fn parse_svg(data: &[u8]) -> Result<String> {
    let xml = String::from_utf8_lossy(data);
    let mut reader = Reader::from_str(&xml);
    let mut text = String::from("<!--IMAGE_START-->\n");
    let mut text_depth = 0usize;
    loop {
        match reader.read_event()? {
            Event::Start(event)
                if event.local_name().as_ref() == b"text"
                    || event.local_name().as_ref() == b"tspan" =>
            {
                text_depth += 1;
            }
            Event::Text(event) if text_depth > 0 => {
                let value = event.unescape()?;
                if !value.trim().is_empty() {
                    text.push_str(value.trim());
                    text.push('\n');
                }
            }
            Event::CData(event) if text_depth > 0 => {
                let value = String::from_utf8_lossy(event.as_ref());
                if !value.trim().is_empty() {
                    text.push_str(value.trim());
                    text.push('\n');
                }
            }
            Event::End(event)
                if event.local_name().as_ref() == b"text"
                    || event.local_name().as_ref() == b"tspan" =>
            {
                text_depth = text_depth.saturating_sub(1);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    text.push_str("<!--IMAGE_END-->\n");
    Ok(text)
}

/// Parse raster image: try OCR, fallback to filename metadata.
fn parse_raster(name: &str, data: &[u8], ocr: Option<&OcrClient>) -> Result<String> {
    let mut text = String::from("<!--IMAGE_START-->\n");

    // Add filename as context
    let basename = std::path::Path::new(name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    text.push_str(&format!("[Image: {}]\n", basename));

    // Try OCR if client is available
    if let Some(client) = ocr {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(client.ocr(data))
        {
            Ok(ocr_text) if !ocr_text.is_empty() => {
                text.push_str(&ocr_text);
            }
            _ => {
                text.push_str(&format!(
                    "[Image dimensions: {} bytes, format: {}]",
                    data.len(),
                    detect_format(data),
                ));
            }
        }
    } else {
        text.push_str(&format!(
            "[Image: {} bytes, format: {} — enable OCR for text extraction]",
            data.len(),
            detect_format(data),
        ));
    }

    text.push_str("\n<!--IMAGE_END-->\n");
    Ok(text)
}

/// Detect image format from magic bytes.
fn detect_format(data: &[u8]) -> &'static str {
    if data.len() < 4 {
        return "unknown";
    }
    match &data[0..4] {
        [0x89, b'P', b'N', b'G'] => "PNG",
        [0xFF, 0xD8, 0xFF, _] => "JPEG",
        [b'G', b'I', b'F', b'8'] => "GIF",
        [b'R', b'I', b'F', b'F'] => "WEBP/TIFF",
        [b'B', b'M', _, _] => "BMP",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_text_parsing_handles_nested_spans_and_entities() {
        let parsed = parse_svg(
            br#"<svg><text>Lead <tspan>A &amp; B</tspan> tail</text><path d="M0 0"/></svg>"#,
        )
        .unwrap();
        assert!(parsed.contains("Lead\nA & B\ntail\n"));
    }
}
