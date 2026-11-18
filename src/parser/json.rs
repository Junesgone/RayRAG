//! JSON parser — full port of RAGFlow `deepdoc/parser/json_parser.py`
//! (adapted from langchain_text_splitters/json.py).
//!
//! `RAGFlowJsonParser` splits JSON into size-bounded chunks while preserving
//! structure: `max_chunk_size = chunk_token_num * 2`,
//! `min_chunk_size = max(max_chunk_size - 200, 50)`. Lists are converted to
//! index-keyed dicts before splitting (`convert_lists`). The document-level
//! entry point detects JSONL (sampled lines ≥ 80% valid JSON after whole-text
//! parse fails) and parses either mode into compact JSON strings.

use crate::parser::{Parse, new_document};
use crate::{Document, Result};
use serde_json::{Map, Value};

/// Port of `RAGFlowJsonParser` (json_parser.py:27-179).
pub struct RAGFlowJsonParser {
    max_chunk_size: usize,
    min_chunk_size: usize,
}

impl Default for RAGFlowJsonParser {
    fn default() -> Self {
        Self::new(2000, None)
    }
}

impl RAGFlowJsonParser {
    /// `__init__` — json_parser.py:28-31.
    pub fn new(max_chunk_size: usize, min_chunk_size: Option<usize>) -> Self {
        let max_chunk_size = max_chunk_size * 2;
        let min_chunk_size =
            min_chunk_size.unwrap_or_else(|| max_chunk_size.saturating_sub(200).max(50));
        Self {
            max_chunk_size,
            min_chunk_size,
        }
    }

    /// `_json_size` — json_parser.py:44-46. Compact serialization, UTF-8 kept.
    fn json_size(data: &Value) -> usize {
        serde_json::to_string(data).map(|s| s.len()).unwrap_or(0)
    }

    /// `_set_nested_dict` — json_parser.py:49-53.
    fn set_nested_dict(d: &mut Map<String, Value>, path: &[String], value: Value) {
        if path.is_empty() {
            return;
        }
        let mut cur = d;
        for key in &path[..path.len() - 1] {
            cur = cur
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Map::new()))
                .as_object_mut()
                .expect("nested path key must be an object");
        }
        cur.insert(path[path.len() - 1].clone(), value);
    }

    /// `_list_to_dict_preprocessing` — json_parser.py:55-64.
    fn list_to_dict_preprocessing(data: &Value) -> Value {
        match data {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Self::list_to_dict_preprocessing(v)))
                    .collect(),
            ),
            Value::Array(arr) => Value::Object(
                arr.iter()
                    .enumerate()
                    .map(|(i, item)| (i.to_string(), Self::list_to_dict_preprocessing(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }

    /// `_json_split` — json_parser.py:66-97. Faithful recursive port: dict
    /// keys are visited in order; a key that fits goes into the current
    /// chunk, one that does not triggers a new chunk (when the current one is
    /// big enough) and recurses. Mirrors langchain's original, which may let
    /// the final fitted key push a chunk slightly past max_chunk_size.
    fn json_split(&self, data: &Value, current_path: &[String], chunks: &mut Vec<Value>) {
        match data {
            Value::Object(map) => {
                for (key, value) in map {
                    let mut new_path = current_path.to_vec();
                    new_path.push(key.clone());
                    let chunk_size = Self::json_size(chunks.last().unwrap());
                    let size = Self::json_size(&Value::Object(Map::from_iter([(
                        key.clone(),
                        value.clone(),
                    )])));
                    let remaining = self.max_chunk_size.saturating_sub(chunk_size);
                    if size < remaining {
                        let last = chunks.last_mut().unwrap().as_object_mut().unwrap();
                        Self::set_nested_dict(last, &new_path, value.clone());
                    } else {
                        if chunk_size >= self.min_chunk_size {
                            chunks.push(Value::Object(Map::new()));
                        }
                        self.json_split(value, &new_path, chunks);
                    }
                }
            }
            other => {
                // Single item: set at current path.
                let last = chunks.last_mut().unwrap().as_object_mut().unwrap();
                Self::set_nested_dict(last, current_path, other.clone());
            }
        }
    }

    /// `split_json` — json_parser.py:99-115.
    pub fn split_json(&self, json_data: &Value, convert_lists: bool) -> Vec<Value> {
        let data = if convert_lists {
            Self::list_to_dict_preprocessing(json_data)
        } else {
            json_data.clone()
        };
        let mut chunks: Vec<Value> = vec![Value::Object(Map::new())];
        self.json_split(&data, &[], &mut chunks);
        if chunks.last().map(is_empty_value).unwrap_or(false) {
            chunks.pop();
        }
        chunks
    }

    /// `split_text` — json_parser.py:117-128. Compact JSON strings (spaces
    /// removed) with the requested ASCII escaping.
    pub fn split_text(
        &self,
        json_data: &Value,
        convert_lists: bool,
        ensure_ascii: bool,
    ) -> Vec<String> {
        self.split_json(json_data, convert_lists)
            .iter()
            .map(|c| {
                let s = serde_json::to_string(c).unwrap_or_default();
                if ensure_ascii { escape_ascii(&s) } else { s }
            })
            .collect()
    }

    /// `_parse_json` — json_parser.py:130-138.
    fn parse_json(&self, content: &str) -> Result<Vec<String>> {
        let json_data = serde_json::from_str::<Value>(content)
            .map_err(|error| anyhow::anyhow!("Invalid JSON: {error}"))?;
        self.split_json(&json_data, true)
            .into_iter()
            .filter(|c| !c.is_null() && !is_empty_value(c))
            .map(|c| serde_json::to_string(&c).map_err(anyhow::Error::from))
            .collect()
    }

    /// `_parse_jsonl` — json_parser.py:140-152.
    fn parse_jsonl(&self, content: &str) -> Result<Vec<String>> {
        let mut all_chunks = Vec::new();
        for (line_number, line) in content.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let data = serde_json::from_str::<Value>(line).map_err(|error| {
                anyhow::anyhow!("Invalid JSONL record on line {}: {error}", line_number + 1)
            })?;
            let chunks = self.split_json(&data, true);
            for chunk in chunks {
                if chunk.is_null() || is_empty_value(&chunk) {
                    continue;
                }
                all_chunks.push(serde_json::to_string(&chunk)?);
            }
        }
        Ok(all_chunks)
    }

    /// `is_jsonl_format` — json_parser.py:154-172.
    pub fn is_jsonl_format(&self, txt: &str, sample_limit: usize, threshold: f64) -> bool {
        let lines: Vec<&str> = txt
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            return false;
        }
        // Whole text parses as one JSON object → not JSONL.
        if serde_json::from_str::<Value>(txt).is_ok() {
            return false;
        }
        let sample_limit = sample_limit.min(lines.len());
        let sample_lines = &lines[..sample_limit];
        let valid = sample_lines
            .iter()
            .filter(|l| Self::is_valid_json(l))
            .count();
        if valid == 0 {
            return false;
        }
        (valid as f64 / sample_lines.len() as f64) >= threshold
    }

    /// `_is_valid_json` — json_parser.py:174-179.
    fn is_valid_json(line: &str) -> bool {
        serde_json::from_str::<Value>(line).is_ok()
    }

    /// `__call__` — json_parser.py:33-41.
    pub fn parse_binary(&self, binary: &[u8]) -> Result<Vec<String>> {
        let txt = std::str::from_utf8(binary)
            .map_err(|error| anyhow::anyhow!("JSON document is not valid UTF-8: {error}"))?;
        if self.is_jsonl_format(&txt, 10, 0.8) {
            self.parse_jsonl(&txt)
        } else {
            self.parse_json(&txt)
        }
    }
}

