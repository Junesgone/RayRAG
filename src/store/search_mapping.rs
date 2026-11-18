//! Executable replacement contract for RAGFlow's chunk search mappings.
//!
//! The fixed Elasticsearch and OpenSearch JSON files are represented exactly
//! as typed JSON builders. RayRAG does not create either native index; the same
//! field rules validate the JSON/PostgreSQL snapshot before it is mirrored to
//! zvec-rust.

use crate::Result;
use crate::search::IndexedChunk;
use crate::settings::{
    ES_CHUNK_KEYWORD_PATTERN, ES_CHUNK_VECTOR_DIMENSIONS, FieldKind,
    OPENSEARCH_CHUNK_VECTOR_DIMENSIONS, SEARCH_CHUNK_DATE_DETECTION, SEARCH_CHUNK_DATE_FORMAT,
    SEARCH_CHUNK_DATE_PATTERN, SEARCH_CHUNK_NUMBER_OF_REPLICAS, SEARCH_CHUNK_NUMBER_OF_SHARDS,
    SEARCH_CHUNK_REFRESH_INTERVAL, SEARCH_CHUNK_SCRIPTED_SIMILARITY, SearchMappingFlavor,
    search_field_kind,
};
use serde_json::{Map, Number, Value, json};

/// Construct the parsed value of the fixed mapping file for one engine.
///
/// JSON string-vs-boolean quirks are intentional: `date_detection`, the first
/// four `store` flags, `object.dynamic`, and unindexed `index` are strings in
/// the fixed sources.
pub fn fixed_search_mapping(flavor: SearchMappingFlavor) -> Value {
    let scripted_similarity = json!({
        "scripted_sim": {
            "type": "scripted",
            "script": { "source": SEARCH_CHUNK_SCRIPTED_SIMILARITY }
        }
    });
    let settings = match flavor {
        SearchMappingFlavor::Elasticsearch => json!({
            "index": {
                "number_of_shards": SEARCH_CHUNK_NUMBER_OF_SHARDS,
                "number_of_replicas": SEARCH_CHUNK_NUMBER_OF_REPLICAS,
                "refresh_interval": SEARCH_CHUNK_REFRESH_INTERVAL
            },
            "similarity": scripted_similarity
        }),
        SearchMappingFlavor::OpenSearch => json!({
            "index": {
                "number_of_shards": SEARCH_CHUNK_NUMBER_OF_SHARDS,
                "number_of_replicas": SEARCH_CHUNK_NUMBER_OF_REPLICAS,
                "refresh_interval": SEARCH_CHUNK_REFRESH_INTERVAL,
                "knn": true,
                "similarity": scripted_similarity
            }
        }),
    };
    json!({
        "settings": settings,
        "mappings": {
            "date_detection": SEARCH_CHUNK_DATE_DETECTION,
            "dynamic_templates": dynamic_templates(flavor),
            "properties": {
                "lat_lon": { "type": "geo_point", "store": "true" }
            }
        }
    })
}

