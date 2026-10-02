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

// ═══════════════════════════════════════════════════════════════════════════
// memory/utils/prompt_util.py + memory/utils/msg_util.py
// ═══════════════════════════════════════════════════════════════════════════

/// Fixed RAGFlow Memory extraction prompt builder.
///
/// Python builds the requested type set with a hash set, so the order of
/// multiple type blocks varies with `PYTHONHASHSEED`. Rust deliberately uses
/// the stable enum order semantic -> episodic -> procedural while preserving
/// every instruction, output field and example from the fixed source.
pub struct PromptAssembler;

impl PromptAssembler {
    pub const SYSTEM_BASE_TEMPLATE: &str = r#"**Memory Extraction Specialist**
You are an expert at analyzing conversations to extract structured memory.

{type_specific_instructions}


**OUTPUT REQUIREMENTS:**
1. Output MUST be valid JSON
2. Follow the specified output format exactly
3. Each extracted item MUST have: content, valid_at, invalid_at
4. Timestamps in {timestamp_format} format
5. Only extract memory types specified above
6. Maximum {max_items} items per type
"#;

    pub const BASE_USER_PROMPT: &str = r#"
**CONVERSATION:**
{conversation}

**CONVERSATION TIME:** {conversation_time}
**CURRENT TIME:** {current_time}
"#;

    /// `PromptAssembler.assemble_system_prompt` with the Python defaults:
    /// ISO-8601 timestamps and at most five items per requested non-RAW type.
    pub fn assemble_system_prompt(memory_types: &[String]) -> String {
        Self::assemble_system_prompt_with_options(memory_types, "ISO 8601", 5)
    }

    pub fn assemble_system_prompt_with_options(
        memory_types: &[String],
        timestamp_format: &str,
        max_items_per_type: usize,
    ) -> String {
        let types = extraction_types(memory_types);
        let type_specific_instructions = types
            .iter()
            .map(|memory_type| type_instruction(memory_type))
            .collect::<Vec<_>>()
            .join("\n");
        let output_format = types
            .iter()
            .map(|memory_type| output_template(memory_type))
            .collect::<Vec<_>>()
            .join(",\n");
        let mut prompt = Self::SYSTEM_BASE_TEMPLATE
            .replace("{type_specific_instructions}", &type_specific_instructions)
            .replace("{timestamp_format}", timestamp_format)
            .replace("{max_items}", &max_items_per_type.to_string());
        prompt.push_str("\n**REQUIRED OUTPUT FORMAT (JSON):**\n```json\n{\n");
        prompt.push_str(&output_format);
        prompt.push_str("\n}\n```\n");

        let examples = types
            .iter()
            .map(|memory_type| extraction_example(memory_type))
            .collect::<Vec<_>>()
            .join("\n");
        if !examples.is_empty() {
            prompt.push_str("\n**EXAMPLES:**\n");
            prompt.push_str(&examples);
            prompt.push('\n');
        }
        prompt
    }

    pub fn assemble_user_prompt(
        conversation: &str,
        conversation_time: Option<&str>,
        current_time: Option<&str>,
    ) -> String {
        let current_time = current_time
            .map(str::to_owned)
            .unwrap_or_else(|| crate::common::time_utils::current_timestamp().to_string());
        Self::BASE_USER_PROMPT
            .replace("{conversation}", conversation)
            .replace(
                "{conversation_time}",
                conversation_time
                    .filter(|value| !value.is_empty())
                    .unwrap_or("Not specified"),
            )
            .replace("{current_time}", &current_time)
    }

    pub const fn get_raw_user_prompt() -> &'static str {
        Self::BASE_USER_PROMPT
    }
}

/// `msg_util.get_json_result_from_llm_response`.
///
/// Only a lowercase leading ` ```json ` fence and a trailing ` ``` ` fence
/// are stripped. Invalid JSON becomes an empty object rather than surfacing an
/// extraction failure, exactly as in the fixed worker.
pub fn get_json_result_from_llm_response(response: &str) -> Value {
    let mut clean = response.trim();
    if let Some(stripped) = clean.strip_prefix("```json") {
        clean = stripped;
    }
    if let Some(stripped) = clean.strip_suffix("```") {
        clean = stripped;
    }
    serde_json::from_str(clean.trim()).unwrap_or_else(|_| Value::Object(Default::default()))
}

