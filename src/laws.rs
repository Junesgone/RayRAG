//! Laws/regulations parser — mirrors `rag/app/laws.py` chunk() plus the
//! nlp helpers it chains: remove_contents_table, make_colon_as_title,
//! tree_merge (Node tree).
//!
//! Ported pure-algorithm core: contents-table removal, colon-title
//! splitting and tree merging. Docx/Pdf/Html/tika extractors stay in the
//! parser modules.

use crate::paper::{BULLET_PATTERN, anchored, bullets_category, not_bullet, not_title};

/// Strip whitespace (spaces, full-width spaces, ideographic space) —
/// mirrors the re.sub in remove_contents_table (nlp:856).
fn collapse_ws(s: &str) -> String {
    s.replace([' ', '\u{3000}', '\t'], "")
}

/// Remove the "Contents" part — mirrors nlp/__init__.py:847-876.
/// sections are strings (pdf sections are already "txt+poss").
pub fn remove_contents_table(sections: &mut Vec<String>, eng: bool) {
    let mut i = 0usize;
    while i < sections.len() {
        let get = |sections: &[String], idx: usize| -> String {
            sections
                .get(idx)
                .map(|s| s.split("@@").next().unwrap_or_default().trim().to_string())
                .unwrap_or_default()
        };
        let compact = collapse_ws(&get(sections, i)).to_lowercase();
        let is_contents = matches!(
            compact.as_str(),
            "contents" | "目录" | "目次" | "table of contents" | "致谢" | "acknowledge"
        );
        if !is_contents {
            i += 1;
            continue;
        }
        sections.remove(i);
        if i >= sections.len() {
            break;
        }
        let mut prefix = if !eng {
            get(sections, i).chars().take(3).collect::<String>()
        } else {
            get(sections, i)
                .split_whitespace()
                .take(2)
                .collect::<Vec<_>>()
                .join(" ")
        };
        while prefix.is_empty() {
            sections.remove(i);
            if i >= sections.len() {
                break;
            }
            prefix = if !eng {
                get(sections, i).chars().take(3).collect::<String>()
            } else {
                get(sections, i)
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ")
            };
        }
        sections.remove(i);
        if i >= sections.len() || prefix.is_empty() {
            break;
        }
        let re = anchored(&regex::escape(&prefix));
        for j in i..(i + 128).min(sections.len()) {
            if !re.is_match(&get(sections, j)) {
                continue;
            }
            sections.drain(i..j);
            break;
        }
    }
}

/// Colon-title splitting — mirrors nlp/__init__.py:879-898.
///
/// NOTE (faithful port): the Python code checks `len(arr[1]) >= 32` where
/// arr[1] is the *separator* captured by re.split (always 1-2 chars), so
/// the condition never holds and no title is ever inserted. We reproduce
/// this exact behavior — tests assert no insertion, matching Python.
pub fn make_colon_as_title(_sections: &mut Vec<(String, String)>) {
    // Python: for each colon-terminated line, reverse it, split on
    // ([。？！!?;；]| \.), then require len(arr[1]) >= 32. arr[1] is the
    // separator itself (length 1-2), so the branch is unreachable.
    // Faithfully ported as a no-op with the check intact for clarity.
    let _ = (BULLET_PATTERN.len(), not_bullet);
}

/// A node in the tree-merge hierarchy — mirrors nlp/__init__.py Node
/// (lines 1512-1591). Children are arena indices.
#[derive(Debug)]
pub struct Node {
    level: i64,
    depth: i64,
    texts: Vec<String>,
    children: Vec<usize>,
}

impl Node {
    fn new(level: i64, depth: i64) -> Self {
        Self {
            level,
            depth,
            texts: Vec::new(),
            children: Vec::new(),
        }
    }

    fn add_text(&mut self, text: &str) {
        self.texts.push(text.to_string());
    }

    /// Build the tree from (level, text) lines into a flat arena —
    /// mirrors build_tree (stack: pop while level <= top.level).
    fn build_tree(arena: &mut Vec<Node>, root: usize, lines: &[(i64, String)], depth: i64) {
        let mut stack: Vec<usize> = vec![root];
        for (level, text) in lines {
            if depth != -1 && *level > depth {
                let leaf = *stack.last().unwrap();
                arena[leaf].add_text(text);
                continue;
            }
            while stack.len() > 1 && *level <= arena[*stack.last().unwrap()].level {
                stack.pop();
            }
            let parent = *stack.last().unwrap();
            let idx = arena.len();
            let mut node = Node::new(*level, depth);
            node.texts.push(text.clone());
            arena.push(node);
            arena[parent].children.push(idx);
            stack.push(idx);
        }
    }

