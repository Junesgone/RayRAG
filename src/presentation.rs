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


