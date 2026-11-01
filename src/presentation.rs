//! Presentation chunker — mirrors RAGFlow `rag/app/presentation.py`.
//!
//! Supported file formats: pptx (and ppt via the same path). Every page is
//! treated as ONE chunk, `doc_type_kwd = "image"`, with page/position
//! metadata — the exact document shape presentation.py emits:
//!
//! ```python
//! d["doc_type_kwd"] = "image"
//! d["page_num_int"] = [pn + 1]
//! d["top_int"] = [0]
//! d["position_int"] = [(pn + 1, 0, 0, 0, 0)]
//! tokenize(d, txt, eng)
//! ```
//!
//! The PDF branch of presentation.py (layout-recognizer dispatch:
//! DeepDOC / Plain Text / MinerU / TCADP / docling / paddleocr) maps to
//! RayRAG's `parser::pdf` page extraction; PDF parsing is exercised through
//! `PdfParser` and is out of scope for this module (RayRAG's PDF pipeline
//! lives in `parser/pdf.rs`). `.ppt` legacy binary files are not supported
//! by the Rust zip-based reader; only `.pptx` containers are handled, which
//! mirrors RAGFlow's python-pptx requirement (tika fallback is not ported).

use crate::Result;
use crate::naive::{ChunkDoc, tokenize_doc};
use crate::parser::ppt::slide_texts;

/// True when the filename is a PPTX/PPT presentation (case-insensitive).
pub fn is_presentation(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".pptx") || lower.ends_with(".ppt")
}

/// Strip the trailing extension — mirrors
/// `re.sub(r"\.[a-zA-Z]+$", "", filename)` for the title tokenizer.
pub fn title_without_extension(filename: &str) -> &str {
    let lower = filename.to_lowercase();
    for ext in [".pptx", ".ppt"] {
        if lower.ends_with(ext) {
            return &filename[..filename.len() - ext.len()];
        }
    }
    filename
}

/// `chunk()` for pptx/ppt — every slide becomes one chunk.
///
/// Mirrors presentation.py lines 140-152: for each slide text `txt`,
/// build `d` with `doc_type_kwd="image"`, `page_num_int=[pn+1]`,
/// `top_int=[0]`, `position_int=[(pn+1,0,0,0,0)]`, then tokenize.
pub fn chunk_pptx(
    filename: &str,
    binary: &[u8],
    from_page: usize,
    to_page: usize,
    lang: &str,
) -> Result<Vec<ChunkDoc>> {
    let eng = lang.trim().eq_ignore_ascii_case("english");
    let title = title_without_extension(filename).to_owned();
    let title_tks = tokenize_title(&title);
    let title_sm_tks = fine_grained_tokenize(&title_tks);

    let slides = slide_texts(binary)?;
    let mut res = Vec::new();
    for (pn, txt) in slides.iter().enumerate() {
        if pn < from_page {
            continue;
        }
        if pn >= to_page {
            break;
        }
        let mut d = ChunkDoc {
            docnm_kwd: filename.to_owned(),
            title_tks: title_tks.clone(),
            title_sm_tks: title_sm_tks.clone(),
            doc_type_kwd: Some("image".into()),
            page_num_int: Some(vec![pn as i64 + 1]),
            top_int: Some(vec![0]),
            position_int: Some(vec![(pn as i64 + 1, 0, 0, 0, 0)]),
            ..Default::default()
        };
        tokenize_doc(&mut d, txt);
        let _ = eng;
        res.push(d);
    }
    Ok(res)
}

/// `rag_tokenizer.tokenize` — whitespace/word split (matches the simplified
/// tokenizer used across RayRAG's naive module).
fn tokenize_title(stem: &str) -> Vec<String> {
    stem.split_whitespace().map(str::to_owned).collect()
}

/// `rag_tokenizer.fine_grained_tokenize` — RayRAG uses the same split as the
/// regular tokenizer (fine-grained CJK segmentation is not implemented);
/// mirrors presentation.py `title_sm_tks` assignment.
fn fine_grained_tokenize(tokens: &[String]) -> Vec<String> {
    tokens.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_detection_matches_extensions() {
        assert!(is_presentation("slides.pptx"));
        assert!(is_presentation("old.ppt"));
        assert!(is_presentation("DECK.PPTX"));
        assert!(!is_presentation("notes.pdf"));
        assert!(!is_presentation("data.xlsx"));
    }

    #[test]
    fn title_strips_extension_case_insensitively() {
        assert_eq!(title_without_extension("report.pptx"), "report");
        assert_eq!(title_without_extension("REPORT.PPT"), "REPORT");
        assert_eq!(title_without_extension("report.pdf"), "report.pdf");
        assert_eq!(title_without_extension("noext"), "noext");
    }

    #[test]
    fn chunk_pptx_emits_image_docs_with_page_metadata() {
        // Build a minimal pptx: [Content_Types].xml + one slide.
        let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#;
        let slide1 = r#"<p:sld><p:sp><p:txBody><a:p><a:r><a:t>Hello slide one</a:t></a:r></a:p></p:txBody></p:sp></p:sld>"#;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("[Content_Types].xml", options).unwrap();
        std::io::Write::write_all(&mut zip, content_types.as_bytes()).unwrap();
        zip.start_file("ppt/slides/slide1.xml", options).unwrap();
        std::io::Write::write_all(&mut zip, slide1.as_bytes()).unwrap();
        let bytes = zip.finish().unwrap().into_inner();

        let chunks = chunk_pptx("deck.pptx", &bytes, 0, 100, "Chinese").unwrap();
        assert_eq!(chunks.len(), 1);
        let d = &chunks[0];
        assert_eq!(d.docnm_kwd, "deck.pptx");
        assert_eq!(d.doc_type_kwd.as_deref(), Some("image"));
        assert_eq!(d.page_num_int.as_deref(), Some([1i64].as_slice()));
        assert_eq!(d.top_int.as_deref(), Some([0i64].as_slice()));
        assert_eq!(
            d.position_int.as_deref(),
            Some([(1i64, 0i64, 0i64, 0i64, 0i64)].as_slice())
        );
        assert!(d.content_with_weight.contains("Hello slide one"));
        assert!(d.title_tks.contains(&"deck".to_string()));
    }

    #[test]
    fn chunk_pptx_respects_page_bounds() {
        // Two slides, request only slide 2 (from_page=1).
        let content_types = r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/><Override PartName="/ppt/slides/slide2.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#;
        let slide1 = r#"<p:sld><p:sp><p:txBody><a:p><a:r><a:t>First</a:t></a:r></a:p></p:txBody></p:sp></p:sld>"#;
        let slide2 = r#"<p:sld><p:sp><p:txBody><a:p><a:r><a:t>Second</a:t></a:r></a:p></p:txBody></p:sp></p:sld>"#;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("[Content_Types].xml", options).unwrap();
        std::io::Write::write_all(&mut zip, content_types.as_bytes()).unwrap();
        for (name, xml) in [("slide1", slide1), ("slide2", slide2)] {
            zip.start_file(&format!("ppt/slides/{name}.xml"), options)
                .unwrap();
            std::io::Write::write_all(&mut zip, xml.as_bytes()).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();

        let chunks = chunk_pptx("deck.pptx", &bytes, 1, 100, "Chinese").unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].content_with_weight.contains("Second"));
        assert_eq!(chunks[0].page_num_int.as_deref(), Some([2i64].as_slice()));
    }
}