fn dynamic_templates(flavor: SearchMappingFlavor) -> Vec<Value> {
    let keyword_pattern = match flavor {
        SearchMappingFlavor::Elasticsearch => ES_CHUNK_KEYWORD_PATTERN,
        SearchMappingFlavor::OpenSearch => crate::settings::OPENSEARCH_CHUNK_KEYWORD_PATTERN,
    };
    let mut templates = vec![
        json!({"int": {"match": "*_int", "mapping": {"type": "integer", "store": "true"}}}),
        json!({"ulong": {"match": "*_ulong", "mapping": {"type": "unsigned_long", "store": "true"}}}),
        json!({"long": {"match": "*_long", "mapping": {"type": "long", "store": "true"}}}),
        json!({"short": {"match": "*_short", "mapping": {"type": "short", "store": "true"}}}),
        json!({"numeric": {"match": "*_flt", "mapping": {"type": "float", "store": true}}}),
        json!({"tks": {"match": "*_tks", "mapping": {"type": "text", "similarity": "scripted_sim", "analyzer": "whitespace", "store": true}}}),
        json!({"ltks": {"match": "*_ltks", "mapping": {"type": "text", "analyzer": "whitespace", "store": true}}}),
        json!({"kwd": {"match_pattern": "regex", "match": keyword_pattern, "mapping": {"type": "keyword", "similarity": "boolean", "store": true}}}),
        json!({"dt": {"match_pattern": "regex", "match": SEARCH_CHUNK_DATE_PATTERN, "mapping": {"type": "date", "format": SEARCH_CHUNK_DATE_FORMAT, "store": true}}}),
        json!({"nested": {"match": "*_nst", "mapping": {"type": "nested"}}}),
        json!({"object": {"match": "*_obj", "mapping": {"type": "object", "dynamic": "true"}}}),
        json!({"string": {"match_pattern": "regex", "match": "^.*_(with_weight|list)$", "mapping": {"type": "text", "index": "false", "store": true}}}),
        json!({"rank_feature": {"match": "*_fea", "mapping": {"type": "rank_feature"}}}),
        json!({"rank_features": {"match": "*_feas", "mapping": {"type": "rank_features"}}}),
    ];
    let dimensions = match flavor {
        SearchMappingFlavor::Elasticsearch => ES_CHUNK_VECTOR_DIMENSIONS,
        SearchMappingFlavor::OpenSearch => OPENSEARCH_CHUNK_VECTOR_DIMENSIONS,
    };
    for dimension in dimensions {
        let pattern = format!("*_{dimension}_vec");
        templates.push(match flavor {
            SearchMappingFlavor::Elasticsearch => json!({
                "dense_vector": {
                    "match": pattern,
                    "mapping": {
                        "type": "dense_vector", "index": true,
                        "similarity": "cosine", "dims": dimension
                    }
                }
            }),
            SearchMappingFlavor::OpenSearch => json!({
                "knn_vector": {
                    "match": pattern,
                    "mapping": {
                        "type": "knn_vector", "index": true,
                        "space_type": "cosinesimil", "dimension": dimension
                    }
                }
            }),
        });
    }
    templates.push(json!({"binary": {"match": "*_bin", "mapping": {"type": "binary"}}}));
    templates
}

/// Project one RayRAG chunk into the value types accepted by a fixed native
/// mapping. The returned document is diagnostic/validation data; JSON remains
/// the source of truth and zvec stores the vector in its own typed column.
pub fn project_chunk_document(
    flavor: SearchMappingFlavor,
    chunk: &IndexedChunk,
) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    for (name, raw) in &chunk.metadata {
        fields.insert(name.clone(), coerce_field(flavor, name, raw)?);
    }
    fields.insert("id".into(), Value::String(chunk.id.clone()));
    fields.insert("docnm_kwd".into(), Value::String(chunk.doc_name.clone()));
    fields.insert(
        "content_with_weight".into(),
        Value::String(chunk.content.clone()),
    );
    fields.insert("content_ltks".into(), Value::String(chunk.content.clone()));
    fields.insert(
        "token_num".into(),
        Value::Number(Number::from(u64::try_from(chunk.token_count)?)),
    );

    let dimension = u64::try_from(chunk.embedding.len())?;
    let native_dimensions = match flavor {
        SearchMappingFlavor::Elasticsearch => ES_CHUNK_VECTOR_DIMENSIONS,
        SearchMappingFlavor::OpenSearch => OPENSEARCH_CHUNK_VECTOR_DIMENSIONS,
    };
    if !chunk.embedding.is_empty() && native_dimensions.contains(&dimension) {
        fields.insert(
            format!("q_{dimension}_vec"),
            Value::Array(
                chunk
                    .embedding
                    .iter()
                    .map(|value| finite_number(f64::from(*value)).map(Value::Number))
                    .collect::<Result<Vec<_>>>()?,
            ),
        );
    }
    Ok(fields)
}

/// Validate all runtime fields against both fixed mappings before an online
/// snapshot commit. Unsupported native vector dimensions remain legal in the
/// RayRAG replacement and are stored only in zvec/JSON.
pub fn validate_replacement_snapshot(chunks: &[IndexedChunk]) -> Result<()> {
    for chunk in chunks {
        project_chunk_document(SearchMappingFlavor::Elasticsearch, chunk)?;
        project_chunk_document(SearchMappingFlavor::OpenSearch, chunk)?;
    }
    Ok(())
}