    /// DFS output — mirrors get_tree/_dfs. Returns joined chunk strings.
    fn get_tree(arena: &[Node], node: usize, titles: &mut Vec<String>, out: &mut Vec<String>) {
        let n = &arena[node];
        let level = n.level;
        let texts = &n.texts;
        let child = &n.children;

        if level == 0 && !texts.is_empty() {
            let mut parts = titles.clone();
            parts.extend(texts.iter().cloned());
            out.push(parts.join("\n"));
        }

        let path_titles: Vec<String> = if 1 <= level && level <= n.depth {
            let mut p = titles.clone();
            p.extend(texts.iter().cloned());
            p
        } else {
            titles.clone()
        };

        if level > n.depth && !texts.is_empty() {
            let mut parts = path_titles.clone();
            parts.extend(texts.iter().cloned());
            out.push(parts.join("\n"));
        } else if child.is_empty() && (1 <= level && level <= n.depth) {
            out.push(path_titles.join("\n"));
        }

        for &c in child {
            Node::get_tree(arena, c, &mut path_titles.clone(), out);
        }
    }
}

/// Tree-merge sections into hierarchical chunks — mirrors
/// nlp/__init__.py:931-977. `sections` are (text, layout) pairs.
pub fn tree_merge(
    bull: i64,
    sections: &[(String, String)],
    depth: usize,
    family_len: usize,
) -> Vec<String> {
    if sections.is_empty() || bull < 0 {
        return sections.iter().map(|(t, _)| t.clone()).collect();
    }

    // filter out position info and bare-number lines (nlp:938-939)
    let filtered: Vec<(String, String)> = sections
        .iter()
        .filter(|(t, _)| {
            let head = t.split('@').next().unwrap_or_default().trim();
            head.chars().count() > 1 && !head.chars().all(|c| c.is_ascii_digit())
        })
        .map(|(t, o)| (t.clone(), o.clone()))
        .collect();

    let get_level = |section: &(String, String)| -> (i64, String) {
        let text = section.0.replace('\u{3000}', " ").trim().to_string();
        for (i, pat) in BULLET_PATTERN[bull as usize].iter().enumerate() {
            if anchored(pat).is_match(&text) {
                return (i as i64 + 1, text);
            }
        }
        if (section.1.contains("title") || section.1.contains("head"))
            && !not_title(text.split('@').next().unwrap_or_default()) {
                return (family_len as i64 + 1, text);
            }
        (family_len as i64 + 2, text)
    };

    let mut level_set: Vec<i64> = Vec::new();
    let mut lines: Vec<(i64, String)> = Vec::new();
    for section in &filtered {
        let (level, text) = get_level(section);
        if text.trim_matches('\n').is_empty() {
            continue;
        }
        if !level_set.contains(&level) {
            level_set.push(level);
        }
        lines.push((level, text));
    }
    level_set.sort_unstable();

    let target_level = if depth <= level_set.len() {
        level_set[depth - 1]
    } else {
        *level_set.last().unwrap()
    };
    let target_level = if target_level == family_len as i64 + 2 {
        if level_set.len() > 1 {
            level_set[level_set.len() - 2]
        } else {
            level_set[0]
        }
    } else {
        target_level
    };

    let mut arena: Vec<Node> = vec![Node::new(0, target_level)];
    Node::build_tree(&mut arena, 0, &lines, target_level);

    let mut out = Vec::new();
    Node::get_tree(&arena, 0, &mut Vec::new(), &mut out);
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

/// Laws chunk() text-family core — mirrors laws.py:172-177 + 207-217:
/// split lines, drop empties, remove contents, classify and tree-merge.
pub fn parse_laws_text(filename: &str, text: &str, depth: usize) -> Result<Vec<String>, String> {
    let lower = filename.to_lowercase();
    let supported = lower.ends_with(".txt")
        || lower.ends_with(".md")
        || lower.ends_with(".markdown")
        || lower.ends_with(".mdx");
    if !supported {
        return Err(format!(
            "file type not supported yet(doc, docx, pdf, txt supported) got {filename}"
        ));
    }
    let mut sections: Vec<String> = text
        .lines()
        .map(String::from)
        .filter(|s| !s.is_empty())
        .collect();
    remove_contents_table(&mut sections, false);
    let bull = bullets_category(&sections);
    let pairs: Vec<(String, String)> = sections
        .iter()
        .map(|s| (s.clone(), String::new()))
        .collect();
    let family_len = BULLET_PATTERN
        .get(bull.max(0) as usize)
        .map(|f| f.len())
        .unwrap_or(0);
    let bull_i = bull;
    let res = if bull >= 0 {
        tree_merge(bull_i, &pairs, depth, family_len)
    } else {
        sections
    };
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_contents_table_strips_section() {
        let mut sections = vec![
            "目录".to_string(),
            "第一章 总则".to_string(),
            "第二章 水质".to_string(),
            "第三条 定义".to_string(),
        ];
        remove_contents_table(&mut sections, false);
        assert!(!sections.iter().any(|s| s == "目录"));
    }

    #[test]
    fn remove_contents_table_english_faithful_noop() {
        // Python: re.sub strips spaces -> "tableofcontents" which does NOT
        // match the "table of contents" alternative (spaces required).
        // Faithful port: nothing is removed.
        let mut sections = vec![
            "Table of Contents".to_string(),
            "Chapter One Introduction".to_string(),
            "1. Overview".to_string(),
        ];
        remove_contents_table(&mut sections, true);
        assert_eq!(
            sections.len(),
            3,
            "Python leaves it untouched: {sections:?}"
        );
    }

    #[test]
    fn remove_contents_table_removes_entries_sharing_prefix() {
        let mut sections = vec![
            "目录".to_string(),
            "第一章".to_string(),
            "第二章".to_string(),
            "正文开始".to_string(),
        ];
        remove_contents_table(&mut sections, false);
        // Python pops the header line AND the prefix-source line
        // ("第一章", first 3 chars "第一"), then looks for another line
        // starting with "第一" — "第二章" does not match.
        assert_eq!(sections, vec!["第二章".to_string(), "正文开始".to_string()]);
    }

    #[test]
    fn make_colon_as_title_never_inserts_faithful_to_python() {
        let mut sections = vec![(
            "第一章：这是总则部分的详细说明内容用于测试标题分割逻辑是否生效。：".to_string(),
            "text".to_string(),
        )];
        let before = sections.len();
        make_colon_as_title(&mut sections);
        assert_eq!(
            sections.len(),
            before,
            "Python arr[1] is the separator, always < 32 chars"
        );
    }

    #[test]
    fn tree_merge_groups_by_title_level() {
        // family 1 (numeric): "1. " → level 1, "1.1 " → level 2, body → 6
        let sections = vec![
            ("1. 总则".to_string(), "title".to_string()),
            ("这是正文内容第一段。".to_string(), "text".to_string()),
            ("2. 水质标准".to_string(), "title".to_string()),
            ("这是正文内容第二段。".to_string(), "text".to_string()),
        ];
        let bull = bullets_category(&["1. 总则".to_string(), "2. 水质标准".to_string()]);
        assert_eq!(bull, 1);
        let family_len = BULLET_PATTERN[1].len();
        let chunks = tree_merge(bull as i64, &sections, 2, family_len);
        assert!(!chunks.is_empty(), "{chunks:?}");
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("1. 总则"));
        assert!(chunks[0].contains("这是正文内容第一段"));
        assert!(chunks[1].contains("2. 水质标准"));
    }

    #[test]
    fn tree_merge_chinese_family() {
        let sections = vec![
            ("第一章 总则".to_string(), "title".to_string()),
            (
                "第一条 本规定适用于全部范围。".to_string(),
                "title".to_string(),
            ),
            ("第二章 水质".to_string(), "title".to_string()),
        ];
        let bull = bullets_category(&[
            "第一章 总则".to_string(),
            "第一条 本规定适用于全部范围。".to_string(),
            "第二章 水质".to_string(),
        ]);
        assert_eq!(bull, 0);
        let family_len = BULLET_PATTERN[0].len();
        let chunks = tree_merge(bull as i64, &sections, 2, family_len);
        assert!(!chunks.is_empty(), "{chunks:?}");
        assert!(chunks.iter().any(|c| c.contains("第一章 总则")));
        assert!(chunks.iter().any(|c| c.contains("第二章 水质")));
    }

    #[test]
    fn parse_laws_text_txt_family() {
        let text = "目录\n第一章 总则\n第一条 定义\n第二章 水质\n正文内容\n";
        let chunks = parse_laws_text("laws.txt", text, 2).unwrap();
        assert!(!chunks.is_empty());
        // "第一章 总则" is the prefix-source line and gets removed by
        // remove_contents_table (Python-verified); remaining sections
        // tree-merge into chunks that include the 第二章 title.
        assert!(
            chunks.iter().any(|c| c.contains("第二章 水质")),
            "{chunks:?}"
        );
        assert!(parse_laws_text("laws.pdf", text, 2).is_err());
    }
}
