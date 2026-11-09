//! Q&A dataset parser — mirrors `rag/app/qa.py` chunk() pure-algorithm core.
//!
//! Supports the two-column Q&A formats RAGFlow handles in-process:
//! - `.txt` / `.csv`: TAB or comma separated question/answer pairs
//! - `.md` / `.markdown` / `.mdx`: markdown heading levels as the question
//!   stack, body text as the answer
//!
//! `beAdoc` mirrors the chunk-document assembly (Question:/Answer: prefixes,
//! rmPrefix stripping, title/content fields). PDF/DOCX/Excel branches route
//! to the existing parser modules in RayRAG (parser::pdf, parser::docx,
//! parser::excel) — this file keeps the dispatch + text-format logic.

use std::collections::HashMap;

/// A parsed Q&A pair (question, answer, row index).
#[derive(Debug, Clone, PartialEq)]
pub struct QaPair {
    pub question: String,
    pub answer: String,
    pub row_num: i64,
}

/// rmPrefix — strip `问题|答案|回答|user|assistant|Q|A|Question|Answer|问|答`
/// followed by whitespace/colon prefixes (qa.py:257-260).
pub fn rm_prefix(txt: &str) -> String {
    let trimmed = txt.trim();
    let lower = trimmed.to_lowercase();
    let prefixes = [
        "问题",
        "答案",
        "回答",
        "user",
        "assistant",
        "q",
        "a",
        "question",
        "answer",
        "问",
        "答",
    ];
    for p in prefixes {
        if lower.starts_with(p) {
            let rest = &trimmed[p.len()..];
            if let Some(stripped) = rest.strip_prefix(['\t', ':', '：', ' ', '　']) {
                return stripped.trim_start().to_string();
            }
        }
    }
    trimmed.to_string()
}

/// beAdoc — assemble a chunk document dict from a Q&A pair (qa.py:291-301).
/// Returns (content_with_weight, content_ltks) — the tokenized question list
/// and the `问题：q\t回答：a` weighted content.
pub fn be_adoc(question: &str, answer: &str, eng: bool) -> (String, Vec<String>) {
    let qprefix = if eng { "Question: " } else { "问题：" };
    let aprefix = if eng { "Answer: " } else { "回答：" };
    let q = rm_prefix(question);
    let a = rm_prefix(answer);
    let content = format!("{qprefix}{q}\t{aprefix}{a}");
    // content_ltks = rag_tokenizer.tokenize(q) — space-split approximation
    // (RayRAG tokenizer lives in chunk/; plain whitespace split mirrors the
    // Q&A keywords that downstream BM25 consumes).
    let ltks: Vec<String> = q
        .split_whitespace()
        .map(String::from)
        .filter(|t| !t.is_empty())
        .collect();
    (content, ltks)
}

/// Detect the delimiter for a txt/csv blob — qa.py:333-343: TAB wins if more
/// lines split into exactly 2 parts by TAB than by comma.
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

/// Parse a txt/csv Q&A blob — qa.py:333-402. Deformed lines either append to
/// the pending answer (when a question is open) or are counted as failures.
/// Returns (pairs, failed_line_numbers).
pub fn parse_qa_text(text: &str, delimiter: char) -> (Vec<QaPair>, Vec<usize>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut pairs = Vec::new();
    let mut fails = Vec::new();
    let mut question = String::new();
    let mut answer = String::new();
    for (i, line) in lines.iter().enumerate() {
        let arr: Vec<&str> = line.split(delimiter).collect();
        if arr.len() != 2 {
            if !question.is_empty() {
                answer.push('\n');
                answer.push_str(line);
            } else {
                fails.push(i + 1);
            }
        } else {
            if !question.is_empty() && !answer.is_empty() {
                pairs.push(QaPair {
                    question: question.clone(),
                    answer: answer.clone(),
                    row_num: i as i64,
                });
            }
            question = arr[0].to_string();
            answer = arr[1].to_string();
        }
    }
    if !question.is_empty() {
        pairs.push(QaPair {
            question,
            answer,
            row_num: lines.len() as i64,
        });
    }
    (pairs, fails)
}

/// Split a CSV line respecting quoted fields (mirrors csv.reader basics):
/// a delimiter inside double quotes is not a separator; "" escapes a quote.
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

/// Parse a csv blob using the csv reader semantics (qa.py:372-402) — rows
/// are split by delimiter, quoted fields are handled minimally (strip
/// surrounding quotes).
pub fn parse_qa_csv(text: &str, delimiter: char) -> (Vec<QaPair>, Vec<usize>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut pairs = Vec::new();
    let mut fails = Vec::new();
    let mut question = String::new();
    let mut answer = String::new();
    let mut row_count = 0usize;
    for (i, line) in lines.iter().enumerate() {
        row_count += 1;
        let row = split_csv_line(line, delimiter);
        if row.len() != 2 {
            if !question.is_empty() {
                answer.push('\n');
                answer.push_str(line);
            } else {
                fails.push(i + 1);
            }
        } else {
            if !question.is_empty() && !answer.is_empty() {
                pairs.push(QaPair {
                    question: question.clone(),
                    answer: answer.clone(),
                    row_num: i as i64,
                });
            }
            question = row[0].clone();
            answer = row[1].clone();
        }
    }
    if !question.is_empty() {
        pairs.push(QaPair {
            question,
            answer,
            row_num: row_count as i64,
        });
    }
    (pairs, fails)
}

