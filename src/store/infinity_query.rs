//! Infinity query-building semantics: analyzer/index selection, generic
//! filter/sort/aggregation construction, and the doc-meta filter translator.
//!
//! RayRAG does not deploy Infinity. This module ports the query-side contract
//! of the fixed RAGFlow v0.26.4 connector family verbatim so the store layer
//! can reproduce Infinity-shaped filters, order-by lists, aggregations, and
//! full-text index selection for diagnostics, snapshots, and future native
//! backends:
//! - analyzer/index selection: `common/doc_store/infinity_conn_base.py` +
//!   `rag/utils/infinity_conn.py` (`convert_select_fields`,
//!   `convert_matching_field`, full-text index naming);
//! - generic filters: `equivalent_condition_to_str`
//!   (`common/doc_store/infinity_conn_base.py`);
//! - doc-meta filters: `common/metadata_infinity_filter.py`
//!   (`MetaFilterTranslator` + `build_infinity_filter`);
//! - generic aggregation: `get_aggregation`
//!   (`common/doc_store/infinity_conn_base.py`).

use crate::Result;
use crate::doc_store::{DocRow, FilterCondition, OrderByExpr};
use crate::settings::{
    INFINITY_CHUNK_FIELDS, InfinityChunkAnalyzer, InfinityChunkFieldSchema,
    infinity_chunk_fulltext_index, infinity_chunk_secondary_index,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Port of `InfinityConnection.field_keyword`: `*_kwd` tag-like columns (and
/// `source_id`) are keyword lists, except the logical alias columns the
/// Python connector exempts.
pub fn field_keyword(field_name: &str) -> bool {
    field_name == "source_id"
        || (field_name.ends_with("_kwd")
            && !matches!(
                field_name,
                "knowledge_graph_kwd" | "docnm_kwd" | "important_kwd" | "question_kwd"
            ))
}

/// Resolve the analyzer used for one matching-field alias, straight from the
/// fixed mapping contract.
///
/// Python rule (`convert_matching_field`): the LAST alias in a column's
/// `comment` maps to the last analyzer (`rag-fine`), every earlier alias maps
/// to the first (`rag-coarse`). Single-analyzer columns map to that analyzer.
pub fn analyzer_for_alias(alias: &str) -> Option<&'static str> {
    for field in INFINITY_CHUNK_FIELDS {
        let Some(comment) = field.comment else {
            continue;
        };
        let aliases: Vec<&str> = comment.split(',').map(str::trim).collect();
        if let Some(position) = aliases.iter().position(|candidate| *candidate == alias) {
            return analyzer_for_position(field, position + 1 == aliases.len());
        }
    }
    None
}

fn analyzer_for_position(field: &InfinityChunkFieldSchema, is_last: bool) -> Option<&'static str> {
    match field.analyzer {
        InfinityChunkAnalyzer::None => None,
        InfinityChunkAnalyzer::Single(analyzer) => Some(analyzer),
        InfinityChunkAnalyzer::Multiple(analyzers) => {
            if is_last {
                analyzers.last().copied()
            } else {
                analyzers.first().copied()
            }
        }
    }
}

fn single_analyzer(name: &str) -> Option<&'static str> {
    let field = INFINITY_CHUNK_FIELDS
        .iter()
        .find(|field| field.name == name)?;
    match field.analyzer {
        InfinityChunkAnalyzer::Single(analyzer) => Some(analyzer),
        _ => None,
    }
}

/// The full-text "matching field" for a logical field name: `{column}@{index}`.
///
/// Mirrors `InfinityConnection.convert_matching_field`'s field part. Alias
/// fields resolve through the mapping `comment`; `tag_kwd` is the one physical
/// column the Python connector rewrites. Unknown names pass through unchanged
/// (Python leaves them as-is).
pub fn fulltext_matching_field(field: &str) -> String {
    for field_schema in INFINITY_CHUNK_FIELDS {
        let Some(comment) = field_schema.comment else {
            continue;
        };
        let aliases: Vec<&str> = comment.split(',').map(str::trim).collect();
        if let Some(position) = aliases.iter().position(|candidate| *candidate == field)
            && let Some(analyzer) =
                analyzer_for_position(field_schema, position + 1 == aliases.len())
        {
            return format!(
                "{}@{}",
                field_schema.name,
                infinity_chunk_fulltext_index(field_schema.name, analyzer)
            );
        }
    }
    if field == "tag_kwd"
        && let Some(analyzer) = single_analyzer("tag_kwd")
    {
        return format!(
            "tag_kwd@{}",
            infinity_chunk_fulltext_index("tag_kwd", analyzer)
        );
    }
    field.to_owned()
}

/// Port of `InfinityConnection.convert_matching_field`: rewrite the field part
/// of a `field^weight` string, keeping the weight suffix.
pub fn convert_matching_field(field_weight_str: &str) -> String {
    let mut tokens = field_weight_str.split('^');
    let field = tokens.next().unwrap_or_default();
    let mapped = fulltext_matching_field(field);
    let rest: Vec<&str> = tokens.collect();
    if rest.is_empty() {
        mapped
    } else {
        format!("{mapped}^{}", rest.join("^"))
    }
}

