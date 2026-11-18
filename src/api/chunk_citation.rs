//! The citation fields a reference chunk carries, under upstream's names.
//!
//! Upstream's retrieval and chat reference chunks are consumed by clients that read a fixed set of
//! keys — the API guide shows them in both places:
//!
//! ```text
//! retrieval:     content, content_ltks, document_id, document_keyword, highlight, id, image_id,
//!                important_keywords, tag_kwd, dataset_id, positions, similarity, term_similarity,
//!                vector_similarity
//! chat/agent:    id, content, document_id, document_name, document_metadata, dataset_id, image_id,
//!                positions, url, similarity, vector_similarity, term_similarity, doc_type
//! ```
//!
//! RayRAG's own chunks carried `chunk_id`/`doc_id`/`kb_id`/`doc_name` instead, and the citation-only
//! fields (`positions`, `image_id`, `url`, `doc_type`, `content_ltks`, `important_keywords`,
//! `tag_kwd`) were simply absent, so a client written against RAGFlow had nothing to render a
//! citation with. This module builds that block once, from whatever the deployment actually stored.
//!
//! **Nothing here is invented.** A field is taken from a stored value and is empty otherwise —
//! upstream itself sends `"positions": []` and `"image_id": ""` when it has no position data, and an
//! empty list is a truthful answer while a fabricated coordinate is not.

use serde_json::{Map, Value, json};

/// Citation fields for a retrieval chunk, taken from the chunk's stored metadata.
///
/// `content_ltks` falls back to tokenising the content the way the keyword index does, so a chunk
/// indexed without stored tokens still reports something a client can use for highlighting.
pub fn citation_fields(
    chunk_id: &str,
    document_id: &str,
    document_name: &str,
    dataset_id: &str,
    content: &str,
    metadata: &std::collections::HashMap<String, String>,
) -> Map<String, Value> {
    let mut fields = Map::new();
    fields.insert("id".into(), json!(chunk_id));
    fields.insert("document_id".into(), json!(document_id));
    fields.insert("document_keyword".into(), json!(document_name));
    fields.insert("dataset_id".into(), json!(dataset_id));
    fields.insert(
        "content_ltks".into(),
        json!(
            metadata
                .get("content_ltks")
                .filter(|tokens| !tokens.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| crate::harness::knowlege_dataset_nav::tokenize(content))
        ),
    );
    fields.insert(
        "image_id".into(),
        json!(
            metadata
                .get("img_id")
                .or_else(|| metadata.get("image_id"))
                .cloned()
                .unwrap_or_default()
        ),
    );
    fields.insert(
        "important_keywords".into(),
        json!(string_list(metadata.get("important_kwd"))),
    );
    fields.insert(
        "tag_kwd".into(),
        json!(string_list(metadata.get("tag_kwd"))),
    );
    fields.insert("positions".into(), json!(stored_positions(metadata)));
    fields.insert(
        "url".into(),
        json!(metadata.get("url").cloned().unwrap_or_default()),
    );
    fields.insert(
        "doc_type".into(),
        json!(metadata.get("doc_type").cloned().unwrap_or_default()),
    );
    fields
}

/// Citation fields for a chat/agent reference, where the stored shape is the reference type itself
/// plus the document metadata map.
pub fn citation_fields_for_reference(reference: &crate::llm::ChunkReference) -> Map<String, Value> {
    let metadata = reference.document_metadata.clone().unwrap_or_default();
    let from_metadata = |keys: &[&str]| -> String {
        keys.iter()
            .find_map(|key| {
                metadata
                    .get(*key)
                    .and_then(|value| match value {
                        Value::String(text) => Some(text.clone()),
                        Value::Null => None,
                        other => Some(other.to_string()),
                    })
                    .filter(|text| !text.trim().is_empty())
            })
            .unwrap_or_default()
    };
    let document_name = reference
        .document_name
        .clone()
        .unwrap_or_else(|| from_metadata(&["document_name", "doc_name", "name"]));
    let mut fields = Map::new();
    fields.insert("id".into(), json!(reference.id));
    fields.insert(
        "document_id".into(),
        json!(reference.document_id.clone().unwrap_or_default()),
    );
    fields.insert("document_name".into(), json!(document_name));
    fields.insert("dataset_id".into(), json!(reference.kb_id));
    fields.insert(
        "image_id".into(),
        json!(from_metadata(&["image_id", "img_id"])),
    );
    fields.insert("url".into(), json!(from_metadata(&["url"])));
    fields.insert("doc_type".into(), json!(from_metadata(&["doc_type"])));
    fields.insert(
        "positions".into(),
        json!(positions_from_metadata(&metadata)),
    );
    fields
}

/// Positions as stored: an array of arrays stays as it is, a JSON string is parsed, and anything
/// else is `[]` rather than a guess.
fn stored_positions(metadata: &std::collections::HashMap<String, String>) -> Value {
    match metadata.get("positions") {
        Some(raw) => parse_positions(raw),
        None => json!([]),
    }
}

fn positions_from_metadata(metadata: &Map<String, Value>) -> Value {
    match metadata.get("positions") {
        Some(Value::String(raw)) => parse_positions(raw),
        Some(value @ Value::Array(_)) => value.clone(),
        _ => json!([]),
    }
}

fn parse_positions(raw: &str) -> Value {
    if raw.trim().is_empty() {
        return json!([]);
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(value @ Value::Array(_)) => value,
        // A stored position may also be a single comma-separated tuple; upstream's shape is a list of
        // tuples, so wrap it rather than dropping a position the deployment really has.
        _ => {
            let parts: Vec<Value> = raw
                .split(',')
                .filter_map(|part| part.trim().parse::<i64>().ok())
                .map(|number| json!(number))
                .collect();
            if parts.is_empty() {
                json!([])
            } else {
                json!([parts])
            }
        }
    }
}

/// A metadata value that is a JSON array of strings, a JSON string, or a bare string, as a list.
fn string_list(raw: Option<&String>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text),
                Value::Null => None,
                other => Some(other.to_string()),
            })
            .filter(|text| !text.trim().is_empty())
            .collect(),
        Ok(Value::String(text)) => vec![text],
        _ => vec![trimmed.to_string()],
    }
}


