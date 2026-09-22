//! `rag/flow` component data contracts — Rust port of the RAGFlow flow schemas.
//!
//! Mirrors the RAGFlow flow-layer semantics that sit on top of the raw
//! parse → chunk → embed → store pipeline:
//!
//! - `rag/flow/base.py` `ProcessBase`: component output-map contract with the
//!   reserved `_created_time` / `_elapsed_time` / `_ERROR` keys, the
//!   `COMPONENT_EXEC_TIMEOUT` timeout and the progress callback.
//! - `rag/flow/file.py` `File`: emits `name` (+ `file` when no doc id is
//!   bound) as the pipeline entry point.
//! - `rag/flow/chunker/schema.py` `TokenChunkerFromUpstream`,
//!   `rag/flow/extractor/schema.py` `ExtractorFromUpstream` and
//!   `rag/flow/tokenizer/schema.py` `TokenizerFromUpstream`: the shared
//!   upstream payload contract (`name` / `file` / `chunks` /
//!   `output_format` / `json` | `markdown` | `text` | `html`) together with
//!   the `_check_payloads` validator.
//! - `rag/flow/pipeline.py` `Pipeline.callback`: per-component trace
//!   accumulation (same component id appends to its last trace entry) and
//!   the weighted overall-progress computation.
//! - `api/apps/restful_apis/chunk_api.py` `Chunk`: the chunk data contract
//!   (`ChunkDoc`), derived from the pipeline's intermediate [`crate::Chunk`]
//!   structure.
//!
//! Resume-entity resources (the deepdoc `parser/resume/entities` corpus:
//! `corp.tks.freq.json`, `good_corp.json`, `corp_tag.json`,
//! `corp_baike_len.csv`, plus the school/region/industry tables) are
//! embedded and loaded by `crate::resume` (`CorporationEntity` /
//! `SchoolEntity` / `RegionEntity` / `IndustryEntity` / `DegreeEntity`).
//! The [`ChunkDoc`] conversion and the tests below exercise that NER
//! vocabulary end-to-end so the pipeline-facing contract stays aligned with
//! the entity-recognition semantics.

use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Reserved output-map keys — `ProcessBase` (rag/flow/base.py:41-58).
pub const CREATED_TIME_KEY: &str = "_created_time";
/// Reserved output-map key set by `ProcessBase.invoke` on success.
pub const ELAPSED_TIME_KEY: &str = "_elapsed_time";
/// Reserved output-map key set by `ProcessBase.invoke` on failure.
pub const ERROR_KEY: &str = "_ERROR";

/// `output_format` literal — shared by the chunker/extractor/tokenizer
/// upstream schemas (`Literal["json", "markdown", "text", "html", "chunks"]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Json,
    Markdown,
    Text,
    Html,
    Chunks,
}

impl OutputFormat {
    /// The wire name of the format.
    pub fn as_str(self) -> &'static str {
        match self {
            OutputFormat::Json => "json",
            OutputFormat::Markdown => "markdown",
            OutputFormat::Text => "text",
            OutputFormat::Html => "html",
            OutputFormat::Chunks => "chunks",
        }
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The upstream payload contract shared by `TokenChunkerFromUpstream`,
/// `ExtractorFromUpstream` and `TokenizerFromUpstream`.
///
/// Field aliases match pydantic `populate_by_name=True`: `_created_time` /
/// `_elapsed_time` / `json` / `markdown` / `text` / `html` are accepted on
/// deserialization and emitted on serialization (by_alias dump semantics).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FromUpstream {
    /// `_created_time` — `ProcessBase.invoke` start timestamp.
    #[serde(
        default,
        rename = "_created_time",
        alias = "created_time",
        skip_serializing_if = "Option::is_none"
    )]
    pub created_time: Option<f64>,
    /// `_elapsed_time` — `ProcessBase.invoke` duration.
    #[serde(
        default,
        rename = "_elapsed_time",
        alias = "elapsed_time",
        skip_serializing_if = "Option::is_none"
    )]
    pub elapsed_time: Option<f64>,
    /// Document/file name flowing from the `File` component.
    #[serde(default)]
    pub name: String,
    /// Original file descriptor dict when no doc id is bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<serde_json::Value>,
    /// Pre-chunked chunk list (list of dicts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunks: Option<Vec<serde_json::Value>>,
    /// Which payload field carries the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<OutputFormat>,
    /// `json` — list-of-dict result payload.
    #[serde(
        default,
        rename = "json",
        alias = "json_result",
        skip_serializing_if = "Option::is_none"
    )]
    pub json_result: Option<Vec<serde_json::Value>>,
    /// `markdown` — markdown string result payload.
    #[serde(
        default,
        rename = "markdown",
        alias = "markdown_result",
        skip_serializing_if = "Option::is_none"
    )]
    pub markdown_result: Option<String>,
    /// `text` — plain-text string result payload.
    #[serde(
        default,
        rename = "text",
        alias = "text_result",
        skip_serializing_if = "Option::is_none"
    )]
    pub text_result: Option<String>,
    /// `html` — HTML string result payload.
    #[serde(
        default,
        rename = "html",
        alias = "html_result",
        skip_serializing_if = "Option::is_none"
    )]
    pub html_result: Option<String>,
}