/// Port of `InfinityConnection.convert_select_fields`: alias → physical base
/// column, plus the `important_kwd` → `important_kwd_empty_count` companion
/// column. Python returns a `set` (unordered); Rust keeps deterministic
/// first-occurrence order.
pub fn convert_select_fields(output_fields: &[String]) -> Vec<String> {
    let mut output: Vec<String> = Vec::new();
    let need_empty_count = output_fields.iter().any(|field| field == "important_kwd");
    for field in output_fields {
        let converted = match field.as_str() {
            "docnm_kwd" | "title_tks" | "title_sm_tks" => "docnm",
            "important_kwd" | "important_tks" => "important_keywords",
            "question_kwd" | "question_tks" => "questions",
            "content_with_weight" | "content_ltks" | "content_sm_ltks" => "content",
            "authors_tks" | "authors_sm_tks" => "authors",
            other => other,
        };
        if !output.iter().any(|existing| existing == converted) {
            output.push(converted.to_owned());
        }
    }
    if need_empty_count
        && !output
            .iter()
            .any(|field| field == "important_kwd_empty_count")
    {
        output.push("important_kwd_empty_count".into());
    }
    output
}

/// All full-text index names the fixed mapping declares: one
/// `ft_{column}_{analyzer}` per (varchar column, analyzer) pair — the set the
/// Python connector creates during `_migrate_db` / `create_idx`.
pub fn fulltext_indexes() -> Vec<String> {
    let mut indexes = Vec::new();
    for field in INFINITY_CHUNK_FIELDS {
        if field.infinity_type != "varchar" {
            continue;
        }
        match field.analyzer {
            InfinityChunkAnalyzer::None => {}
            InfinityChunkAnalyzer::Single(analyzer) => {
                indexes.push(infinity_chunk_fulltext_index(field.name, analyzer));
            }
            InfinityChunkAnalyzer::Multiple(analyzers) => {
                for analyzer in analyzers {
                    indexes.push(infinity_chunk_fulltext_index(field.name, analyzer));
                }
            }
        }
    }
    indexes
}

/// All secondary index names the fixed mapping declares (`kb_id`,
/// `available_int` carry `index_type: secondary`).
pub fn secondary_indexes() -> Vec<String> {
    INFINITY_CHUNK_FIELDS
        .iter()
        .filter(|field| field.secondary_cardinality.is_some())
        .map(|field| infinity_chunk_secondary_index(field.name))
        .collect()
}

/// Python falsiness for a condition value: null, false, zero, empty string,
/// empty array, empty object.
fn value_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|value| value != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
    }
}

/// Python `str()` rendering for the scalar branch of the condition builder.
fn python_scalar_repr(value: &Value) -> String {
    match value {
        Value::Bool(flag) => {
            if *flag {
                "True".into()
            } else {
                "False".into()
            }
        }
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// SQL single-quote escaping used by the connector: `'` → `''`.
fn escape_sql_string(value: &str) -> String {
    value.replace('\'', "''")
}

/// Render the column default for `exists()` filters. Python `find("cha")` is
/// always truthy for varchar/integer/float types, so the quoted branch runs:
/// falsy defaults collapse to `''`.
fn column_default_repr(column: &str) -> Option<String> {
    let field = INFINITY_CHUNK_FIELDS
        .iter()
        .find(|field| field.name == column)?;
    let default = match field.default {
        crate::settings::InfinityChunkDefault::Text(text) => text,
        crate::settings::InfinityChunkDefault::Integer(value) if value != 0 => {
            return Some(value.to_string());
        }
        crate::settings::InfinityChunkDefault::Float(value) if value != 0.0 => {
            return Some(value.to_string());
        }
        crate::settings::InfinityChunkDefault::Integer(_)
        | crate::settings::InfinityChunkDefault::Float(_) => "",
    };
    if default.is_empty() {
        Some(String::new())
    } else {
        Some(default.to_owned())
    }
}

/// Port of the `exists(cln)` inner helper. The fixed chunk schema has only
/// varchar/integer/float columns (none start with `cha`), so the quoted
/// branch always runs; unknown columns raise exactly like Python's assert.
fn exists_filter(column: &str) -> Result<String> {
    let default = column_default_repr(column)
        .ok_or_else(|| anyhow::anyhow!("'{column}' should be in the fixed chunk schema."))?;
    Ok(if default.is_empty() {
        format!(" {column}!='' ")
    } else {
        format!(" {column}!='{default}' ")
    })
}

/// Port of `InfinityConnectionBase.equivalent_condition_to_str`: build the
/// conjunctive Infinity filter string for one condition map.
///
/// Faithful branch order (Python evaluates in this exact sequence): falsy
/// values are skipped, keyword fields render `filter_fulltext`, list values
/// render `IN`, `must_not`/`exists` render `NOT (col!='default')`, strings
/// render `k='v'` with `''` escaping, and `available_int` only accepts 0/1.
pub fn equivalent_condition_to_str(condition: &FilterCondition) -> Result<String> {
    if condition.contains_key("_id") {
        anyhow::bail!("Infinity conditions must not address the physical _id column");
    }
    let mut parts: Vec<String> = Vec::new();
    for (key, value) in condition {
        if key == "available_int" {
            match value {
                Value::Number(number) if number.as_i64() == Some(0) => {
                    parts.push("available_int=0".into());
                }
                Value::Number(number) if number.as_i64() == Some(1) => {
                    parts.push("available_int=1".into());
                }
                _ => {}
            }
            continue;
        }
        if !value_truthy(value) {
            continue;
        }
        if field_keyword(key) {
            match value {
                Value::Array(values) => {
                    let mut in_conditions: Vec<String> = Vec::new();
                    for item in values {
                        if !value_truthy(item) {
                            continue;
                        }
                        let rendered = python_scalar_repr(item);
                        in_conditions.push(format!(
                            "filter_fulltext('{}', '{}')",
                            convert_matching_field(key),
                            escape_sql_string(&rendered)
                        ));
                    }
                    if !in_conditions.is_empty() {
                        parts.push(format!("({})", in_conditions.join(" or ")));
                    }
                }
                other => {
                    let rendered = python_scalar_repr(other);
                    parts.push(format!(
                        "filter_fulltext('{}', '{}')",
                        convert_matching_field(key),
                        escape_sql_string(&rendered)
                    ));
                }
            }
        } else if let Value::Array(values) = value {
            let mut in_conditions: Vec<String> = Vec::new();
            for item in values {
                if !value_truthy(item) {
                    continue;
                }
                match item {
                    Value::String(text) => {
                        in_conditions.push(format!("'{}'", escape_sql_string(text)));
                    }
                    other => in_conditions.push(python_scalar_repr(other)),
                }
            }
            if !in_conditions.is_empty() {
                parts.push(format!("{key} IN ({})", in_conditions.join(", ")));
            }
        } else if key == "must_not" {
            if let Value::Object(entries) = value {
                for (nested_key, nested_value) in entries {
                    if nested_key == "exists"
                        && let Value::String(column) = nested_value
                    {
                        parts.push(format!("NOT ({})", exists_filter(column)?));
                    }
                }
            }
        } else if let Value::String(text) = value {
            parts.push(format!("{key}='{}'", escape_sql_string(text)));
        } else if key == "exists" {
            if let Value::String(column) = value {
                parts.push(exists_filter(column)?);
            } else {
                parts.push(format!("{key}={}", python_scalar_repr(value)));
            }
        } else {
            parts.push(format!("{key}={}", python_scalar_repr(value)));
        }
    }
    Ok(if parts.is_empty() {
        "1=1".into()
    } else {
        parts.join(" AND ")
    })
}

/// Python sort-direction rule: `order_field[1] == 0` → Ascending, otherwise
/// Descending. RayRAG's `OrderByExpr` stores `(field, descending)`, so
/// `descending == false` maps to the Python `0`.
fn sort_descending(descending: bool) -> bool {
    descending
}

/// Deterministic total order over arbitrary JSON values for generic sorting:
/// Null < Bool < Number < String < Array < Object.
fn value_rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        Value::Array(_) => 4,
        Value::Object(_) => 5,
    }
}

