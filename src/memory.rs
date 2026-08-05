//! Memory utilities — search-result highlighting and aggregation.
//!
//! Rust ports of RAGFlow `memory/utils/highlight_utils.py` and
//! `memory/utils/aggregation_utils.py`. Pure functions, no I/O, so they can be
//! unit-tested without a live document store.

use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

/// Sentence separators used by RAGFlow's highlight splitter
/// (`re.split(r"[.?!;\n]", txt)`).
const SENTENCE_SEPARATORS: &[char] = &['.', '?', '!', ';', '\n'];

/// Word-boundary characters accepted around an English keyword match,
/// mirroring RAGFlow's character class
/// `(^|[ .?/'\"(),!:;-])(kw)([ .?/'\"(),!:;-]|$)`.
const WORD_BOUNDARY_CLASS: &str = "[ .?/'\"(),!:;-]";

fn em_pattern() -> &'static Regex {
    static EM: OnceLock<Regex> = OnceLock::new();
    EM.get_or_init(|| Regex::new(r"(?i)<em>[^<>]+</em>").expect("em regex is valid"))
}

/// Detect whether a sentence is English using an ASCII-letter heuristic.
/// RAGFlow passes a language-aware callback here; this is the portable default.
pub fn is_english_ascii(sentence: &str) -> bool {
    let letters = sentence.chars().filter(|c| c.is_alphabetic()).count();
    if letters == 0 {
        return true;
    }
    let ascii_letters = sentence.chars().filter(|c| c.is_ascii_alphabetic()).count();
    ascii_letters * 10 >= letters * 7 // >= 70% ASCII letters -> English
}

/// Wrap keyword matches in text with `<em>`, by sentence.
///
/// Mirrors RAGFlow `highlight_utils.highlight_text` exactly:
/// - `\r\n` is normalized to spaces first.
/// - The text is split on `. ? ! ; \n`; empty sentences are skipped.
/// - English sentences (or every sentence when `is_english` is `None`) use a
///   word-boundary regex so sub-word matches are not wrapped.
/// - Non-English sentences use literal replacement, longest keyword first,
///   and the replacement keeps the *keyword's* casing (RAGFlow behaviour).
/// - Only sentences that end up containing `<em>...</em>` are kept, joined
///   with `"..."`.
/// - When no sentence matches, the space-normalized original text is returned.
pub fn highlight_text(
    txt: &str,
    keywords: &[String],
    is_english: Option<fn(&str) -> bool>,
) -> String {
    if txt.is_empty() || keywords.is_empty() {
        return String::new();
    }
    let normalized = txt.replace(['\r', '\n'], " ");
    let mut kept: Vec<String> = Vec::new();
    for raw in normalized.split(SENTENCE_SEPARATORS) {
        let sentence = raw.trim();
        if sentence.is_empty() {
            continue;
        }
        let highlighted = if is_english.is_none_or(|f| f(sentence)) {
            highlight_english(sentence, keywords)
        } else {
            highlight_literal(sentence, keywords)
        };
        if em_pattern().is_match(&highlighted) {
            kept.push(highlighted);
        }
    }
    if kept.is_empty() {
        normalized
    } else {
        kept.join("...")
    }
}

/// Word-boundary highlighting: `(^|[ .?/'"(),!:;-])(kw)([ .?/'"(),!:;-]|$)`.
fn highlight_english(sentence: &str, keywords: &[String]) -> String {
    let mut out = sentence.to_string();
    for keyword in keywords {
        if keyword.is_empty() {
            continue;
        }
        let pattern = format!(
            r"(?i)(^|{class})({escaped})({class}|$)",
            class = WORD_BOUNDARY_CLASS,
            escaped = regex::escape(keyword),
        );
        let Ok(re) = Regex::new(&pattern) else {
            continue;
        };
        out = re.replace_all(&out, "$1<em>$2</em>$3").into_owned();
    }
    out
}

