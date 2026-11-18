//! Paper (PDF) section-structure parser — mirrors `rag/app/paper.py`
//! chunk() pure-algorithm core + `rag/nlp/__init__.py` helpers
//! (bullets_category / title_frequency / not_bullet / not_title).
//!
//! The paper parser keeps the abstract as one whole chunk and groups body
//! text by detected section titles. RayRAG ports the title-category
//! detection and section-grouping algorithms; PDF layout recognition and
//! table/image cropping remain in the parser modules.

use regex::Regex;

/// Compile a pattern with re.match semantics (anchored at the start).
/// Python's `re.match` anchors; Rust's `Regex::is_match` searches anywhere.
pub fn anchored(p: &str) -> Regex {
    Regex::new(&format!("^(?:{p})")).unwrap()
}

/// Compile a pattern with re.search semantics (unanchored).
fn search_re(p: &str) -> Regex {
    Regex::new(p).unwrap()
}

/// BULLET_PATTERN — five title-style families (rag/nlp/__init__.py:169-207).
/// Order matters: bullets_category picks the family with the most hits.
pub const BULLET_PATTERN: &[&[&str]] = &[
    &[
        r"第[零一二三四五六七八九十百0-9]+(分?编|部分)",
        r"第[零一二三四五六七八九十百0-9]+章",
        r"第[零一二三四五六七八九十百0-9]+节",
        r"第[零一二三四五六七八九十百0-9]+条",
        r"[\(（][零一二三四五六七八九十百]+[\)）]",
    ],
    &[
        r"第[0-9]+章",
        r"第[0-9]+节",
        r"[0-9]{0,2}[\. 、]",
        r"[0-9]{0,2}\.[0-9]{0,2}[^a-zA-Z/%~-]",
        r"[0-9]{0,2}\.[0-9]{0,2}\.[0-9]{0,2}",
        r"[0-9]{0,2}\.[0-9]{0,2}\.[0-9]{0,2}\.[0-9]{0,2}",
    ],
    &[
        r"第[零一二三四五六七八九十百0-9]+章",
        r"第[零一二三四五六七八九十百0-9]+节",
        r"[零一二三四五六七八九十百]+[ 、]",
        r"[\(（][零一二三四五六七八九十百]+[\)）]",
        r"[\(（][0-9]{0,2}[\)）]",
    ],
    &[
        r"PART (ONE|TWO|THREE|FOUR|FIVE|SIX|SEVEN|EIGHT|NINE|TEN)",
        r"Chapter (I+V?|VI*|XI|IX|X)",
        r"Section [0-9]+",
        r"Article [0-9]+",
    ],
    &[
        r"^#[^#]",
        r"^##[^#]",
        r"^###.*",
        r"^####.*",
        r"^#####.*",
        r"^######.*",
    ],
];

/// not_bullet — lines that look numbered but are not section titles
/// (nlp/__init__.py:209-213).
pub fn not_bullet(line: &str) -> bool {
    let patt = [r"0", r"[0-9]+ +[0-9~个只-]", r"[0-9]+\.{2,}"];
    patt.iter().any(|p| anchored(p).is_match(line))
}

/// not_title — a candidate title is rejected when too long or containing
/// sentence punctuation (nlp/__init__.py:923-928).
pub fn not_title(txt: &str) -> bool {
    if anchored(r"第[零一二三四五六七八九十百0-9]+条").is_match(txt) {
        return false;
    }
    if txt.split_whitespace().count() > 12 || (txt.find(' ').is_none() && txt.chars().count() >= 32)
    {
        return true;
    }
    search_re(r"[,;，。；！!]").is_match(txt)
}

/// bullets_category — pick the title-style family with the most hits
/// (nlp/__init__.py:216-233). Returns -1 when nothing matches.
pub fn bullets_category(sections: &[String]) -> i64 {
    let mut hits = vec![0usize; BULLET_PATTERN.len()];
    for (i, family) in BULLET_PATTERN.iter().enumerate() {
        for sec in sections {
            let sec = sec.trim();
            for p in *family {
                if anchored(p).is_match(sec) && !not_bullet(sec) {
                    hits[i] += 1;
                    break;
                }
            }
        }
    }
    let mut maximum = 0usize;
    let mut res: i64 = -1;
    for (i, h) in hits.iter().enumerate() {
        if *h <= maximum {
            continue;
        }
        res = i as i64;
        maximum = *h;
    }
    res
}

