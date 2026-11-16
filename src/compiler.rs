//! Knowledge-compilation flow schema — RAGFlow v0.27.2
//! `rag/flow/compiler/schema.py`.
//!
//! `CompilerFromUpstream` is the payload a Compiler node may carry from an
//! upstream pipeline: identity/timing metadata, the source file and chunk
//! list, the requested `output_format` and the per-format results. Upstream
//! uses pydantic with `populate_by_name=True, extra="forbid"`; the serde
//! equivalents here are field `alias`es plus `deny_unknown_fields`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upstream `Literal["json", "markdown", "text", "html", "chunks"]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompilerOutputFormat {
    Json,
    Markdown,
    Text,
    Html,
    Chunks,
}

/// `rag/flow/compiler/schema.py::CompilerFromUpstream`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompilerFromUpstream {
    #[serde(default, alias = "_created_time")]
    pub created_time: Option<f64>,
    #[serde(default, alias = "_elapsed_time")]
    pub elapsed_time: Option<f64>,
    pub name: String,
    #[serde(default)]
    pub file: Option<Value>,
    #[serde(default)]
    pub chunks: Option<Vec<Value>>,
    #[serde(default)]
    pub output_format: Option<CompilerOutputFormat>,
    #[serde(default, alias = "json")]
    pub json_result: Option<Vec<Value>>,
    #[serde(default, alias = "markdown")]
    pub markdown_result: Option<String>,
    #[serde(default, alias = "text")]
    pub text_result: Option<String>,
    #[serde(default, alias = "html")]
    pub html_result: Option<String>,
}

/// `rag/flow/compiler/compiler.py::CompilerParam` (ProcessParamBase +
/// LLMParam): compilation-template group selection. The singular key is
/// preferred; the legacy plural key stays accepted as a fallback and
/// `check()` normalizes the chosen value into the plural list field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CompilerParam {
    #[serde(default)]
    pub compilation_template_group_id: String,
    #[serde(default)]
    pub compilation_template_group_ids: Vec<String>,
}

impl CompilerParam {
    /// `ComponentParamBase.check_empty` message for the group field.
    pub const GROUP_EMPTY_MESSAGE: &'static str =
        "Compilation Template Group does not support empty value.";

    /// `CompilerParam.check()`: prefer the singular group id, fall back to
    /// the legacy plural key, then normalize into `compilation_template_group_ids`.
    pub fn check(&mut self) -> Result<(), String> {
        let groups: Vec<String> = if !self.compilation_template_group_id.is_empty() {
            vec![self.compilation_template_group_id.clone()]
        } else {
            self.compilation_template_group_ids.clone()
        };
        if groups.is_empty() {
            return Err(Self::GROUP_EMPTY_MESSAGE.to_owned());
        }
        self.compilation_template_group_ids = groups;
        Ok(())
    }
}

