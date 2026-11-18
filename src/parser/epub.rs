//! EPUB parser — full port of RAGFlow `deepdoc/parser/epub_parser.py`.
//!
//! Reads the EPUB ZIP container: `META-INF/container.xml` → OPF package →
//! `<manifest>` (id → href + media-type) → `<spine>` (idref reading order) →
//! each XHTML content item is delegated to `RAGFlowHtmlParser` for chunking.
//! Falls back to alphabetically sorted `.xhtml/.html/.htm` files (excluding
//! `META-INF/`) when the container/OPF is missing or malformed.

use crate::parser::html::RAGFlowHtmlParser;
use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::io::Cursor;

/// OPF XML namespace (epub_parser.py:26).
const OPF_NS: &str = "http://www.idpf.org/2007/opf";
/// OCF container namespace (epub_parser.py:27).
const CONTAINER_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:container";

/// Media types whose content is readable XHTML (epub_parser.py:30).
const XHTML_MEDIA_TYPES: &[&str] = &["application/xhtml+xml", "text/html", "text/xml"];

/// Port of `RAGFlowEpubParser` (epub_parser.py:35-145).
#[derive(Default)]
pub struct RAGFlowEpubParser;

impl RAGFlowEpubParser {
    pub fn new() -> Self {
        Self
    }

    /// `__call__` — epub_parser.py:39-73. Returns chunked sections for every
    /// spine item in reading order.
    pub fn parse_binary(&self, binary: &[u8], chunk_token_num: usize) -> Result<Vec<String>> {
        if binary.is_empty() {
            return Err(anyhow::anyhow!("Empty EPUB binary payload"));
        }
        let cursor = Cursor::new(binary);
        let mut zf =
            zip::ZipArchive::new(cursor).map_err(|e| anyhow::anyhow!("Invalid EPUB zip: {e}"))?;

        let content_items = self.get_spine_items(&mut zf);
        let mut all_sections = Vec::new();
        let html_parser = RAGFlowHtmlParser::new();

        for item_path in &content_items {
            // Index lookup avoids borrowing conflicts with read_to_end.
            let mut found: Option<Vec<u8>> = None;
            for i in 0..zf.len() {
                let mut f = match zf.by_index(i) {
                    Ok(f) => f,
                    Err(error) => {
                        tracing::warn!(%error, index = i, "Skipping an unreadable EPUB archive entry");
                        continue;
                    }
                };
                if f.name() == item_path {
                    // Capped read: an epub chapter is archive-controlled data.
                    let declared = f.size();
                    let buf = crate::parser::read_member_limited(
                        &mut f,
                        declared,
                        crate::parser::MAX_ZIP_MEMBER_BYTES,
                        &format!("EPUB item '{item_path}'"),
                    )
                    .unwrap_or_default();
                    found = Some(buf);
                    break;
                }
            }
            let Some(html_bytes) = found else {
                continue; // KeyError → skip
            };
            if html_bytes.is_empty() {
                continue;
            }
            let html = String::from_utf8_lossy(&html_bytes).to_string();
            let sections = html_parser.parser_txt(&html, chunk_token_num);
            all_sections.extend(sections);
        }
        Ok(all_sections)
    }

