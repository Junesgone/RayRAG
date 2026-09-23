//! PPT parser — extract text from PowerPoint slides (PPTX).
//!
//! Mirrors RAGFlow `deepdoc/parser/ppt_parser.py` (RAGFlowPptParser):
//! - shapes are sorted by `(top // 10, left)` (EMU coordinates)
//! - text frames: per-paragraph text with bullet detection
//!   (`a:pPr/a:buChar|buAutoNum|buBlip` → `"  " * level + "." + text`)
//! - tables (shape_type 19): row 0 is the header; rows 1..N become
//!   `"header: value; header: value; ..."` lines
//! - group shapes (shape_type 6): recursive extraction, children sorted
//! - each slide yields one text block; `slide_texts` returns per-page texts
//!
//! PPTX is a ZIP container with `ppt/slides/slideN.xml` parts.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use std::io::Read;

/// Minimal XML node tree used to walk slide markup.
#[derive(Debug, Default, Clone)]
struct XmlNode {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<XmlNode>,
    text: String,
}

impl XmlNode {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn children_named(&self, name: &str) -> impl Iterator<Item = &XmlNode> {
        self.children.iter().filter(move |c| c.name == name)
    }

    fn first_child(&self, name: &str) -> Option<&XmlNode> {
        self.children_named(name).next()
    }
}

/// Parse an XML string into a tree of `XmlNode`s. Namespace prefixes are
/// stripped; text content is accumulated on the nearest element.
fn parse_xml(xml: &str) -> Result<XmlNode> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<XmlNode> = Vec::new();
    let mut root = XmlNode::default();
    let mut buf = String::new();
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Start(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let attrs = e
                    .attributes()
                    .filter_map(|a| a.ok())
                    .map(|a| {
                        (
                            String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned(),
                            String::from_utf8_lossy(&a.value).into_owned(),
                        )
                    })
                    .collect();
                stack.push(XmlNode {
                    name,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                });
            }
            quick_xml::events::Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let attrs = e
                    .attributes()
                    .filter_map(|a| a.ok())
                    .map(|a| {
                        (
                            String::from_utf8_lossy(a.key.local_name().as_ref()).into_owned(),
                            String::from_utf8_lossy(&a.value).into_owned(),
                        )
                    })
                    .collect();
                let node = XmlNode {
                    name,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(node),
                    None => root = node,
                }
            }
            quick_xml::events::Event::Text(e) => {
                let t = String::from_utf8_lossy(e.as_ref()).into_owned();
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&t);
                }
            }
            quick_xml::events::Event::End(_) => {
                if let Some(node) = stack.pop() {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(node),
                        None => root = node,
                    }
                }
            }
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    let _ = &mut buf;
    Ok(root)
}

/// One parsed shape: its position (EMU) and extracted text.
#[derive(Debug, Clone)]
struct Shape {
    top: i64,
    left: i64,
    text: String,
}

impl Shape {
    fn sort_key(&self) -> (i64, i64) {
        (self.top / 10, self.left)
    }
}

