//! Naive chunker — mirrors `rag/app/naive.py` chunk() plus the nlp
//! helpers it chains: tokenize, split_with_pattern, tokenize_chunks,
//! add_positions (nlp/__init__.py:268-327, 832-844) and
//! normalize_overlapped_percent (common/float_utils.py:50-58).
//!
//! Ported pure-algorithm core: ES-document assembly (content_with_weight/
//! content_ltks/content_sm_ltks/page_num_int/position_int/top_int),
//! child-delimiter splitting, markdown section merging with overlap
//! carry, and the txt-family dispatch. DeepDoc extractors and
//! pdf_parser.crop/image handling stay in the parser modules.

use regex::Regex;
use serde::Serialize;

/// ES document shape produced by tokenize_chunks.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ChunkDoc {
    pub docnm_kwd: String,
    pub title_tks: Vec<String>,
    pub title_sm_tks: Vec<String>,
    pub content_with_weight: String,
    pub content_ltks: Vec<String>,
    pub content_sm_ltks: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_num_int: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_int: Option<Vec<(i64, i64, i64, i64, i64)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_int: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mom_with_weight: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_type_kwd: Option<String>,
}

impl ChunkDoc {
    fn base(docnm_kwd: &str, title_tks: Vec<String>, title_sm_tks: Vec<String>) -> Self {
        Self {
            docnm_kwd: docnm_kwd.to_string(),
            title_tks,
            title_sm_tks,
            ..Default::default()
        }
    }
}

/// normalize_overlapped_percent — mirrors common/float_utils.py:50-58.
pub fn normalize_overlapped_percent(overlapped_percent: f64) -> i64 {
    let value = if !overlapped_percent.is_finite() {
        0.0
    } else {
        overlapped_percent
    };
    let mut value = if (0.0..1.0).contains(&value) {
        value * 100.0
    } else {
        value
    };
    value = value.floor();
    value.clamp(0.0, 90.0) as i64
}

/// add_positions — mirrors nlp:832-844. `poss` rows are
/// (pn, left, right, top, bottom) floats; page numbers become pn+1.
pub fn add_positions(d: &mut ChunkDoc, poss: &[(f64, f64, f64, f64, f64)]) {
    if poss.is_empty() {
        return;
    }
    let mut page_num_int = Vec::new();
    let mut top_int = Vec::new();
    let mut position_int = Vec::new();
    for (pn, left, right, top, bottom) in poss {
        let p = (*pn + 1.0) as i64;
        page_num_int.push(p);
        top_int.push(*top as i64);
        position_int.push((p, *left as i64, *right as i64, *top as i64, *bottom as i64));
    }
    d.page_num_int = Some(page_num_int);
    d.position_int = Some(position_int);
    d.top_int = Some(top_int);
}

/// tokenize — mirrors nlp:268-273: strip table tags before tokenizing.
pub fn tokenize_doc(d: &mut ChunkDoc, txt: &str) {
    d.content_with_weight = txt.to_string();
    let t = Regex::new(r"</?(table|td|caption|tr|th)( [^<>]{0,12})?>")
        .unwrap()
        .replace_all(txt, " ")
        .to_string();
    // rag_tokenizer.tokenize: whitespace/word split (simplified)
    d.content_ltks = t.split_whitespace().map(String::from).collect();
    d.content_sm_ltks = d.content_ltks.clone();
}

/// split_with_pattern — mirrors nlp:276-299: split keeping separators,
/// pair them back (text + trailing separator), skip empties; invalid
/// pattern falls back to a single chunk. Rust's Regex::split drops
/// capture groups, so this re-implements Python's split-with-capturing
/// semantics via captures_iter.
pub fn split_with_pattern(d: &ChunkDoc, pattern: &str, content: &str) -> Result<Vec<ChunkDoc>, ()> {
    let compiled = match Regex::new(&format!("({pattern})")) {
        Ok(r) => r,
        Err(_) => {
            let mut dd = d.clone();
            tokenize_doc(&mut dd, content);
            return Ok(vec![dd]);
        }
    };
    // Python re.split with one capture group yields
    // [text, sep, text, sep, ...]; pair text[i] with sep[i].
    let mut segments: Vec<String> = Vec::new();
    let mut last = 0usize;
    for cap in compiled.captures_iter(content) {
        let m = cap.get(0).unwrap();
        segments.push(content[last..m.start()].to_string());
        segments.push(m.as_str().to_string());
        last = m.end();
    }
    segments.push(content[last..].to_string());

    let mut docs = Vec::new();
    let mut k = 0usize;
    while k < segments.len() {
        let mut txt = segments[k].clone();
        if k + 1 < segments.len() {
            txt.push_str(&segments[k + 1]);
        }
        if !txt.is_empty() {
            let mut dd = d.clone();
            tokenize_doc(&mut dd, &txt);
            docs.push(dd);
        }
        k += 2;
    }
    Ok(docs)
}