/// A (text, layout) section — layout is used for the title/head fallback.
pub type Section<'a> = (&'a str, &'a str);

/// title_frequency — per-section level within the chosen family, plus the
/// most frequent level used as the merge pivot (nlp/__init__.py:901-920).
/// Levels range 0..family_len; a title-layout section without a bullet
/// match gets family_len; non-title sections get family_len + 1.
/// Returns (most_level, levels).
pub fn title_frequency(bull: i64, sections: &[Section]) -> (usize, Vec<usize>) {
    let family_len = if bull >= 0 && (bull as usize) < BULLET_PATTERN.len() {
        BULLET_PATTERN[bull as usize].len()
    } else {
        0
    };
    let mut levels = vec![family_len + 1; sections.len()];
    if sections.is_empty() || bull < 0 {
        return (family_len + 1, levels);
    }
    for (i, (txt, layout)) in sections.iter().enumerate() {
        let mut matched = false;
        for (j, p) in BULLET_PATTERN[bull as usize].iter().enumerate() {
            if anchored(p).is_match(txt.trim()) && !not_bullet(txt) {
                levels[i] = j;
                matched = true;
                break;
            }
        }
        if !matched
            && (layout.contains("title") || layout.contains("head"))
            && !not_title(txt.split('@').next().unwrap_or(txt))
        {
            levels[i] = family_len;
        }
    }
    let mut counts: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for level in &levels {
        if *level <= family_len {
            *counts.entry(*level).or_insert(0) += 1;
        }
    }
    let most_level = counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|(level, _)| level)
        .unwrap_or(family_len + 1);
    (most_level, levels)
}

/// Group sections by title level — mirrors paper.py:232-254: a new section
/// id starts when the level is <= the pivot and differs from the previous
/// section's level; same-id texts merge into one chunk.
pub fn group_sections(sections: &[Section]) -> Vec<String> {
    if sections.is_empty() {
        return Vec::new();
    }
    let bulls: Vec<String> = sections.iter().map(|(t, _)| t.to_string()).collect();
    let bull = bullets_category(&bulls);
    let (most_level, levels) = title_frequency(bull, sections);

    let mut sec_ids = vec![0usize; sections.len()];
    let mut sid = 0usize;
    for i in 1..sections.len() {
        if levels[i] <= most_level && levels[i] != levels[i - 1] {
            sid += 1;
        }
        sec_ids[i] = sid;
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut last_sid: i64 = -2;
    for ((txt, _), sec_id) in sections.iter().zip(sec_ids.iter()) {
        if *sec_id as i64 == last_sid
            && let Some(last) = chunks.last_mut()
        {
            last.push('\n');
            last.push_str(txt);
            continue;
        }
        chunks.push(txt.to_string());
        last_sid = *sec_id as i64;
    }
    chunks
}

/// Split the abstract/important-keyword doc assembly — mirrors paper.py:221-230:
/// the abstract is one whole chunk marked with important keywords.
pub fn abstract_chunk(
    abstract_txt: &str,
    doc_title: &str,
    authors: &str,
) -> (String, Vec<String>, Vec<String>) {
    let mut important_kwd = vec![
        "abstract".to_string(),
        "总结".to_string(),
        "概括".to_string(),
        "summary".to_string(),
        "summarize".to_string(),
    ];
    let important_tks = important_kwd.join(" ");
    let mut doc_fields = vec![
        format!("docnm_kwd: {doc_title}"),
        format!("title_tks: {doc_title}"),
        format!("authors_tks: {authors}"),
        format!("important_tks: {important_tks}"),
        format!("content_with_weight: {abstract_txt}"),
    ];
    let _ = &mut important_kwd;
    let _ = &mut doc_fields;
    (
        abstract_txt.to_string(),
        important_kwd,
        abstract_txt.split_whitespace().map(String::from).collect(),
    )
}


