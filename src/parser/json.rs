//! JSON parser — flatten JSON objects/arrays into key-value text.
//! Handles nested structures by flattening paths.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};

#[derive(Default)]
pub struct JsonParser;

impl JsonParser {
    pub fn new() -> Self {
        Self
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        let value: serde_json::Value = serde_json::from_slice(data)?;
        Ok(flatten_json(&value, ""))
    }
}

impl Parse for JsonParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(data)?;
        Ok(new_document(name, content, "application/json", data.len()))
    }
}

/// Flatten a JSON value into key: value text lines.
fn flatten_json(value: &serde_json::Value, prefix: &str) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut text = String::new();
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };
                text.push_str(&flatten_json(v, &path));
                text.push('\n');
            }
            text
        }
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                return format!("{}: []", prefix);
            }
            // If array of objects, expand each
            if arr.iter().all(|v| v.is_object()) {
                let mut text = String::new();
                for (i, v) in arr.iter().enumerate() {
                    text.push_str(&flatten_json(v, &format!("{}[{}]", prefix, i)));
                    text.push('\n');
                }
                text
            } else {
                // Simple array: join values
                let values: Vec<String> = arr.iter().map(value_to_string).collect();
                format!("{}: [{}]", prefix, values.join(", "))
            }
        }
        _ => format!("{}: {}", prefix, value_to_string(value)),
    }
}

fn value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        _ => value.to_string(),
    }
}