/// tokenize_chunks — mirrors nlp:302-327 (pdf_parser branch simplified
/// away; positions default to [[ii]*5] per chunk).
pub fn tokenize_chunks(
    chunks: &[String],
    docnm_kwd: &str,
    title_tks: Vec<String>,
    title_sm_tks: Vec<String>,
    child_delimiters_pattern: Option<&str>,
) -> Vec<ChunkDoc> {
    let mut res = Vec::new();
    for (ii, ck) in chunks.iter().enumerate() {
        if ck.trim().is_empty() {
            continue;
        }
        let mut d = ChunkDoc::base(docnm_kwd, title_tks.clone(), title_sm_tks.clone());
        add_positions(
            &mut d,
            &[(ii as f64, ii as f64, ii as f64, ii as f64, ii as f64)],
        );
        if let Some(pattern) = child_delimiters_pattern {
            d.mom_with_weight = Some(ck.clone());
            if let Ok(docs) = split_with_pattern(&d, pattern, ck) {
                res.extend(docs);
            }
            continue;
        }
        tokenize_doc(&mut d, ck);
        res.push(d);
    }
    res
}

/// Markdown section merging — mirrors naive.py:1056-1095: merge sections
/// into chunks up to chunk_limit with overlap tail carry.
pub fn merge_markdown_sections(
    sections: &[(String, String)],
    chunk_limit: usize,
    overlapped_percent: i64,
) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    let mut current_text = String::new();
    let mut current_tokens = 0usize;

    for sec in sections {
        let text = sec.0.clone();
        let sec_tokens = text.chars().count();
        if !current_text.is_empty() && current_tokens + sec_tokens > chunk_limit {
            merged.push(current_text.clone());
            let mut overlap_part = String::new();
            if overlapped_percent > 0 {
                let overlap_len = current_text.chars().count() * overlapped_percent as usize / 100;
                if overlap_len > 0 {
                    overlap_part = current_text
                        .chars()
                        .rev()
                        .take(overlap_len)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                }
            }
            current_text = overlap_part;
            current_tokens = current_text.chars().count();
        }
        if !current_text.is_empty() {
            current_text.push('\n');
            current_text.push_str(&text);
        } else {
            current_text = text;
        }
        current_tokens += sec_tokens;
    }
    if !current_text.is_empty() {
        merged.push(current_text);
    }
    merged
}

