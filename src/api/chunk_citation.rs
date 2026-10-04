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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn metadata(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn every_field_upstream_names_is_present() {
        let fields = citation_fields(
            "c1",
            "d1",
            "guide.md",
            "kb-1",
            "Retrieval augmented generation",
            &metadata(&[]),
        );
        for key in [
            "id",
            "document_id",
            "document_keyword",
            "dataset_id",
            "content_ltks",
            "image_id",
            "important_keywords",
            "tag_kwd",
            "positions",
            "url",
            "doc_type",
        ] {
            assert!(fields.contains_key(key), "missing {key}");
        }
        assert_eq!(fields["id"], json!("c1"));
        assert_eq!(fields["document_keyword"], json!("guide.md"));
        assert_eq!(fields["dataset_id"], json!("kb-1"));
        // Empty is the honest answer when the deployment stored nothing.
        assert_eq!(fields["positions"], json!([]));
        assert_eq!(fields["image_id"], json!(""));
        assert_eq!(fields["important_keywords"], json!([]));
        assert_eq!(fields["tag_kwd"], json!([]));
        // Content without stored tokens is still tokenised, not left empty.
        assert!(
            !fields["content_ltks"].as_str().unwrap().trim().is_empty(),
            "{fields:?}"
        );
    }

    #[test]
    fn stored_values_are_used_verbatim_and_parsed_when_they_are_json() {
        let fields = citation_fields(
            "c1",
            "d1",
            "guide.md",
            "kb-1",
            "text",
            &metadata(&[
                ("img_id", "img-9"),
                ("url", "https://example.cn/doc"),
                ("doc_type", "pdf"),
                ("content_ltks", "already tokenised"),
                ("important_kwd", "[\"alpha\",\"beta\"]"),
                ("tag_kwd", "gamma"),
                ("positions", "[[12,11,11,11,11]]"),
            ]),
        );
        assert_eq!(fields["image_id"], json!("img-9"));
        assert_eq!(fields["url"], json!("https://example.cn/doc"));
        assert_eq!(fields["doc_type"], json!("pdf"));
        assert_eq!(fields["content_ltks"], json!("already tokenised"));
        assert_eq!(fields["important_keywords"], json!(["alpha", "beta"]));
        assert_eq!(fields["tag_kwd"], json!(["gamma"]));
        assert_eq!(fields["positions"], json!([[12, 11, 11, 11, 11]]));
    }

    #[test]
    fn a_position_that_is_not_json_is_wrapped_rather_than_dropped() {
        assert_eq!(parse_positions("12,11,11"), json!([[12, 11, 11]]));
        assert_eq!(parse_positions(""), json!([]));
        assert_eq!(parse_positions("no numbers here"), json!([]));
        // Malformed JSON must not panic, and it must not be guessed at either: the numbers inside a
        // truncated tuple are not a position this deployment ever stored.
        assert_eq!(parse_positions("[[12,"), json!([]));
    }

    #[test]
    fn a_reference_reports_the_same_keys_from_its_metadata() {
        let mut document_metadata = Map::new();
        document_metadata.insert("url".into(), json!("https://example.cn/a"));
        document_metadata.insert("doc_type".into(), json!("markdown"));
        document_metadata.insert("img_id".into(), json!("img-1"));
        document_metadata.insert("positions".into(), json!([[1, 2, 3]]));
        let reference = crate::llm::ChunkReference {
            document_id: Some("d1".into()),
            document_name: Some("a.md".into()),
            id: "c1".into(),
            kb_id: "kb-1".into(),
            content: "text".into(),
            similarity: Some(0.9),
            vector_similarity: Some(0.8),
            term_similarity: Some(0.7),
            document_metadata: Some(document_metadata),
        };
        let fields = citation_fields_for_reference(&reference);
        assert_eq!(fields["id"], json!("c1"));
        assert_eq!(fields["document_name"], json!("a.md"));
        assert_eq!(fields["dataset_id"], json!("kb-1"));
        assert_eq!(fields["url"], json!("https://example.cn/a"));
        assert_eq!(fields["doc_type"], json!("markdown"));
        assert_eq!(fields["image_id"], json!("img-1"));
        assert_eq!(fields["positions"], json!([[1, 2, 3]]));

        // A reference with no metadata still answers every key.
        let bare = crate::llm::ChunkReference {
            document_id: None,
            document_name: None,
            id: "c2".into(),
            kb_id: "kb-1".into(),
            content: "text".into(),
            similarity: None,
            vector_similarity: None,
            term_similarity: None,
            document_metadata: None,
        };
        let fields = citation_fields_for_reference(&bare);
        assert_eq!(fields["positions"], json!([]));
        assert_eq!(fields["url"], json!(""));
        assert_eq!(fields["doc_type"], json!(""));
        assert_eq!(fields["document_name"], json!(""));
    }
}