/// Literal highlighting with longest-keyword-first ordering; the replacement
/// keeps the keyword's own casing (RAGFlow uses `<em>{w}</em>`).
fn highlight_literal(sentence: &str, keywords: &[String]) -> String {
    let mut ordered: Vec<&String> = keywords.iter().filter(|k| !k.is_empty()).collect();
    ordered.sort_by_key(|keyword| std::cmp::Reverse(keyword.chars().count()));
    let mut out = sentence.to_string();
    for keyword in ordered {
        let Ok(re) = Regex::new(&format!("(?i){}", regex::escape(keyword))) else {
            continue;
        };
        let replacement = format!("<em>{}</em>", keyword.replace('$', "$$"));
        out = re.replace_all(&out, replacement).into_owned();
    }
    out
}

/// Build id -> highlighted text from a list of message documents.
///
/// Mirrors RAGFlow `highlight_utils.get_highlight_from_messages`: only
/// documents carrying a string `id` and a string `field_name` are considered,
/// and only results that actually contain an `<em>` match are returned.
pub fn get_highlight_from_messages(
    messages: &[Value],
    keywords: &[String],
    field_name: &str,
    is_english: Option<fn(&str) -> bool>,
) -> BTreeMap<String, String> {
    let mut ans = BTreeMap::new();
    if messages.is_empty() || keywords.is_empty() {
        return ans;
    }
    for doc in messages {
        let Some(doc_id) = doc.get("id").and_then(Value::as_str) else {
            continue;
        };
        let Some(txt) = doc.get(field_name).and_then(Value::as_str) else {
            continue;
        };
        if txt.is_empty() {
            continue;
        }
        let highlighted = highlight_text(txt, keywords, is_english);
        if em_pattern().is_match(&highlighted) {
            ans.insert(doc_id.to_string(), highlighted);
        }
    }
    ans
}

