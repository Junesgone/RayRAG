//! Canonical codec for the fixed RAGFlow v0.26.4 Infinity chunk mapping.
//!
//! RayRAG does not deploy Infinity. This module keeps its JSON/PostgreSQL
//! snapshot and zvec metadata projection faithful to Infinity's ordered base
//! schema while search remains RayRAG's documented Rust replacement.

use crate::settings::{
    INFINITY_CHUNK_FIELDS, InfinityChunkDefault, InfinityChunkFieldSchema,
    infinity_chunk_vector_field,
};
use crate::{Chunk, Result};
use serde_json::{Map, Number, Value};
use std::collections::HashMap;

/// Materialize one chunk as an Infinity-shaped physical record.
///
/// All 73 fixed columns receive their declared defaults. The current embedding
/// is emitted as the one legal dynamic `q_{dimension}_vec` column. Alias
/// precedence follows the fixed Python/Go connector contract.
pub fn encode_chunk_record(chunk: &Chunk, kb_id: &str) -> Result<Map<String, Value>> {
    let mut record = Map::with_capacity(INFINITY_CHUNK_FIELDS.len() + 2);
    for field in INFINITY_CHUNK_FIELDS {
        record.insert(field.name.into(), default_value(field.default));
    }

    for (name, raw) in &chunk.metadata {
        if let Some(field) = INFINITY_CHUNK_FIELDS
            .iter()
            .find(|field| field.name == name)
        {
            insert_physical(&mut record, field, raw)?;
        }
    }

    project_aliases(&mut record, &chunk.metadata)?;
    if record
        .get("docnm")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .is_empty()
        && let Some(file_name) = chunk.metadata.get("file_name")
    {
        record.insert("docnm".into(), Value::String(file_name.clone()));
    }

    record.insert("id".into(), Value::String(chunk.id.clone()));
    record.insert("doc_id".into(), Value::String(chunk.doc_id.to_string()));
    record.insert("kb_id".into(), Value::String(kb_id.to_owned()));
    record.insert("content".into(), Value::String(chunk.content.clone()));
    record.insert(
        "chunk_order_int".into(),
        Value::Number(Number::from(u64::try_from(chunk.position)?)),
    );
    record.insert(
        "token_num".into(),
        Value::Number(Number::from(u64::try_from(chunk.token_count)?)),
    );
    if let Some(embedding) = &chunk.embedding {
        record.insert(
            infinity_chunk_vector_field(embedding.len()),
            Value::Array(
                embedding
                    .iter()
                    .map(|value| {
                        Number::from_f64(f64::from(*value))
                            .map(Value::Number)
                            .ok_or_else(|| anyhow::anyhow!("embedding contains a non-finite value"))
                    })
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    if let Some(raw) = chunk.metadata.get("chunk_data") {
        record.insert("chunk_data".into(), parse_json_or_string(raw));
    }
    Ok(record)
}

/// Return the non-default physical fields suitable for `IndexedChunk.metadata`.
/// Database defaults remain schema-driven instead of bloating every snapshot.
pub fn sparse_record_metadata(record: &Map<String, Value>) -> HashMap<String, String> {
    let mut metadata = HashMap::new();
    for field in INFINITY_CHUNK_FIELDS {
        let Some(value) = record.get(field.name) else {
            continue;
        };
        let retain_default = matches!(field.name, "kb_id" | "available_int");
        if retain_default || *value != default_value(field.default) {
            metadata.insert(field.name.into(), metadata_string(value));
        }
    }
    if let Some(value) = record.get("chunk_data") {
        metadata.insert("chunk_data".into(), metadata_string(value));
    }
    metadata
}

/// Decode physical fields stored in the sparse snapshot back to RayRAG's
/// logical metadata representation.
pub fn decode_sparse_metadata(
    metadata: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    let mut decoded = metadata.clone();

    if let Some(value) = decoded.remove("docnm") {
        for alias in ["docnm_kwd", "title_tks", "title_sm_tks"] {
            decoded.insert(alias.into(), value.clone());
        }
    }
    if let Some(value) = decoded.remove("important_keywords") {
        let mut keywords = if value.is_empty() {
            Vec::new()
        } else {
            value.split(',').map(str::to_owned).collect()
        };
        let empty_count = decoded
            .remove("important_kwd_empty_count")
            .map(|raw| raw.parse::<usize>())
            .transpose()?
            .unwrap_or(0);
        keywords.extend(std::iter::repeat_n(String::new(), empty_count));
        decoded.insert("important_kwd".into(), serde_json::to_string(&keywords)?);
        decoded.insert("important_tks".into(), value);
    }
    if let Some(value) = decoded.remove("questions") {
        let questions = if value.is_empty() {
            Vec::new()
        } else {
            value.lines().map(str::to_owned).collect::<Vec<_>>()
        };
        decoded.insert("question_kwd".into(), serde_json::to_string(&questions)?);
        decoded.insert("question_tks".into(), value);
    }
    if let Some(value) = decoded.remove("authors") {
        decoded.insert("authors_tks".into(), value.clone());
        decoded.insert("authors_sm_tks".into(), value);
    }
    for field in INFINITY_CHUNK_FIELDS {
        if is_keyword_field(field.name)
            && let Some(value) = decoded.get_mut(field.name)
        {
            let values = if value.is_empty() {
                Vec::new()
            } else {
                value.split("###").map(str::to_owned).collect::<Vec<_>>()
            };
            *value = serde_json::to_string(&values)?;
        }
    }
    for field in ["position_int", "page_num_int", "top_int"] {
        if let Some(value) = decoded.get_mut(field) {
            let group = (field == "position_int").then_some(5);
            *value = serde_json::to_string(&decode_hex_array(value, group)?)?;
        }
    }
    Ok(decoded)
}

fn project_aliases(
    record: &mut Map<String, Value>,
    metadata: &HashMap<String, String>,
) -> Result<()> {
    for field in INFINITY_CHUNK_FIELDS
        .iter()
        .filter(|field| field.comment.is_some())
    {
        if metadata.contains_key(field.name) {
            continue;
        }
        let Some((alias, raw)) = field.comment.and_then(|comment| {
            comment
                .split(',')
                .map(str::trim)
                .find_map(|alias| metadata.get(alias).map(|value| (alias, value)))
        }) else {
            continue;
        };
        let value = match field.name {
            "important_keywords" if alias == "important_kwd" => {
                let values = string_list(raw);
                let empty_count = values.iter().filter(|value| value.is_empty()).count();
                record.insert(
                    "important_kwd_empty_count".into(),
                    Value::Number(Number::from(u64::try_from(empty_count)?)),
                );
                Value::String(
                    values
                        .into_iter()
                        .filter(|value| !value.is_empty())
                        .collect::<Vec<_>>()
                        .join(","),
                )
            }
            "questions" if alias == "question_kwd" => Value::String(string_list(raw).join("\n")),
            _ => Value::String(raw.clone()),
        };
        record.insert(field.name.into(), value);
    }
    Ok(())
}

fn insert_physical(
    record: &mut Map<String, Value>,
    field: &InfinityChunkFieldSchema,
    raw: &str,
) -> Result<()> {
    let value = match field.default {
        InfinityChunkDefault::Integer(_) => Value::Number(Number::from(raw.parse::<i64>()?)),
        InfinityChunkDefault::Float(_) => {
            let parsed = raw.parse::<f64>()?;
            Value::Number(
                Number::from_f64(parsed)
                    .ok_or_else(|| anyhow::anyhow!("{} must contain a finite float", field.name))?,
            )
        }
        InfinityChunkDefault::Text(_) => encode_text_field(field, raw)?,
    };
    record.insert(field.name.into(), value);
    Ok(())
}

fn encode_text_field(field: &InfinityChunkFieldSchema, raw: &str) -> Result<Value> {
    if matches!(field.name, "position_int" | "page_num_int" | "top_int")
        && let Ok(value) = serde_json::from_str::<Value>(raw)
        && value.is_array()
    {
        return Ok(Value::String(encode_hex_array(&value)?));
    }
    if field.name == "kb_id"
        && let Ok(Value::Array(values)) = serde_json::from_str::<Value>(raw)
    {
        return Ok(values
            .into_iter()
            .next()
            .unwrap_or(Value::String(String::new())));
    }
    if field.name == "important_keywords" {
        let values = string_list(raw);
        return Ok(Value::String(values.join(",")));
    }
    if field.name == "questions" {
        return Ok(Value::String(string_list(raw).join("\n")));
    }
    if is_keyword_field(field.name)
        && let Ok(Value::Array(_)) = serde_json::from_str::<Value>(raw)
    {
        return Ok(Value::String(string_list(raw).join("###")));
    }
    if field.name == "extra" || field.name.ends_with("_feas") {
        return Ok(Value::String(metadata_string(&parse_json_or_string(raw))));
    }
    Ok(Value::String(raw.to_owned()))
}

fn default_value(default: InfinityChunkDefault) -> Value {
    match default {
        InfinityChunkDefault::Text(value) => Value::String(value.into()),
        InfinityChunkDefault::Integer(value) => Value::Number(Number::from(value)),
        InfinityChunkDefault::Float(value) => {
            Value::Number(Number::from_f64(value).expect("fixed Infinity defaults must be finite"))
        }
    }
}

fn is_keyword_field(name: &str) -> bool {
    name == "source_id"
        || (name.ends_with("_kwd")
            && !matches!(
                name,
                "knowledge_graph_kwd" | "docnm_kwd" | "important_kwd" | "question_kwd"
            ))
}

fn string_list(raw: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Array(values)) => values
            .into_iter()
            .map(|value| match value {
                Value::String(value) => value,
                other => metadata_string(&other),
            })
            .collect(),
        _ => vec![raw.to_owned()],
    }
}

fn encode_hex_array(value: &Value) -> Result<String> {
    fn flatten(value: &Value, output: &mut Vec<u64>) -> Result<()> {
        match value {
            Value::Array(values) => {
                for value in values {
                    flatten(value, output)?;
                }
            }
            Value::Number(value) => output.push(value.as_u64().ok_or_else(|| {
                anyhow::anyhow!("Infinity coordinates must be unsigned integers")
            })?),
            _ => anyhow::bail!("Infinity coordinates must be integer arrays"),
        }
        Ok(())
    }

    let mut values = Vec::new();
    flatten(value, &mut values)?;
    Ok(values
        .into_iter()
        .map(|value| format!("{value:08x}"))
        .collect::<Vec<_>>()
        .join("_"))
}

fn decode_hex_array(raw: &str, group: Option<usize>) -> Result<Value> {
    let values = if raw.is_empty() {
        Vec::new()
    } else {
        raw.split('_')
            .map(|value| {
                u64::from_str_radix(value, 16)
                    .map(Number::from)
                    .map(Value::Number)
            })
            .collect::<std::result::Result<Vec<_>, _>>()?
    };
    Ok(match group {
        Some(size) => Value::Array(
            values
                .chunks(size)
                .map(|row| Value::Array(row.to_vec()))
                .collect(),
        ),
        None => Value::Array(values),
    })
}

fn parse_json_or_string(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

fn metadata_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{InfinityChunkAnalyzer, infinity_chunk_field_for_alias};
    use uuid::Uuid;

    #[test]
    fn fixed_schema_codec_materializes_defaults_aliases_and_dynamic_vector() {
        let chunk = Chunk {
            id: "chunk-1".into(),
            content: "canonical content".into(),
            content_type: "table".into(),
            doc_id: Uuid::nil(),
            position: 7,
            token_count: 13,
            embedding: Some(vec![1.0, 0.5]),
            metadata: HashMap::from([
                ("docnm_kwd".into(), "manual.pdf".into()),
                ("title_tks".into(), "lower priority".into()),
                ("important_kwd".into(), r#"["","rust",""]"#.into()),
                ("question_kwd".into(), r#"["one","two"]"#.into()),
                ("tag_kwd".into(), r#"["a","b"]"#.into()),
                ("position_int".into(), "[[1,10,100,20,40]]".into()),
                ("page_num_int".into(), "[1,2]".into()),
                ("tag_feas".into(), r#"{"rust":0.75}"#.into()),
                ("extra".into(), r#"{"kind":"raptor"}"#.into()),
                ("chunk_data".into(), r#"{"cells":["a"]}"#.into()),
            ]),
        };

        let record = encode_chunk_record(&chunk, "kb-a").unwrap();
        assert_eq!(record.len(), 75); // 73 base + vector + chunk_data.
        assert_eq!(record["available_int"], 1);
        assert_eq!(record["weight_flt"], 0.0);
        assert_eq!(record["docnm"], "manual.pdf");
        assert_eq!(record["important_keywords"], "rust");
        assert_eq!(record["important_kwd_empty_count"], 2);
        assert_eq!(record["questions"], "one\ntwo");
        assert_eq!(record["tag_kwd"], "a###b");
        assert_eq!(
            record["position_int"],
            "00000001_0000000a_00000064_00000014_00000028"
        );
        assert_eq!(record["page_num_int"], "00000001_00000002");
        assert_eq!(record["q_2_vec"], serde_json::json!([1.0, 0.5]));
        assert_eq!(record["chunk_data"], serde_json::json!({"cells": ["a"]}));

        let sparse = sparse_record_metadata(&record);
        assert!(!sparse.contains_key("weight_int"));
        assert_eq!(sparse.get("available_int").map(String::as_str), Some("1"));
        let decoded = decode_sparse_metadata(&sparse).unwrap();
        assert_eq!(
            decoded.get("important_kwd").map(String::as_str),
            Some(r#"["rust","",""]"#)
        );
        assert_eq!(
            decoded.get("question_kwd").map(String::as_str),
            Some(r#"["one","two"]"#)
        );
        assert_eq!(
            decoded.get("position_int").map(String::as_str),
            Some("[[1,10,100,20,40]]")
        );
    }

    #[test]
    fn codec_rejects_invalid_typed_or_coordinate_values() {
        let base = Chunk {
            id: "bad".into(),
            content: String::new(),
            content_type: "text".into(),
            doc_id: Uuid::nil(),
            position: 0,
            token_count: 0,
            embedding: None,
            metadata: HashMap::from([("available_int".into(), "yes".into())]),
        };
        assert!(encode_chunk_record(&base, "kb").is_err());

        let mut bad_position = base;
        bad_position.metadata = HashMap::from([("position_int".into(), "[[1,-1]]".into())]);
        assert!(encode_chunk_record(&bad_position, "kb").is_err());
        assert_eq!(
            infinity_chunk_field_for_alias("content_ltks"),
            Some("content")
        );
        assert!(matches!(
            INFINITY_CHUNK_FIELDS[9].analyzer,
            InfinityChunkAnalyzer::Single("whitespace-#")
        ));
    }
}