/// `Compiler._compile_language`: prefer the request language, then the canvas
/// language, then the document chunking config language, defaulting to
/// `English`. Blank strings fall through exactly like the upstream
/// `strip()` + falsiness checks.
pub fn compile_language(
    request_language: Option<&str>,
    canvas_language: Option<&str>,
    doc_config_language: Option<&str>,
) -> String {
    let clean = |value: Option<&str>| {
        value
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    clean(request_language)
        .or_else(|| clean(canvas_language))
        .or_else(|| clean(doc_config_language))
        .unwrap_or_else(|| "English".to_owned())
}

/// `Compiler._compile_progress`: `None` progress reports as `0.0` and the
/// message passes through unchanged.
pub fn compile_progress(progress: Option<f64>, message: &str) -> (f64, String) {
    (progress.unwrap_or(0.0), message.to_owned())
}

use crate::chunk::tokenizer::token_count;

/// Upstream `Compiler._PARSER_CANDIDATE_TOKEN_SIZE`.
pub const PARSER_CANDIDATE_TOKEN_SIZE: usize = 128;
/// Upstream `Compiler._PARSER_MIN_COARSE_CHUNK_TOKEN_SIZE`.
pub const PARSER_MIN_COARSE_CHUNK_TOKEN_SIZE: usize = 256;

/// Python truthiness over JSON values (`bool(x)`): empty containers and
/// zero numbers are falsy.
fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(true),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// Python `str()` over the shapes used for chunk ids.
fn value_to_id(value: &Value) -> String {
    match value {
        Value::Null => "None".to_owned(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => {
            if *flag {
                "True".to_owned()
            } else {
                "False".to_owned()
            }
        }
        other => other.to_string(),
    }
}

/// `Compiler._is_markdown_table_separator`.
pub fn is_markdown_table_separator(line: &str) -> bool {
    line.trim().trim_matches('|').split('|').all(|cell| {
        let trimmed = cell.trim();
        let body = trimmed.strip_prefix(':').unwrap_or(trimmed);
        let body = body.strip_suffix(':').unwrap_or(body);
        body.chars().count() >= 3 && body.chars().all(|ch| ch == '-')
    })
}

fn starts_with_pipe(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

fn fence_open(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let fence_char = trimmed.chars().next()?;
    if fence_char != '`' && fence_char != '~' {
        return None;
    }
    let length = trimmed.chars().take_while(|ch| *ch == fence_char).count();
    (length >= 3).then_some((fence_char, length))
}

fn fence_closes(line: &str, fence_char: char, min_length: usize) -> bool {
    let trimmed = line.trim_end_matches(['\n', '\r']).trim_start();
    let run = trimmed.chars().take_while(|ch| *ch == fence_char).count();
    run >= min_length && trimmed.chars().skip(run).all(char::is_whitespace)
}

/// `Compiler._split_markdown_blocks`: tables and fenced code blocks stay
/// atomic; `None` marks ordinary text runs. Line splitting follows `\n`
/// (CRLF included since the `\r` stays attached).
pub fn split_markdown_blocks(text: &str) -> Vec<(String, Option<&'static str>)> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut blocks: Vec<(String, Option<&'static str>)> = Vec::new();
    let mut ordinary: String = String::new();
    let mut index = 0usize;
    while index < lines.len() {
        if let Some((fence_char, fence_length)) = fence_open(lines[index]) {
            if !ordinary.is_empty() {
                blocks.push((std::mem::take(&mut ordinary), None));
            }
            let mut code = String::from(lines[index]);
            index += 1;
            while index < lines.len() {
                code.push_str(lines[index]);
                let closing = fence_closes(lines[index], fence_char, fence_length);
                index += 1;
                if closing {
                    break;
                }
            }
            blocks.push((code, Some("fenced_code")));
            continue;
        }
        if index + 1 < lines.len()
            && starts_with_pipe(lines[index])
            && is_markdown_table_separator(lines[index + 1])
        {
            if !ordinary.is_empty() {
                blocks.push((std::mem::take(&mut ordinary), None));
            }
            let mut table = String::from(lines[index]);
            table.push_str(lines[index + 1]);
            index += 2;
            while index < lines.len() && starts_with_pipe(lines[index]) {
                table.push_str(lines[index]);
                index += 1;
            }
            blocks.push((table, Some("table")));
            continue;
        }
        ordinary.push_str(lines[index]);
        index += 1;
    }
    if !ordinary.is_empty() {
        blocks.push((ordinary, None));
    }
    blocks
}

/// `Compiler._is_atomic_json_record`: structured or position-bound Parser
/// records survive splitting untouched.
pub fn is_atomic_json_record(record: &Value) -> bool {
    let lowercase = |key: &str| {
        record
            .get(key)
            .and_then(Value::as_str)
            .map(|text| text.trim().to_lowercase())
            .unwrap_or_default()
    };
    let doc_type = lowercase("doc_type_kwd");
    let layout_type = lowercase("layout_type");
    let text = record
        .get("text")
        .or_else(|| record.get("content_with_weight"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut has_markdown_table = false;
    let mut has_html_table = false;
    let mut has_fenced_code = false;
    if !text.is_empty() {
        let lines: Vec<&str> = text.lines().collect();
        has_markdown_table = (0..lines.len()).any(|index| {
            index + 1 < lines.len()
                && starts_with_pipe(lines[index])
                && is_markdown_table_separator(lines[index + 1])
        });
        has_html_table = regex::Regex::new(r"(?i)<table(?:\s|>)")
            .map(|pattern| pattern.is_match(text))
            .unwrap_or(false);
        has_fenced_code = lines.iter().any(|line| fence_open(line).is_some());
    }
    let flag = |key: &str| record.get(key).map(json_truthy).unwrap_or(false);
    doc_type == "table"
        || doc_type == "image"
        || flag("img_id")
        || flag("image")
        || layout_type == "figure"
        || layout_type == "table"
        || flag("bbox")
        || has_markdown_table
        || has_html_table
        || has_fenced_code
}

/// `Compiler._formalize_rechunked_chunks`: replace parser chunks with the
/// semantic groups produced by rechunking, dropping stale embeddings and
/// position columns. Groups without any surviving source chunk are skipped.
pub fn formalize_rechunked_chunks(
    chunks: &[Value],
    rechunked_chunks: &[Value],
    doc_id: &str,
) -> Vec<Value> {
    use std::collections::HashMap;
    let originals: HashMap<String, &Value> = chunks
        .iter()
        .filter_map(|chunk| {
            chunk
                .get("id")
                .filter(|id| json_truthy(id))
                .map(|id| (value_to_id(id), chunk))
        })
        .collect();
    let mut result = Vec::new();
    for grouped in rechunked_chunks {
        let source_ids: Vec<String> = grouped
            .get("source_chunk_ids")
            .and_then(Value::as_array)
            .map(|ids| ids.iter().map(value_to_id).collect())
            .unwrap_or_default();
        let Some(source) = source_ids
            .iter()
            .find_map(|source_id| originals.get(source_id).copied())
        else {
            continue;
        };
        let mut item = source.clone();
        if let Some(object) = item.as_object_mut() {
            object.insert(
                "id".into(),
                Value::String(grouped.get("id").map(value_to_id).unwrap_or_default()),
            );
            object.insert("doc_id".into(), Value::String(doc_id.to_owned()));
            object.insert(
                "text".into(),
                Value::String(
                    grouped
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                ),
            );
            let stale: Vec<String> = object
                .keys()
                .filter(|key| key.starts_with("q_") && key.ends_with("_vec"))
                .cloned()
                .collect();
            for key in stale {
                object.remove(&key);
            }
            for key in [
                "content_with_weight",
                "content_ltks",
                "content_sm_ltks",
                "position_int",
                "page_num_int",
                "top_int",
            ] {
                object.remove(key);
            }
        }
        result.push(item);
    }
    result
}

/// `Compiler._merge_small_text_chunks`: merge adjacent ordinary text chunks up
/// to the coarse target; below the coarse floor nothing changes.
pub fn merge_small_text_chunks(chunks: Vec<Value>, target_token_size: usize) -> Vec<Value> {
    if target_token_size < PARSER_MIN_COARSE_CHUNK_TOKEN_SIZE {
        return chunks;
    }
    let mut merged: Vec<Value> = Vec::new();
    let mut pending: Option<Value> = None;
    for chunk in chunks {
        let text = chunk
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty());
        if text.is_none() || is_atomic_json_record(&chunk) {
            if let Some(previous) = pending.take() {
                merged.push(previous);
            }
            merged.push(chunk);
            continue;
        }
        let text = text.unwrap_or_default().to_owned();
        match pending.take() {
            None => pending = Some(chunk),
            Some(mut previous) => {
                let previous_text = previous
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let combined = format!("{previous_text}\n\n{text}");
                let allow = token_count(&previous_text) < PARSER_MIN_COARSE_CHUNK_TOKEN_SIZE
                    && token_count(&combined) <= target_token_size;
                if allow {
                    if let Some(object) = previous.as_object_mut() {
                        object.insert("text".into(), Value::String(combined));
                    }
                    pending = Some(previous);
                } else {
                    merged.push(previous);
                    pending = Some(chunk);
                }
            }
        }
    }
    if let Some(previous) = pending {
        merged.push(previous);
    }
    merged
}

/// `Compiler._slumber_candidates`: paragraph then sentence splitting with a
/// token fallback, mirroring the upstream level list.
pub fn slumber_candidates(
    text: &str,
    level: usize,
    target_token_size: Option<usize>,
) -> Vec<String> {
    const LEVELS: [&[&str]; 2] = [
        &["\n\n", "\r\n", "\n", "\r"],
        &[". ", "! ", "? ", "。", "！", "？", "；", ";"],
    ];
    let target = match target_token_size {
        Some(size) if size > 0 => size,
        _ => PARSER_TEXT_CHUNK_TOKEN_SIZE,
    };
    if text.trim().is_empty() {
        return Vec::new();
    }
    if level >= LEVELS.len() {
        return crate::book::naive_merge(&[(text.to_owned(), String::new())], target, "", 0)
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect();
    }
    let splits = split_slumber_level(text, LEVELS[level]);
    if splits.len() <= 1 && level + 1 < LEVELS.len() {
        return slumber_candidates(text, level + 1, Some(target));
    }
    let mut candidates = Vec::new();
    for split in splits {
        if token_count(&split) > target {
            candidates.extend(slumber_candidates(&split, level + 1, Some(target)));
        } else {
            candidates.push(split);
        }
    }
    candidates
}

/// `Compiler._normalize_upstream_chunks`: normalize direct Parser output into
/// Compiler chunks. `chunks` output preserves atomic table/image records
/// unless rechunking is explicitly requested; textual outputs split by the
/// token target and then merge coarse fragments.
pub fn normalize_upstream_chunks(
    upstream: &Value,
    split_json_text: bool,
    target_token_size: Option<usize>,
) -> Vec<Value> {
    let target = match target_token_size {
        Some(size) if size > 0 => size,
        _ => PARSER_TEXT_CHUNK_TOKEN_SIZE,
    };
    let output_format = upstream
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut records: Option<Vec<Value>> = None;
    match output_format {
        "chunks" => {
            let chunks: Vec<Value> = upstream
                .get("chunks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if !split_json_text {
                return chunks
                    .into_iter()
                    .map(|mut chunk| {
                        if let Some(object) = chunk.as_object_mut() {
                            let text = object
                                .get("text")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .or_else(|| {
                                    object
                                        .get("content_with_weight")
                                        .and_then(Value::as_str)
                                        .map(str::to_owned)
                                })
                                .unwrap_or_default();
                            object.insert("text".into(), Value::String(text));
                        }
                        chunk
                    })
                    .collect();
            }
            records = Some(chunks);
        }
        "json" => {
            let source = upstream.get("json").or_else(|| upstream.get("json_result"));
            records = Some(
                source
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
            );
        }
        _ => {}
    }
    if let Some(records) = records {
        let mut chunks: Vec<Value> = Vec::new();
        for record in records {
            if !record.is_object() {
                continue;
            }
            let mut chunk = record;
            let text = chunk
                .get("text")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    chunk
                        .get("content_with_weight")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
            match text {
                Some(text) if !text.trim().is_empty() => {
                    if is_atomic_json_record(&chunk) || !split_json_text {
                        if let Some(object) = chunk.as_object_mut() {
                            object.insert("text".into(), Value::String(text));
                        }
                        chunks.push(chunk);
                    } else {
                        for part in slumber_candidates(&text, 0, Some(target)) {
                            let mut split_chunk = chunk.clone();
                            if let Some(object) = split_chunk.as_object_mut() {
                                object.insert("text".into(), Value::String(part));
                            }
                            chunks.push(split_chunk);
                        }
                    }
                }
                other => {
                    if let Some(object) = chunk.as_object_mut() {
                        object.insert("text".into(), Value::String(other.unwrap_or_default()));
                    }
                    chunks.push(chunk);
                }
            }
        }
        if split_json_text {
            return merge_small_text_chunks(chunks, target);
        }
        return chunks;
    }
    if matches!(output_format, "markdown" | "text" | "html") {
        let text = upstream
            .get(output_format)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                upstream
                    .get(format!("{output_format}_result"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
            return Vec::new();
        };
        if !split_json_text {
            return vec![serde_json::json!({"text": text})];
        }
        let chunks: Vec<Value> = if output_format == "markdown" {
            let mut chunks = Vec::new();
            for (block, block_type) in split_markdown_blocks(&text) {
                match block_type {
                    Some("table") => {
                        chunks.push(serde_json::json!({"text": block, "doc_type_kwd": "table"}));
                    }
                    Some("fenced_code") => {
                        chunks.push(serde_json::json!({"text": block}));
                    }
                    _ => {
                        for part in slumber_candidates(&block, 0, Some(target)) {
                            if !part.trim().is_empty() {
                                chunks.push(serde_json::json!({"text": part}));
                            }
                        }
                    }
                }
            }
            chunks
        } else {
            slumber_candidates(&text, 0, Some(target))
                .into_iter()
                .filter(|part| !part.trim().is_empty())
                .map(|part| serde_json::json!({"text": part}))
                .collect()
        };
        return merge_small_text_chunks(chunks, target);
    }
    Vec::new()
}

/// Upstream `Compiler._PARSER_TEXT_CHUNK_TOKEN_SIZE`.
pub const PARSER_TEXT_CHUNK_TOKEN_SIZE: usize = 1024;
/// Upstream `Compiler._PARSER_MIN_SPLIT_CHARACTERS`.
pub const PARSER_MIN_SPLIT_CHARACTERS: usize = 24;

/// `Compiler._split_slumber_level`: split at boundaries while keeping the
/// delimiter attached to the previous part. A boundary only closes a part
/// once the accumulated text reaches `PARSER_MIN_SPLIT_CHARACTERS`; shorter
/// trailing fragments merge into their neighbour, and blank parts drop out.
pub fn split_slumber_level(text: &str, delimiters: &[&str]) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut sorted: Vec<&str> = delimiters
        .iter()
        .copied()
        .filter(|delimiter| !delimiter.is_empty())
        .collect();
    sorted.sort_by_key(|delimiter| std::cmp::Reverse(delimiter.chars().count()));
    if sorted.is_empty() {
        return vec![text.to_owned()];
    }
    let pattern = sorted
        .iter()
        .map(|delimiter| regex::escape(delimiter))
        .collect::<Vec<_>>()
        .join("|");
    let Ok(regex) = regex::Regex::new(&format!("(?s){pattern}")) else {
        return vec![text.to_owned()];
    };
    let mut parts: Vec<String> = Vec::new();
    let mut start = 0usize;
    for boundary in regex.find_iter(text) {
        let end = boundary.end();
        if text[start..end].chars().count() >= PARSER_MIN_SPLIT_CHARACTERS {
            parts.push(text[start..end].to_owned());
            start = end;
        }
    }
    if start < text.len() {
        parts.push(text[start..].to_owned());
    }
    let mut merged: Vec<String> = Vec::new();
    for part in parts {
        if !merged.is_empty() && part.trim().chars().count() < PARSER_MIN_SPLIT_CHARACTERS {
            merged.last_mut().expect("non-empty").push_str(&part);
        } else {
            merged.push(part);
        }
    }
    merged.retain(|part| !part.trim().is_empty());
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn markdown_table_separator_and_blocks() {
        assert!(is_markdown_table_separator("|---|---|"));
        assert!(is_markdown_table_separator(" | :---: | --- | "));
        assert!(!is_markdown_table_separator("|--|--|"));
        assert!(!is_markdown_table_separator("| a | b |"));

        let text =
            "intro line\n| a | b |\n| --- | --- |\n| 1 | 2 |\n```rust\nfn main() {}\n```\noutro\n";
        let blocks = split_markdown_blocks(text);
        assert_eq!(blocks.len(), 4);
        assert_eq!(blocks[0], ("intro line\n".to_owned(), None));
        assert_eq!(blocks[1].1, Some("table"));
        assert!(blocks[1].0.contains("| 1 | 2 |"));
        assert_eq!(blocks[2].1, Some("fenced_code"));
        assert!(blocks[2].0.contains("fn main"));
        assert_eq!(blocks[3], ("outro\n".to_owned(), None));

        // Unclosed fence swallows the remainder.
        let open = "```\ncode\nmore\n";
        let blocks = split_markdown_blocks(open);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].1, Some("fenced_code"));
    }

    #[test]
    fn atomic_json_records_mirror_upstream() {
        let plain = serde_json::json!({"text": "hello"});
        assert!(!is_atomic_json_record(&plain));
        assert!(is_atomic_json_record(
            &serde_json::json!({"doc_type_kwd": " Table ", "text": "x"})
        ));
        assert!(is_atomic_json_record(
            &serde_json::json!({"layout_type": "figure"})
        ));
        assert!(is_atomic_json_record(
            &serde_json::json!({"text": "x", "bbox": {"x0": 0}})
        ));
        assert!(is_atomic_json_record(
            &serde_json::json!({"text": "x", "img_id": "img-1"})
        ));
        assert!(is_atomic_json_record(&serde_json::json!({
            "text": "| a |\n| --- |\n| 1 |"
        })));
        assert!(is_atomic_json_record(
            &serde_json::json!({"text": "<table style=x>"})
        ));
        assert!(is_atomic_json_record(
            &serde_json::json!({"text": "```py\nprint(1)\n```"})
        ));
        // Falsy id-style fields do not make a record atomic.
        assert!(!is_atomic_json_record(
            &serde_json::json!({"text": "x", "img_id": "", "bbox": []})
        ));
    }

    #[test]
    fn merge_small_text_chunks_respects_coarse_floor() {
        let chunks = vec![
            serde_json::json!({"text": "first"}),
            serde_json::json!({"text": "second"}),
        ];
        assert_eq!(merge_small_text_chunks(chunks.clone(), 128).len(), 2);

        let merged = merge_small_text_chunks(chunks, 512);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0]["text"], serde_json::json!("first\n\nsecond"));

        // An atomic record flushes the pending merge and stays intact.
        let chunks = vec![
            serde_json::json!({"text": "first"}),
            serde_json::json!({"doc_type_kwd": "table", "text": "| a |"}),
            serde_json::json!({"text": "third"}),
        ];
        let merged = merge_small_text_chunks(chunks, 512);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[1]["doc_type_kwd"], serde_json::json!("table"));
    }

    #[test]
    fn normalize_upstream_chunks_branches() {
        // chunks passthrough fills text from content_with_weight.
        let upstream = serde_json::json!({
            "output_format": "chunks",
            "chunks": [{"content_with_weight": "body", "id": "c1"}]
        });
        let chunks = normalize_upstream_chunks(&upstream, false, None);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0]["text"], serde_json::json!("body"));

        // json records split at the token target unless atomic.
        let upstream = serde_json::json!({
            "output_format": "json",
            "json": [
                {"text": "first sentence. second sentence."},
                {"doc_type_kwd": "table", "text": "| a |\n| --- |"}
            ]
        });
        let chunks = normalize_upstream_chunks(&upstream, true, Some(2));
        assert!(chunks.len() >= 2);
        assert!(
            chunks
                .iter()
                .any(|chunk| chunk["doc_type_kwd"] == serde_json::json!("table"))
        );

        // Markdown keeps tables atomic and chunks ordinary text.
        let upstream = serde_json::json!({
            "output_format": "markdown",
            "markdown": "para one\n\npara two\n"
        });
        let chunks = normalize_upstream_chunks(&upstream, true, Some(1024));
        assert!(!chunks.is_empty());
        assert!(chunks.iter().all(|chunk| chunk.get("text").is_some()));
        let passthrough = normalize_upstream_chunks(&upstream, false, None);
        assert_eq!(passthrough.len(), 1);
        assert_eq!(
            passthrough[0]["text"],
            serde_json::json!("para one\n\npara two\n")
        );

        // Unknown format yields nothing.
        let upstream = serde_json::json!({"output_format": "pdf"});
        assert!(normalize_upstream_chunks(&upstream, true, None).is_empty());
    }

    #[test]
    fn slumber_candidates_levels_and_fallback() {
        assert!(slumber_candidates("", 0, None).is_empty());
        // A boundary only counts once the accumulated part reaches 24
        // characters, so both paragraphs here are long enough to split.
        let paragraphs = "paragraph one text is long enough\n\nparagraph two text is long enough";
        let parts = slumber_candidates(paragraphs, 0, Some(1024));
        assert_eq!(parts.len(), 2);
        assert!(parts[0].ends_with("\n\n"));

        // Text below the minimum with a single delimiter stays whole.
        let tiny = "para one\n\npara two";
        assert_eq!(slumber_candidates(tiny, 0, Some(1024)).len(), 1);
        // Single split at the paragraph level falls back to sentences.
        let single = "Sentence one is long enough. Sentence two is long enough.";
        let parts = slumber_candidates(single, 0, Some(1024));
        assert!(parts.len() >= 2);
        assert!(parts[0].contains(". "));
    }

    #[test]
    fn compiler_param_check_prefers_singular_and_normalizes() {
        let mut param = CompilerParam::default();
        assert_eq!(
            param.check(),
            Err(CompilerParam::GROUP_EMPTY_MESSAGE.to_owned())
        );

        let mut param = CompilerParam {
            compilation_template_group_id: "g1".to_owned(),
            compilation_template_group_ids: vec!["old".to_owned()],
        };
        assert_eq!(param.check(), Ok(()));
        assert_eq!(param.compilation_template_group_ids, vec!["g1".to_owned()]);

        let mut param = CompilerParam {
            compilation_template_group_id: String::new(),
            compilation_template_group_ids: vec!["g2".to_owned(), "g3".to_owned()],
        };
        assert_eq!(param.check(), Ok(()));
        assert_eq!(
            param.compilation_template_group_ids,
            vec!["g2".to_owned(), "g3".to_owned()]
        );

        // Upstream list-truthiness quirk: a single blank string entry still
        // passes check_empty because the list itself is non-empty.
        let mut param = CompilerParam {
            compilation_template_group_id: String::new(),
            compilation_template_group_ids: vec![String::new()],
        };
        assert_eq!(param.check(), Ok(()));
        assert_eq!(param.compilation_template_group_ids, vec![String::new()]);

        // Serde keeps the upstream key names and tolerates missing keys.
        let parsed: CompilerParam = serde_json::from_value(serde_json::json!({
            "compilation_template_group_id": "g9"
        }))
        .unwrap();
        assert_eq!(parsed.compilation_template_group_id, "g9");
        assert!(parsed.compilation_template_group_ids.is_empty());
    }

    #[test]
    fn compile_language_falls_back_to_english() {
        assert_eq!(compile_language(None, None, None), "English");
        assert_eq!(compile_language(Some(" Chinese "), None, None), "Chinese");
        assert_eq!(
            compile_language(Some("  "), Some("Japanese"), None),
            "Japanese"
        );
        assert_eq!(compile_language(None, Some(""), Some("Korean")), "Korean");
        let (progress, message) = compile_progress(None, "start");
        assert_eq!((progress, message.as_str()), (0.0, "start"));
        let (progress, _) = compile_progress(Some(0.05), "x");
        assert!((progress - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn split_slumber_level_mirrors_upstream() {
        assert!(split_slumber_level("", &["。"]).is_empty());
        assert_eq!(
            split_slumber_level("short", &["。"]),
            vec!["short".to_owned()]
        );
        assert_eq!(
            split_slumber_level("abcdef", &[]),
            vec!["abcdef".to_owned()]
        );

        // Two parts, each at least 24 characters, split and keep the
        // delimiter on the preceding fragment.
        let long = format!("{}。{}。", "a".repeat(24), "b".repeat(24));
        let parts = split_slumber_level(&long, &["。"]);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].ends_with('。'));

        // A short trailing fragment merges into the previous part.
        let tail = format!("{}。短。", "a".repeat(24));
        assert_eq!(split_slumber_level(&tail, &["。"]).len(), 1);

        // Regex metacharacter delimiters are literal.
        let piped = format!("{}|{}|", "c".repeat(24), "d".repeat(24));
        assert_eq!(split_slumber_level(&piped, &["|"]).len(), 2);
    }

    #[test]
    fn deserializes_aliased_payload_and_round_trips() {
        let payload = json!({
            "_created_time": 1.5,
            "_elapsed_time": 2.5,
            "name": "doc.md",
            "file": {"id": "f1"},
            "chunks": [{"text": "a"}],
            "output_format": "markdown",
            "json": [{"text": "a"}],
            "markdown": "# a",
            "text": "a",
            "html": "<p>a</p>"
        });
        let parsed: CompilerFromUpstream = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(parsed.created_time, Some(1.5));
        assert_eq!(parsed.elapsed_time, Some(2.5));
        assert_eq!(parsed.name, "doc.md");
        assert_eq!(parsed.output_format, Some(CompilerOutputFormat::Markdown));
        assert_eq!(parsed.json_result.as_ref().unwrap().len(), 1);
        assert_eq!(parsed.markdown_result.as_deref(), Some("# a"));
        assert_eq!(parsed.text_result.as_deref(), Some("a"));
        assert_eq!(parsed.html_result.as_deref(), Some("<p>a</p>"));
        // Round-trip keeps the field names serializable (alias output is
        // upstream-only; serialization uses the canonical names).
        let encoded = serde_json::to_value(&parsed).unwrap();
        assert_eq!(encoded["created_time"], json!(1.5));
        assert_eq!(encoded["name"], json!("doc.md"));
    }

    #[test]
    fn accepts_canonical_field_names_too() {
        let parsed: CompilerFromUpstream = serde_json::from_value(json!({
            "created_time": 1.0,
            "elapsed_time": 2.0,
            "name": "n",
            "json_result": [{"x": 1}],
            "markdown_result": "m",
            "text_result": "t",
            "html_result": "h",
            "output_format": "chunks"
        }))
        .unwrap();
        assert_eq!(parsed.output_format, Some(CompilerOutputFormat::Chunks));
        assert_eq!(parsed.json_result.as_ref().unwrap()[0]["x"], json!(1));
    }

    #[test]
    fn rejects_unknown_fields_and_bad_formats() {
        let unknown = serde_json::from_value::<CompilerFromUpstream>(json!({
            "name": "n",
            "extra": 1
        }));
        assert!(unknown.is_err(), "extra=forbid parity");
        let bad_format = serde_json::from_value::<CompilerFromUpstream>(json!({
            "name": "n",
            "output_format": "pdf"
        }));
        assert!(bad_format.is_err());
        let nullish = serde_json::from_value::<CompilerFromUpstream>(json!({
            "name": "n",
            "output_format": null
        }))
        .unwrap();
        assert!(nullish.output_format.is_none());
        // All five upstream formats parse.
        for (name, expected) in [
            ("json", CompilerOutputFormat::Json),
            ("markdown", CompilerOutputFormat::Markdown),
            ("text", CompilerOutputFormat::Text),
            ("html", CompilerOutputFormat::Html),
            ("chunks", CompilerOutputFormat::Chunks),
        ] {
            let parsed: CompilerFromUpstream = serde_json::from_value(json!({
                "name": "n",
                "output_format": name
            }))
            .unwrap();
            assert_eq!(parsed.output_format, Some(expected));
        }
    }
}
