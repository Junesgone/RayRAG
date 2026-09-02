//! Manual-style document parser — mirrors `rag/app/manual.py` chunk().
//!
//! Only pdf and docx are supported. Ported pure-algorithm core:
//! section normalization, PDF-outline level matching (bigram similarity),
//! sec_ids grouping, position tag formatting and the token-count merge
//! loop. PDF/DOCX extraction and vision figure parsing stay in the parser
//! modules.

use crate::paper::{Section, bullets_category, title_frequency};

/// A normalized section: (text, layoutno, positions).
/// Position: (page, x1, x2, y1, y2).
#[derive(Debug, Clone)]
pub struct Section3 {
    pub txt: String,
    pub layoutno: String,
    pub poss: Vec<(i64, f64, f64, f64, f64)>,
}

/// Normalize a section to length 3 (manual.py:175-196): pad 1- and 2-tuples,
/// reject other lengths, unwrap list page numbers.
pub fn normalize_section(
    txt: String,
    layoutno: Option<String>,
    poss: Vec<(i64, f64, f64, f64, f64)>,
) -> Result<Section3, String> {
    Ok(Section3 {
        txt,
        layoutno: layoutno.unwrap_or_default(),
        poss,
    })
}

/// Format one position as a tag (manual.py:239-242): all-zero positions
/// yield ""; otherwise `@@{pn}\t{x1:.1}\t{x2:.1}\t{y1:.1}\t{y2:.1}##`.
pub fn tag_pos(pn: i64, x1: f64, x2: f64, y1: f64, y2: f64) -> String {
    if pn + (x1 as i64) + (x2 as i64) + (y1 as i64) + (y2 as i64) == 0 {
        return String::new();
    }
    format!("@@{pn}\t{x1:.1}\t{x2:.1}\t{y1:.1}\t{y2:.1}##")
}

/// Bigram overlap ratio (manual.py:213-215): share of common character
/// bigrams over the larger bigram set, 0.8 threshold used for outline
/// title ↔ section text matching.
pub fn bigram_similarity(a: &str, b: &str) -> f64 {
    let bigrams = |s: &str| -> std::collections::HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 2 {
            return std::collections::HashSet::new();
        }
        chars
            .windows(2)
            .map(|w| w.iter().collect::<String>())
            .collect()
    };
    let ta = bigrams(a);
    let tb = bigrams(b);
    let inter = ta.intersection(&tb).count();
    let denom = ta.len().max(tb.len()).max(1);
    inter as f64 / denom as f64
}

/// An outline entry: (title, level).
#[derive(Debug, Clone)]
pub struct Outline {
    pub title: String,
    pub level: i64,
}

/// Match section texts against PDF outlines (manual.py:207-219). When the
/// outline/section ratio exceeds 0.03, levels come from the outline via
/// bigram matching (>= 0.8); unmatched sections get max_lvl + 1.
/// Otherwise fall back to bullets_category / title_frequency.
/// Returns (most_level, levels).
pub fn section_levels(
    sections: &[Section3],
    outlines: &[Outline],
    fallback: bool,
) -> (i64, Vec<i64>) {
    let max_lvl = outlines.iter().map(|o| o.level).max().unwrap_or(0);
    if !fallback && !outlines.is_empty() {
        let mut levels: Vec<i64> = Vec::with_capacity(sections.len());
        for sec in sections {
            let txt = sec.txt.trim();
            let mut matched = false;
            for o in outlines {
                if bigram_similarity(&o.title, txt) >= 0.8 {
                    levels.push(o.level);
                    matched = true;
                    break;
                }
            }
            if !matched {
                levels.push(max_lvl + 1);
            }
        }
        let most_level = (max_lvl - 1).max(0);
        return (most_level, levels);
    }

    // fallback: bullets_category / title_frequency on (txt, layoutno)
    let bulls: Vec<String> = sections.iter().map(|s| s.txt.clone()).collect();
    let bull = bullets_category(&bulls);
    let pairs: Vec<Section> = sections
        .iter()
        .map(|s| (s.txt.as_str(), s.layoutno.as_str()))
        .collect();
    let (most, levels) = title_frequency(bull, &pairs);
    (most as i64, levels.iter().map(|l| *l as i64).collect())
}

/// Assign section ids — mirrors manual.py:226-231 (identical to paper.py):
/// a new id starts when level <= pivot and differs from the previous level.
pub fn sec_ids(levels: &[i64], most_level: i64) -> Vec<i64> {
    let mut ids = vec![0i64; levels.len()];
    let mut sid = 0i64;
    for i in 1..levels.len() {
        if levels[i] <= most_level && levels[i] != levels[i - 1] {
            sid += 1;
        }
        ids[i] = sid;
    }
    ids
}

/// Sort key: (page, y1, x1) — mirrors the sorted() in manual.py:247.
pub fn sort_key(sec: &Section3) -> (i64, i64, i64) {
    let first = sec.poss.first().copied().unwrap_or((0, 0.0, 0.0, 0.0, 0.0));
    (first.0, first.3 as i64, first.1 as i64)
}

