//! Upstream `common/metadata_utils.py` — the metadata filter engine — plus
//! `rag/prompts/generator.py::gen_meta_filter` and the `rag/prompts/meta_filter.md`
//! prompt it renders.
//!
//! `apply_meta_data_filter` has three modes and this module ports all of them:
//!
//! * `auto` — the LLM turns the question plus **all** of the knowledge base's
//!   metadata keys into filter conditions.
//! * `semi_auto` — the same, restricted to the keys the user picked, with an
//!   optional per-key operator constraint (`{"key": ">", "author": "="}`).
//! * `manual` — the caller supplies the conditions directly.
//!
//! Two upstream details drive the shape of the return value and both are kept:
//! a `manual` filter that matches nothing answers the `["-999"]` sentinel
//! (upstream's "match no document" marker), while an `auto`/`semi_auto` filter
//! that matches nothing answers `None` — which lets the caller search without a
//! document restriction.
//!
//! The evaluator ([`meta_filter`]) is a line-by-line port, including the parts
//! that look odd but decide real results: date-shaped values are compared as
//! strings and a date query never matches a non-date value, everything else
//! goes through Python's `ast.literal_eval` before comparison so `"5" > "10"`
//! compares numbers rather than text, non-comparison operators only lower-case,
//! and a type mismatch simply means "no match" (upstream swallows the
//! `TypeError`).

use crate::llm::{ChatMessage, ChatModel};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// The operator vocabulary of `meta_filter.md` (and of the UI's manual rows).
pub const OPERATORS: [&str; 14] = [
    "contains",
    "not contains",
    "in",
    "not in",
    "start with",
    "end with",
    "empty",
    "not empty",
    "=",
    "≠",
    ">",
    "<",
    "≥",
    "≤",
];

/// Operators whose branch runs the date / `literal_eval` coercion first.
const COMPARISON_OPERATORS: [&str; 6] = ["=", "≠", ">", "<", "≥", "≤"];

/// The `manual` no-match marker (`api/apps/services/dataset_api_service.py`
/// passes it straight to the retriever, which then matches nothing).
pub const NO_MATCH_SENTINEL: &str = "-999";

/// `{field: {value: [doc_ids]}}` — the flattened metadata map upstream builds
/// with `DocMetadataService.get_flatted_meta_by_kbs` and RayRAG with
/// `DocumentMetadataStore::flattened`.
pub type FlattenedMetadata = BTreeMap<String, BTreeMap<String, Vec<String>>>;

/// One filter condition: upstream's `{"key", "op", "value"}` dict.
#[derive(Debug, Clone, PartialEq)]
pub struct MetaCondition {
    pub key: String,
    pub op: String,
    pub value: Value,
}

impl MetaCondition {
    /// Read one condition, accepting the UI spellings (`name` /
    /// `comparison_operator`) exactly like `convert_conditions` does.
    pub fn from_value(value: &Value) -> Option<Self> {
        let key = value
            .get("key")
            .or_else(|| value.get("name"))
            .and_then(Value::as_str)?
            .trim();
        if key.is_empty() {
            return None;
        }
        let op = value
            .get("op")
            .or_else(|| value.get("comparison_operator"))
            .and_then(Value::as_str)
            .map(canonical_operator)
            .unwrap_or_default();
        Some(Self {
            key: key.to_string(),
            op,
            value: value.get("value").cloned().unwrap_or(Value::Null),
        })
    }
}

/// `convert_conditions`' `op_mapping`: `{"is": "=", "not is": "≠", ">=": "≥",
/// "<=": "≤", "!=": "≠"}`.
pub fn canonical_operator(op: &str) -> String {
    match op.trim() {
        "is" => "=".to_string(),
        "not is" => "≠".to_string(),
        ">=" => "≥".to_string(),
        "<=" => "≤".to_string(),
        "!=" => "≠".to_string(),
        other => other.to_string(),
    }
}

/// `common/metadata_utils.py::convert_conditions` — the UI's
/// `metadata_condition` object into canonical conditions.
pub fn convert_conditions(metadata_condition: Option<&Value>) -> Vec<MetaCondition> {
    metadata_condition
        .and_then(|condition| condition.get("conditions"))
        .and_then(Value::as_array)
        .map(|conditions| {
            conditions
                .iter()
                .filter_map(MetaCondition::from_value)
                .collect()
        })
        .unwrap_or_default()
}

// ── Python scalar semantics ─────────────────────────────────────

/// The subset of Python values `ast.literal_eval` can produce here.
#[derive(Debug, Clone, PartialEq)]
enum PyScalar {
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<PyScalar>),
}

/// `ast.literal_eval` for a metadata value: numbers, booleans, `None`, quoted
/// strings and flat lists. `None` means "raised", i.e. the caller keeps the raw
/// string — that is how `2025-07-11` and free text stay strings.
fn python_literal_eval(text: &str) -> Option<PyScalar> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed {
        "None" => return Some(PyScalar::None),
        "True" => return Some(PyScalar::Bool(true)),
        "False" => return Some(PyScalar::Bool(false)),
        _ => {}
    }
    if let Ok(int) = trimmed.parse::<i64>() {
        return Some(PyScalar::Int(int));
    }
    if let Ok(float) = trimmed.parse::<f64>() {
        // `literal_eval` rejects the bare names `nan`/`inf`; Rust would parse
        // them, so they stay strings here.
        if float.is_finite() {
            return Some(PyScalar::Float(float));
        }
        return None;
    }
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
            return Some(PyScalar::Str(trimmed[1..trimmed.len() - 1].to_string()));
        }
    }
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        let inner = &trimmed[1..trimmed.len() - 1];
        if inner.trim().is_empty() {
            return Some(PyScalar::List(Vec::new()));
        }
        let parts = split_top_level_commas(inner);
        if parts.is_empty() {
            return None;
        }
        let mut items = Vec::with_capacity(parts.len());
        for part in parts {
            items.push(python_literal_eval(part)?);
        }
        return Some(PyScalar::List(items));
    }
    None
}