pub fn judge_system_prompt_is_default(system_prompt: &str, memory_types: &[String]) -> bool {
    system_prompt == PromptAssembler::assemble_system_prompt(memory_types)
}

/// `memory_utils.py::calculate_memory_type` — OR the bit value of every
/// valid lowercase `MemoryType` name (`RAW=0b0001`, `SEMANTIC=0b0010`,
/// `EPISODIC=0b0100`, `PROCEDURAL=0b1000`). Unknown names are silently
/// skipped, exactly like the Python type_value_map lookup.
pub fn calculate_memory_type(memory_type_name_list: &[String]) -> u8 {
    memory_type_name_list
        .iter()
        .map(|name| memory_type_bit(name))
        .fold(0u8, std::ops::BitOr::bitor)
}

/// Bit value of a single `MemoryType` name (trimmed + lowercased), 0 when
/// the name is not a valid memory type.
pub fn memory_type_bit(name: &str) -> u8 {
    match name.trim().to_ascii_lowercase().as_str() {
        "raw" => 0b0001,
        "semantic" => 0b0010,
        "episodic" => 0b0100,
        "procedural" => 0b1000,
        _ => 0,
    }
}

/// `memory_utils.py::get_memory_type_human` — enum-declaration-order lowercase
/// names of every set bit (`[mem_type.name.lower() for mem_type in MemoryType
/// if memory_type & mem_type.value]`).
pub fn get_memory_type_human(memory_type: u8) -> Vec<String> {
    ["raw", "semantic", "episodic", "procedural"]
        .into_iter()
        .filter(|name| memory_type & memory_type_bit(name) != 0)
        .map(str::to_owned)
        .collect()
}

fn extraction_types(memory_types: &[String]) -> Vec<&'static str> {
    ["semantic", "episodic", "procedural"]
        .into_iter()
        .filter(|candidate| memory_types.iter().any(|value| value == candidate))
        .collect()
}

fn type_instruction(memory_type: &str) -> &'static str {
    match memory_type {
        "semantic" => {
            r#"
        **EXTRACT SEMANTIC KNOWLEDGE:**
        - Universal facts, definitions, concepts, relationships
        - Time-invariant, generally true information
        - Examples: "The capital of France is Paris", "Water boils at 100°C"

        **Timestamp Rules for Semantic Knowledge:**
        - valid_at: When the fact became true (e.g., law enactment, discovery)
        - invalid_at: When it becomes false (e.g., repeal, disproven) or empty if still true
        - Default: valid_at = conversation time, invalid_at = "" for timeless facts
        "#
        }
        "episodic" => {
            r#"
        **EXTRACT EPISODIC KNOWLEDGE:**
        - Specific experiences, events, personal stories
        - Time-bound, person-specific, contextual
        - Examples: "Yesterday I fixed the bug", "User reported issue last week"

        **Timestamp Rules for Episodic Knowledge:**
        - valid_at: Event start/occurrence time
        - invalid_at: Event end time or empty if instantaneous
        - Extract explicit times: "at 3 PM", "last Monday", "from X to Y"
        "#
        }
        "procedural" => {
            r#"
        **EXTRACT PROCEDURAL KNOWLEDGE:**
        - Processes, methods, step-by-step instructions
        - Goal-oriented, actionable, often includes conditions
        - Examples: "To reset password, click...", "Debugging steps: 1)..."

        **Timestamp Rules for Procedural Knowledge:**
        - valid_at: When procedure becomes valid/effective
        - invalid_at: When it expires/becomes obsolete or empty if current
        - For version-specific: use release dates
        - For best practices: invalid_at = ""
        "#
        }
        _ => "",
    }
}

fn output_template(memory_type: &str) -> &'static str {
    match memory_type {
        "semantic" => {
            r#"
        "semantic": [
            {
                "content": "Clear factual statement",
                "valid_at": "timestamp or empty",
                "invalid_at": "timestamp or empty"
            }
        ]
        "#
        }
        "episodic" => {
            r#"
        "episodic": [
            {
                "content": "Narrative event description",
                "valid_at": "event start timestamp",
                "invalid_at": "event end timestamp or empty"
            }
        ]
        "#
        }
        "procedural" => {
            r#"
        "procedural": [
            {
                "content": "Actionable instructions",
                "valid_at": "procedure effective timestamp",
                "invalid_at": "procedure expiration timestamp or empty"
            }
        ]
        "#
        }
        _ => "",
    }
}

