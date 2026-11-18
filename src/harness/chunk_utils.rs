//! Shared chunk field accessors for the agentic-RAG harness — RAGFlow v0.27.2
//! `rag/advanced_rag/harness/chunk_utils.py`.
//!
//! Chunk rows reach the harness from several places (hybrid search, grep,
//! compiled-structure expansion, navigation outlines) and carry the same
//! fields under a few legacy aliases. These accessors are the single
//! definition used by the search and navigation tools.

use serde_json::Value;

fn str_of(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => {
            if *flag {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        other => other.to_string(),
    }
}

/// Python truthiness (`bool(x)`): null/false/zero/empty string/empty
/// container are falsy.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `_xml_escape`: escape a value for embedding in an XML attribute/element.
pub fn xml_escape(value: &Value) -> String {
    str_of(value)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `_chunk_text`: chunk body text across the historical content aliases
/// (`content_with_weight` or `content` or `text`) — full Python falsiness.
pub fn chunk_text(chunk: &Value) -> String {
    for key in ["content_with_weight", "content", "text"] {
        if let Some(value) = chunk.get(key)
            && truthy(value)
        {
            return str_of(value);
        }
    }
    String::new()
}

/// `_chunk_attr`: first value among `keys` that is not `None`/`""`.
pub fn chunk_attr(chunk: &Value, keys: &[&str]) -> String {
    for key in keys {
        match chunk.get(*key) {
            None | Some(Value::Null) => continue,
            Some(Value::String(text)) if text.is_empty() => continue,
            Some(value) => return str_of(value),
        }
    }
    String::new()
}

/// `_doc_id`.
pub fn doc_id(chunk: &Value) -> String {
    chunk_attr(chunk, &["doc_id", "docid", "document_id"])
}

/// `_dataset_id`.
pub fn dataset_id(chunk: &Value) -> String {
    chunk_attr(chunk, &["dataset_id", "kb_id", "knowledgebase_id"])
}

/// `_doc_title`.
pub fn doc_title(chunk: &Value) -> String {
    chunk_attr(chunk, &["docnm_kwd", "doc_title", "title", "document_name"])
}

/// `_chunk_id` (always a string, so mixed str/int ids still dedup).
pub fn chunk_id(chunk: &Value) -> String {
    chunk_attr(chunk, &["chunk_id", "id"])
}

/// `_snippet`: truncate to `n` chars on a char boundary with an ellipsis.
pub fn snippet(text: &str, n: usize) -> String {
    let trimmed = text.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= n {
        return trimmed.to_string();
    }
    let head: String = chars[..n].iter().collect();
    format!("{}...", head.trim_end())
}


