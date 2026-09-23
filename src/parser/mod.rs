//! Document parsers — ported from RAGFlow's `deepdoc/parser/`.
//!
//! Each parser takes raw bytes and produces a `Document` with
//! extracted text content and metadata.

pub mod docling;
pub mod docx;
pub mod epub;
pub mod excel;
pub mod figure;
pub mod html;
pub mod json;
pub mod markdown;
pub mod mineru;
pub mod opendataloader;
pub mod paddleocr_layout;
pub mod pdf;
pub mod pdf_stream;
pub mod pdf_text;
pub mod pdfbox;
pub mod ppt;
pub mod resume;
pub mod somark;
pub mod tcadp;
pub mod txt;

use crate::{Document, Result};
use std::collections::HashMap;
use std::io::Read;
use uuid::Uuid;

/// Largest decompressed member accepted from a ZIP-based document (docx/xlsx/pptx/
/// epub) when the member is text or XML.
///
/// These formats are archives, and an archive declares how large each member is
/// *before* it is read: a few kilobytes on disk can claim gigabytes. Reading with a
/// cap (and refusing the declared size up front) keeps a malformed or hostile
/// document from turning into process memory.
pub const MAX_ZIP_MEMBER_BYTES: u64 = 64 << 20;

/// Largest decompressed member accepted for embedded media (images, audio).
pub const MAX_ZIP_MEDIA_BYTES: u64 = 128 << 20;

/// Read one archive member, refusing anything above `limit`.
///
/// `Read::take` bounds the read even when the member's declared size lies, and the
/// capacity is reserved from the *actual* size rather than the header's claim.
pub fn read_member_limited<R: Read>(
    reader: &mut R,
    declared_size: u64,
    limit: u64,
    what: &str,
) -> Result<Vec<u8>> {
    if declared_size > limit {
        anyhow::bail!(
            "{what} declares {declared_size} bytes, above the {} MiB safety limit",
            limit / (1024 * 1024)
        );
    }
    let mut bytes = Vec::with_capacity(declared_size.min(limit) as usize);
    reader.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        anyhow::bail!(
            "{what} exceeds the {} MiB safety limit while decompressing",
            limit / (1024 * 1024)
        );
    }
    Ok(bytes)
}

/// Trait for all document parsers.
pub trait Parse: Send + Sync {
    /// Parse raw bytes into a Document.
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document>;
}

/// Create a new document with common fields populated.
pub(crate) fn new_document(name: &str, content: String, mime_type: &str, size: usize) -> Document {
    Document {
        id: Uuid::new_v4(),
        name: name.to_string(),
        content,
        mime_type: mime_type.to_string(),
        size,
        metadata: HashMap::new(),
    }
}

/// Registry of available parsers, indexed by MIME type.
pub fn get_parser(mime_type: &str) -> Option<Box<dyn Parse>> {
    match mime_type {
        "application/pdf" => Some(Box::new(pdf::PdfParser::new())),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some(Box::new(docx::DocxParser::new()))
        }
        "text/plain" => Some(Box::new(txt::TxtParser::new())),
        "text/markdown" => Some(Box::new(markdown::MarkdownParser::new())),
        "text/html" => Some(Box::new(html::HtmlParser::new())),
        "text/csv" => Some(Box::new(excel::ExcelParser::new())),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => {
            Some(Box::new(excel::ExcelParser::new()))
        }
        "application/epub+zip" => Some(Box::new(epub::EpubParser::new())),
        "application/json" => Some(Box::new(json::JsonParser::new())),
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            Some(Box::new(ppt::PptParser::new()))
        }
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/svg+xml"
        | "image/tiff" => Some(Box::new(figure::FigureParser::new())),
        "application/xml" | "text/xml" => {
            Some(Box::new(opendataloader::OpenDataLoaderParser::new()))
        }
        _ => None,
    }
}

/// Infer MIME type from file extension.
pub fn mime_from_extension(name: &str) -> Option<&'static str> {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    match ext.to_lowercase().as_str() {
        "pdf" => Some("application/pdf"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "txt" => Some("text/plain"),
        "md" | "markdown" => Some("text/markdown"),
        "html" | "htm" => Some("text/html"),
        "csv" => Some("text/csv"),
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "epub" => Some("application/epub+zip"),
        "json" => Some("application/json"),
        "pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        "tiff" | "tif" => Some("image/tiff"),
        "xml" => Some("text/xml"),
        "yaml" | "yml" => Some("application/x-yaml"),
        "toml" => Some("application/toml"),
        "log" => Some("text/plain"),
        _ => None,
    }
}

#[cfg(test)]
mod zip_member_limit_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn oversized_members_are_refused_before_reading() {
        // A member that *declares* more than the limit is rejected up front: the
        // allocation never happens, which is the whole point (a few kilobytes on
        // disk can claim gigabytes).
        let mut reader = Cursor::new(vec![0u8; 32]);
        let error = read_member_limited(&mut reader, 8 << 30, MAX_ZIP_MEMBER_BYTES, "test part")
            .unwrap_err()
            .to_string();
        assert!(error.contains("safety limit"), "{error}");
        assert!(error.contains("declares"), "{error}");
    }

    #[test]
    fn members_that_lie_about_their_size_are_still_bounded() {
        // The header claims 1 byte, the stream keeps going: `take(limit + 1)` stops it
        // and the post-check reports the overflow instead of returning a giant buffer.
        let mut reader = Cursor::new(vec![b'x'; 4096]);
        let error = read_member_limited(&mut reader, 1, 1024, "lying part")
            .unwrap_err()
            .to_string();
        assert!(error.contains("while decompressing"), "{error}");
    }

    #[test]
    fn members_within_the_limit_are_returned_whole() {
        let payload = vec![b'y'; 2048];
        let mut reader = Cursor::new(payload.clone());
        let bytes =
            read_member_limited(&mut reader, payload.len() as u64, 4096, "small part").unwrap();
        assert_eq!(bytes, payload);
    }
}