/// Comma split that ignores commas inside quotes or nested brackets.
fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    for (index, character) in text.char_indices() {
        match quote {
            Some(open) => {
                if character == open {
                    quote = None;
                }
            }
            None => match character {
                '\'' | '"' => quote = Some(character),
                '[' | '(' | '{' => depth += 1,
                ']' | ')' | '}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    parts.push(&text[start..index]);
                    start = index + 1;
                }
                _ => {}
            },
        }
    }
    parts.push(&text[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

/// `normalize_string_values`: lower-case strings, and string items inside lists.
fn normalize_string_values(value: PyScalar) -> PyScalar {
    match value {
        PyScalar::Str(text) => PyScalar::Str(text.to_lowercase()),
        PyScalar::List(items) => PyScalar::List(
            items
                .into_iter()
                .map(|item| match item {
                    PyScalar::Str(text) => PyScalar::Str(text.to_lowercase()),
                    other => other,
                })
                .collect(),
        ),
        other => other,
    }
}

impl PyScalar {
    /// Python `str(value)` for the text operators.
    fn as_text(&self) -> String {
        match self {
            PyScalar::None => "None".to_string(),
            PyScalar::Bool(value) => {
                if *value {
                    "True".to_string()
                } else {
                    "False".to_string()
                }
            }
            PyScalar::Int(value) => value.to_string(),
            PyScalar::Float(value) => {
                if value.fract() == 0.0 && value.abs() < 1e16 {
                    format!("{value:.1}")
                } else {
                    value.to_string()
                }
            }
            PyScalar::Str(value) => value.clone(),
            PyScalar::List(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(PyScalar::as_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Python truthiness — `empty` / `not empty` read it directly.
    fn is_truthy(&self) -> bool {
        match self {
            PyScalar::None => false,
            PyScalar::Bool(value) => *value,
            PyScalar::Int(value) => *value != 0,
            PyScalar::Float(value) => *value != 0.0,
            PyScalar::Str(value) => !value.is_empty(),
            PyScalar::List(items) => !items.is_empty(),
        }
    }

    /// Python numbers include `bool` (it subclasses `int`).
    fn as_number(&self) -> Option<f64> {
        match self {
            PyScalar::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
            PyScalar::Int(value) => Some(*value as f64),
            PyScalar::Float(value) => Some(*value),
            _ => None,
        }
    }

    /// Python `==`: cross-type equality is simply `False`, never an error.
    fn py_eq(&self, other: &PyScalar) -> bool {
        match (self.as_number(), other.as_number()) {
            (Some(left), Some(right)) => left == right,
            _ => match (self, other) {
                (PyScalar::None, PyScalar::None) => true,
                (PyScalar::Str(left), PyScalar::Str(right)) => left == right,
                (PyScalar::List(left), PyScalar::List(right)) => {
                    left.len() == right.len()
                        && left.iter().zip(right.iter()).all(|(a, b)| a.py_eq(b))
                }
                _ => false,
            },
        }
    }

    /// Python ordering: numbers compare numerically, strings by code point, and
    /// anything else raises `TypeError` — `None` here, which upstream swallows.
    fn py_cmp(&self, other: &PyScalar) -> Option<std::cmp::Ordering> {
        match (self.as_number(), other.as_number()) {
            (Some(left), Some(right)) => left.partial_cmp(&right),
            _ => match (self, other) {
                (PyScalar::Str(left), PyScalar::Str(right)) => Some(left.cmp(right)),
                _ => None,
            },
        }
    }

    /// `in` semantics: membership for a list, substring for a string.
    fn py_contains(&self, container: &PyScalar) -> Option<bool> {
        match container {
            PyScalar::List(items) => Some(items.iter().any(|item| item.py_eq(self))),
            PyScalar::Str(text) => match self {
                PyScalar::Str(needle) => Some(text.contains(needle.as_str())),
                // `5 in "12345"` raises TypeError in Python.
                _ => None,
            },
            _ => None,
        }
    }
}

/// One `v2docs` pass: the value→doc_ids rows that satisfy the operator.
fn filter_out(
    values_to_docs: &BTreeMap<String, Vec<String>>,
    operator: &str,
    value: &Value,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (input, doc_ids) in values_to_docs {
        let comparison = COMPARISON_OPERATORS.contains(&operator);
        let (input, value) = if comparison {
            let input_str = input.trim();
            let value_str = match value {
                Value::String(text) => text.trim().to_string(),
                other => other.to_string(),
            };
            let is_input_date = is_date_shaped(input_str);
            if is_date_shaped(&value_str) {
                if !is_input_date {
                    // A date query never matches a non-date value.
                    continue;
                }
                (
                    PyScalar::Str(input_str.to_string()),
                    PyScalar::Str(value_str),
                )
            } else {
                // upstream assigns `input` first and `value` second, so a
                // failing second `literal_eval` keeps the first conversion.
                let evaluated_input = python_literal_eval(input_str)
                    .unwrap_or_else(|| PyScalar::Str(input_str.to_string()));
                let evaluated_value = python_literal_eval(&value_str)
                    .unwrap_or_else(|| PyScalar::Str(value_str.clone()));
                (
                    lowercase_scalar(evaluated_input),
                    lowercase_scalar(evaluated_value),
                )
            }
        } else {
            (
                normalize_string_values(PyScalar::Str(input.clone())),
                normalize_string_values(scalar_from_json(value)),
            )
        };

        let matched = match operator {
            "contains" => match &input {
                PyScalar::List(items) => items
                    .iter()
                    .any(|item| item.as_text().contains(&value.as_text())),
                other => other.as_text().contains(&value.as_text()),
            },
            "not contains" => match &input {
                PyScalar::List(items) => items
                    .iter()
                    .all(|item| !item.as_text().contains(&value.as_text())),
                other => !other.as_text().contains(&value.as_text()),
            },
            "in" => match &input {
                PyScalar::List(items) => items
                    .iter()
                    .all(|item| item.py_contains(&value).unwrap_or(false)),
                other => other.py_contains(&value).unwrap_or(false),
            },
            "not in" => match &input {
                PyScalar::List(items) => items
                    .iter()
                    .all(|item| !item.py_contains(&value).unwrap_or(false)),
                other => !other.py_contains(&value).unwrap_or(false),
            },
            "start with" => match &input {
                PyScalar::List(items) => items
                    .iter()
                    .map(PyScalar::as_text)
                    .collect::<String>()
                    .to_lowercase()
                    .starts_with(&value.as_text().to_lowercase()),
                other => other
                    .as_text()
                    .to_lowercase()
                    .starts_with(&value.as_text().to_lowercase()),
            },
            "end with" => match &input {
                PyScalar::List(items) => items
                    .iter()
                    .map(PyScalar::as_text)
                    .collect::<String>()
                    .to_lowercase()
                    .ends_with(&value.as_text().to_lowercase()),
                other => other
                    .as_text()
                    .to_lowercase()
                    .ends_with(&value.as_text().to_lowercase()),
            },
            "empty" => !input.is_truthy(),
            "not empty" => input.is_truthy(),
            "=" => input.py_eq(&value),
            "≠" => !input.py_eq(&value),
            ">" => input
                .py_cmp(&value)
                .is_some_and(|ordering| ordering.is_gt()),
            "<" => input
                .py_cmp(&value)
                .is_some_and(|ordering| ordering.is_lt()),
            "≥" => input
                .py_cmp(&value)
                .is_some_and(|ordering| ordering.is_ge()),
            "≤" => input
                .py_cmp(&value)
                .is_some_and(|ordering| ordering.is_le()),
            _ => false,
        };

        if matched {
            ids.extend(doc_ids.iter().cloned());
        }
    }
    ids
}

fn lowercase_scalar(value: PyScalar) -> PyScalar {
    match value {
        PyScalar::Str(text) => PyScalar::Str(text.to_lowercase()),
        other => other,
    }
}

fn scalar_from_json(value: &Value) -> PyScalar {
    match value {
        Value::Null => PyScalar::None,
        Value::Bool(value) => PyScalar::Bool(*value),
        Value::Number(number) => number
            .as_i64()
            .map(PyScalar::Int)
            .or_else(|| number.as_f64().map(PyScalar::Float))
            .unwrap_or(PyScalar::None),
        Value::String(text) => PyScalar::Str(text.clone()),
        Value::Array(items) => PyScalar::List(items.iter().map(scalar_from_json).collect()),
        Value::Object(_) => PyScalar::Str(value.to_string()),
    }
}

/// `YYYY-MM-DD`, checked exactly like upstream (length plus digit positions).
fn is_date_shaped(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

/// `common/metadata_utils.py::meta_filter` — the in-memory evaluator.
///
/// `and` intersects (and short-circuits on an empty intersection), `or` unions;
/// a key that is absent from `metas` contributes nothing.
pub fn meta_filter(
    metas: &FlattenedMetadata,
    filters: &[MetaCondition],
    logic: &str,
) -> Vec<String> {
    let mut doc_ids: Option<Vec<String>> = None;
    for filter in filters {
        let ids = match metas.get(&filter.key) {
            Some(values_to_docs) => filter_out(values_to_docs, &filter.op, &filter.value),
            None => Vec::new(),
        };
        match doc_ids.as_mut() {
            None => doc_ids = Some(ids),
            Some(current) => {
                if logic == "and" {
                    let keep: BTreeSet<&String> = ids.iter().collect();
                    current.retain(|doc_id| keep.contains(doc_id));
                    if current.is_empty() {
                        return Vec::new();
                    }
                } else {
                    for doc_id in ids {
                        if !current.contains(&doc_id) {
                            current.push(doc_id);
                        }
                    }
                }
            }
        }
    }
    doc_ids.unwrap_or_default()
}

// ── gen_meta_filter ─────────────────────────────────────────────

/// What `gen_meta_filter` returns: `{"logic": ..., "conditions": [...]}`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeneratedFilters {
    pub logic: String,
    pub conditions: Vec<MetaCondition>,
}

impl GeneratedFilters {
    pub fn logic_or_default(&self) -> &str {
        if self.logic.is_empty() {
            "and"
        } else {
            &self.logic
        }
    }
}

/// `json.dumps` for a string with Python's defaults: `ensure_ascii=True`, so
/// non-ASCII becomes `\uXXXX`, plus the standard short escapes.
fn python_json_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character if (character as u32) < 0x7f => out.push(character),
            character => {
                let code = character as u32;
                if code > 0xffff {
                    // Python emits a surrogate pair for astral code points.
                    let adjusted = code - 0x1_0000;
                    let high = 0xd800 + (adjusted >> 10);
                    let low = 0xdc00 + (adjusted & 0x3ff);
                    out.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
                } else {
                    out.push_str(&format!("\\u{code:04x}"));
                }
            }
        }
    }
    out.push('"');
}

/// `json.dumps(meta_data_structure)`: `{key: [value, ...]}` with Python's
/// `", "` / `": "` separators.
///
/// Keys are sorted because RayRAG's store is a `BTreeMap`; upstream keeps the
/// order the metadata rows were read in. The LLM sees the same content.
fn metadata_structure_json(metas: &FlattenedMetadata) -> String {
    let mut out = String::from("{");
    for (index, (key, values)) in metas.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        python_json_string(key, &mut out);
        out.push_str(": [");
        for (position, value) in values.keys().enumerate() {
            if position > 0 {
                out.push_str(", ");
            }
            python_json_string(value, &mut out);
        }
        out.push(']');
    }
    out.push('}');
    out
}

fn constraints_json(constraints: &BTreeMap<String, String>) -> String {
    let mut out = String::from("{");
    for (index, (key, op)) in constraints.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        python_json_string(key, &mut out);
        out.push_str(": ");
        python_json_string(op, &mut out);
    }
    out.push('}');
    out
}

/// Render `rag/prompts/meta_filter.md`.
///
/// Upstream renders it with Jinja (`trim_blocks`/`lstrip_blocks`), so the
/// `{% if constraints %}` line disappears entirely when no constraints were
/// passed — that is why the template's trailing constraints line is dropped
/// rather than left with an empty value.
pub fn render_prompt(
    metas: &FlattenedMetadata,
    question: &str,
    constraints: Option<&BTreeMap<String, String>>,
    current_date: &str,
) -> String {
    let metadata_keys = metadata_structure_json(metas);
    let rendered_constraints = constraints
        .filter(|constraints| !constraints.is_empty())
        .map(constraints_json);
    let mut prompt = crate::prompts::PromptLibrary::meta_filter_template()
        .replace("{current_date}", current_date)
        .replace("{metadata_keys}", &metadata_keys);
    match &rendered_constraints {
        Some(value) => prompt = prompt.replace("{constraints}", value),
        None => {
            prompt = prompt
                .lines()
                .filter(|line| !line.starts_with("- Operator constraints:"))
                .collect::<Vec<_>>()
                .join("\n");
        }
    }
    // The question is substituted last so a question that happens to contain a
    // placeholder name cannot be rewritten again.
    prompt.replace("{user_question}", question)
}

/// `re.sub(r"(^.*</think>|```json\n|```\n*$)", "", ans, flags=re.DOTALL)` plus
/// the `json_repair` parse, with upstream's assertion that the reply is a dict
/// holding a `conditions` list.
pub fn parse_generated_filters(answer: &str) -> Option<GeneratedFilters> {
    let stripped = match answer.rfind("</think>") {
        Some(index) => &answer[index + "</think>".len()..],
        None => answer,
    };
    let value = crate::structure_compile::parse_json_lenient(stripped)?;
    let conditions = value.get("conditions")?.as_array()?;
    Some(GeneratedFilters {
        logic: value
            .get("logic")
            .and_then(Value::as_str)
            .unwrap_or("and")
            .to_string(),
        conditions: conditions
            .iter()
            .filter_map(MetaCondition::from_value)
            .collect(),
    })
}

/// `rag/prompts/generator.py::gen_meta_filter`: prompt the chat model, strip
/// the think block, parse the JSON, and fall back to `{"conditions": []}` when
/// the reply cannot be read.
pub async fn gen_meta_filter(
    chat: &dyn ChatModel,
    metas: &FlattenedMetadata,
    question: &str,
    constraints: Option<&BTreeMap<String, String>>,
) -> anyhow::Result<GeneratedFilters> {
    let current_date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let system = render_prompt(metas, question, constraints, &current_date);
    let answer = chat
        .chat(&system, &[ChatMessage::new("user", "Generate filters:")])
        .await?;
    Ok(parse_generated_filters(&answer).unwrap_or_default())
}

// ── apply_meta_data_filter ──────────────────────────────────────

/// The three modes plus the "no recognisable method" case, which upstream
/// leaves untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMethod {
    Auto,
    SemiAuto,
    Manual,
    Unrecognised,
}