    /// `_get_spine_items` — epub_parser.py:76-135.
    fn get_spine_items(&self, zf: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Vec<String> {
        // 1. container.xml (index lookup avoids borrow overlap with fallback).
        let container_xml = match read_entry(zf, "META-INF/container.xml") {
            Some(s) => s,
            None => return Self::fallback_xhtml_order(zf),
        };
        let opf_path = match parse_container(&container_xml) {
            Some(p) if !p.is_empty() => p,
            _ => return Self::fallback_xhtml_order(zf),
        };

        // 2. OPF package
        let opf_xml = match read_entry(zf, &opf_path) {
            Some(s) => s,
            None => return Self::fallback_xhtml_order(zf),
        };
        let (manifest, spine_ids) = match parse_opf(&opf_xml) {
            Some(v) => v,
            None => return Self::fallback_xhtml_order(zf),
        };
        if spine_ids.is_empty() {
            return Self::fallback_xhtml_order(zf);
        }

        // Base directory of the OPF file.
        let opf_dir = match opf_path.rfind('/') {
            Some(idx) => opf_path[..idx + 1].to_string(),
            None => String::new(),
        };

        // 3. Walk spine in reading order.
        let mut spine_items = Vec::new();
        for idref in spine_ids {
            let Some((href, media_type)) = manifest.get(&idref) else {
                continue;
            };
            if !XHTML_MEDIA_TYPES.contains(&media_type.as_str()) {
                continue;
            }
            spine_items.push(format!("{opf_dir}{href}"));
        }
        if spine_items.is_empty() {
            Self::fallback_xhtml_order(zf)
        } else {
            spine_items
        }
    }

    /// `_fallback_xhtml_order` — epub_parser.py:137-145.
    fn fallback_xhtml_order(zf: &zip::ZipArchive<Cursor<&[u8]>>) -> Vec<String> {
        let mut names: Vec<String> = zf
            .file_names()
            .filter(|n| {
                let lower = n.to_lowercase();
                (lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm"))
                    && !n.starts_with("META-INF/")
            })
            .map(str::to_string)
            .collect();
        names.sort();
        names
    }
}

/// Read a named zip entry as a UTF-8 string (None on missing/read error).
fn read_entry(zf: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Option<String> {
    let mut found: Option<Vec<u8>> = None;
    for i in 0..zf.len() {
        let mut f = zf.by_index(i).ok()?;
        if f.name() == name {
            let declared = f.size();
            let buf = crate::parser::read_member_limited(
                &mut f,
                declared,
                crate::parser::MAX_ZIP_MEMBER_BYTES,
                &format!("EPUB entry '{name}'"),
            )
            .ok()?;
            found = Some(buf);
            break;
        }
    }
    found.map(|b| String::from_utf8_lossy(&b).to_string())
}

/// Parse `META-INF/container.xml`, returning the OPF `full-path` attribute.
fn parse_container(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_rootfile = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() == b"rootfile" {
                    in_rootfile = true;
                    // Resolve attributes: full-path may be namespaced.
                    for attr in e.attributes().flatten() {
                        let key =
                            String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
                        if key == "full-path" {
                            let v = String::from_utf8_lossy(&attr.value).to_string();
                            if !v.is_empty() {
                                return Some(v);
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                tracing::warn!(%error, "EPUB XML stream failed; the spine/manifest scan stops here");
                return None;
            }
            _ => {}
        }
        buf.clear();
    }
    let _ = in_rootfile;
    None
}

/// Parse the OPF package: returns (manifest id→(href, media-type), spine idrefs).
fn parse_opf(
    xml: &str,
) -> Option<(
    std::collections::HashMap<String, (String, String)>,
    Vec<String>,
)> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut manifest: std::collections::HashMap<String, (String, String)> =
        std::collections::HashMap::new();
    let mut spine_ids: Vec<String> = Vec::new();
    let mut depth = 0usize;
    let mut in_manifest = false;
    let mut in_spine = false;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = e.local_name().as_ref().to_vec();
                depth += 1;
                match name.as_slice() {
                    b"manifest" => in_manifest = true,
                    b"spine" => in_spine = true,
                    b"item" if in_manifest => {
                        let mut id = String::new();
                        let mut href = String::new();
                        let mut media_type = String::new();
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            match key.as_str() {
                                "id" => id = val,
                                "href" => href = val,
                                "media-type" => media_type = val,
                                _ => {}
                            }
                        }
                        if !id.is_empty() && !href.is_empty() {
                            manifest.insert(id, (href, media_type));
                        }
                    }
                    b"itemref" if in_spine => {
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
                            if key == "idref" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                if !val.is_empty() {
                                    spine_ids.push(val);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"item" if in_manifest => {
                        let mut id = String::new();
                        let mut href = String::new();
                        let mut media_type = String::new();
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            match key.as_str() {
                                "id" => id = val,
                                "href" => href = val,
                                "media-type" => media_type = val,
                                _ => {}
                            }
                        }
                        if !id.is_empty() && !href.is_empty() {
                            manifest.insert(id, (href, media_type));
                        }
                    }
                    b"itemref" if in_spine => {
                        for attr in e.attributes().flatten() {
                            let key =
                                String::from_utf8_lossy(attr.key.local_name().as_ref()).to_string();
                            if key == "idref" {
                                let val = String::from_utf8_lossy(&attr.value).to_string();
                                if !val.is_empty() {
                                    spine_ids.push(val);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = e.local_name().as_ref().to_vec();
                match name.as_slice() {
                    b"manifest" => in_manifest = false,
                    b"spine" => in_spine = false,
                    _ => {}
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                tracing::warn!(%error, "EPUB XML stream failed; the spine/manifest scan stops here");
                return None;
            }
            _ => {}
        }
        buf.clear();
    }
    if manifest.is_empty() && spine_ids.is_empty() {
        None
    } else {
        Some((manifest, spine_ids))
    }
}

/// Compatibility `Parse` adapter — joins all sections into the document body.
#[derive(Default)]
pub struct EpubParser {
    inner: RAGFlowEpubParser,
}

impl EpubParser {
    pub fn new() -> Self {
        Self {
            inner: RAGFlowEpubParser::new(),
        }
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        Ok(self.inner.parse_binary(data, 512)?.join("\n"))
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