/// True when the value is an empty JSON object (`{}`).
fn is_empty_value(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.is_empty())
}

/// Escape non-ASCII characters as `\uXXXX` (mirrors `ensure_ascii=True`).
fn escape_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else {
            let mut buf = [0u16; 2];
            let encoded = ch.encode_utf16(&mut buf);
            for u in encoded {
                out.push_str(&format!("\\u{:04x}", u));
            }
        }
    }
    out
}

/// Flatten a JSON value into key: value text lines (legacy helper).
fn flatten_json(value: &Value, prefix: &str) -> String {
    match value {
        Value::Object(map) => {
            let mut text = String::new();
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                text.push_str(&flatten_json(v, &path));
                text.push('\n');
            }
            text
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                return format!("{prefix}: []");
            }
            if arr.iter().all(|v| v.is_object()) {
                let mut text = String::new();
                for (i, v) in arr.iter().enumerate() {
                    text.push_str(&flatten_json(v, &format!("{prefix}[{i}]")));
                    text.push('\n');
                }
                text
            } else {
                let values: Vec<String> = arr.iter().map(value_to_string).collect();
                format!("{prefix}: [{}]", values.join(", "))
            }
        }
        _ => format!("{prefix}: {}", value_to_string(value)),
    }
}

fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        _ => value.to_string(),
    }
}

/// Port of `JsonParser.__call__` — parses binary JSON/JSONL into joined text.
#[derive(Default)]
pub struct JsonParser {
    inner: RAGFlowJsonParser,
}

impl JsonParser {
    pub fn new() -> Self {
        Self {
            inner: RAGFlowJsonParser::default(),
        }
    }

    fn extract_text(&self, data: &[u8]) -> Result<String> {
        let sections = self.inner.parse_binary(data).map_err(|error| {
            anyhow::anyhow!(
                "Failed to parse JSON document: {error:#}. Ensure the file is UTF-8 encoded and contains valid JSON or JSONL (one valid JSON value per non-empty line), then upload it again."
            )
        })?;
        Ok(sections.join("\n"))
    }
}

impl Parse for JsonParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = self.extract_text(data)?;
        Ok(new_document(name, content, "application/json", data.len()))
    }
}