/// mdQuestionLevel — qa.py:303-306: a markdown heading `#`...`######` returns
/// (level, heading text without markers); other lines return (0, "").
pub fn md_question_level(line: &str) -> (usize, String) {
    let trimmed = line.trim();
    let mut level = 0usize;
    for c in trimmed.chars() {
        if c == '#' {
            level += 1;
        } else {
            break;
        }
    }
    if level == 0 || level > 6 {
        return (0, String::new());
    }
    let rest = trimmed[level..].trim();
    (level, rest.to_string())
}

/// Parse a markdown Q&A blob — qa.py:413-448: headings (≤6) form a question
/// stack; body text accumulates into the answer of the current question.
/// Code fences (` ``` `) are skipped as questions.
pub fn parse_qa_markdown(text: &str) -> Vec<QaPair> {
    let lines: Vec<&str> = text.lines().collect();
    let mut pairs = Vec::new();
    let mut question_stack: Vec<String> = Vec::new();
    let mut level_stack: Vec<usize> = Vec::new();
    let mut last_answer = String::new();
    let mut code_block = false;
    let mut index = 0usize;

    for line in &lines {
        if line.trim_start().starts_with("```") {
            code_block = !code_block;
        }
        let (question_level, question) = if code_block {
            (0, String::new())
        } else {
            md_question_level(line)
        };

        if question_level == 0 || question_level > 6 {
            // not a question → append to answer
            if !last_answer.is_empty() {
                last_answer.push('\n');
            }
            last_answer.push_str(line);
        } else {
            // is a question
            if !last_answer.trim().is_empty() {
                let sum_question = question_stack.join("\n");
                if !sum_question.is_empty() {
                    pairs.push(QaPair {
                        question: sum_question,
                        answer: last_answer.trim().to_string(),
                        row_num: index as i64,
                    });
                }
                last_answer.clear();
            }
            let i = question_level;
            while !question_stack.is_empty() && i <= *level_stack.last().unwrap_or(&usize::MAX) {
                question_stack.pop();
                level_stack.pop();
            }
            question_stack.push(question);
            level_stack.push(question_level);
        }
        index += 1;
    }
    if !last_answer.trim().is_empty() {
        let sum_question = question_stack.join("\n");
        if !sum_question.is_empty() {
            pairs.push(QaPair {
                question: sum_question,
                answer: last_answer.trim().to_string(),
                row_num: lines.len() as i64,
            });
        }
    }
    pairs
}

/// Dispatch entry — mirrors qa.py chunk() for the text formats. Returns
/// per-pair chunk docs as (content_with_weight, content_ltks, row_num).
pub fn parse_qa_file(
    filename: &str,
    text: &str,
) -> Result<Vec<(String, Vec<String>, i64)>, String> {
    let lower = filename.to_lowercase();
    let eng = false; // Q&A parser defaults to Chinese prefix in RAGFlow chunk()
    let doc_title: String = {
        // title_tks = rag_tokenizer.tokenize(re.sub(r"\.[a-zA-Z]+$", "", filename))
        let stem = lower
            .rfind('.')
            .map(|idx| filename[..idx].to_string())
            .unwrap_or_else(|| filename.to_string());
        stem.split_whitespace()
            .map(String::from)
            .collect::<Vec<_>>()
            .join(" ")
    };
    let _ = doc_title;

    let mut pairs: Vec<QaPair> = Vec::new();
    if lower.ends_with(".txt") {
        let delimiter = detect_delimiter(text);
        let (p, _fails) = parse_qa_text(text, delimiter);
        pairs = p;
    } else if lower.ends_with(".csv") {
        let delimiter = if text.lines().any(|l| l.contains('\t')) {
            '\t'
        } else {
            ','
        };
        let (p, _fails) = parse_qa_csv(text, delimiter);
        pairs = p;
    } else if lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".mdx") {
        pairs = parse_qa_markdown(text);
    } else {
        return Err(format!(
            "Excel, csv(txt), pdf, markdown and docx format files are supported (got {filename})"
        ));
    }

    Ok(pairs
        .into_iter()
        .map(|pair| {
            let (content, ltks) = be_adoc(&pair.question, &pair.answer, eng);
            (content, ltks, pair.row_num)
        })
        .collect())
}