fn compare_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    let rank = value_rank(left).cmp(&value_rank(right));
    if rank != std::cmp::Ordering::Equal {
        return rank;
    }
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a
            .as_f64()
            .partial_cmp(&b.as_f64())
            .unwrap_or(std::cmp::Ordering::Equal),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Array(a), Value::Array(b)) => a.len().cmp(&b.len()),
        _ => std::cmp::Ordering::Equal,
    }
}

/// Generic row sort implementing the connector's order-by list semantics:
/// rows are compared field-by-field, ascending for `descending == false`
/// (Python `0`) and descending otherwise. Nulls and heterogeneous JSON
/// values follow [`compare_values`].
pub fn sort_rows(rows: &mut [DocRow], order_by: &OrderByExpr) {
    rows.sort_by(|a, b| {
        for (field, descending) in &order_by.fields {
            let left = a.get(field);
            let right = b.get(field);
            let ordering = match (left, right) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(left), Some(right)) => compare_values(left, right),
            };
            if ordering != std::cmp::Ordering::Equal {
                return if sort_descending(*descending) {
                    ordering.reverse()
                } else {
                    ordering
                };
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// Port of `InfinityConnectionBase.get_aggregation`: manual tag counting for
/// tag fields. `tag_kwd` splits on `###`; other string fields fall back to
/// comma splitting; JSON arrays count each string member. Ordering follows
/// `Counter.most_common()`: count descending, ties by first appearance.
pub fn tag_aggregation(rows: &[DocRow], field_name: &str) -> Vec<(String, usize)> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut first_seen: Vec<String> = Vec::new();
    let mut count = |tag: String, counts: &mut std::collections::HashMap<String, usize>| {
        if !counts.contains_key(&tag) {
            first_seen.push(tag.clone());
        }
        *counts.entry(tag).or_insert(0) += 1;
    };

    for row in rows {
        let Some(value) = row.get(field_name) else {
            continue;
        };
        match value {
            Value::Null => continue,
            Value::String(text) => {
                if text.is_empty() {
                    continue;
                }
                let tags: Vec<&str> = if field_name == "tag_kwd" && text.contains("###") {
                    text.split("###").collect()
                } else {
                    text.split(',').collect()
                };
                for tag in tags
                    .into_iter()
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                {
                    count(tag.to_owned(), &mut counts);
                }
            }
            Value::Array(tags) => {
                for tag in tags {
                    if let Value::String(tag) = tag {
                        let tag = tag.trim();
                        if !tag.is_empty() {
                            count(tag.to_owned(), &mut counts);
                        }
                    }
                }
            }
            other => {
                if !value_truthy(other) {
                    continue;
                }
                count(python_scalar_repr(other), &mut counts);
            }
        }
    }
    let mut ordered: Vec<(String, usize)> = first_seen
        .into_iter()
        .map(|tag| {
            let count = counts[&tag];
            (tag, count)
        })
        .collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1));
    ordered
}

// ---------------------------------------------------------------------------
// Doc-meta filter translator (`common/metadata_infinity_filter.py`)
// ---------------------------------------------------------------------------

const METADATA_KEY_PATTERN: &str = r"^[a-zA-Z_][a-zA-Z0-9_]*$";

const SUPPORTED_OPERATORS: &[&str] = &[
    "=",
    "≠",
    ">",
    "<",
    "≥",
    "≤",
    "in",
    "not in",
    "contains",
    "not contains",
    "start with",
    "end with",
    "empty",
    "not empty",
];