impl FromUpstream {
    /// `_check_payloads` — tokenizer/schema.py:38-55.
    ///
    /// - `output_format=chunks` with a present (possibly empty) `chunks`
    ///   array is a valid upstream result for nearly empty files;
    /// - `output_format` in {markdown, text, html} requires the matching
    ///   payload field;
    /// - otherwise a `json` list payload (or `chunks`) is required.
    pub fn check_payloads(&self) -> Result<()> {
        if self.output_format == Some(OutputFormat::Chunks) && self.chunks.is_some() {
            return Ok(());
        }
        match self.output_format {
            Some(OutputFormat::Markdown) => {
                if self.markdown_result.is_none() {
                    anyhow::bail!(
                        "output_format=markdown requires a markdown payload (field: 'markdown' or 'markdown_result')."
                    );
                }
            }
            Some(OutputFormat::Text) => {
                if self.text_result.is_none() {
                    anyhow::bail!(
                        "output_format=text requires a text payload (field: 'text' or 'text_result')."
                    );
                }
            }
            Some(OutputFormat::Html) => {
                if self.html_result.is_none() {
                    // Message parity with tokenizer/schema.py (which reuses
                    // the text wording for html).
                    anyhow::bail!(
                        "output_format=text requires a html payload (field: 'html' or 'html_result')."
                    );
                }
            }
            _ => {
                if self.json_result.is_none() && self.chunks.is_none() {
                    anyhow::bail!(
                        "When no chunks are provided and output_format is not markdown/text, a JSON list payload is required (field: 'json' or 'json_result')."
                    );
                }
            }
        }
        Ok(())
    }
}

/// Component output map — `ProcessBase` semantics (rag/flow/base.py:41-59).
///
/// `invoke()` stamps `_created_time` at start, copies upstream kwargs into
/// the output map, runs the component, and stamps `_elapsed_time`; failures
/// surface as `_ERROR` (unless the component declared an exception default).
#[derive(Debug, Clone, Default)]
pub struct ComponentOutput {
    fields: HashMap<String, serde_json::Value>,
    created: Option<f64>,
}

/// Seconds since the Unix epoch (stand-in for `time.perf_counter()` used to
/// compute `_elapsed_time` deltas).
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl ComponentOutput {
    /// Start a component invocation, stamping `_created_time`.
    pub fn begin() -> Self {
        let mut out = Self::default();
        out.created = Some(now_secs());
        out.fields
            .insert(CREATED_TIME_KEY.to_string(), json!(now_secs()));
        out
    }

    /// `set_output` — base.py:43-44 (`self.set_output(k, v)`).
    pub fn set_output(&mut self, key: &str, value: serde_json::Value) {
        self.fields.insert(key.to_string(), value);
    }

    /// `set_output("_ERROR", ...)` — base.py:55.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.fields
            .insert(ERROR_KEY.to_string(), json!(message.into()));
    }

    /// Whether the component invocation failed.
    pub fn error(&self) -> Option<&str> {
        self.fields
            .get(ERROR_KEY)
            .and_then(serde_json::Value::as_str)
    }

    /// Named output accessor.
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        self.fields.get(key)
    }

    /// `set_output("_elapsed_time", ...)` — base.py:58: stamp the duration
    /// since `_created_time` at the end of an invocation.
    pub fn finish(&mut self) -> &mut Self {
        let elapsed = self.created.map(|t| now_secs() - t).unwrap_or(0.0);
        self.fields
            .insert(ELAPSED_TIME_KEY.to_string(), json!(elapsed));
        self
    }

    /// The complete output map (`ProcessBase.output()`).
    pub fn into_map(self) -> HashMap<String, serde_json::Value> {
        self.fields
    }
}