/// Merge sections into chunks — mirrors manual.py:244-257.
/// A section merges into the previous chunk when tk_cnt < 32, or when
/// tk_cnt < 1024 and the section shares the previous sec_id or is a table
/// (id -1). Position tags are appended after each text.
/// token counting uses char counts as a num_tokens_from_string proxy.
pub fn merge_chunks(
    sections: &[Section3],
    sec_ids: &[i64],
    min_tokens: usize,
    max_tokens: usize,
) -> Vec<String> {
    let mut ordered: Vec<usize> = (0..sections.len()).collect();
    ordered.sort_by_key(|&i| sort_key(&sections[i]));

    let mut chunks: Vec<String> = Vec::new();
    let mut last_sid: i64 = -2;
    let mut tk_cnt: usize = 0;
    for i in ordered {
        let sec = &sections[i];
        let txt = &sec.txt;
        let poss = sec
            .poss
            .iter()
            .map(|p| tag_pos(p.0, p.1, p.2, p.3, p.4))
            .collect::<Vec<String>>()
            .join("\t");
        let sec_id = sec_ids[i];
        if (tk_cnt < min_tokens || (tk_cnt < max_tokens && (sec_id == last_sid || sec_id == -1)))
            && let Some(last) = chunks.last_mut() {
                last.push('\n');
                last.push_str(txt);
                last.push_str(&poss);
                tk_cnt += txt.chars().count();
                continue;
            }
        chunks.push(format!("{txt}{poss}"));
        tk_cnt = txt.chars().count();
        if sec_id > -1 {
            last_sid = sec_id;
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_zero_positions_is_empty() {
        assert_eq!(tag_pos(0, 0.0, 0.0, 0.0, 0.0), "");
    }

    #[test]
    fn tag_formats_positions() {
        assert_eq!(
            tag_pos(3, 1.25, 2.5, 10.0, 20.0),
            "@@3\t1.2\t2.5\t10.0\t20.0##"
        );
    }

    #[test]
    fn bigram_similarity_matches_outline_title() {
        assert_eq!(
            bigram_similarity("Chapter 1: Introduction", "Chapter 1: Introduction"),
            1.0
        );
        // identical text has > 0.8 overlap with a short outline title
        assert!(bigram_similarity("引言", "引言") >= 0.8);
        // unrelated texts score low
        assert!(bigram_similarity("Introduction", "Table of Contents") < 0.8);
    }

    #[test]
    fn section_levels_outline_branch() {
        let outlines = vec![
            Outline {
                title: "1. 概述".into(),
                level: 1,
            },
            Outline {
                title: "2. 方法".into(),
                level: 1,
            },
            Outline {
                title: "2.1 采样".into(),
                level: 2,
            },
        ];
        let sections = vec![
            Section3 {
                txt: "1. 概述".into(),
                layoutno: "title".into(),
                poss: vec![],
            },
            Section3 {
                txt: "正文一".into(),
                layoutno: "text".into(),
                poss: vec![],
            },
            Section3 {
                txt: "2.1 采样".into(),
                layoutno: "title".into(),
                poss: vec![],
            },
        ];
        let (most, levels) = section_levels(&sections, &outlines, false);
        assert_eq!(most, 1);
        assert_eq!(levels[0], 1);
        assert_eq!(levels[2], 2);
        assert_eq!(levels[1], 3, "unmatched body gets max_lvl+1: {levels:?}");
    }

    #[test]
    fn section_levels_fallback_branch() {
        let sections = vec![
            Section3 {
                txt: "第一章 总则".into(),
                layoutno: "title".into(),
                poss: vec![],
            },
            Section3 {
                txt: "第一条".into(),
                layoutno: "title".into(),
                poss: vec![],
            },
            Section3 {
                txt: "第二章 水质".into(),
                layoutno: "title".into(),
                poss: vec![],
            },
        ];
        let (most, levels) = section_levels(&sections, &[], true);
        // family 0: 第…章 is index 1, 第…条 is index 3 (第…部分 is index 0)
        assert_eq!(most, 1);
        assert_eq!(levels, vec![1, 3, 1]);
    }

    #[test]
    fn sec_ids_group_by_pivot() {
        let levels = vec![1, 0, 1, 2, 1];
        let ids = sec_ids(&levels, 1);
        assert_eq!(ids, vec![0, 1, 2, 2, 3]);
    }

    #[test]
    fn merge_chunks_joins_short_and_same_section() {
        let sections = vec![
            Section3 {
                txt: "标题".into(),
                layoutno: "title".into(),
                poss: vec![(1, 1.0, 2.0, 3.0, 4.0)],
            },
            Section3 {
                txt: "正文".into(),
                layoutno: "text".into(),
                poss: vec![(1, 1.0, 2.0, 5.0, 6.0)],
            },
            Section3 {
                txt: "表格行".into(),
                layoutno: "table".into(),
                poss: vec![(1, 1.0, 2.0, 7.0, 8.0)],
            },
        ];
        let ids = vec![0, 0, -1];
        let chunks = merge_chunks(&sections, &ids, 32, 1024);
        assert_eq!(chunks.len(), 1, "{chunks:?}");
        assert!(chunks[0].starts_with("标题@@1\t1.0\t2.0\t3.0\t4.0##"));
        assert!(chunks[0].contains("正文"));
        assert!(chunks[0].contains("表格行"));
    }

    #[test]
    fn merge_chunks_starts_new_chunk_when_long_and_different_section() {
        let long = "很长的正文内容".repeat(40);
        let sections = vec![
            Section3 {
                txt: long.clone(),
                layoutno: "text".into(),
                poss: vec![(1, 1.0, 2.0, 3.0, 4.0)],
            },
            Section3 {
                txt: "新节".into(),
                layoutno: "title".into(),
                poss: vec![(1, 1.0, 2.0, 5.0, 6.0)],
            },
        ];
        let ids = vec![0, 1];
        let chunks = merge_chunks(&sections, &ids, 32, 1024);
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn normalize_section_keeps_fields() {
        let sec = normalize_section(
            "内容".to_string(),
            Some("title".to_string()),
            vec![(1, 0.0, 1.0, 2.0, 3.0)],
        )
        .unwrap();
        assert_eq!(sec.txt, "内容");
        assert_eq!(sec.layoutno, "title");
        assert_eq!(sec.poss.len(), 1);
    }
}