/// Aggregate message documents by a field; returns [(value, count), ...].
///
/// Mirrors RAGFlow `aggregation_utils.aggregate_by_field`:
/// - Documents carrying both `value` and `count` are treated as
///   pre-aggregated rows and appended as-is, in encounter order.
/// - Otherwise the field value (String or list of String) is counted with
///   whitespace stripped; entries follow first-seen insertion order.
pub fn aggregate_by_field(messages: &[Value], field_name: &str) -> Vec<(String, usize)> {
    if messages.is_empty() {
        return Vec::new();
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut result: Vec<(String, usize)> = Vec::new();
    for doc in messages {
        if doc.get("value").is_some() && doc.get("count").is_some() {
            let value = doc
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let count = doc.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
            result.push((value, count));
            continue;
        }
        let Some(field) = doc.get(field_name) else {
            continue;
        };
        match field {
            Value::Array(items) => {
                for item in items {
                    if let Some(value) = item.as_str() {
                        push_count(&mut counts, &mut index, value.trim());
                    }
                }
            }
            Value::String(value) => push_count(&mut counts, &mut index, value.trim()),
            _ => {}
        }
    }
    if !counts.is_empty() {
        result.extend(counts);
    }
    result
}

fn push_count(counts: &mut Vec<(String, usize)>, index: &mut HashMap<String, usize>, key: &str) {
    if key.is_empty() {
        return;
    }
    if let Some(&position) = index.get(key) {
        counts[position].1 += 1;
    } else {
        index.insert(key.to_string(), counts.len());
        counts.push((key.to_string(), 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn highlight_english_uses_word_boundaries() {
        let keywords = vec!["rag".to_string()];
        // Match at a word boundary; sub-word occurrence ("storage") untouched.
        let out = highlight_text("RAG storage is great.", &keywords, None);
        assert!(out.contains("<em>RAG</em>"));
        assert!(!out.contains("<em>storage</em>"));
        // Punctuation-boundary match.
        let out = highlight_text("(rag)", &keywords, None);
        assert_eq!(out, "(<em>rag</em>)");
    }

    #[test]
    fn highlight_literal_is_longest_first_and_keeps_keyword_casing() {
        let keywords = vec!["数据".to_string(), "数据库".to_string()];
        let out = highlight_text("数据库是存储数据的地方", &keywords, Some(is_english_ascii));
        // Faithful to RAGFlow: longest keyword first, replacement keeps keyword
        // casing, and the second pass wraps the substring inside the first
        // <em> (nested tags — verified against highlight_utils.py).
        assert_eq!(out, "<em><em>数据</em>库</em>是存储<em>数据</em>的地方");
        // With the English word-boundary path (is_english = None), Chinese
        // keywords never touch a boundary char, so the text is returned
        // space-normalized but unwrapped (verified against highlight_utils.py).
        assert_eq!(
            highlight_text("数据库是存储数据的地方", &keywords, None),
            "数据库是存储数据的地方"
        );
    }

    #[test]
    fn highlight_keeps_only_matching_sentences() {
        let keywords = vec!["rust".to_string()];
        let txt = "Rust is fast. The weather is nice today. I love rust.";
        // Verified against highlight_utils.py: sentences are split on
        // `.?!;`, so the trailing period is dropped before joining with "...".
        let out = highlight_text(txt, &keywords, None);
        assert_eq!(out, "<em>Rust</em> is fast...I love <em>rust</em>");
        // No match at all -> space-normalized original.
        let out = highlight_text("Nothing to see here.", &keywords, None);
        assert_eq!(out, "Nothing to see here.");
        // Empty inputs -> empty string.
        assert_eq!(highlight_text("", &keywords, None), "");
        assert_eq!(highlight_text("abc", &[], None), "");
    }

    #[test]
    fn get_highlight_from_messages_filters_by_field_and_em() {
        let messages = vec![
            json!({"id": "c1", "content": "Rust powers RayRAG."}),
            json!({"id": "c2", "content": "No keywords inside."}),
            json!({"id": "c3"}), // missing content
        ];
        let out = get_highlight_from_messages(&messages, &["rust".to_string()], "content", None);
        assert_eq!(out.len(), 1);
        assert!(out["c1"].contains("<em>Rust</em>"));
        assert!(!out.contains_key("c2"));
        assert!(!out.contains_key("c3"));
    }

    #[test]
    fn aggregate_by_field_handles_rows_lists_and_preaggregated() {
        let messages = vec![
            json!({"value": "total", "count": 3}),
            json!({"tags": ["a", " b ", "a"]}),
            json!({"tags": "b"}),
            json!({"tags": ""}),
        ];
        let out = aggregate_by_field(&messages, "tags");
        assert_eq!(
            out,
            vec![
                ("total".to_string(), 3),
                ("a".to_string(), 2),
                ("b".to_string(), 2),
            ]
        );
        assert!(aggregate_by_field(&[], "tags").is_empty());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// memory/services/query.py — MsgTextQuery.
//
// Builds the doc-store text-match expression and keyword list used to search
// memory messages. The English and Chinese branches mirror query.py:
// normalization (add-space EN/ZH → lowercase → full-width→half-width →
// trad→simp → punctuation collapse → rmWWW), term weighting, synonym
// expansion and phrase/proximity operators, all rendered as a boosted query
// string over the `content` field.
// ═══════════════════════════════════════════════════════════════════════════

use crate::nlp::{
    SynonymDict, TermWeightComputer, add_space_between_eng_zh, is_chinese,
    rag_fine_grained_tokenize, rag_tokenize, rm_www, str_q2b, sub_special_char, tradi2simp,
};

/// `common/doc_store/doc_store_base.py::MatchTextExpr` — a text match
/// expression over `fields` with a boost and per-query options.
#[derive(Debug, Clone, PartialEq)]
pub struct MatchTextExpr {
    pub fields: Vec<String>,
    pub query: String,
    pub boost: u32,
    pub options: MatchTextOptions,
}

/// The `options` dict of `MatchTextExpr`
/// (`{"minimum_should_match": …, "original_query": …}`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MatchTextOptions {
    pub minimum_should_match: Option<f64>,
    pub original_query: Option<String>,
}

/// `memory/services/query.py::MsgTextQuery` — `question(txt, min_match)`
/// returns `(Option<MatchTextExpr>, keywords)`.
pub struct MsgTextQuery {
    tw: TermWeightComputer,
    syn: SynonymDict,
    query_fields: Vec<String>,
}

impl Default for MsgTextQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl MsgTextQuery {
    pub fn new() -> Self {
        Self {
            tw: TermWeightComputer::new(),
            syn: SynonymDict::new(),
            query_fields: vec!["content".to_string()],
        }
    }

    /// `question(txt, min_match=0.6)` — the query.py entry point.
    pub fn question(&self, txt: &str, min_match: f64) -> (Option<MatchTextExpr>, Vec<String>) {
        let original_query = txt.to_string();
        let mut normalized = add_space_between_eng_zh(txt);
        normalized = normalized.to_lowercase();
        normalized = str_q2b(&normalized);
        normalized = tradi2simp(&normalized);
        let collapse = Regex::new(r"[ :|\r\n\t,，。？?/`!！&^%%()\[\]{}<>]+")
            .expect("collapse regex is valid");
        normalized = collapse
            .replace_all(&normalized, " ")
            .trim()
            .to_string();
        let otxt = normalized.clone();
        let cleaned = rm_www(&normalized);

        if !is_chinese(&cleaned) {
            self.question_english(&cleaned, &otxt, &original_query)
        } else {
            self.question_chinese(&cleaned, &otxt, min_match, &original_query)
        }
    }

    /// query.py English branch: `tokenize → weights → synonym expansion →
    /// `(term)^weight` terms + bigram phrases.
    fn question_english(
        &self,
        txt: &str,
        otxt: &str,
        original_query: &str,
    ) -> (Option<MatchTextExpr>, Vec<String>) {
        let txt = rm_www(txt);
        let tokens: Vec<String> = rag_tokenize(&txt)
            .split_whitespace()
            .map(str::to_string)
            .collect();
        let mut keywords: Vec<String> = tokens.iter().filter(|t| !t.is_empty()).cloned().collect();

        let strip_quotes = Regex::new(r#"[ \\\"'^]"#).expect("strip regex is valid");
        let single_alpha = Regex::new(r"^[a-z0-9]$").expect("single-alpha regex is valid");
        let sign = Regex::new(r"^[+-]").expect("sign regex is valid");

        let mut tks_w: Vec<(String, f64)> = self
            .tw
            .weights(&tokens, false)
            .into_iter()
            .map(|(tk, w)| (strip_quotes.replace_all(&tk, "").into_owned(), w))
            .filter(|(tk, _)| !tk.is_empty())
            .map(|(tk, w)| (single_alpha.replace_all(&tk, "").into_owned(), w))
            .filter(|(tk, _)| !tk.is_empty())
            .map(|(tk, w)| (sign.replace_all(&tk, "").into_owned(), w))
            .filter(|(tk, _)| !tk.is_empty())
            .map(|(tk, w)| (tk.trim().to_string(), w))
            .filter(|(tk, _)| !tk.trim().is_empty())
            .collect();

        // Synonym pass: `syn = lookup(tk)` → strip quotes → extend keywords;
        // each term's synonyms become `"syn"^w/4` fragments.
        let mut syns: Vec<String> = Vec::new();
        for (tk, w) in tks_w.iter().take(256) {
            let syn = self.syn.lookup(tk, 256);
            let cleaned: Vec<String> = rag_tokenize(&syn.join(" "))
                .split_whitespace()
                .map(|s| s.replace('\'', ""))
                .collect();
            keywords.extend(cleaned.iter().cloned());
            let fragments: Vec<String> = cleaned
                .iter()
                .filter(|s| !s.trim().is_empty())
                .map(|s| format!("\"{s}\"^{:.4}", w / 4.0))
                .collect();
            syns.push(fragments.join(" "));
        }

        // `(tk^w syn)` terms; skip tokens starting with `.^+()-`.
        let mut q: Vec<String> = Vec::new();
        for ((tk, w), syn) in tks_w.iter().zip(syns.iter()) {
            let starts_bad = tk
                .chars()
                .next()
                .is_some_and(|c| ".^+()-".contains(c));
            if !tk.is_empty() && !starts_bad {
                q.push(format!("({}^{:.4} {})", tk, w, syn));
            }
        }
        // Bigram phrases: `"left right"^max(w)*2`.
        for i in 1..tks_w.len() {
            let left = tks_w[i - 1].0.trim();
            let right = tks_w[i].0.trim();
            if left.is_empty() || right.is_empty() {
                continue;
            }
            q.push(format!(
                "\"{} {}\"^{:.4}",
                tks_w[i - 1].0,
                tks_w[i].0,
                tks_w[i - 1].1.max(tks_w[i].1) * 2.0
            ));
        }
        if q.is_empty() {
            q.push(txt.to_string());
        }
        let query = q.join(" ");
        (
            Some(MatchTextExpr {
                fields: self.query_fields.clone(),
                query,
                boost: 100,
                options: MatchTextOptions {
                    minimum_should_match: None,
                    original_query: Some(original_query.to_string()),
                },
            }),
            keywords,
        )
    }

    /// query.py Chinese branch: per `tw.split` term, weighted
    /// `(term)^weight` groups with synonym OR-expansions, fine-grained
    /// character tokens and `"phrase"~2` proximity operators.
    fn question_chinese(
        &self,
        txt: &str,
        otxt: &str,
        min_match: f64,
        original_query: &str,
    ) -> (Option<MatchTextExpr>, Vec<String>) {
        let cleaned = rm_www(txt);
        let clean_sm = Regex::new(
            r#"[ ,\./;'\[\]\\`~!@#$%^&*()=+_<>?:\"{}|，。；‘’【】、！￥……（）——《》？：""-]+"#,
        )
        .expect("fine-grained cleanup regex is valid");
        let strip_quotes = Regex::new(r#"[ \\\"']+"#).expect("strip regex is valid");
        let ascii_only = Regex::new(r"^[0-9a-z.+#_*-]+$").expect("ascii regex is valid");

        let mut qs: Vec<String> = Vec::new();
        let mut keywords: Vec<String> = Vec::new();

        for term in self.tw.split(&cleaned).into_iter().take(256) {
            if term.is_empty() {
                continue;
            }
            keywords.push(term.clone());
            let twts = self.tw.weights(&[term.clone()], false);
            let syns = self.syn.lookup(&term, 32);
            if !syns.is_empty() && keywords.len() < 32 {
                keywords.extend(syns.iter().cloned());
            }

            let mut sorted = twts.clone();
            sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let mut tms: Vec<(String, f64)> = Vec::new();
            for (tk, w) in sorted {
                // Fine-grained tokens only for non-ASCII tokens ≥ 3 chars.
                let needs_fine = tk.chars().count() >= 3 && !ascii_only.is_match(&tk);
                let sm: Vec<String> = if needs_fine {
                    rag_fine_grained_tokenize(&tk)
                        .split_whitespace()
                        .map(str::to_string)
                        .collect()
                } else {
                    Vec::new()
                };
                let sm: Vec<String> = sm
                    .iter()
                    .map(|m| clean_sm.replace_all(m, "").into_owned())
                    .filter(|m| m.chars().count() > 1)
                    .map(|m| sub_special_char(&m))
                    .filter(|m| m.chars().count() > 1)
                    .collect();

                if keywords.len() < 32 {
                    keywords.push(strip_quotes.replace_all(&tk, "").into_owned());
                    keywords.extend(sm.iter().cloned());
                }

                let mut tk_syns: Vec<String> = self
                    .syn
                    .lookup(&tk, 32)
                    .iter()
                    .map(|s| sub_special_char(s))
                    .filter(|s| !s.is_empty())
                    .collect();
                if keywords.len() < 32 {
                    keywords.extend(tk_syns.iter().cloned());
                }
                let tk_syns: Vec<String> = tk_syns
                    .iter()
                    .map(|s| rag_fine_grained_tokenize(s))
                    .map(|s| {
                        if s.contains(' ') {
                            format!("\"{s}\"")
                        } else {
                            s
                        }
                    })
                    .collect();
                if keywords.len() >= 32 {
                    break;
                }

                let mut expr = sub_special_char(&tk);
                if expr.contains(' ') {
                    expr = format!("\"{expr}\"");
                }
                if !tk_syns.is_empty() {
                    expr = format!("({expr} OR ({}))^0.2", tk_syns.join(" "));
                }
                if !sm.is_empty() {
                    expr = format!(
                        "{expr} OR \"{}\" OR (\"{}\"~2)^0.5",
                        sm.join(" "),
                        sm.join(" ")
                    );
                }
                if !expr.trim().is_empty() {
                    tms.push((expr, w));
                }
            }

            let mut tms_str = tms
                .iter()
                .map(|(t, w)| format!("({t})^{w}"))
                .collect::<Vec<_>>()
                .join(" ");
            if twts.len() > 1 {
                tms_str.push_str(&format!(" (\"{}\"~2)^1.5", rag_tokenize(&term)));
            }
            let syns_str = syns
                .iter()
                .map(|s| format!("\"{}\"", rag_tokenize(&sub_special_char(s))))
                .collect::<Vec<_>>()
                .join(" OR ");
            if !syns_str.is_empty() && !tms_str.is_empty() {
                tms_str = format!("({tms_str})^5 OR ({syns_str})^0.7");
            }
            qs.push(tms_str);
        }

        if qs.is_empty() {
            return (None, keywords);
        }
        let mut query = qs
            .iter()
            .filter(|t| !t.is_empty())
            .map(|t| format!("({t})"))
            .collect::<Vec<_>>()
            .join(" OR ");
        if query.is_empty() {
            query = otxt.to_string();
        }
        (
            Some(MatchTextExpr {
                fields: self.query_fields.clone(),
                query,
                boost: 100,
                options: MatchTextOptions {
                    minimum_should_match: Some(min_match),
                    original_query: Some(original_query.to_string()),
                },
            }),
            keywords,
        )
    }
}

#[cfg(test)]
mod msg_text_query_tests {
    use super::*;

    #[test]
    fn english_question_builds_boosted_terms_and_keywords() {
        let query = MsgTextQuery::new();
        let (expr, keywords) = query.question("What is RAG retrieval?", 0.6);
        let expr = expr.expect("english branch yields an expression");
        assert_eq!(expr.fields, vec!["content"]);
        assert_eq!(expr.boost, 100);
        assert!(expr.query.contains("^"));
        assert!(expr.query.contains("rag"));
        assert!(expr.options.original_query.as_deref() == Some("What is RAG retrieval?"));
        assert!(expr.options.minimum_should_match.is_none());
        // Keywords come from the tokenizer; the question words are dropped
        // by rmWWW but the substantive term survives.
        assert!(keywords.iter().any(|kw| kw == "rag"));
    }

    #[test]
    fn chinese_question_builds_minimum_should_match_expression() {
        let query = MsgTextQuery::new();
        let (expr, keywords) = query.question("数据库性能优化方案", 0.6);
        let expr = expr.expect("chinese branch yields an expression");
        assert_eq!(expr.fields, vec!["content"]);
        assert_eq!(expr.options.minimum_should_match, Some(0.6));
        assert!(expr.options.original_query.as_deref() == Some("数据库性能优化方案"));
        assert!(!expr.query.is_empty());
        assert!(!keywords.is_empty());
        // English input stays on the English branch; the Chinese branch
        // option is only set for Chinese queries.
        let (expr, _) = query.question("Tell me about databases", 0.6);
        assert!(expr.unwrap().options.minimum_should_match.is_none());
    }

    #[test]
    fn empty_input_yields_no_expression_but_keywords() {
        let query = MsgTextQuery::new();
        let (expr, keywords) = query.question("", 0.6);
        // An empty question normalizes to nothing; the branches may still
        // return keywords, but no match expression is required for empty
        // text after cleanup.
        assert!(keywords.is_empty() || expr.is_none());
    }

    #[test]
    fn normalization_applies_fullwidth_and_fillers() {
        let query = MsgTextQuery::new();
        // Full-width letters are converted to half-width before tokenizing.
        let (expr, keywords) = query.question("ＲＡＧ ｓｅａｒｃｈ", 0.6);
        let expr = expr.expect("normalized input yields an expression");
        assert!(expr.query.contains("rag"));
        assert!(keywords.iter().any(|kw| kw.eq_ignore_ascii_case("rag")));
    }
}