/// `File` component output — `rag/flow/file.py` `File._invoke`.
///
/// With a bound doc id only `name` is emitted; otherwise the raw `file`
/// descriptor dict is carried alongside `name` (base.py:31-48).
pub fn file_outputs(name: impl Into<String>, file: Option<serde_json::Value>) -> ComponentOutput {
    let mut out = ComponentOutput::begin();
    out.set_output("name", json!(name.into()));
    if let Some(file) = file {
        out.set_output("file", file);
    }
    out
}

/// One progress sample — `pipeline.py` callback trace entry:
/// `{progress, message, datetime, timestamp, elapsed_time}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    /// Progress in [0, 1]; -1 signals a canceled/failed component.
    pub progress: f64,
    /// Human-readable progress message.
    pub message: String,
    /// `%H:%M:%S` local wall-clock stamp.
    pub datetime: String,
    /// Monotonic-ish timestamp (seconds since Unix epoch).
    pub timestamp: f64,
    /// Delta since the previous trace entry of the same component.
    pub elapsed_time: f64,
}

/// One component's trace — `pipeline.py` callback `obj` element:
/// `{component_id, trace: [...]}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentTrace {
    pub component_id: String,
    pub trace: Vec<TraceEntry>,
}

/// The accumulated flow log — `pipeline.py` callback `obj` list.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FlowLog {
    pub components: Vec<ComponentTrace>,
}

impl FlowLog {
    /// `Pipeline.callback` — pipeline.py:43-98: append a progress sample to
    /// the component's trace. A repeated component id extends its last trace
    /// entry (with `elapsed_time` vs. the previous sample); a new component
    /// starts a fresh trace entry with `elapsed_time = 0`.
    pub fn append(&mut self, component_id: &str, progress: f64, message: &str) {
        let timestamp = now_secs();
        let datetime = chrono::Local::now().format("%H:%M:%S").to_string();
        if let Some(last) = self.components.last_mut()
            && last.component_id == component_id
        {
            let prev = last.trace.last().map(|t| t.timestamp).unwrap_or(timestamp);
            last.trace.push(TraceEntry {
                progress,
                message: message.to_string(),
                datetime,
                timestamp,
                elapsed_time: timestamp - prev,
            });
            return;
        }
        self.components.push(ComponentTrace {
            component_id: component_id.to_string(),
            trace: vec![TraceEntry {
                progress,
                message: message.to_string(),
                datetime,
                timestamp,
                elapsed_time: 0.0,
            }],
        });
    }

    /// Weighted overall progress — pipeline.py:78-95:
    /// `finished += last_progress * (1 / component_count)`; a negative
    /// progress short-circuits the whole run to -1.
    pub fn overall_progress(&self, component_count: usize) -> f64 {
        if component_count == 0 {
            return 0.0;
        }
        let percentage = 1.0 / component_count as f64;
        let mut finished = 0.0;
        for component in &self.components {
            if let Some(entry) = component.trace.last() {
                if entry.progress < 0.0 {
                    finished = -1.0;
                    break;
                }
                finished += entry.progress * percentage;
            }
        }
        finished
    }
}