/// One user document-metadata filter clause (`{"op", "key", "value"}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetadataFilter {
    pub op: String,
    pub key: String,
    #[serde(default)]
    pub value: Value,
}

/// Coercion result of `_coerce_scalar` / `_coerce_range_value`.
#[derive(Debug, Clone, PartialEq)]
enum Scalar {
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
}

impl Scalar {
    /// Python `str()` rendering: bools render capitalized (`True`/`False`).
    fn render(&self) -> String {
        match self {
            Scalar::Int(value) => value.to_string(),
            Scalar::Float(value) => value.to_string(),
            Scalar::Bool(value) => {
                if *value {
                    "True".into()
                } else {
                    "False".into()
                }
            }
            Scalar::Text(value) => value.clone(),
        }
    }

    fn is_numeric(&self) -> bool {
        matches!(self, Scalar::Int(_) | Scalar::Float(_))
    }
}

/// Python `ast.literal_eval(str(value).strip())` restricted to the int/float/
/// bool surface the translator consumes.
fn literal_eval_scalar(raw: &str) -> Option<Scalar> {
    let trimmed = raw.trim();
    if trimmed == "True" {
        return Some(Scalar::Bool(true));
    }
    if trimmed == "False" {
        return Some(Scalar::Bool(false));
    }
    if let Ok(value) = trimmed.parse::<i64>() {
        return Some(Scalar::Int(value));
    }
    if let Ok(value) = trimmed.parse::<f64>()
        && value.is_finite()
    {
        return Some(Scalar::Float(value));
    }
    None
}

fn coerce_scalar(value: &Value, filter: &MetadataFilter) -> Result<Scalar> {
    match value {
        Value::Null => anyhow::bail!("scalar comparison value is None: {filter:?}"),
        Value::Array(_) | Value::Object(_) => {
            anyhow::bail!("scalar comparison value is non-scalar: {filter:?}")
        }
        Value::Bool(flag) => Ok(Scalar::Bool(*flag)),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(Scalar::Int(value))
            } else if let Some(value) = number.as_f64() {
                Ok(Scalar::Float(value))
            } else {
                Ok(Scalar::Text(number.to_string()))
            }
        }
        Value::String(text) => {
            if let Some(parsed) = literal_eval_scalar(text)
                && matches!(parsed, Scalar::Int(_) | Scalar::Float(_) | Scalar::Bool(_))
            {
                Ok(parsed)
            } else {
                Ok(Scalar::Text(text.clone()))
            }
        }
    }
}

fn coerce_range_value(value: &Value, filter: &MetadataFilter) -> Result<Scalar> {
    match value {
        Value::Null => anyhow::bail!("range comparison value is None: {filter:?}"),
        Value::String(text) => match literal_eval_scalar(text) {
            Some(parsed) if parsed.is_numeric() || matches!(parsed, Scalar::Bool(_)) => Ok(parsed),
            _ => Ok(Scalar::Text(text.clone())),
        },
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                Ok(Scalar::Int(value))
            } else if let Some(value) = number.as_f64() {
                Ok(Scalar::Float(value))
            } else {
                Ok(Scalar::Text(number.to_string()))
            }
        }
        Value::Bool(flag) => Ok(Scalar::Bool(*flag)),
        Value::Array(_) | Value::Object(_) => {
            anyhow::bail!("range comparison value is non-scalar: {filter:?}")
        }
    }
}

fn coerce_string(value: &Value, filter: &MetadataFilter) -> Result<String> {
    match value {
        Value::Null => anyhow::bail!("string-operator value is None: {filter:?}"),
        Value::Array(_) | Value::Object(_) => {
            anyhow::bail!("string-operator value must be a scalar: {filter:?}")
        }
        other => {
            let text = python_scalar_repr(other);
            if text.is_empty() {
                anyhow::bail!("string-operator value is empty: {filter:?}")
            }
            Ok(text)
        }
    }
}

/// `_csv_or_list`: accept lists, JSON-list strings, or comma-separated text.
/// String members are lowercased and stripped exactly like the Python port;
/// numeric coercion happens later in [`partition_members`], mirroring the
/// translator's separate `_coerce_range_value` pass.
fn csv_or_list(value: &Value, filter: &MetadataFilter) -> Result<Vec<Value>> {
    let members: Vec<Value> = match value {
        Value::Null => anyhow::bail!("membership value is None: {filter:?}"),
        Value::Array(values) => values
            .iter()
            .map(|member| match member {
                Value::String(text) => Value::String(text.to_lowercase().trim().to_owned()),
                other => other.clone(),
            })
            .collect(),
        Value::String(text) => {
            let parsed = serde_json::from_str::<Value>(text);
            match parsed {
                Ok(Value::Array(values)) => values
                    .iter()
                    .map(|member| match member {
                        Value::String(text) => Value::String(text.to_lowercase().trim().to_owned()),
                        other => other.clone(),
                    })
                    .collect(),
                _ => text
                    .split(',')
                    .map(str::trim)
                    .filter(|member| !member.is_empty())
                    .map(|member| Value::String(member.to_lowercase()))
                    .collect(),
            }
        }
        other => vec![other.clone()],
    };
    if members.is_empty() {
        anyhow::bail!("membership value resolved to empty list: {filter:?}")
    }
    Ok(members)
}

fn escape_like_wildcards(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn validate_key(key: &str, filter: &MetadataFilter) -> Result<()> {
    let valid = !key.is_empty()
        && key
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        anyhow::bail!("invalid key format (must be identifier-like): {filter:?}");
    }
    Ok(())
}