fn extraction_example(memory_type: &str) -> &'static str {
    match memory_type {
        "semantic" => {
            r#"
            **Semantic Example:**
            Input: "Python lists are mutable and support various operations."
            Output: {"semantic": [{"content": "Python lists are mutable data structures", "valid_at": "2024-01-15T10:00:00", "invalid_at": ""}]}
            "#
        }
        "episodic" => {
            r#"
            **Episodic Example:**
            Input: "I deployed the new feature yesterday afternoon."
            Output: {"episodic": [{"content": "User deployed new feature", "valid_at": "2024-01-14T14:00:00", "invalid_at": "2024-01-14T18:00:00"}]}
            "#
        }
        "procedural" => {
            r#"
            **Procedural Example:**
            Input: "To debug API errors: 1) Check logs 2) Verify endpoints 3) Test connectivity."
            Output: {"procedural": [{"content": "API error debugging: 1. Check logs 2. Verify endpoints 3. Test connectivity", "valid_at": "2024-01-15T10:00:00", "invalid_at": ""}]}
            "#
        }
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    fn sha256(value: &str) -> String {
        hex::encode(Sha256::digest(value.as_bytes()))
    }

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

    #[test]
    fn prompt_assembler_filters_raw_deduplicates_and_emits_each_requested_type() {
        let prompt = PromptAssembler::assemble_system_prompt(&[
            "raw".into(),
            "semantic".into(),
            "procedural".into(),
            "semantic".into(),
            "unknown".into(),
        ]);
        assert!(prompt.starts_with("**Memory Extraction Specialist**\n"));
        assert_eq!(prompt.matches("**EXTRACT SEMANTIC KNOWLEDGE:**").count(), 1);
        assert_eq!(
            prompt.matches("**EXTRACT PROCEDURAL KNOWLEDGE:**").count(),
            1
        );
        assert!(!prompt.contains("EXTRACT RAW"));
        assert!(!prompt.contains("EXTRACT EPISODIC"));
        assert!(prompt.contains("\"semantic\": ["));
        assert!(prompt.contains("\"procedural\": ["));
        assert!(prompt.contains("Maximum 5 items per type"));
        assert!(prompt.contains("**Procedural Example:**"));
    }

    #[test]
    fn prompt_assembler_matches_raw_only_and_custom_option_contracts() {
        let raw = PromptAssembler::assemble_system_prompt(&["raw".into()]);
        assert_eq!(raw.chars().count(), 435);
        assert_eq!(
            sha256(&raw),
            "0353e9f4f62134073050a0a1cd362100c935c9a65bed6e34c605dfa658ed8f70"
        );
        assert!(raw.contains("\n\n\n\n**OUTPUT REQUIREMENTS:**"));
        assert!(raw.contains("```json\n{\n\n}\n```\n"));
        assert!(!raw.contains("**EXAMPLES:**"));

        let configured = PromptAssembler::assemble_system_prompt_with_options(
            &["episodic".into()],
            "RFC 3339",
            2,
        );
        assert!(configured.contains("Timestamps in RFC 3339 format"));
        assert!(configured.contains("Maximum 2 items per type"));
        assert!(configured.contains("**Episodic Example:**"));

        for (memory_type, expected_len, expected_sha256) in [
            (
                "semantic",
                1497,
                "d941d0ad4fe092d493e3978ac2c38c37fd5a77eb375c71d1527b91100bfe6d59",
            ),
            (
                "episodic",
                1432,
                "d518bb2605636b6d3b1dbdc8a12fe1f5f1eb8fc62e7c807fdf228edb83780ecb",
            ),
            (
                "procedural",
                1569,
                "8adfd8ba68e430144a92ee58be1bc826ec4ce7df5dcb907b41e6329d56363eaa",
            ),
        ] {
            let prompt = PromptAssembler::assemble_system_prompt(&[memory_type.into()]);
            assert_eq!(
                prompt.chars().count(),
                expected_len,
                "{memory_type} prompt drifted"
            );
            assert_eq!(sha256(&prompt), expected_sha256);
            assert!(prompt.ends_with("\n            \n"));
        }
    }

    #[test]
    fn prompt_assembler_user_prompt_and_raw_template_match_fixed_shape() {
        let prompt = PromptAssembler::assemble_user_prompt(
            "User Input: hi\nAgent Response: hello",
            Some("2026-08-12 10:00:00"),
            Some("2026-08-12 10:00:01"),
        );
        assert_eq!(
            prompt,
            "\n**CONVERSATION:**\nUser Input: hi\nAgent Response: hello\n\n**CONVERSATION TIME:** 2026-08-12 10:00:00\n**CURRENT TIME:** 2026-08-12 10:00:01\n"
        );
        assert_eq!(
            sha256(&PromptAssembler::assemble_user_prompt(
                "x",
                Some("conversation"),
                Some("current")
            )),
            "2b71b613730f9096ebe4990468e857a03144e06cc6d8c4c3452e0da93664a6bd"
        );
        assert_eq!(
            PromptAssembler::get_raw_user_prompt(),
            PromptAssembler::BASE_USER_PROMPT
        );
        assert!(
            PromptAssembler::assemble_user_prompt("x", Some(""), Some("now"))
                .contains("**CONVERSATION TIME:** Not specified")
        );
    }

    #[test]
    fn memory_llm_json_parser_only_strips_fixed_lowercase_fence() {
        let expected = json!({
            "semantic": [{
                "content": "Rust is memory safe",
                "valid_at": "2026-08-12T10:00:00",
                "invalid_at": ""
            }]
        });
        assert_eq!(
            get_json_result_from_llm_response(&expected.to_string()),
            expected
        );
        assert_eq!(
            get_json_result_from_llm_response(&format!("```json\n{expected}\n```")),
            expected
        );
        assert_eq!(get_json_result_from_llm_response("not json"), json!({}));
        assert_eq!(
            get_json_result_from_llm_response(&format!("```JSON\n{expected}\n```")),
            json!({})
        );
        assert_eq!(
            get_json_result_from_llm_response(&format!("prefix {expected} suffix")),
            json!({})
        );
        assert_eq!(get_json_result_from_llm_response("[1, 2]"), json!([1, 2]));
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
        normalized = collapse.replace_all(&normalized, " ").trim().to_string();
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
        _otxt: &str,
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

        let tks_w: Vec<(String, f64)> = self
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
            let starts_bad = tk.chars().next().is_some_and(|c| ".^+()-".contains(c));
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
            let twts = self.tw.weights(std::slice::from_ref(&term), false);
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

                let tk_syns: Vec<String> = self
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

#[cfg(test)]
mod memory_utils_tests {
    use super::*;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn calculate_memory_type_ors_valid_names_and_skips_unknowns() {
        assert_eq!(calculate_memory_type(&names(&["raw", "semantic"])), 0b0011);
        assert_eq!(calculate_memory_type(&names(&["procedural"])), 0b1000);
        assert_eq!(calculate_memory_type(&names(&["raw"])), 0b0001);
        // Unknown names contribute 0, exactly like the Python lookup.
        assert_eq!(
            calculate_memory_type(&names(&["episodic", "nope", ""])),
            0b0100
        );
        assert_eq!(calculate_memory_type(&names(&[])), 0);
        // Trimming + case-folding mirror `name.lower()`.
        assert_eq!(calculate_memory_type(&names(&["  SEMANTIC "])), 0b0010);
    }

    #[test]
    fn get_memory_type_human_emits_enum_order_lowercase_names() {
        assert_eq!(
            get_memory_type_human(0b0101),
            vec!["raw".to_string(), "episodic".to_string()]
        );
        assert_eq!(
            get_memory_type_human(0b1111),
            vec![
                "raw".to_string(),
                "semantic".to_string(),
                "episodic".to_string(),
                "procedural".to_string(),
            ]
        );
        assert!(get_memory_type_human(0).is_empty());
        // Unknown bits are not named.
        assert_eq!(get_memory_type_human(0b1000_0000), Vec::<String>::new());
    }
}