/// Chunk data contract — `api/apps/restful_apis/chunk_api.py` `Chunk`.
///
/// The REST-facing shape of a chunk, converted from the pipeline's
/// intermediate [`crate::Chunk`] (whose free-form `metadata` map carries the
/// `docnm_kwd` / `important_kwd` / `tag_kwd` / `questions` / `question_tks`
/// / `image_id` / `available_int` / `position_int` keys produced by the
/// chunkers and the key-term extractor).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkDoc {
    /// Chunk id (`doc_id_position`).
    #[serde(default)]
    pub id: String,
    /// Chunk text content.
    #[serde(default)]
    pub content: String,
    /// Source document id.
    #[serde(default)]
    pub document_id: String,
    /// Source document name.
    #[serde(default)]
    pub docnm_kwd: String,
    /// Auto-extracted key terms (`important_kwd`).
    #[serde(default)]
    pub important_keywords: Vec<String>,
    /// Chunk tags (`tag_kwd`).
    #[serde(default)]
    pub tag_kwd: Vec<String>,
    /// RAGFlow `chunk_questions` derived questions.
    #[serde(default)]
    pub questions: Vec<String>,
    /// Tokenized question string.
    #[serde(default)]
    pub question_tks: String,
    /// Referenced image id for image chunks.
    #[serde(default)]
    pub image_id: String,
    /// Whether the chunk participates in retrieval.
    #[serde(default = "default_available")]
    pub available: bool,
    /// `[x1, y1, x2, y2, page]` bounding boxes (each sublist length 5).
    #[serde(default)]
    pub positions: Vec<Vec<i32>>,
}

fn default_available() -> bool {
    true
}

/// Parse a JSON list-of-strings metadata value (e.g. `tag_kwd`, `questions`),
/// falling back to whitespace splitting for plain string values.
fn string_list_from_metadata(metadata: &HashMap<String, String>, key: &str) -> Vec<String> {
    let Some(raw) = metadata.get(key) else {
        return Vec::new();
    };
    if let Ok(value) = serde_json::from_str::<Vec<String>>(raw) {
        return value;
    }
    raw.split_whitespace().map(str::to_string).collect()
}

impl From<&crate::Chunk> for ChunkDoc {
    fn from(chunk: &crate::Chunk) -> Self {
        let metadata = &chunk.metadata;
        let docnm_kwd = metadata
            .get("docnm_kwd")
            .or_else(|| metadata.get("file_name"))
            .cloned()
            .unwrap_or_default();
        let available = metadata
            .get("available_int")
            .map(|v| v != "0")
            .unwrap_or(true);
        let positions = metadata
            .get("position_int")
            .and_then(|raw| serde_json::from_str::<Vec<Vec<i32>>>(raw).ok())
            .unwrap_or_default()
            .into_iter()
            // pydantic validator: each sublist must have length 5.
            .filter(|position| position.len() == 5)
            .collect();
        ChunkDoc {
            id: chunk.id.clone(),
            content: chunk.content.clone(),
            document_id: chunk.doc_id.to_string(),
            docnm_kwd,
            important_keywords: string_list_from_metadata(metadata, "important_kwd"),
            tag_kwd: string_list_from_metadata(metadata, "tag_kwd"),
            questions: string_list_from_metadata(metadata, "questions"),
            question_tks: metadata.get("question_tks").cloned().unwrap_or_default(),
            image_id: metadata.get("image_id").cloned().unwrap_or_default(),
            available,
            positions,
        }
    }
}

impl From<crate::Chunk> for ChunkDoc {
    fn from(chunk: crate::Chunk) -> Self {
        ChunkDoc::from(&chunk)
    }
}