/// Tokenize a doc into a HashMap of fields (used by callers to build chunks).
pub fn qa_doc_fields(
    filename: &str,
    question: &str,
    answer: &str,
    eng: bool,
    row_num: i64,
) -> HashMap<String, String> {
    let (content, ltks) = be_adoc(question, answer, eng);
    let mut m = HashMap::new();
    m.insert("docnm_kwd".to_string(), filename.to_string());
    m.insert("content_with_weight".to_string(), content);
    m.insert("content_ltks".to_string(), ltks.join(" "));
    m.insert("row_num".to_string(), row_num.to_string());
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rm_prefix_strips_common_prefixes() {
        assert_eq!(rm_prefix("问题：水产怎么养"), "水产怎么养");
        assert_eq!(rm_prefix("Q: What is pH?"), "What is pH?");
        assert_eq!(rm_prefix("Answer: 42"), "42");
        assert_eq!(rm_prefix("  answer： 直接回答"), "直接回答");
        assert_eq!(rm_prefix("普通文本"), "普通文本");
    }

    #[test]
    fn be_adoc_builds_weighted_content() {
        let (content, ltks) = be_adoc("水质", "保持pH中性", false);
        assert_eq!(content, "问题：水质\t回答：保持pH中性");
        assert_eq!(ltks, vec!["水质".to_string()]);
        let (content_en, _) = be_adoc("water", "keep pH neutral", true);
        assert_eq!(content_en, "Question: water\tAnswer: keep pH neutral");
    }

    #[test]
    fn detect_delimiter_prefers_tab_on_ties() {
        assert_eq!(detect_delimiter("a\tb\nc\td\n"), '\t');
        assert_eq!(detect_delimiter("a,b\nc,d\n"), ',');
        // tie → tab
        assert_eq!(detect_delimiter("a\tb\nc,d\n"), '\t');
    }

    #[test]
    fn parse_qa_text_handles_continuations_and_fails() {
        let text = "问题1\t答案1\n问题2\t答案2\n续行1\n续行2\n问题3\t答案3\n坏行";
        let (pairs, fails) = parse_qa_text(text, '\t');
        assert_eq!(pairs.len(), 3);
        // 问题2's answer absorbs the two continuation lines
        assert!(pairs[1].answer.contains("续行1"));
        assert!(pairs[1].answer.contains("续行2"));
        // trailing bad line after an open question becomes its answer (RAGFlow semantics)
        assert!(fails.is_empty(), "no fail while question open: {fails:?}");
        assert!(pairs[2].answer.contains("坏行"));
    }

    #[test]
    fn parse_qa_text_fails_without_open_question() {
        let text = "坏行1\n坏行2\n问题1\t答案1\n";
        let (pairs, fails) = parse_qa_text(text, '\t');
        assert_eq!(pairs.len(), 1);
        assert_eq!(fails, vec![1, 2]);
    }

    #[test]
    fn parse_qa_csv_handles_quoted_fields() {
        let text = "\"Q1\",\"A, with comma\"\nQ2,A2\n";
        let (pairs, fails) = parse_qa_csv(text, ',');
        assert!(fails.is_empty());
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].question, "Q1");
        assert_eq!(pairs[0].answer, "A, with comma");
    }

    #[test]
    fn md_question_level_parses_headings() {
        assert_eq!(
            md_question_level("## 水质管理"),
            (2, "水质管理".to_string())
        );
        assert_eq!(md_question_level("# 一、概述"), (1, "一、概述".to_string()));
        assert_eq!(md_question_level("####### too deep"), (0, String::new()));
        assert_eq!(md_question_level("plain text"), (0, String::new()));
    }

    #[test]
    fn parse_qa_markdown_builds_question_stack() {
        let md = "# 概述\n这是概述内容\n## 水质\n水质怎么测\n## 投喂\n投喂量多少\n正文第二行\n";
        let pairs = parse_qa_markdown(md);
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[0].question, "概述");
        assert!(pairs[0].answer.contains("这是概述内容"));
        assert_eq!(pairs[1].question, "概述\n水质");
        assert!(pairs[1].answer.contains("水质怎么测"));
        assert_eq!(pairs[2].question, "概述\n投喂");
        assert!(pairs[2].answer.contains("投喂量多少"));
        assert!(pairs[2].answer.contains("正文第二行"));
    }

    #[test]
    fn parse_qa_markdown_skips_code_fences() {
        let md = "# Q\n```\n# not a question\n```\nanswer text\n";
        let pairs = parse_qa_markdown(md);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].question, "Q");
        assert!(pairs[0].answer.contains("# not a question"));
    }

    #[test]
    fn parse_qa_file_dispatch_rejects_unsupported() {
        assert!(parse_qa_file("data.pdf", "x").is_err());
        let out = parse_qa_file("qa.txt", "q1\ta1\nq2\ta2\n").unwrap();
        assert_eq!(out.len(), 2);
        assert!(out[0].0.starts_with("问题：q1"));
    }
}