fn coerce_field(flavor: SearchMappingFlavor, name: &str, raw: &str) -> Result<Value> {
    let kind = search_field_kind(flavor, name);
    match kind {
        FieldKind::Int | FieldKind::Long | FieldKind::Short => {
            let value = parse_json_or_string(raw);
            validate_integer_value(name, &value)?;
            Ok(value)
        }
        FieldKind::Ulong => {
            let value = parse_json_or_string(raw);
            validate_unsigned_value(name, &value)?;
            Ok(value)
        }
        FieldKind::Float | FieldKind::RankFeature => {
            let value = parse_json_or_string(raw);
            validate_number_value(name, &value)?;
            Ok(value)
        }
        FieldKind::Keyword => {
            let value = parse_json_or_string(raw);
            validate_keyword_value(name, &value)?;
            Ok(value)
        }
        FieldKind::Nested => {
            let value: Value = serde_json::from_str(raw)?;
            if !value.is_array() {
                anyhow::bail!("{name} must contain a JSON array for a nested field");
            }
            Ok(value)
        }
        FieldKind::Object | FieldKind::RankFeatures => {
            let value: Value = serde_json::from_str(raw)?;
            if !value.is_object() {
                anyhow::bail!("{name} must contain a JSON object");
            }
            if kind == FieldKind::RankFeatures {
                validate_number_value(name, &value)?;
            }
            Ok(value)
        }
        FieldKind::DenseVector(dimension) => {
            let value: Value = serde_json::from_str(raw)?;
            let Some(values) = value.as_array() else {
                anyhow::bail!("{name} must contain a JSON vector");
            };
            if values.len() != usize::try_from(dimension)? {
                anyhow::bail!(
                    "{name} vector dimension mismatch: expected {dimension}, got {}",
                    values.len()
                );
            }
            validate_number_value(name, &value)?;
            Ok(value)
        }
        FieldKind::GeoPoint => {
            let value = parse_json_or_string(raw);
            validate_geo_point(name, &value)?;
            Ok(value)
        }
        FieldKind::Tokens
        | FieldKind::LongTokens
        | FieldKind::DateTime
        | FieldKind::UnindexedText
        | FieldKind::Binary
        | FieldKind::Text => Ok(Value::String(raw.to_owned())),
    }
}

fn parse_json_or_string(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

fn validate_integer_value(name: &str, value: &Value) -> Result<()> {
    match value {
        Value::Number(number) if number.is_i64() || number.is_u64() => Ok(()),
        Value::String(raw) => raw
            .parse::<i64>()
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("{name} must contain an integer")),
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| validate_integer_value(name, value)),
        _ => anyhow::bail!("{name} must contain an integer or integer array"),
    }
}

fn validate_number_value(name: &str, value: &Value) -> Result<()> {
    match value {
        Value::Number(number) if number.as_f64().is_some_and(f64::is_finite) => Ok(()),
        Value::String(raw) => raw
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|_| ())
            .ok_or_else(|| anyhow::anyhow!("{name} must contain a finite number")),
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| validate_number_value(name, value)),
        Value::Object(values) => values
            .values()
            .try_for_each(|value| validate_number_value(name, value)),
        _ => anyhow::bail!("{name} must contain finite numeric values"),
    }
}

fn validate_unsigned_value(name: &str, value: &Value) -> Result<()> {
    match value {
        Value::Number(number) if number.is_u64() => Ok(()),
        Value::String(raw) => raw
            .parse::<u64>()
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("{name} must contain an unsigned integer")),
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| validate_unsigned_value(name, value)),
        _ => anyhow::bail!("{name} must contain an unsigned integer or array"),
    }
}

fn validate_geo_point(name: &str, value: &Value) -> Result<()> {
    match value {
        Value::String(raw) if !raw.trim().is_empty() => Ok(()),
        Value::Array(values) if values.len() == 2 => validate_number_value(name, value),
        Value::Object(values) if values.contains_key("lat") && values.contains_key("lon") => {
            validate_number_value(name, &values["lat"])?;
            validate_number_value(name, &values["lon"])
        }
        _ => anyhow::bail!("{name} must contain a geo-point string, [lon,lat], or lat/lon object"),
    }
}

fn validate_keyword_value(name: &str, value: &Value) -> Result<()> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| validate_keyword_value(name, value)),
        _ => anyhow::bail!("{name} must contain a scalar or scalar array"),
    }
}

fn finite_number(value: f64) -> Result<Number> {
    Number::from_f64(value).ok_or_else(|| anyhow::anyhow!("embedding contains a non-finite value"))
}


