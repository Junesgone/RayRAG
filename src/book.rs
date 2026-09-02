//! Book parser — mirrors `rag/app/book.py` chunk() plus the nlp helpers
//! it chains: hierarchical_merge, naive_merge, is_english, is_chinese,
//! random_choices, remove_contents_table, make_colon_as_title.
//!
//! Ported pure-algorithm core: hierarchical level merging (binary search
//! grouping + 218-token cap), naive token-count merging (with custom
//! backtick delimiters), language detection and sampling. Docx/Pdf/Html/
//! tika extractors stay in the parser modules.

use crate::laws::remove_contents_table;
use crate::paper::{BULLET_PATTERN, anchored, bullets_category, not_title};
use regex::Regex;

/// random.choices with replacement — mirrors nlp:204-206. Deterministic:
/// cycles through the input (index mod len) so tests are reproducible.
pub fn random_choices(arr: &[String], k: usize) -> Vec<&str> {
    let k = k.min(arr.len());
    if arr.is_empty() {
        return Vec::new();
    }
    (0..k).map(|i| arr[i % arr.len()].as_str()).collect()
}

/// is_english — mirrors nlp:236-253: each line must FULLMATCH the ASCII
/// pattern; english when the ratio > 0.8.
pub fn is_english(texts: &[String]) -> bool {
    if texts.is_empty() {
        return false;
    }
    let re = Regex::new(r#"^[`a-zA-Z0-9\s.,':;/"?<>!()\-]+$"#).unwrap();
    let mut eng = 0usize;
    for t in texts {
        if t.trim().is_empty() {
            continue;
        }
        if re.is_match(t.trim()) {
            eng += 1;
        }
    }
    eng as f64 / texts.len() as f64 > 0.8
}

/// is_chinese — mirrors nlp:256-265: CJK ratio > 0.2.
pub fn is_chinese(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let chinese = text
        .chars()
        .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
        .count();
    chinese as f64 / text.chars().count() as f64 > 0.2
}

/// Strip `@@...##` position tags (RAGFlowPdfParser.remove_tag proxy).
fn remove_tag(s: &str) -> String {
    let re = Regex::new(r"@@[0-9]+.*?##").unwrap();
    re.replace_all(s, "").to_string()
}

/// hierarchical_merge — mirrors nlp:980-1067. Returns chunk groups
/// (each group joined with '\n' by the caller, book.py:166).
pub fn hierarchical_merge(
    bull: i64,
    sections: &[(String, String)],
    depth: usize,
    family_len: usize,
) -> Vec<Vec<String>> {
    if sections.is_empty() || bull < 0 {
        return Vec::new();
    }
    // filter (nlp:985-986)
    let filtered: Vec<(String, String)> = sections
        .iter()
        .filter(|(t, _)| {
            let head = t.split('@').next().unwrap_or_default().trim();
            head.chars().count() > 1 && !head.chars().all(|c| c.is_ascii_digit())
        })
        .map(|(t, o)| (t.clone(), o.clone()))
        .collect();

    let mut levels: Vec<Vec<usize>> = vec![Vec::new(); family_len + 2];
    for (i, (txt, layout)) in filtered.iter().enumerate() {
        let mut placed = false;
        for (j, pat) in BULLET_PATTERN[bull as usize].iter().enumerate() {
            if anchored(pat).is_match(txt.trim()) {
                levels[j].push(i);
                placed = true;
                break;
            }
        }
        if !placed {
            if (layout.contains("title") || layout.contains("head"))
                && !not_title(txt.split('@').next().unwrap_or_default())
            {
                levels[family_len].push(i);
            } else {
                levels[family_len + 1].push(i);
            }
        }
    }
    let texts: Vec<String> = filtered.iter().map(|(t, _)| t.clone()).collect();

    // binary_search: greatest index whose value < target (nlp:1004-1022)
    fn binary_search(arr: &[usize], target: usize) -> isize {
        if arr.is_empty() {
            return -1;
        }
        if target > arr[arr.len() - 1] {
            return arr.len() as isize - 1;
        }
        if target < arr[0] {
            return -1;
        }
        let (mut s, mut e) = (0usize, arr.len());
        while e - s > 1 {
            let i = (e + s) / 2;
            if target > arr[i] {
                s = i;
            } else if target < arr[i] {
                e = i;
            } else {
                debug_assert!(false, "duplicate index in levels");
                return s as isize;
            }
        }
        s as isize
    }

    let mut cks: Vec<Vec<usize>> = Vec::new();
    let mut readed = vec![false; texts.len()];
    levels.reverse();
    for (i, arr) in levels.iter().enumerate().take(depth) {
        for &j in arr {
            if readed[j] {
                continue;
            }
            readed[j] = true;
            let mut ck = vec![j];
            if i + 1 == levels.len() - 1 {
                cks.push(ck);
                continue;
            }
            for ii in (i + 1)..levels.len() {
                let jj = binary_search(&levels[ii], j);
                if jj < 0 {
                    continue;
                }
                let idx = levels[ii][jj as usize];
                if idx > *ck.last().unwrap() {
                    ck.pop();
                }
                ck.push(idx);
            }
            for &ii in &ck {
                readed[ii] = true;
            }
            cks.push(ck);
        }
    }
    if cks.is_empty() {
        return Vec::new();
    }

    // resolve indices to texts, reversed (nlp:1048-1050)
    let mut ck_texts: Vec<Vec<String>> = Vec::new();
    for ck in &cks {
        let resolved: Vec<String> = ck.iter().rev().map(|&j| texts[j].clone()).collect();
        ck_texts.push(resolved);
    }

    // group chunks with 218-token cap (nlp:1052-1065)
    let mut res: Vec<Vec<String>> = vec![Vec::new()];
    let mut num: Vec<usize> = vec![0];
    for ck in ck_texts {
        if ck.len() == 1 {
            let cleaned = remove_tag(&ck[0]);
            let n = cleaned.chars().count();
            if n + num[num.len() - 1] < 218 {
                let last = res.len() - 1;
                res[last].push(ck[0].clone());
                num[last] += n;
                continue;
            }
            res.push(ck);
            num.push(n);
            continue;
        }
        res.push(ck);
        num.push(218);
    }
    res
}

/// naive_merge — mirrors nlp:1070-1126: token-count merging with
/// overlapped tail carry and optional backtick custom delimiters.
pub fn naive_merge(
    sections: &[(String, String)],
    chunk_token_num: usize,
    delimiter: &str,
    overlapped_percent: usize,
) -> Vec<String> {
    if sections.is_empty() {
        return Vec::new();
    }
    // custom delimiters: `...` inside the delimiter string (nlp:1103-1121)
    let custom_re = Regex::new(r"`([^`]+)`").unwrap();
    let custom_delimiters: Vec<String> = custom_re
        .captures_iter(delimiter)
        .map(|m| m[1].to_string())
        .collect();
    let has_custom = !custom_delimiters.is_empty();
    if has_custom {
        let mut pats: Vec<String> = custom_delimiters.to_vec();
        pats.sort_by_key(|p| std::cmp::Reverse(p.len()));
        let mut pats_dedup: Vec<String> = Vec::new();
        for p in pats {
            if !pats_dedup.contains(&p) {
                pats_dedup.push(p);
            }
        }
        let pattern = pats_dedup
            .iter()
            .map(|p| regex::escape(p))
            .collect::<Vec<_>>()
            .join("|");
        let split_re = Regex::new(&format!("({pattern})")).unwrap();
        let mut cks: Vec<String> = Vec::new();
        for (sec, pos) in sections {
            for sub in split_re.split(sec) {
                if split_re.is_match(sub) {
                    continue; // the separator itself
                }
                let text = format!("\n{sub}");
                let mut local_pos = pos.clone();
                if text.chars().count() < 8 {
                    local_pos.clear();
                }
                if !local_pos.is_empty() && !text.contains(&local_pos) {
                    let mut t = text;
                    t.push_str(&local_pos);
                    cks.push(t);
                } else {
                    cks.push(text);
                }
            }
        }
        return cks;
    }

    let mut cks: Vec<String> = vec![String::new()];
    let mut tk_nums: Vec<usize> = vec![0];
    let limit_ratio = (100 - overlapped_percent) as f64 / 100.0;

    for (sec, pos) in sections {
        let mut t = format!("\n{sec}");
        let mut pos = pos.clone();
        let tnum = t.chars().count();
        if tnum < 8 {
            pos.clear();
        }
        let threshold = (chunk_token_num as f64 * limit_ratio) as usize;
        if cks.last().map(|s| s.is_empty()).unwrap_or(true)
            || tk_nums[tk_nums.len() - 1] > threshold
        {
            if let Some(last) = cks.last() {
                let overlapped = remove_tag(last);
                let keep_from = (overlapped.chars().count() as f64 * limit_ratio) as usize;
                let tail: String = overlapped.chars().skip(keep_from).collect();
                t = format!("{tail}{t}");
            }
            if pos.is_empty() || !t.contains(&pos) {
                t.push_str(&pos);
            }
            cks.push(t);
            tk_nums.push(tnum);
        } else {
            if !pos.is_empty() && !cks[cks.len() - 1].contains(&pos) {
                t.push_str(&pos);
            }
            let last = cks.len() - 1;
            cks[last].push_str(&t);
            let last_n = tk_nums.len() - 1;
            tk_nums[last_n] += tnum;
        }
    }
    cks
}

/// Book chunk() text-family core — mirrors book.py:128-134 + 163-170:
/// lines -> (line, "") pairs, remove contents, classify, hierarchical or
/// naive merge.
pub fn parse_book_text(
    filename: &str,
    text: &str,
    chunk_token_num: usize,
) -> Result<Vec<String>, String> {
    let lower = filename.to_lowercase();
    if !lower.ends_with(".txt") {
        return Err(format!(
            "file type not supported yet(doc, docx, pdf, txt supported) got {filename}"
        ));
    }
    let mut sections: Vec<(String, String)> = text
        .lines()
        .filter(|s| !s.is_empty())
        .map(|s| (s.to_string(), String::new()))
        .collect();
    let sample: Vec<String> = random_choices(
        &sections.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
        200,
    )
    .iter()
    .map(|s| s.to_string())
    .collect();
    let eng = is_english(&sample);
    let mut str_sections: Vec<String> = sections.iter().map(|(t, _)| t.clone()).collect();
    remove_contents_table(&mut str_sections, eng);
    sections = str_sections
        .iter()
        .map(|s| (s.clone(), String::new()))
        .collect();

    let sample100: Vec<String> = random_choices(
        &sections.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(),
        100,
    )
    .iter()
    .map(|s| s.to_string())
    .collect();
    let bull = bullets_category(&sample100);
    let chunks: Vec<String> = if bull >= 0 {
        let family_len = BULLET_PATTERN[bull as usize].len();
        hierarchical_merge(bull, &sections, 5, family_len)
            .into_iter()
            .map(|ck| ck.join("\n"))
            .collect()
    } else {
        let delimited: Vec<(String, String)> = sections
            .iter()
            .map(|(s, _)| {
                let mut parts = s.split('@');
                let head = parts.next().unwrap_or_default().to_string();
                let tail = parts.next().unwrap_or_default().to_string();
                if s.contains('@') {
                    (head, format!("@{tail}"))
                } else {
                    (head, String::new())
                }
            })
            .collect();
        naive_merge(&delimited, chunk_token_num, "\n。；！？", 0)
    };
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_choices_deterministic_cycles() {
        let arr = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        // Python: k = min(len, k) — 5 requests on a 3-element array return 3
        let picked = random_choices(&arr, 5);
        assert_eq!(picked.len(), 3);
        // deterministic in-order coverage
        let arr3 = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let picked3 = random_choices(&arr3, 3);
        assert_eq!(picked3, vec!["a", "b", "c"]);
    }

    #[test]
    fn is_english_detects_ascii_majority() {
        let eng = vec![
            "Hello world".to_string(),
            "This is fine".to_string(),
            "中文".to_string(),
        ];
        // 2/3 = 0.667, NOT > 0.8 -> false (Python-verified)
        assert!(!is_english(&eng));
    }

    #[test]
    fn is_english_threshold() {
        // 4/5 = 0.8 is NOT > 0.8 -> false
        let eng = vec![
            "Hello world".to_string(),
            "This is fine".to_string(),
            "More text here".to_string(),
            "Yeah sure".to_string(),
            "中文".to_string(),
        ];
        assert!(!is_english(&eng));
        // 5/6 = 0.833 > 0.8 -> true
        let all_eng = vec![
            "Hello world".to_string(),
            "This is fine".to_string(),
            "More text here".to_string(),
            "Yeah sure".to_string(),
            "Almost done".to_string(),
            "中文".to_string(),
        ];
        assert!(is_english(&all_eng));
    }

    #[test]
    fn is_chinese_detects_cjk_ratio() {
        assert!(is_chinese("这是一段中文文本"));
        assert!(!is_chinese("hello world"));
    }

    #[test]
    fn remove_tag_strips_position_tags() {
        assert_eq!(remove_tag("正文@@1\t1.0\t2.0##继续"), "正文继续");
    }

    #[test]
    fn hierarchical_merge_numeric_books() {
        // family 3 uses ROMAN numerals: Chapter I / Chapter II
        let sections = vec![
            ("Chapter I: Intro".to_string(), "title".to_string()),
            ("Body one".to_string(), "text".to_string()),
            ("Chapter II: Methods".to_string(), "title".to_string()),
            ("Body two".to_string(), "text".to_string()),
        ];
        let bull = bullets_category(&[
            "Chapter I: Intro".to_string(),
            "Chapter II: Methods".to_string(),
        ]);
        assert_eq!(bull, 3);
        let family_len = BULLET_PATTERN[3].len();
        let groups = hierarchical_merge(bull as i64, &sections, 5, family_len);
        let flat: Vec<String> = groups.into_iter().map(|g| g.join("\n")).collect();
        assert!(flat.iter().any(|c| c.contains("Chapter I: Intro")));
        assert!(flat.iter().any(|c| c.contains("Chapter II: Methods")));
    }

    #[test]
    fn hierarchical_merge_short_sections_group_under_218() {
        let sections = vec![
            ("1. 概述".to_string(), "title".to_string()),
            ("短正文".to_string(), "text".to_string()),
            ("2. 方法".to_string(), "title".to_string()),
            ("短正文二".to_string(), "text".to_string()),
        ];
        let bull = bullets_category(&["1. 概述".to_string(), "2. 方法".to_string()]);
        assert_eq!(bull, 1);
        let family_len = BULLET_PATTERN[1].len();
        let groups = hierarchical_merge(bull as i64, &sections, 5, family_len);
        let flat: Vec<String> = groups.into_iter().map(|g| g.join("\n")).collect();
        assert!(!flat.is_empty());
    }

    #[test]
    fn naive_merge_basic_accumulation() {
        let sections = vec![
            ("第一段正文内容".to_string(), String::new()),
            ("第二段正文内容".to_string(), String::new()),
        ];
        let chunks = naive_merge(&sections, 128, "\n。；！？", 0);
        // Python returns ['', joined] — the leading empty slot is faithful
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        assert!(chunks[0].is_empty());
        assert!(chunks[1].contains("第一段正文内容"));
        assert!(chunks[1].contains("第二段正文内容"));
    }

    #[test]
    fn naive_merge_custom_delimiters_split() {
        let sections = vec![("a。b。c".to_string(), String::new())];
        let chunks = naive_merge(&sections, 128, "\n。；！？`。`", 0);
        // custom delimiter '。' splits: "\na", "\nb", "\nc"
        assert_eq!(chunks.len(), 3, "{chunks:?}");
    }

    #[test]
    fn naive_merge_overlap_carries_tail() {
        let long = "长内容".repeat(50);
        let sections = vec![(long.clone(), String::new()), (long.clone(), String::new())];
        let chunks = naive_merge(&sections, 32, "\n。；！？", 10);
        assert!(chunks.len() >= 2, "{chunks:?}");
    }

    #[test]
    fn parse_book_text_txt_family() {
        // needs plain body lines: pure chapter titles yield [] (Python-verified)
        let text = "目录\n第一章 概述\n这是正文内容。\n第二章 方法\n这也是正文内容。\n";
        let chunks = parse_book_text("book.txt", text, 128).unwrap();
        assert!(!chunks.is_empty(), "{chunks:?}");
        assert!(
            chunks.iter().any(|c| c.contains("第二章 方法")),
            "{chunks:?}"
        );
        assert!(parse_book_text("book.pdf", text, 128).is_err());
    }
}