/// Naive chunk() txt-family core — mirrors naive.py:944-951 + 1054-1113:
/// TxtParser-style line sections, normalize, naive_merge, tokenize_chunks.
pub fn naive_chunk_text(
    filename: &str,
    text: &str,
    chunk_token_num: usize,
    delimiter: &str,
    overlapped_percent: f64,
    child_delimiters_pattern: Option<&str>,
) -> Result<Vec<ChunkDoc>, String> {
    let lower = filename.to_lowercase();
    let supported = [
        "txt", "py", "js", "java", "c", "cpp", "h", "php", "go", "ts", "sh", "cs", "kt", "sql",
    ]
    .iter()
    .any(|ext| lower.ends_with(&format!(".{ext}")));
    if !supported {
        return Err(format!("file type not supported yet got {filename}"));
    }
    // TxtParser-style: split on the delimiter, keep non-empty pieces
    let delim_re = Regex::new(delimiter).unwrap();
    let pieces: Vec<String> = delim_re
        .split(text)
        .map(String::from)
        .filter(|s| !s.is_empty())
        .collect();
    let sections: Vec<(String, String)> =
        pieces.iter().map(|s| (s.clone(), String::new())).collect();

    let ov = normalize_overlapped_percent(overlapped_percent);
    let chunks = crate::book::naive_merge(&sections, chunk_token_num, delimiter, ov as usize);

    let title = filename.rsplit('.').next().map(|_| {
        filename
            .trim_end_matches(|c: char| c == '.' || c.is_ascii_alphabetic())
            .to_string()
    });
    let _ = title;
    let base: String = filename
        .chars()
        .rev()
        .skip_while(|c| *c != '.')
        .skip(1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let title_tks: Vec<String> = base.split_whitespace().map(String::from).collect();
    let docs = tokenize_chunks(
        &chunks,
        filename,
        title_tks.clone(),
        title_tks,
        child_delimiters_pattern,
    );
    Ok(docs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_overlapped_clamps_90() {
        assert_eq!(normalize_overlapped_percent(0.0), 0);
        assert_eq!(normalize_overlapped_percent(0.5), 50);
        assert_eq!(normalize_overlapped_percent(95.0), 90);
        assert_eq!(normalize_overlapped_percent(f64::NAN), 0);
    }

    #[test]
    fn add_positions_increments_page() {
        let mut d = ChunkDoc::base("f.txt", vec![], vec![]);
        add_positions(&mut d, &[(0.0, 1.0, 2.0, 3.0, 4.0)]);
        assert_eq!(d.page_num_int.as_ref().unwrap(), &vec![1]);
        assert_eq!(d.top_int.as_ref().unwrap(), &vec![3]);
        assert_eq!(d.position_int.as_ref().unwrap(), &vec![(1, 1, 2, 3, 4)]);
    }

    #[test]
    fn tokenize_strips_table_tags() {
        let mut d = ChunkDoc::base("f.txt", vec![], vec![]);
        tokenize_doc(&mut d, "a<table><tr><td>cell</td></tr></table>b");
        assert_eq!(
            d.content_with_weight,
            "a<table><tr><td>cell</td></tr></table>b"
        );
        assert_eq!(d.content_ltks, vec!["a", "cell", "b"]);
    }

    #[test]
    fn split_with_pattern_pairs_separators() {
        let d = ChunkDoc::base("f.txt", vec![], vec![]);
        let docs = split_with_pattern(&d, "。", "第一句。第二句。").unwrap();
        assert_eq!(docs.len(), 2);
        assert!(docs[0].content_with_weight.contains("第一句。"));
        assert!(docs[1].content_with_weight.contains("第二句。"));
    }

    #[test]
    fn split_with_pattern_invalid_falls_back() {
        let d = ChunkDoc::base("f.txt", vec![], vec![]);
        let docs = split_with_pattern(&d, "(", "内容").unwrap();
        assert_eq!(docs.len(), 1);
    }

    #[test]
    fn tokenize_chunks_skips_empty_and_adds_positions() {
        let docs = tokenize_chunks(
            &[
                "正文一".to_string(),
                "   ".to_string(),
                "正文二".to_string(),
            ],
            "f.txt",
            vec![],
            vec![],
            None,
        );
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].page_num_int.as_ref().unwrap(), &vec![1]);
        assert_eq!(docs[1].page_num_int.as_ref().unwrap(), &vec![3]);
    }

    #[test]
    fn tokenize_chunks_child_delimiter_sets_mom() {
        let docs = tokenize_chunks(
            &["第一句。第二句。".to_string()],
            "f.txt",
            vec![],
            vec![],
            Some("。"),
        );
        assert_eq!(docs.len(), 2);
        assert!(docs[0].mom_with_weight.is_some());
        assert_eq!(
            docs[0].mom_with_weight.as_deref().unwrap(),
            "第一句。第二句。"
        );
    }

    #[test]
    fn merge_markdown_sections_with_overlap() {
        let long = "长内容".repeat(60);
        let sections = vec![(long.clone(), String::new()), (long.clone(), String::new())];
        let merged = merge_markdown_sections(&sections, 32, 10);
        assert_eq!(merged.len(), 2);
        // 10% of 180 chars = 18-char overlap tail carried into next chunk
        let overlap_len = long.chars().count() * 10 / 100;
        let tail: String = long
            .chars()
            .rev()
            .take(overlap_len)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert!(merged[1].starts_with(&tail), "{:?}", merged);
    }

    #[test]
    fn naive_chunk_text_txt_family() {
        let text = "第一段。第二段。第三段。";
        let docs = naive_chunk_text("code.py", text, 128, "\n。；！？", 0.0, None).unwrap();
        assert!(!docs.is_empty());
        assert_eq!(docs[0].docnm_kwd, "code.py");
        assert!(naive_chunk_text("book.pdf", text, 128, "\n。；！？", 0.0, None).is_err());
    }
}