/// Port of `MetaFilterTranslator.translate`: one filter clause → one Infinity
/// SQL fragment over the `meta_fields` JSON column.
pub fn translate_metadata_filter(filter: &MetadataFilter) -> Result<String> {
    validate_key(&filter.key, filter)?;
    if !SUPPORTED_OPERATORS.contains(&filter.op.as_str()) {
        anyhow::bail!("unknown operator: {:?}, filter: {:?}", filter.op, filter);
    }
    let key = &filter.key;
    let rendered = match filter.op.as_str() {
        "empty" => format!("JSON_EXTRACT_STRING(meta_fields, '$.{key}') = '\"\"'"),
        "not empty" => format!("JSON_EXTRACT_STRING(meta_fields, '$.{key}') != '\"\"'"),
        "=" => {
            let coerced = coerce_scalar(&filter.value, filter)?;
            match &coerced {
                Scalar::Text(text) => format!(
                    "JSON_CONTAINS(meta_fields, '$.{key}', '\"{}\"')",
                    escape_sql_string(text)
                ),
                _ => format!(
                    "JSON_CONTAINS(meta_fields, '$.{key}', {})",
                    coerced.render()
                ),
            }
        }
        "≠" => {
            let coerced = coerce_scalar(&filter.value, filter)?;
            match &coerced {
                Scalar::Text(text) => format!(
                    "NOT JSON_CONTAINS(meta_fields, '$.{key}', '\"{}\"')",
                    escape_sql_string(text)
                ),
                _ => format!(
                    "NOT JSON_CONTAINS(meta_fields, '$.{key}', {})",
                    coerced.render()
                ),
            }
        }
        ">" | "<" | "≥" | "≤" => {
            let sql_op = match filter.op.as_str() {
                ">" => ">",
                "<" => "<",
                "≥" => ">=",
                "≤" => "<=",
                _ => unreachable!(),
            };
            let coerced = coerce_range_value(&filter.value, filter)?;
            match &coerced {
                Scalar::Text(text) => format!(
                    "JSON_EXTRACT_STRING(meta_fields, '$.{key}') {sql_op} '{}'",
                    escape_sql_string(text)
                ),
                _ => format!(
                    "JSON_EXTRACT_DOUBLE(meta_fields, '$.{key}') {sql_op} {}",
                    coerced.render()
                ),
            }
        }
        "in" => {
            let members = csv_or_list(&filter.value, filter)?;
            let (string_parts, number_parts) = partition_members(key, members, false, filter)?;
            wrap_conditions(string_parts, number_parts, " OR ")
        }
        "not in" => {
            let members = csv_or_list(&filter.value, filter)?;
            let (string_parts, number_parts) = partition_members(key, members, true, filter)?;
            wrap_conditions(string_parts, number_parts, " AND ")
        }
        "contains" => {
            let coerced = coerce_range_value(&filter.value, filter)?;
            match &coerced {
                Scalar::Text(text) => {
                    if text.is_empty() {
                        anyhow::bail!("contains value is empty: {filter:?}")
                    }
                    format!(
                        "JSON_CONTAINS(meta_fields, '$.{key}', '\"{}\"')",
                        escape_sql_string(text)
                    )
                }
                _ => format!(
                    "JSON_CONTAINS(meta_fields, '$.{key}', {})",
                    coerced.render()
                ),
            }
        }
        "not contains" => {
            let text = coerce_string(&filter.value, filter)?;
            format!(
                "NOT JSON_CONTAINS(meta_fields, '$.{key}', '\"{}\"')",
                escape_sql_string(&text)
            )
        }
        "start with" => {
            let text = coerce_string(&filter.value, filter)?;
            format!(
                "JSON_EXTRACT_STRING(meta_fields, '$.{key}') LIKE '{}%'",
                escape_sql_string(&escape_like_wildcards(&text))
            )
        }
        "end with" => {
            let text = coerce_string(&filter.value, filter)?;
            format!(
                "JSON_EXTRACT_STRING(meta_fields, '$.{key}') LIKE '%{}'",
                escape_sql_string(&escape_like_wildcards(&text))
            )
        }
        _ => anyhow::bail!(
            "no handler for operator: {:?}, filter: {:?}",
            filter.op,
            filter
        ),
    };
    Ok(rendered)
}

fn partition_members(
    key: &str,
    members: Vec<Value>,
    negate: bool,
    filter: &MetadataFilter,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut string_parts = Vec::new();
    let mut number_parts = Vec::new();
    for member in members {
        let coerced = coerce_range_value(&member, filter)?;
        match coerced {
            Scalar::Text(text) => string_parts.push(format!(
                "{}JSON_CONTAINS(meta_fields, '$.{key}', '\"{}\"')",
                if negate { "NOT " } else { "" },
                escape_sql_string(&text)
            )),
            other => number_parts.push(format!(
                "{}JSON_CONTAINS(meta_fields, '$.{key}', {})",
                if negate { "NOT " } else { "" },
                other.render()
            )),
        }
    }
    Ok((string_parts, number_parts))
}

fn wrap_conditions(string_parts: Vec<String>, number_parts: Vec<String>, joiner: &str) -> String {
    let mut conditions = Vec::new();
    if !string_parts.is_empty() {
        conditions.push(format!("({})", string_parts.join(joiner)));
    }
    if !number_parts.is_empty() {
        conditions.push(format!("({})", number_parts.join(joiner)));
    }
    format!("({})", conditions.join(joiner))
}