impl ChunkDoc {
    /// The pydantic `@validator("positions")` — every sublist must have
    /// exactly 5 elements (`[x1, y1, x2, y2, page]`).
    pub fn validate_positions(&self) -> Result<()> {
        for position in &self.positions {
            if position.len() != 5 {
                anyhow::bail!(
                    "Each sublist in positions must have a length of 5 (got {:?})",
                    position
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Chunk;
    use uuid::Uuid;

    fn sample_chunk() -> Chunk {
        let mut chunk = Chunk {
            id: "doc_0".into(),
            content: "Some chunk text".into(),
            content_type: "text".into(),
            doc_id: Uuid::new_v4(),
            position: 0,
            token_count: 4,
            embedding: None,
            metadata: HashMap::new(),
        };
        chunk.metadata.insert("file_name".into(), "cv.pdf".into());
        chunk
            .metadata
            .insert("important_kwd".into(), "rust rag pipeline".into());
        chunk
            .metadata
            .insert("tag_kwd".into(), r#"["好公司","行业好公司"]"#.into());
        chunk
            .metadata
            .insert("position_int".into(), "[[1,1,3,2,4],[2,2,4,3,5]]".into());
        chunk.metadata.insert("available_int".into(), "1".into());
        chunk
    }

    #[test]
    fn from_upstream_check_payloads_mirrors_python_validator() {
        // output_format=chunks with a present (even empty) chunks list is valid.
        let chunks_ok = FromUpstream {
            output_format: Some(OutputFormat::Chunks),
            chunks: Some(vec![]),
            ..FromUpstream::default()
        };
        assert!(chunks_ok.check_payloads().is_ok());

        // markdown requires the markdown payload.
        let markdown_missing = FromUpstream {
            output_format: Some(OutputFormat::Markdown),
            ..FromUpstream::default()
        };
        let err = markdown_missing.check_payloads().unwrap_err().to_string();
        assert!(err.contains("output_format=markdown requires a markdown payload"));
        let markdown_ok = FromUpstream {
            output_format: Some(OutputFormat::Markdown),
            markdown_result: Some("# t".into()),
            ..FromUpstream::default()
        };
        assert!(markdown_ok.check_payloads().is_ok());

        // text requires the text payload; html reuses the same message.
        let text_err = FromUpstream {
            output_format: Some(OutputFormat::Text),
            ..FromUpstream::default()
        }
        .check_payloads()
        .unwrap_err()
        .to_string();
        assert!(text_err.contains("output_format=text requires a text payload"));
        let html_err = FromUpstream {
            output_format: Some(OutputFormat::Html),
            ..FromUpstream::default()
        }
        .check_payloads()
        .unwrap_err()
        .to_string();
        assert!(html_err.contains("requires a html payload"));

        // No chunks and no json payload → error, even with a text result set.
        let json_missing = FromUpstream {
            text_result: Some("hello".into()),
            ..FromUpstream::default()
        };
        let err = json_missing.check_payloads().unwrap_err().to_string();
        assert!(err.contains("JSON list payload is required"));

        // json payload present → ok.
        let json_ok = FromUpstream {
            json_result: Some(vec![json!({"content": "c"})]),
            ..FromUpstream::default()
        };
        assert!(json_ok.check_payloads().is_ok());
    }

    #[test]
    fn from_upstream_accepts_underscore_aliases_on_deserialize_and_serializes_by_alias() {
        // pydantic populate_by_name: `_created_time` and `json` wire names.
        let parsed: FromUpstream = serde_json::from_str(
            r#"{"_created_time": 1.5, "_elapsed_time": 0.25, "name": "a.pdf", "json": [{"content":"x"}]}"#,
        )
        .expect("aliases accepted");
        assert_eq!(parsed.created_time, Some(1.5));
        assert_eq!(parsed.elapsed_time, Some(0.25));
        assert_eq!(parsed.name, "a.pdf");
        assert!(parsed.json_result.is_some());
        assert!(parsed.check_payloads().is_ok());

        // Serialization emits the by-alias wire names.
        let value = serde_json::to_value(&parsed).expect("serialize");
        assert_eq!(value["_created_time"], 1.5);
        assert_eq!(value["_elapsed_time"], 0.25);
        assert_eq!(value["json"][0]["content"], "x");
    }

    #[test]
    fn component_output_tracks_created_elapsed_and_error_keys() {
        let mut out = ComponentOutput::begin();
        out.set_output("name", json!("resume.pdf"));
        assert!(out.error().is_none());
        out.set_error("boom");
        assert_eq!(out.error(), Some("boom"));
        out.finish();

        let map = out.into_map();
        assert!(map.contains_key(CREATED_TIME_KEY));
        assert!(map.contains_key(ELAPSED_TIME_KEY));
        assert_eq!(map[ERROR_KEY], "boom");
        assert_eq!(map["name"], "resume.pdf");
    }

    #[test]
    fn file_outputs_emits_name_and_optional_file() {
        let with_file = file_outputs("a.pdf", Some(json!({"id": "f1", "name": "a.pdf"})));
        let map = with_file.into_map();
        assert_eq!(map["name"], "a.pdf");
        assert_eq!(map["file"]["id"], "f1");

        // Bound-doc mode: only name.
        let without = file_outputs("b.pdf", None).into_map();
        assert_eq!(without["name"], "b.pdf");
        assert!(!without.contains_key("file"));
    }

    #[test]
    fn flow_log_accumulates_traces_and_computes_overall_progress() {
        let mut log = FlowLog::default();
        log.append("Begin", 1.0, "File fetched.");
        log.append("Begin", 1.0, "File fetched again.");
        log.append("Chunk", 0.5, "Half chunked.");
        log.append("END", 1.0, "{}");

        // Same component id appends to its own trace.
        assert_eq!(log.components.len(), 3);
        assert_eq!(log.components[0].component_id, "Begin");
        assert_eq!(log.components[0].trace.len(), 2);
        assert_eq!(log.components[0].trace[1].message, "File fetched again.");
        assert!(log.components[0].trace[1].elapsed_time >= 0.0);

        // 3 components: (1.0 + 0.5 + 1.0) / 3.
        let progress = log.overall_progress(3);
        assert!((progress - 5.0 / 6.0).abs() < 1e-9);

        // A canceled component short-circuits to -1 (pipeline.py:84-87).
        let mut canceled = FlowLog::default();
        canceled.append("A", -1.0, "[CANCEL]");
        canceled.append("B", 1.0, "done");
        assert_eq!(canceled.overall_progress(2), -1.0);

        // Zero components → 0.
        assert_eq!(FlowLog::default().overall_progress(4), 0.0);
    }

    #[test]
    fn chunk_doc_converts_from_pipeline_chunk_contract() {
        let chunk = sample_chunk();
        let doc = ChunkDoc::from(&chunk);

        assert_eq!(doc.id, "doc_0");
        assert_eq!(doc.content, "Some chunk text");
        assert_eq!(doc.document_id, chunk.doc_id.to_string());
        // docnm_kwd falls back to file_name.
        assert_eq!(doc.docnm_kwd, "cv.pdf");
        assert_eq!(doc.important_keywords, vec!["rust", "rag", "pipeline"]);
        assert_eq!(doc.tag_kwd, vec!["好公司", "行业好公司"]);
        assert!(doc.available);
        assert_eq!(
            doc.positions,
            vec![vec![1, 1, 3, 2, 4], vec![2, 2, 4, 3, 5]]
        );
        doc.validate_positions().expect("all positions length 5");

        // available_int = "0" hides the chunk from retrieval.
        let mut hidden = chunk.clone();
        hidden.metadata.insert("available_int".into(), "0".into());
        assert!(!ChunkDoc::from(&hidden).available);

        // Invalid positions are filtered at conversion and rejected by the
        // REST validator.
        let mut bad = chunk.clone();
        bad.metadata
            .insert("position_int".into(), "[[1,2,3]]".into());
        let doc = ChunkDoc::from(&bad);
        assert!(doc.positions.is_empty());
        assert!(doc.validate_positions().is_ok()); // nothing left to reject
        let mut manual = doc;
        manual.positions.push(vec![1, 2, 3]);
        assert!(manual.validate_positions().is_err());
    }

    #[test]
    fn resume_corp_entity_resources_align_with_ner_semantics() {
        // CorporationEntity normalization — corporations.py corpNorm:
        // 腾讯科技有限公司 → suffix-stripped 腾讯 (early return < 5 chars).
        assert_eq!(crate::resume::corp_norm("腾讯科技有限公司", false), "腾讯");
        // 阿里巴巴集团 → CORP_TKS ("集团") dropped during token filtering.
        assert_eq!(crate::resume::corp_norm("阿里巴巴集团", false), "阿里巴巴");
        // rmNoise strips parenthetical content.
        assert_eq!(crate::resume::rm_noise("蚂蚁集团（杭州）"), "蚂蚁集团");

        // good_corp.json: 蚂蚁集团 is a normalized member of the corpus.
        assert!(crate::resume::corp_is_good("蚂蚁集团"));
        // 外派 always disqualifies (corporations.py:104-105).
        assert!(!crate::resume::corp_is_good("蚂蚁集团外派"));
        assert!(!crate::resume::corp_is_good("某不知名小公司"));

        // corp_tag.json exact-match on alphanumeric keys (2k games).
        assert_eq!(
            crate::resume::corp_tag("2k games"),
            vec!["好游戏", "行业好公司"]
        );

        // corp_baike_len.csv: cid 376 → len 155.
        assert_eq!(crate::resume::baike("376", 0), 155);
        assert_eq!(crate::resume::baike("99999999", 7), 7);
    }
}
