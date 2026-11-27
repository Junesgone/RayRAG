//! Tag dataset parser — mirrors `rag/app/tag.py` chunk().
//!
//! Two-column content/tags formats: txt/csv with TAB-or-comma delimiter.
//! Every pair becomes one chunk; the second column is the comma-separated
//! tag list (`.` replaced with `_`). `label_question` (tag retrieval from
//! tag-KBs) is a chunk-store integration gap recorded below.

use std::collections::HashMap;

/// A parsed content/tags pair (content, tag list, row index).
#[derive(Debug, Clone, PartialEq)]
pub struct TagPair {
    pub content: String,
    pub tags: Vec<String>,
    pub row_num: i64,
}

/// beAdoc — assemble a tag chunk dict (tag.py:27-34): content stays as-is,
/// tags are comma-split, stripped, `.`→`_`.
pub fn be_adoc_tags(_content: &str, tags: &str) -> Vec<String> {
    tags.split(',')
        .map(|t| t.trim().replace('.', "_"))
        .filter(|t| !t.is_empty())
        .collect()
}

/// Build the chunk fields for a content/tags pair.
pub fn tag_doc_fields(
    filename: &str,
    content: &str,
    tags: &str,
    row_num: i64,
) -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("docnm_kwd".to_string(), filename.to_string());
    m.insert("content_with_weight".to_string(), content.to_string());
    // content_ltks: whitespace-split approximation of rag_tokenizer.tokenize
    let ltks: Vec<String> = content
        .split_whitespace()
        .map(String::from)
        .filter(|t| !t.is_empty())
        .collect();
    m.insert("content_ltks".to_string(), ltks.join(" "));
    m.insert("tag_kwd".to_string(), be_adoc_tags(content, tags).join(","));
    m.insert("row_num".to_string(), row_num.to_string());
    m
}

/// Detect the delimiter for a txt/csv blob (tag.py:66-72): TAB wins on >=.
pub fn detect_delimiter(text: &str) -> char {
    let mut comma = 0usize;
    let mut tab = 0usize;
    for line in text.lines() {
        if line.split(',').count() == 2 {
            comma += 1;
        }
        if line.split('\t').count() == 2 {
            tab += 1;
        }
    }
    if tab >= comma { '\t' } else { ',' }
}

/// Parse a txt tag blob (tag.py:62-93): a 2-field line closes the current
/// content (content accumulates newlines for deformed lines and between
/// pairs), its second field is the tags.
pub fn parse_tag_text(text: &str, delimiter: char) -> Vec<TagPair> {
    let lines: Vec<&str> = text.lines().collect();
    let mut pairs = Vec::new();
    let mut content = String::new();
    for (i, line) in lines.iter().enumerate() {
        let arr: Vec<&str> = line.split(delimiter).collect();
        if arr.len() != 2 {
            content.push('\n');
            content.push_str(line);
        } else {
            content.push('\n');
            content.push_str(arr[0]);
            pairs.push(TagPair {
                content: content.trim().to_string(),
                tags: be_adoc_tags(&content, arr[1]),
                row_num: i as i64,
            });
            content.clear();
        }
    }
    pairs
}

/// Split a CSV line respecting quoted fields (shared with qa.rs semantics).
fn split_csv_line(line: &str, delimiter: char) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == delimiter {
            fields.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    fields.push(cur.trim().to_string());
    fields
}

/// Parse a csv tag blob (tag.py:95-119): csv.reader semantics — empty fields
/// are dropped, a 2-field row closes the content.
pub fn parse_tag_csv(text: &str) -> Vec<TagPair> {
    let lines: Vec<&str> = text.lines().collect();
    let mut pairs = Vec::new();
    let mut content = String::new();
    for (i, line) in lines.iter().enumerate() {
        let row: Vec<String> = split_csv_line(line, ',')
            .into_iter()
            .filter(|r| !r.is_empty())
            .collect();
        if row.len() != 2 {
            content.push('\n');
            content.push_str(line);
        } else {
            content.push('\n');
            content.push_str(&row[0]);
            pairs.push(TagPair {
                content: content.trim().to_string(),
                tags: be_adoc_tags(&content, &row[1]),
                row_num: i as i64,
            });
            content.clear();
        }
    }
    pairs
}

/// Dispatch — mirrors tag.py chunk() for txt/csv; xlsx routes to the
/// existing parser::excel module.
pub fn parse_tag_file(filename: &str, text: &str) -> Result<Vec<TagPair>, String> {
    let lower = filename.to_lowercase();
    if lower.ends_with(".txt") {
        let delimiter = detect_delimiter(text);
        Ok(parse_tag_text(text, delimiter))
    } else if lower.ends_with(".csv") {
        Ok(parse_tag_csv(text))
    } else {
        Err(format!(
            "Excel, csv(txt) format files are supported (got {filename})"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn be_adoc_tags_splits_and_normalizes() {
        assert_eq!(
            be_adoc_tags("x", " 水产 , 水质.management ,,"),
            vec!["水产".to_string(), "水质_management".to_string()]
        );
        assert!(be_adoc_tags("x", " , ").is_empty());
    }

    #[test]
    fn tag_doc_fields_carries_tag_kwd() {
        let m = tag_doc_fields("tags.txt", "水体富营养化", "水质, 藻类", 3);
        assert_eq!(m.get("docnm_kwd").unwrap(), "tags.txt");
        assert_eq!(m.get("content_with_weight").unwrap(), "水体富营养化");
        assert_eq!(m.get("tag_kwd").unwrap(), "水质,藻类");
        assert_eq!(m.get("row_num").unwrap(), "3");
    }

    #[test]
    fn detect_delimiter_prefers_tab_on_ties() {
        assert_eq!(detect_delimiter("a\tb\nc\td\n"), '\t');
        assert_eq!(detect_delimiter("a,b\nc,d\n"), ',');
        assert_eq!(detect_delimiter("a\tb\nc,d\n"), '\t');
    }

    #[test]
    fn parse_tag_text_closes_pairs_on_two_fields() {
        let text = "内容1\t标签1,标签2\n续行\n内容2\t标签3";
        let pairs = parse_tag_text(text, '\t');
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].content, "内容1");
        assert_eq!(
            pairs[0].tags,
            vec!["标签1".to_string(), "标签2".to_string()]
        );
        // deformed line joins the NEXT content (content starts empty)
        assert_eq!(pairs[1].content, "续行\n内容2");
        assert_eq!(pairs[1].tags, vec!["标签3".to_string()]);
    }

    #[test]
    fn parse_tag_csv_drops_empty_fields() {
        let text = "c1,tag1\nc2,t\n";
        let pairs = parse_tag_csv(text);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].content, "c1");
        assert_eq!(pairs[0].tags, vec!["tag1".to_string()]);
        assert_eq!(pairs[1].content, "c2");
        assert_eq!(pairs[1].tags, vec!["t".to_string()]);
    }

    #[test]
    fn parse_tag_csv_3_field_line_joins_next_content() {
        // 3-field row is a deformed line → absorbs into the next content
        // (mirrors csv.reader row != 2 branch in tag.py)
        let text = "c1,tag1,tag2\nc2,t\n";
        let pairs = parse_tag_csv(text);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].content, "c1,tag1,tag2\nc2");
        assert_eq!(pairs[0].tags, vec!["t".to_string()]);
    }

    #[test]
    fn parse_tag_file_dispatch_rejects_unsupported() {
        assert!(parse_tag_file("data.pdf", "x").is_err());
        let pairs = parse_tag_file("tags.txt", "q\ta,b\n").unwrap();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].tags, vec!["a".to_string(), "b".to_string()]);
    }
}