/// `__get_bulleted_text` — bullet detection via `a:pPr` children.
fn get_paragraph_text(paragraph: &XmlNode) -> String {
    let ppr = paragraph.first_child("pPr");
    let is_bulleted = ppr
        .map(|p| {
            p.children
                .iter()
                .any(|c| matches!(c.name.as_str(), "buChar" | "buAutoNum" | "buBlip"))
        })
        .unwrap_or(false);
    let level: usize = ppr
        .and_then(|p| p.attr("lvl"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let text = paragraph
        .children_named("r")
        .map(|r| {
            r.children_named("t")
                .map(|t| t.text.clone())
                .collect::<Vec<_>>()
                .join("")
        })
        .collect::<Vec<_>>()
        .join("");
    let text = text.trim().to_owned();
    if text.is_empty() {
        return String::new();
    }
    if is_bulleted {
        format!("{:indent$}.{text}", "", indent = level * 2)
    } else {
        text
    }
}

/// Text-frame extraction for `p:sp` shapes (shape_type text frame).
fn extract_text_frame(shape: &XmlNode) -> String {
    let mut texts = Vec::new();
    if let Some(tx_body) = shape.first_child("txBody") {
        for paragraph in tx_body.children_named("p") {
            let t = get_paragraph_text(paragraph);
            if !t.is_empty() {
                texts.push(t);
            }
        }
    }
    texts.join("\n")
}

/// Table extraction for `a:tbl` (shape_type 19): header row 0 + data rows.
fn extract_table(tbl: &XmlNode) -> String {
    let rows: Vec<Vec<String>> = tbl
        .children_named("tr")
        .map(|tr| {
            tr.children_named("tc")
                .map(|tc| {
                    tc.children_named("txBody")
                        .flat_map(|tb| tb.children_named("p"))
                        .map(|p| {
                            p.children_named("r")
                                .map(|r| {
                                    r.children_named("t")
                                        .map(|t| t.text.clone())
                                        .collect::<Vec<_>>()
                                        .join("")
                                })
                                .collect::<Vec<_>>()
                                .join("")
                                .trim()
                                .to_owned()
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect()
        })
        .collect();

    if rows.len() < 2 {
        return String::new();
    }
    let header = &rows[0];
    let mut lines = Vec::new();
    for row in rows.iter().skip(1) {
        let cells: Vec<String> = header
            .iter()
            .zip(row.iter())
            .map(|(h, v)| format!("{h}: {v}"))
            .collect();
        lines.push(cells.join("; "));
    }
    lines.join("\n")
}

/// `__extract` — recursive shape extraction.
fn extract_shape(node: &XmlNode) -> String {
    match node.name.as_str() {
        // Text frame (p:sp)
        "sp" => extract_text_frame(node),
        // Table (a:tbl)
        "tbl" => extract_table(node),
        // Group (p:grpSp)
        "grpSp" => {
            let mut texts = Vec::new();
            let mut shapes = collect_shapes(node);
            shapes.sort_by_key(Shape::sort_key);
            for s in shapes {
                if !s.text.is_empty() {
                    texts.push(s.text);
                }
            }
            texts.join("\n")
        }
        _ => String::new(),
    }
}

/// Recursively collect shapes (sp / tbl / grpSp) with their positions.
fn collect_shapes(node: &XmlNode) -> Vec<Shape> {
    let mut out = Vec::new();
    for child in &node.children {
        match child.name.as_str() {
            "sp" | "tbl" | "grpSp" => {
                let (top, left) = shape_position(child);
                let text = extract_shape(child);
                out.push(Shape { top, left, text });
            }
            _ => {
                out.extend(collect_shapes(child));
            }
        }
    }
    out
}

/// Position from `p:xfrm/a:off` (EMU). Missing → 0 (python-pptx None → 0).
fn shape_position(node: &XmlNode) -> (i64, i64) {
    if let Some(xfrm) = node.first_child("xfrm")
        && let Some(off) = xfrm.first_child("off")
    {
        let x = off.attr("x").and_then(|v| v.parse().ok()).unwrap_or(0);
        let y = off.attr("y").and_then(|v| v.parse().ok()).unwrap_or(0);
        return (y, x);
    }
    (0, 0)
}

/// Extract all slide texts (one per slide), sorted by slide number.
/// `from_page`/`to_page` mirror `RAGFlowPptParser.__call__` (0-based,
/// `from_page <= i < to_page` kept); defaults include every slide.
pub fn slide_texts_paged(data: &[u8], from_page: usize, to_page: usize) -> Result<Vec<String>> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)?;

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

    let mut texts = Vec::with_capacity(slide_files.len());
    for (slide_index, slide_name) in slide_files.iter().enumerate() {
        // Python: `for i, slide in enumerate(ppt.slides)` with
        // `if i < from_page: continue; if i >= to_page: break`.
        if slide_index < from_page {
            continue;
        }
        if slide_index >= to_page {
            break;
        }
        let mut slide = match archive.by_name(slide_name) {
            Ok(f) => f,
            Err(_) => continue,
        };
        // Capped read: a pptx member's declared size is attacker-controlled, and an
        // unbounded read of it is a straight path to exhausting memory.
        let declared = slide.size();
        // A slide that is present but too large is an error, not a slide to skip
        // silently: the deck would otherwise lose content with no explanation.
        let bytes = crate::parser::read_member_limited(
            &mut slide,
            declared,
            crate::parser::MAX_ZIP_MEMBER_BYTES,
            &format!("PPTX slide '{slide_name}'"),
        )?;
        let xml_str = String::from_utf8(bytes)
            .map_err(|error| anyhow::anyhow!("PPTX slide '{slide_name}' is not UTF-8: {error}"))?;
        let root = parse_xml(&xml_str)?;
        let mut shapes = collect_shapes(&root);
        shapes.sort_by_key(Shape::sort_key);
        let slide_text = shapes
            .into_iter()
            .map(|s| s.text)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        texts.push(slide_text);
    }
    Ok(texts)
}

/// Backward-compatible wrapper — full page range.
pub fn slide_texts(data: &[u8]) -> Result<Vec<String>> {
    slide_texts_paged(data, 0, usize::MAX)
}

#[derive(Default)]
pub struct PptParser;

impl PptParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        let texts = slide_texts(data)?;
        let mut out = String::new();
        for (idx, text) in texts.iter().enumerate() {
            if !text.trim().is_empty() {
                out.push_str(&format!("\n<!-- Slide {} -->\n", idx + 1));
                out.push_str(text);
                out.push('\n');
            }
        }
        Ok(out)
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
    fn slide_xml_parses_into_tree() {
        let root = parse_xml(
            r#"<p:sld xmlns:p="x"><p:sp><p:xfrm><a:off x="100" y="200"/></p:xfrm></p:sp></p:sld>"#,
        )
        .unwrap();
        assert_eq!(root.name, "sld");
        let sp = root.first_child("sp").unwrap();
        let off = sp.first_child("xfrm").unwrap().first_child("off").unwrap();
        assert_eq!(off.attr("x"), Some("100"));
        assert_eq!(off.attr("y"), Some("200"));
    }

    #[test]
    fn bullet_detection_prefixes_level() {
        // Bulleted paragraph at level 2.
        let root = parse_xml(
            r#"<p:p><p:pPr lvl="2"><a:buChar char="&#8226;"/></p:pPr><a:r><a:t>item</a:t></a:r></p:p>"#,
        )
        .unwrap();
        let text = get_paragraph_text(&root);
        assert_eq!(text, "    .item");

        // Plain paragraph, no pPr → no prefix.
        let root2 = parse_xml(r#"<p:p><a:r><a:t>plain</a:t></a:r></p:p>"#).unwrap();
        assert_eq!(get_paragraph_text(&root2), "plain");
    }

    #[test]
    fn table_extraction_uses_header_row() {
        let root = parse_xml(
            r#"<a:tbl><a:tr><a:tc><a:txBody><a:p><a:r><a:t>name</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>age</a:t></a:r></a:p></a:txBody></a:tc></a:tr><a:tr><a:tc><a:txBody><a:p><a:r><a:t>Alice</a:t></a:r></a:p></a:txBody></a:tc><a:tc><a:txBody><a:p><a:r><a:t>30</a:t></a:r></a:p></a:txBody></a:tc></a:tr></a:tbl>"#,
        )
        .unwrap();
        let text = extract_table(&root);
        assert_eq!(text, "name: Alice; age: 30");
    }

    #[test]
    fn shapes_sort_by_top_then_left() {
        let mut shapes = vec![
            Shape {
                top: 300,
                left: 10,
                text: "late".into(),
            },
            Shape {
                top: 100,
                left: 500,
                text: "top".into(),
            },
            Shape {
                top: 100,
                left: 10,
                text: "top-left".into(),
            },
        ];
        shapes.sort_by_key(Shape::sort_key);
        let order: Vec<&str> = shapes.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(order, ["top-left", "top", "late"]);
    }

    #[test]
    fn group_shape_recurses_and_sorts_children() {
        let root = parse_xml(
            r#"<p:grpSp><p:sp><p:xfrm><a:off x="0" y="300"/></p:xfrm><p:txBody><a:p><a:r><a:t>B</a:t></a:r></a:p></p:txBody></p:sp><p:sp><p:xfrm><a:off x="0" y="100"/></p:xfrm><p:txBody><a:p><a:r><a:t>A</a:t></a:r></a:p></p:txBody></p:sp></p:grpSp>"#,
        )
        .unwrap();
        let text = extract_shape(&root);
        assert_eq!(text, "A\nB");
    }
}