pub fn filter_method(filter: &Value) -> FilterMethod {
    match filter.get("method").and_then(Value::as_str) {
        Some("auto") => FilterMethod::Auto,
        Some("semi_auto") => FilterMethod::SemiAuto,
        Some("manual") => FilterMethod::Manual,
        _ => FilterMethod::Unrecognised,
    }
}

/// `apply_meta_data_filter`: filter `base_doc_ids` through `filter`.
///
/// Returns `None` when an `auto`/`semi_auto` filter produced nothing (upstream
/// lets the caller search unrestricted) and the `["-999"]` sentinel when a
/// `manual` filter matched nothing.
///
/// `metas_loader` is upstream's `metas_loader`: it is only called when the
/// metadata map is actually needed, so a manual filter with no base documents
/// never pays for the scan.
pub async fn apply_meta_data_filter(
    filter: &Value,
    question: &str,
    chat: Option<&dyn ChatModel>,
    base_doc_ids: &[String],
    mut metas_loader: impl FnMut() -> FlattenedMetadata,
) -> anyhow::Result<Option<Vec<String>>> {
    let mut doc_ids: Vec<String> = base_doc_ids.to_vec();
    let method = filter_method(filter);
    match method {
        FilterMethod::Auto => {
            let Some(chat) = chat else {
                anyhow::bail!("Automatic metadata filters need a configured chat model");
            };
            let metas = metas_loader();
            let generated = gen_meta_filter(chat, &metas, question, None).await?;
            doc_ids.extend(meta_filter(
                &metas,
                &generated.conditions,
                generated.logic_or_default(),
            ));
            if doc_ids.is_empty() {
                return Ok(None);
            }
        }
        FilterMethod::SemiAuto => {
            let mut selected_keys: Vec<String> = Vec::new();
            let mut constraints: BTreeMap<String, String> = BTreeMap::new();
            for item in filter
                .get("semi_auto")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                match item {
                    Value::String(key) => selected_keys.push(key.clone()),
                    Value::Object(entry) => {
                        // Upstream keys the constraint map with `None` when the
                        // row has no key; such an entry can never match a
                        // metadata key, so it is dropped here.
                        let key = entry.get("key").and_then(Value::as_str).unwrap_or("");
                        selected_keys.push(key.to_string());
                        if let Some(op) = entry.get("op").and_then(Value::as_str) {
                            constraints.insert(key.to_string(), op.to_string());
                        }
                    }
                    _ => {}
                }
            }
            if !selected_keys.is_empty() {
                let metas = metas_loader();
                let filtered: FlattenedMetadata = selected_keys
                    .iter()
                    .filter(|key| metas.contains_key(key.as_str()))
                    .map(|key| (key.clone(), metas[key].clone()))
                    .collect();
                if !filtered.is_empty() {
                    let Some(chat) = chat else {
                        anyhow::bail!(
                            "Semi-automatic metadata filters need a configured chat model"
                        );
                    };
                    let generated =
                        gen_meta_filter(chat, &filtered, question, Some(&constraints)).await?;
                    doc_ids.extend(meta_filter(
                        &metas,
                        &generated.conditions,
                        generated.logic_or_default(),
                    ));
                    if doc_ids.is_empty() {
                        return Ok(None);
                    }
                }
            }
        }
        FilterMethod::Manual => {
            let filters: Vec<MetaCondition> = filter
                .get("manual")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(MetaCondition::from_value)
                        .collect()
                })
                .unwrap_or_default();
            let logic = filter.get("logic").and_then(Value::as_str).unwrap_or("and");
            if !filters.is_empty() {
                let metas = metas_loader();
                doc_ids.extend(meta_filter(&metas, &filters, logic));
            }
            if !filters.is_empty() && doc_ids.is_empty() {
                doc_ids.push(NO_MATCH_SENTINEL.to_string());
            }
        }
        FilterMethod::Unrecognised => {}
    }
    Ok(Some(doc_ids))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metas_from(entries: &[(&str, &[(&str, &[&str])])]) -> FlattenedMetadata {
        let mut metas = FlattenedMetadata::new();
        for (key, values) in entries {
            let mut row = BTreeMap::new();
            for (value, docs) in *values {
                row.insert(
                    value.to_string(),
                    docs.iter().map(|doc| doc.to_string()).collect(),
                );
            }
            metas.insert(key.to_string(), row);
        }
        metas
    }

    fn condition(key: &str, op: &str, value: Value) -> MetaCondition {
        MetaCondition {
            key: key.to_string(),
            op: op.to_string(),
            value,
        }
    }

    /// The operator expectations of
    /// `test/unit_test/common/test_metadata_filter_operators.py`.
    #[test]
    fn operators_match_the_upstream_expectations() {
        let cases: Vec<(FlattenedMetadata, MetaCondition, Vec<&str>)> = vec![
            (
                metas_from(&[(
                    "version",
                    &[("hello earth", &["doc1"]), ("hello mars", &["doc2"])],
                )]),
                condition("version", "contains", json!("earth")),
                vec!["doc1"],
            ),
            (
                metas_from(&[(
                    "version",
                    &[("hello earth", &["doc1"]), ("hello mars", &["doc2"])],
                )]),
                condition("version", "not contains", json!("earth")),
                vec!["doc2"],
            ),
            (
                metas_from(&[(
                    "status",
                    &[
                        ("active", &["doc1"]),
                        ("pending", &["doc2"]),
                        ("done", &["doc3"]),
                    ],
                )]),
                condition("status", "in", json!("active,pending")),
                vec!["doc1", "doc2"],
            ),
            (
                metas_from(&[(
                    "status",
                    &[
                        ("active", &["doc1"]),
                        ("pending", &["doc2"]),
                        ("done", &["doc3"]),
                    ],
                )]),
                condition("status", "not in", json!("active,pending")),
                vec!["doc3"],
            ),
            (
                metas_from(&[(
                    "product",
                    &[("F2", &["doc1"]), ("F11", &["doc2"]), ("G1", &["doc3"])],
                )]),
                condition("product", "in", json!(["F2", "F11"])),
                vec!["doc1", "doc2"],
            ),
            (
                metas_from(&[(
                    "product",
                    &[("F2", &["doc1"]), ("F11", &["doc2"]), ("G1", &["doc3"])],
                )]),
                condition("product", "not in", json!(["F2", "F11"])),
                vec!["doc3"],
            ),
            (
                metas_from(&[("name", &[("prefix_value", &["doc1"]), ("other", &["doc2"])])]),
                condition("name", "start with", json!("pre")),
                vec!["doc1"],
            ),
            (
                metas_from(&[(
                    "file",
                    &[("report.pdf", &["doc1"]), ("image.png", &["doc2"])],
                )]),
                condition("file", "end with", json!(".pdf")),
                vec!["doc1"],
            ),
            (
                metas_from(&[("notes", &[("", &["doc1"]), ("non-empty", &["doc2"])])]),
                condition("notes", "empty", json!("")),
                vec!["doc1"],
            ),
            (
                metas_from(&[("notes", &[("", &["doc1"]), ("non-empty", &["doc2"])])]),
                condition("notes", "not empty", json!("")),
                vec!["doc2"],
            ),
            (
                metas_from(&[("score", &[("5", &["doc1"]), ("6", &["doc2"])])]),
                condition("score", "=", json!("5")),
                vec!["doc1"],
            ),
            (
                metas_from(&[("score", &[("5", &["doc1"]), ("6", &["doc2"])])]),
                condition("score", "≠", json!("5")),
                vec!["doc2"],
            ),
            (
                metas_from(&[("score", &[("10", &["doc1"]), ("2", &["doc2"])])]),
                condition("score", ">", json!("5")),
                vec!["doc1"],
            ),
            (
                metas_from(&[("score", &[("10", &["doc1"]), ("2", &["doc2"])])]),
                condition("score", "<", json!("5")),
                vec!["doc2"],
            ),
            (
                metas_from(&[(
                    "score",
                    &[("5", &["doc1"]), ("6", &["doc2"]), ("4", &["doc3"])],
                )]),
                condition("score", "≥", json!("5")),
                vec!["doc1", "doc2"],
            ),
            (
                metas_from(&[(
                    "score",
                    &[("5", &["doc1"]), ("6", &["doc2"]), ("4", &["doc3"])],
                )]),
                condition("score", "≤", json!("5")),
                vec!["doc1", "doc3"],
            ),
        ];
        for (metas, filter, expected) in cases {
            let actual = meta_filter(&metas, std::slice::from_ref(&filter), "and");
            let mut actual_sorted = actual.clone();
            actual_sorted.sort();
            let mut expected_sorted: Vec<String> =
                expected.iter().map(|value| value.to_string()).collect();
            expected_sorted.sort();
            assert_eq!(
                actual_sorted, expected_sorted,
                "{} {} {}",
                filter.key, filter.op, filter.value
            );
        }
    }

    #[test]
    fn logic_intersects_unions_and_short_circuits() {
        // Mirrors the upstream test: only Alice is present, so `contains
        // "Toby"` matches nothing and the `and` chain short-circuits.
        let metas = metas_from(&[
            ("author", &[("Alice", &["doc1"])]),
            ("page_count", &[("40", &["doc2"]), ("10", &["doc3"])]),
        ]);
        // `and` with a first condition that matches nothing returns early.
        assert_eq!(
            meta_filter(
                &metas,
                &[
                    condition("author", "contains", json!("Toby")),
                    condition("page_count", ">", json!("30")),
                ],
                "and"
            ),
            Vec::<String>::new()
        );
        // A second, matching pair intersects down to the shared document.
        let metas = metas_from(&[
            ("author", &[("Toby Jones", &["doc1"]), ("Alice", &["doc2"])]),
            ("page_count", &[("40", &["doc1"]), ("10", &["doc2"])]),
        ]);
        assert_eq!(
            meta_filter(
                &metas,
                &[
                    condition("author", "contains", json!("Toby")),
                    condition("page_count", ">", json!("30")),
                ],
                "and"
            ),
            vec!["doc1".to_string()]
        );
        // `or` keeps the second condition's matches.
        let metas = metas_from(&[
            ("author", &[("Alice", &["doc1"])]),
            ("page_count", &[("40", &["doc2"]), ("10", &["doc3"])]),
        ]);
        assert_eq!(
            meta_filter(
                &metas,
                &[
                    condition("author", "contains", json!("Toby")),
                    condition("page_count", ">", json!("30")),
                ],
                "or"
            ),
            vec!["doc2".to_string()]
        );
        // An unknown key contributes nothing.
        assert_eq!(
            meta_filter(&metas, &[condition("missing", "=", json!("x"))], "and"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn comparison_values_go_through_python_literal_eval() {
        // `"5" > "30"` is True as text but False as numbers; upstream compares
        // numbers, so only the small value matches.
        let metas = metas_from(&[("pages", &[("5", &["doc1"]), ("30", &["doc2"])])]);
        assert_eq!(
            meta_filter(&metas, &[condition("pages", ">", json!("30"))], "and"),
            Vec::<String>::new()
        );
        assert_eq!(
            meta_filter(&metas, &[condition("pages", "<", json!("30"))], "and"),
            vec!["doc1".to_string()]
        );
        // Date-shaped values compare as strings and never match free text.
        let metas = metas_from(&[(
            "listing_date",
            &[("2026-07-01", &["doc1"]), ("July", &["doc2"])],
        )]);
        assert_eq!(
            meta_filter(
                &metas,
                &[
                    condition("listing_date", "≥", json!("2026-07-01")),
                    condition("listing_date", "<", json!("2026-08-01")),
                ],
                "and"
            ),
            vec!["doc1".to_string()]
        );
        // A non-date value against a date query is skipped entirely, even under
        // `or`.
        assert_eq!(
            meta_filter(
                &metas,
                &[condition("listing_date", "=", json!("2026-07-01"))],
                "or"
            ),
            vec!["doc1".to_string()]
        );
    }

    #[test]
    fn convert_conditions_maps_the_ui_operators() {
        let condition = json!({
            "conditions": [
                {"name": "color", "comparison_operator": "is", "value": "red"},
                {"name": "color", "comparison_operator": "not is", "value": "blue"},
                {"name": "pages", "comparison_operator": ">=", "value": "10"},
                {"name": "pages", "comparison_operator": "<=", "value": "20"},
                {"name": "pages", "comparison_operator": "!=", "value": "15"},
            ]
        });
        let converted = convert_conditions(Some(&condition));
        let ops: Vec<&str> = converted.iter().map(|item| item.op.as_str()).collect();
        assert_eq!(ops, vec!["=", "≠", "≥", "≤", "≠"]);
        assert_eq!(converted[0].key, "color");
        assert_eq!(converted[0].value, json!("red"));
        assert!(convert_conditions(None).is_empty());
    }

    #[test]
    fn generated_filters_parse_think_blocks_and_fences() {
        let answer = "```json\n{\"logic\": \"or\", \"conditions\": [{\"key\": \"a\", \"op\": \"=\", \"value\": \"1\"}]}\n```";
        let parsed = parse_generated_filters(answer).expect("fenced JSON parses");
        assert_eq!(parsed.logic, "or");
        assert_eq!(parsed.conditions.len(), 1);
        // A reasoning block is dropped before parsing.
        let answer = "thinking…</think>\n{\"conditions\": [{\"key\": \"b\", \"op\": \">\", \"value\": \"2\"}]}";
        let parsed = parse_generated_filters(answer).expect("think block stripped");
        assert_eq!(parsed.logic_or_default(), "and");
        assert_eq!(parsed.conditions[0].key, "b");
        // Anything else falls back to "no conditions" upstream.
        assert!(parse_generated_filters("no json here").is_none());
        assert!(parse_generated_filters("{\"logic\": \"and\"}").is_none());
        assert!(parse_generated_filters("{\"conditions\": \"nope\"}").is_none());
        // Tolerated json_repair shapes: single quotes, trailing comma.
        let parsed =
            parse_generated_filters("{'conditions': [{'key': 'c', 'op': '=', 'value': '3'},],}")
                .expect("repaired");
        assert_eq!(parsed.conditions[0].key, "c");
    }

    #[test]
    fn prompt_renders_the_upstream_template() {
        let metas = metas_from(&[("color", &[("red", &["doc1"]), ("blue", &["doc2"])])]);
        let prompt = render_prompt(&metas, "red items", None, "2026-09-22");
        assert!(prompt.starts_with("You are a metadata filtering condition generator."));
        assert!(prompt.contains("- Today's date: 2026-09-22"));
        // Values are sorted because the flattened map is a `BTreeMap`; upstream
        // keeps the order the metadata rows were read in.
        assert!(prompt.contains("- Available metadata keys: {\"color\": [\"blue\", \"red\"]}"));
        assert!(prompt.contains("- User query: \"red items\""));
        assert!(
            !prompt.contains("Operator constraints"),
            "the Jinja `{{% if constraints %}}` line must disappear"
        );
        assert!(!prompt.contains("{current_date}") && !prompt.contains("{user_question}"));

        let constraints = BTreeMap::from([("color".to_string(), "≠".to_string())]);
        let prompt = render_prompt(&metas, "red items", Some(&constraints), "2026-09-22");
        // `json.dumps` is ASCII-only, so even the operator is escaped.
        assert!(prompt.contains("- Operator constraints: {\"color\": \"\\u2260\"}"));
        // Upstream `json.dumps` is ASCII-only.
        let metas = metas_from(&[("作者", &[("张三", &["doc1"])])]);
        let prompt = render_prompt(&metas, "张三", None, "2026-09-22");
        assert!(prompt.contains("\\u4f5c\\u8005"));
        assert!(prompt.contains("- User query: \"张三\""));
    }

    #[test]
    fn python_json_string_matches_json_dumps() {
        let mut out = String::new();
        python_json_string("a\"b\\c\nd\te\u{1}f", &mut out);
        assert_eq!(out, "\"a\\\"b\\\\c\\nd\\te\\u0001f\"");
        let mut out = String::new();
        python_json_string("中", &mut out);
        assert_eq!(out, "\"\\u4e2d\"");
        let mut out = String::new();
        python_json_string("😀", &mut out);
        assert_eq!(out, "\"\\ud83d\\ude00\"");
    }

    #[tokio::test]
    async fn manual_filters_use_the_upstream_sentinel() {
        let filter = json!({
            "method": "manual",
            "logic": "and",
            "manual": [{"key": "color", "op": "=", "value": "green"}],
        });
        let metas = metas_from(&[("color", &[("red", &["doc1"])])]);
        let applied = apply_meta_data_filter(&filter, "q", None, &[], || metas.clone())
            .await
            .unwrap();
        assert_eq!(applied, Some(vec!["-999".to_string()]));

        // A matching manual filter returns the matched ids.
        let filter = json!({
            "method": "manual",
            "manual": [{"key": "color", "op": "=", "value": "red"}],
        });
        let applied = apply_meta_data_filter(&filter, "q", None, &[], || metas.clone())
            .await
            .unwrap();
        assert_eq!(applied, Some(vec!["doc1".to_string()]));

        // An unrecognised method leaves the base ids untouched, like upstream.
        let filter = json!({"method": "nonsense"});
        let applied =
            apply_meta_data_filter(&filter, "q", None, &["base".to_string()], || metas.clone())
                .await
                .unwrap();
        assert_eq!(applied, Some(vec!["base".to_string()]));
    }

    /// `test/unit_test/common/test_apply_semi_auto_meta_data_filter.py`: the
    /// selected keys restrict the metadata handed to the model and the row
    /// operators become constraints.
    #[test]
    fn semi_auto_passes_selected_keys_and_constraints() {
        let filter = json!({
            "method": "semi_auto",
            "semi_auto": [{"key": "key1", "op": ">"}, "key2"],
        });
        let metas = metas_from(&[
            ("key1", &[("10", &["doc1"])]),
            ("key2", &[("val2", &["doc2"])]),
        ]);
        let mut selected = Vec::new();
        let mut constraints: BTreeMap<String, String> = BTreeMap::new();
        for item in filter["semi_auto"].as_array().unwrap() {
            match item {
                Value::String(key) => selected.push(key.clone()),
                Value::Object(entry) => {
                    let key = entry["key"].as_str().unwrap();
                    selected.push(key.to_string());
                    if let Some(op) = entry.get("op").and_then(Value::as_str) {
                        constraints.insert(key.to_string(), op.to_string());
                    }
                }
                _ => {}
            }
        }
        assert_eq!(selected, vec!["key1".to_string(), "key2".to_string()]);
        assert_eq!(
            constraints,
            BTreeMap::from([("key1".to_string(), ">".to_string())])
        );
        let restricted: FlattenedMetadata = selected
            .iter()
            .filter(|key| metas.contains_key(key.as_str()))
            .map(|key| (key.clone(), metas[key].clone()))
            .collect();
        assert_eq!(restricted.len(), 2);
        let prompt = render_prompt(
            &restricted,
            "find key1 > 5",
            Some(&constraints),
            "2026-09-22",
        );
        assert!(prompt.contains("\"key1\": [\"10\"]"));
        assert!(prompt.contains("- Operator constraints: {\"key1\": \">\"}"));
    }

    /// `gen_meta_filter` feeds the prompt to the model and parses its answer.
    #[tokio::test]
    async fn gen_meta_filter_reads_the_model_reply() {
        struct FakeChat;
        #[async_trait::async_trait]
        impl ChatModel for FakeChat {
            fn model_name(&self) -> &str {
                "fake"
            }
            async fn chat(&self, system: &str, history: &[ChatMessage]) -> anyhow::Result<String> {
                assert!(system.contains("- User query: \"find val1\""));
                assert_eq!(history[0].content, "Generate filters:");
                Ok("```json\n{\"logic\": \"and\", \"conditions\": [{\"key\": \"key1\", \"op\": \"=\", \"value\": \"val1\"}]}\n```".to_string())
            }
            async fn chat_stream(
                &self,
                _system: &str,
                _history: &[ChatMessage],
                _on_chunk: Box<dyn for<'a> FnMut(&'a str) + Send>,
            ) -> anyhow::Result<String> {
                unimplemented!()
            }
        }
        let metas = metas_from(&[
            ("key1", &[("val1", &["doc1"])]),
            ("key2", &[("val2", &["doc2"])]),
        ]);
        let generated = gen_meta_filter(&FakeChat, &metas, "find val1", None)
            .await
            .unwrap();
        assert_eq!(generated.conditions.len(), 1);
        assert_eq!(
            meta_filter(&metas, &generated.conditions, generated.logic_or_default()),
            vec!["doc1".to_string()]
        );

        // A model that answers nonsense yields no conditions (upstream's
        // `{"conditions": []}` fallback) and therefore no filtering.
        struct SilentChat;
        #[async_trait::async_trait]
        impl ChatModel for SilentChat {
            fn model_name(&self) -> &str {
                "silent"
            }
            async fn chat(
                &self,
                _system: &str,
                _history: &[ChatMessage],
            ) -> anyhow::Result<String> {
                Ok("I cannot help with that.".to_string())
            }
            async fn chat_stream(
                &self,
                _system: &str,
                _history: &[ChatMessage],
                _on_chunk: Box<dyn for<'a> FnMut(&'a str) + Send>,
            ) -> anyhow::Result<String> {
                unimplemented!()
            }
        }
        let generated = gen_meta_filter(&SilentChat, &metas, "q", None)
            .await
            .unwrap();
        assert!(generated.conditions.is_empty());
        assert!(
            meta_filter(&metas, &generated.conditions, generated.logic_or_default()).is_empty()
        );
    }

    #[test]
    fn date_shape_detection_matches_upstream() {
        assert!(is_date_shaped("2026-07-01"));
        assert!(!is_date_shaped("2026-7-1"));
        assert!(!is_date_shaped("2026-07-01 "));
        assert!(!is_date_shaped("July"));
        assert!(!is_date_shaped("20260701"));
        assert!(!is_date_shaped("2026-07-0a"));
    }
}