/// Port of `plan_pushdown`: translate every clause in order.
pub fn plan_pushdown(filters: &[MetadataFilter]) -> Result<Vec<String>> {
    filters.iter().map(translate_metadata_filter).collect()
}

/// Port of `build_infinity_filter`: join translated clauses with the requested
/// logic; an empty filter list yields `1=1`.
pub fn build_infinity_filter(filters: &[MetadataFilter], logic: &str) -> Result<String> {
    if logic != "and" && logic != "or" {
        anyhow::bail!("unknown logic {logic:?}");
    }
    if filters.is_empty() {
        return Ok("1=1".into());
    }
    let fragments = plan_pushdown(filters)?;
    let joiner = if logic == "and" { " AND " } else { " OR " };
    Ok(format!("({})", fragments.join(joiner)))
}

/// Port of `is_pushdown_supported`: every clause carries a supported operator
/// and a non-empty string key.
pub fn is_pushdown_supported(filters: &[MetadataFilter]) -> bool {
    filters
        .iter()
        .all(|filter| SUPPORTED_OPERATORS.contains(&filter.op.as_str()) && !filter.key.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn condition(entries: &[(&str, Value)]) -> FilterCondition {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn matching_field_resolution_follows_the_python_alias_contract() {
        assert_eq!(
            convert_matching_field("docnm_kwd"),
            "docnm@ft_docnm_rag_coarse"
        );
        assert_eq!(
            convert_matching_field("title_tks"),
            "docnm@ft_docnm_rag_coarse"
        );
        assert_eq!(
            convert_matching_field("title_sm_tks"),
            "docnm@ft_docnm_rag_fine"
        );
        assert_eq!(
            convert_matching_field("important_kwd"),
            "important_keywords@ft_important_keywords_rag_coarse"
        );
        assert_eq!(
            convert_matching_field("important_tks"),
            "important_keywords@ft_important_keywords_rag_fine"
        );
        assert_eq!(
            convert_matching_field("question_kwd"),
            "questions@ft_questions_rag_coarse"
        );
        assert_eq!(
            convert_matching_field("question_tks"),
            "questions@ft_questions_rag_fine"
        );
        assert_eq!(
            convert_matching_field("content_with_weight"),
            "content@ft_content_rag_coarse"
        );
        assert_eq!(
            convert_matching_field("content_ltks"),
            "content@ft_content_rag_coarse"
        );
        assert_eq!(
            convert_matching_field("content_sm_ltks"),
            "content@ft_content_rag_fine"
        );
        assert_eq!(
            convert_matching_field("authors_tks"),
            "authors@ft_authors_rag_coarse"
        );
        assert_eq!(
            convert_matching_field("authors_sm_tks"),
            "authors@ft_authors_rag_fine"
        );
        assert_eq!(
            convert_matching_field("tag_kwd"),
            "tag_kwd@ft_tag_kwd_whitespace__"
        );
        // Unknown fields pass through untouched, like Python.
        assert_eq!(convert_matching_field("entities_kwd"), "entities_kwd");
    }

    #[test]
    fn matching_field_keeps_weight_suffixes() {
        assert_eq!(
            convert_matching_field("content_ltks^2"),
            "content@ft_content_rag_coarse^2"
        );
        assert_eq!(
            convert_matching_field("title_sm_tks^0.5"),
            "docnm@ft_docnm_rag_fine^0.5"
        );
    }

    #[test]
    fn analyzer_names_come_from_the_mapping_contract() {
        assert_eq!(analyzer_for_alias("docnm_kwd"), Some("rag-coarse"));
        assert_eq!(analyzer_for_alias("title_sm_tks"), Some("rag-fine"));
        assert_eq!(analyzer_for_alias("important_tks"), Some("rag-fine"));
        assert_eq!(analyzer_for_alias("question_kwd"), Some("rag-coarse"));
        assert_eq!(analyzer_for_alias("unknown_field"), None);
    }

    #[test]
    fn select_field_conversion_appends_important_kwd_empty_count() {
        assert_eq!(
            convert_select_fields(&[
                "docnm_kwd".into(),
                "title_tks".into(),
                "important_kwd".into(),
                "question_tks".into(),
            ]),
            vec![
                "docnm",
                "important_keywords",
                "questions",
                "important_kwd_empty_count"
            ]
        );
        // Without important_kwd the companion column is not added.
        assert_eq!(
            convert_select_fields(&["content_ltks".into(), "content_sm_ltks".into()]),
            vec!["content"]
        );
    }

    #[test]
    fn fulltext_and_secondary_index_selection_matches_the_fixed_contract() {
        let fulltext = fulltext_indexes();
        assert_eq!(fulltext.len(), 45);
        assert!(fulltext.iter().any(|index| index == "ft_docnm_rag_coarse"));
        assert!(fulltext.iter().any(|index| index == "ft_docnm_rag_fine"));
        assert!(
            fulltext
                .iter()
                .any(|index| index == "ft_tag_kwd_whitespace__")
        );
        assert!(
            fulltext
                .iter()
                .any(|index| index == "ft_tag_feas_rankfeatures")
        );
        assert_eq!(
            secondary_indexes(),
            vec!["sec_kb_id".to_owned(), "sec_available_int".to_owned()]
        );
    }

    #[test]
    fn condition_builder_handles_every_python_branch() {
        // available_int 0/1 (other values dropped), falsy skipped, strings
        // escaped, lists become IN, numbers render bare, keyword fields
        // become filter_fulltext with the converted matching field.
        let filters = condition(&[
            ("available_int", json!(1)),
            ("doc_id", json!("doc-1")),
            ("kb_id", json!(["kb-a", "kb-b"])),
            ("weight_int", json!(0)),
            ("tag_kwd", json!(["rust", "search"])),
            ("doc_type_kwd", json!("article")),
            ("knowledge_graph_kwd", json!(["entity", "graph"])),
        ]);
        let rendered = equivalent_condition_to_str(&filters).unwrap();
        assert!(rendered.contains("available_int=1"));
        assert!(rendered.contains("doc_id='doc-1'"));
        assert!(rendered.contains("kb_id IN ('kb-a', 'kb-b')"));
        assert!(!rendered.contains("weight_int"));
        assert!(rendered.contains(
            "(filter_fulltext('tag_kwd@ft_tag_kwd_whitespace__', 'rust') or filter_fulltext('tag_kwd@ft_tag_kwd_whitespace__', 'search'))"
        ));
        // doc_type_kwd is keyword-like but NOT in the Python alias rewrite
        // list, so `convert_matching_field` leaves it bare.
        assert!(rendered.contains("filter_fulltext('doc_type_kwd', 'article')"));
        // knowledge_graph_kwd is exempt from keyword handling → list IN.
        assert!(rendered.contains("knowledge_graph_kwd IN ('entity', 'graph')"));
    }

    #[test]
    fn condition_builder_escapes_quotes_and_handles_must_not_exists() {
        // docnm_kwd is exempt from keyword handling → plain string equality.
        let filters = condition(&[
            ("docnm_kwd", json!("it's")),
            ("must_not", json!({"exists": "source_id"})),
        ]);
        let rendered = equivalent_condition_to_str(&filters).unwrap();
        assert!(rendered.contains("docnm_kwd='it''s'"));
        assert!(rendered.contains("NOT ( source_id!='' )"));
    }

    #[test]
    fn condition_builder_existence_checks_use_schema_defaults() {
        // Faithful Python branch order: a string "exists" value hits the
        // string-equality branch BEFORE the exists() helper, so the chunk
        // connector renders it as a literal equality. The exists() filter is
        // only reachable through the `must_not` dict path.
        let by_pagerank = condition(&[("exists", json!("pagerank_fea"))]);
        assert_eq!(
            equivalent_condition_to_str(&by_pagerank).unwrap(),
            "exists='pagerank_fea'"
        );
        // pagerank_fea defaults to 0 (falsy) → quoted empty default.
        let must_not_pagerank = condition(&[("must_not", json!({"exists": "pagerank_fea"}))]);
        assert_eq!(
            equivalent_condition_to_str(&must_not_pagerank).unwrap(),
            "NOT ( pagerank_fea!='' )"
        );
        // available_int defaults to 1 → non-empty default.
        let must_not_available = condition(&[("must_not", json!({"exists": "available_int"}))]);
        assert_eq!(
            equivalent_condition_to_str(&must_not_available).unwrap(),
            "NOT ( available_int!='1' )"
        );
        // Unknown columns raise exactly like Python's assert.
        let unknown = condition(&[("must_not", json!({"exists": "not_a_column"}))]);
        assert!(equivalent_condition_to_str(&unknown).is_err());
        // The physical _id column is rejected.
        let forbidden = condition(&[("_id", json!("x"))]);
        assert!(equivalent_condition_to_str(&forbidden).is_err());
    }

    #[test]
    fn empty_condition_renders_tautology() {
        assert_eq!(equivalent_condition_to_str(&condition(&[])).unwrap(), "1=1");
    }

    #[test]
    fn generic_sort_matches_infinity_order_by_semantics() {
        let row = |id: &str, rank: i64| {
            let mut map = DocRow::new();
            map.insert("id".into(), json!(id));
            map.insert("rank_int".into(), json!(rank));
            map
        };
        let mut rows = vec![row("b", 2), row("a", 1), row("c", 3)];
        sort_rows(&mut rows, &OrderByExpr::default().asc("rank_int"));
        let ids: Vec<&str> = rows.iter().map(|row| row["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);

        sort_rows(&mut rows, &OrderByExpr::default().desc("rank_int"));
        let ids: Vec<&str> = rows.iter().map(|row| row["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["c", "b", "a"]);
    }

    #[test]
    fn tag_aggregation_splits_tag_kwd_on_hash_delimiters() {
        let rows = vec![
            {
                let mut map = DocRow::new();
                map.insert("tag_kwd".into(), json!("rust###search"));
                map
            },
            {
                let mut map = DocRow::new();
                map.insert("tag_kwd".into(), json!("rust,search"));
                map
            },
            {
                let mut map = DocRow::new();
                map.insert("tag_kwd".into(), json!(["rust", "infinity"]));
                map
            },
        ];
        let aggregated = tag_aggregation(&rows, "tag_kwd");
        assert_eq!(aggregated[0], ("rust".to_owned(), 3));
        assert_eq!(aggregated[1], ("search".to_owned(), 2));
        assert_eq!(aggregated[2], ("infinity".to_owned(), 1));
    }

    #[test]
    fn tag_aggregation_ignores_empty_and_missing_values() {
        let rows = vec![
            {
                let mut map = DocRow::new();
                map.insert("tag_kwd".into(), json!(""));
                map
            },
            {
                let mut map = DocRow::new();
                map.insert("tag_kwd".into(), json!("a"));
                map
            },
        ];
        assert_eq!(tag_aggregation(&rows, "tag_kwd"), vec![("a".to_owned(), 1)]);
        assert!(tag_aggregation(&rows, "missing").is_empty());
    }

    #[test]
    fn metadata_filter_translator_matches_python_output() {
        let clause = |op: &str, key: &str, value: Value| MetadataFilter {
            op: op.into(),
            key: key.into(),
            value,
        };
        assert_eq!(
            translate_metadata_filter(&clause("empty", "author", Value::Null)).unwrap(),
            "JSON_EXTRACT_STRING(meta_fields, '$.author') = '\"\"'"
        );
        assert_eq!(
            translate_metadata_filter(&clause("not empty", "author", Value::Null)).unwrap(),
            "JSON_EXTRACT_STRING(meta_fields, '$.author') != '\"\"'"
        );
        assert_eq!(
            translate_metadata_filter(&clause("=", "author", json!("Alice"))).unwrap(),
            "JSON_CONTAINS(meta_fields, '$.author', '\"Alice\"')"
        );
        assert_eq!(
            translate_metadata_filter(&clause("=", "year", json!(2024))).unwrap(),
            "JSON_CONTAINS(meta_fields, '$.year', 2024)"
        );
        assert_eq!(
            translate_metadata_filter(&clause(">", "year", json!("2024"))).unwrap(),
            "JSON_EXTRACT_DOUBLE(meta_fields, '$.year') > 2024"
        );
        assert_eq!(
            translate_metadata_filter(&clause(">", "author", json!("a"))).unwrap(),
            "JSON_EXTRACT_STRING(meta_fields, '$.author') > 'a'"
        );
        assert_eq!(
            translate_metadata_filter(&clause("contains", "tags", json!("rust"))).unwrap(),
            "JSON_CONTAINS(meta_fields, '$.tags', '\"rust\"')"
        );
        assert_eq!(
            translate_metadata_filter(&clause("start with", "name", json!("ru"))).unwrap(),
            "JSON_EXTRACT_STRING(meta_fields, '$.name') LIKE 'ru%'"
        );
        assert_eq!(
            translate_metadata_filter(&clause("end with", "name", json!("st"))).unwrap(),
            "JSON_EXTRACT_STRING(meta_fields, '$.name') LIKE '%st'"
        );
    }

    #[test]
    fn metadata_filter_in_and_not_in_partition_strings_and_numbers() {
        let clause = |op: &str, value: Value| MetadataFilter {
            op: op.into(),
            key: "year".into(),
            value,
        };
        assert_eq!(
            translate_metadata_filter(&clause("in", json!(["2024", "2025"]))).unwrap(),
            "((JSON_CONTAINS(meta_fields, '$.year', 2024) OR JSON_CONTAINS(meta_fields, '$.year', 2025)))"
        );
        assert_eq!(
            translate_metadata_filter(&clause("in", json!(["a", "b"]))).unwrap(),
            "((JSON_CONTAINS(meta_fields, '$.year', '\"a\"') OR JSON_CONTAINS(meta_fields, '$.year', '\"b\"')))"
        );
        assert_eq!(
            translate_metadata_filter(&clause("not in", json!(["a", "b"]))).unwrap(),
            "((NOT JSON_CONTAINS(meta_fields, '$.year', '\"a\"') AND NOT JSON_CONTAINS(meta_fields, '$.year', '\"b\"')))"
        );
    }

    #[test]
    fn metadata_filter_rejects_unknown_operators_and_bad_keys() {
        let bad_op = MetadataFilter {
            op: "matches".into(),
            key: "author".into(),
            value: Value::Null,
        };
        assert!(translate_metadata_filter(&bad_op).is_err());
        let bad_key = MetadataFilter {
            op: "=".into(),
            key: "bad key!".into(),
            value: json!("x"),
        };
        assert!(translate_metadata_filter(&bad_key).is_err());
        assert!(!is_pushdown_supported(&[bad_op.clone()]));
    }

    #[test]
    fn infinity_filter_builder_joins_with_requested_logic() {
        let clause = |op: &str, key: &str, value: Value| MetadataFilter {
            op: op.into(),
            key: key.into(),
            value,
        };
        let filters = vec![
            clause("=", "author", json!("Alice")),
            clause(">", "year", json!(2020)),
        ];
        assert_eq!(
            build_infinity_filter(&filters, "and").unwrap(),
            "(JSON_CONTAINS(meta_fields, '$.author', '\"Alice\"') AND JSON_EXTRACT_DOUBLE(meta_fields, '$.year') > 2020)"
        );
        assert_eq!(
            build_infinity_filter(&filters, "or").unwrap(),
            "(JSON_CONTAINS(meta_fields, '$.author', '\"Alice\"') OR JSON_EXTRACT_DOUBLE(meta_fields, '$.year') > 2020)"
        );
        assert_eq!(build_infinity_filter(&[], "and").unwrap(), "1=1");
        assert!(build_infinity_filter(&filters, "xor").is_err());
    }

    #[test]
    fn metadata_membership_values_are_lowercased_and_trimmed() {
        // Python `_csv_or_list` normalises string members to lower().strip().
        let clause = MetadataFilter {
            op: "in".into(),
            key: "tags".into(),
            value: json!(" Rust , Go "),
        };
        assert_eq!(
            translate_metadata_filter(&clause).unwrap(),
            "((JSON_CONTAINS(meta_fields, '$.tags', '\"rust\"') OR JSON_CONTAINS(meta_fields, '$.tags', '\"go\"')))"
        );
    }

    #[test]
    fn like_wildcards_are_escaped_for_start_and_end_with() {
        let clause = MetadataFilter {
            op: "start with".into(),
            key: "name".into(),
            value: json!("100%_real"),
        };
        assert_eq!(
            translate_metadata_filter(&clause).unwrap(),
            "JSON_EXTRACT_STRING(meta_fields, '$.name') LIKE '100\\%\\_real%'"
        );
    }
}
