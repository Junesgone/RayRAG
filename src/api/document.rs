//! Document lifecycle manager.
//! Full document flow: upload → parse status → re-parse → delete → download.
//! Replaces RAGFlow's document_api.py (79K lines).

use axum::{
    Json,
    body::Bytes,
    extract::{Extension, Path, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, RwLock};

use crate::api::metadata_config::{py_value_str, python_repr, raised_error};
use crate::server::{AppState, AuthContext, api_error_result, code, kb_accessible, kb_manageable};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocRecord {
    pub id: String,
    pub name: String,
    pub kb_id: String,
    pub size: usize,
    /// Stored filename relative to the uploads directory.
    #[serde(default)]
    pub storage_name: String,
    /// xxHash128 content fingerprint, compatible with RAGFlow change detection semantics.
    #[serde(default)]
    pub content_hash: String,
    /// Content fingerprint that produced the currently committed search index.
    #[serde(default)]
    pub indexed_content_hash: String,
    /// Parse status: "UNSTARTED", "RUNNING", "DONE", "FAILED"
    pub run: String,
    pub progress: f32,
    pub progress_msg: String,
    pub chunk_count: usize,
    pub created_at: u64,
    pub updated_at: u64,
    /// Upstream `Document.status`: `"1"` when the document takes part in retrieval,
    /// `"0"` when the operator disabled it. Files on disk written before this field
    /// existed are enabled, which is what they were.
    #[serde(default = "enabled_status")]
    pub status: String,
    /// Upstream `Document.parser_id`: the document-scoped chunk method a PATCH may set.
    /// `None` keeps following the dataset's parser face, which is what every upload does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser_id: Option<String>,
    /// Upstream `Document.parser_config`: the document-scoped parser configuration a PATCH
    /// may replace, stored as sent after the request-level legacy normalization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parser_config: Option<serde_json::Value>,
    /// Upstream `Document.pipeline_id`. Stored for parity; pipeline execution is not
    /// implemented in this engine, so the value round-trips without changing parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipeline_id: Option<String>,
}

/// Default document status: enabled (upstream stores `"1"`).
pub fn enabled_status() -> String {
    "1".to_string()
}

#[derive(Deserialize)]
pub struct UploadRequest {
    pub kb_id: String,
    pub name: String,
}

/// Global document store.
pub struct DocStore {
    docs: RwLock<HashMap<String, DocRecord>>,
    file_path: String,
    save_lock: Mutex<()>,
}

impl DocStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        let docs = if std::path::Path::new(path).exists() {
            let data = std::fs::read_to_string(path)?;
            let list: Vec<DocRecord> = serde_json::from_str(&data).map_err(|error| {
                anyhow::anyhow!("Failed to parse document metadata '{}': {error}", path)
            })?;
            list.into_iter().map(|d| (d.id.clone(), d)).collect()
        } else {
            HashMap::new()
        };
        Ok(Self {
            docs: RwLock::new(docs),
            file_path: path.into(),
            save_lock: Mutex::new(()),
        })
    }

    pub fn list(&self, kb_id: &str) -> Vec<DocRecord> {
        self.docs
            .read()
            .unwrap()
            .values()
            .filter(|d| d.kb_id == kb_id)
            .cloned()
            .collect()
    }

    pub fn list_all(&self) -> Vec<DocRecord> {
        self.docs.read().unwrap().values().cloned().collect()
    }

    pub fn get(&self, id: &str) -> Option<DocRecord> {
        self.docs.read().unwrap().get(id).cloned()
    }

    pub fn insert(&self, doc: DocRecord) -> anyhow::Result<()> {
        self.mutate(|docs| {
            docs.insert(doc.id.clone(), doc);
            Ok(())
        })
    }

    /// Insert a document while reserving a RAGFlow-compatible unique name in its KB.
    pub fn insert_unique(&self, mut doc: DocRecord) -> anyhow::Result<DocRecord> {
        self.mutate(|docs| {
            if docs.contains_key(&doc.id) {
                anyhow::bail!("Document ID already exists");
            }
            doc.name = crate::naming::duplicate_name(&doc.name, |candidate| {
                docs.values()
                    .any(|existing| existing.kb_id == doc.kb_id && existing.name == candidate)
            })?;
            docs.insert(doc.id.clone(), doc.clone());
            Ok(doc)
        })
    }

    pub fn update_status(
        &self,
        id: &str,
        run: &str,
        progress: f32,
        msg: &str,
    ) -> anyhow::Result<bool> {
        self.update_parse_result(id, run, progress, msg, None)
    }

    /// Set the retrieval status (`"1"` enabled, `"0"` disabled) of many documents.
    ///
    /// Returns the ids that changed, so the caller can report a truthful count instead
    /// of assuming every requested document was updated.
    pub fn set_status(&self, ids: &[String], status: &str) -> anyhow::Result<Vec<String>> {
        if !matches!(status, "0" | "1") {
            anyhow::bail!("Document status must be '0' or '1'");
        }
        self.mutate_if_changed(|docs| {
            let mut changed = Vec::new();
            for id in ids {
                if let Some(doc) = docs.get_mut(id)
                    && doc.status != status
                {
                    doc.status = status.to_string();
                    doc.updated_at = crate::api::document::now_ms();
                    changed.push(id.clone());
                }
            }
            let dirty = !changed.is_empty();
            Ok((changed, dirty))
        })
    }

    /// Rename a document (display name only; storage name unchanged).
    pub fn rename(&self, id: &str, new_name: &str) -> anyhow::Result<bool> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            anyhow::bail!("Document name cannot be empty");
        }
        self.mutate_if_changed(|docs| {
            let Some(d) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            d.name = new_name.to_string();
            Ok((true, true))
        })
    }

    /// `reset_document_for_reparse`: progress fields back to zero, optionally a new parser
    /// face. `pipeline_id` always carries the upstream call's value — the empty string clears
    /// the pipeline; `parser_id` is only overwritten when a new method is given.
    pub fn reset_for_reparse(
        &self,
        id: &str,
        parser_id: Option<&str>,
        pipeline_id: &str,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let Some(d) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            d.run = "UNSTARTED".to_string();
            d.progress = 0.0;
            d.progress_msg = String::new();
            if let Some(parser) = parser_id {
                d.parser_id = Some(parser.to_string());
            }
            d.pipeline_id = if pipeline_id.is_empty() {
                None
            } else {
                Some(pipeline_id.to_string())
            };
            d.updated_at = now_ms();
            Ok((true, true))
        })
    }

    /// Replace the document-scoped parser configuration
    /// (`DocumentService.update_parser_config`).
    pub fn set_parser_config(
        &self,
        id: &str,
        parser_config: serde_json::Value,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let Some(d) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            if d.parser_config.as_ref() == Some(&parser_config) {
                return Ok((true, false));
            }
            d.parser_config = Some(parser_config);
            d.updated_at = now_ms();
            Ok((true, true))
        })
    }

    pub fn update_parse_result(
        &self,
        id: &str,
        run: &str,
        progress: f32,
        msg: &str,
        chunk_count: Option<usize>,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let Some(d) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            d.run = run.into();
            d.progress = progress;
            d.progress_msg = msg.into();
            if let Some(chunk_count) = chunk_count {
                d.chunk_count = chunk_count;
            }
            d.updated_at = now_ms();
            Ok((true, true))
        })
    }

    pub fn mark_indexed(
        &self,
        id: &str,
        content_hash: &str,
        chunk_count: usize,
        msg: &str,
    ) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let Some(doc) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            doc.run = "DONE".into();
            doc.progress = 1.0;
            doc.progress_msg = msg.into();
            doc.chunk_count = chunk_count;
            doc.indexed_content_hash = content_hash.into();
            doc.updated_at = now_ms();
            Ok((true, true))
        })
    }

    pub fn set_chunk_count(&self, id: &str, chunk_count: usize) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let Some(doc) = docs.get_mut(id) else {
                return Ok((false, false));
            };
            if doc.chunk_count == chunk_count {
                return Ok((true, false));
            }
            doc.chunk_count = chunk_count;
            doc.updated_at = now_ms();
            Ok((true, true))
        })
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|docs| {
            let deleted = docs.remove(id).is_some();
            Ok((deleted, deleted))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, DocRecord>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_if_changed(|docs| mutation(docs).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, DocRecord>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut docs = self.docs.write().unwrap();
        let previous = docs.clone();
        let (value, changed) = mutation(&mut docs)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<DocRecord> = docs.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *docs = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist(&self, docs: &[DocRecord]) -> anyhow::Result<()> {
        let data = serde_json::to_vec_pretty(&docs)?;
        crate::persistence::atomic_write(std::path::Path::new(&self.file_path), &data)
    }
}

/// Upstream `TaskStatus.name` for each status this engine stores: fetch
/// surfaces report `UNSTART` / `FAIL` / `CANCEL`, while the engine keeps the
/// longer `UNSTARTED` / `FAILED` / `CANCELLED` spellings internally.
pub(crate) fn run_api_text(run: &str) -> &str {
    match run {
        "UNSTARTED" => "UNSTART",
        "RUNNING" => "RUNNING",
        "CANCELLED" => "CANCEL",
        "DONE" => "DONE",
        "FAILED" => "FAIL",
        _ => "UNSTART",
    }
}

/// Internal status → upstream numeric `TaskStatus.value` (`"0".."5"`).
fn run_numeric(run: &str) -> Option<&'static str> {
    match run {
        "UNSTARTED" => Some("0"),
        "RUNNING" => Some("1"),
        "CANCELLED" => Some("2"),
        "DONE" => Some("3"),
        "FAILED" => Some("4"),
        _ => None,
    }
}

/// `api/utils/file_utils.py::filename_type` — the `type` a row reports.
fn filename_type(name: &str) -> &'static str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() || base.chars().count() > 255 {
        return "other";
    }
    let lower = base.to_lowercase();
    if lower.ends_with(".pdf") {
        return "pdf";
    }
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    const DOC_EXTS: &[&str] = &[
        "msg", "eml", "doc", "docx", "ppt", "pptx", "yml", "xml", "htm", "json", "jsonl", "ldjson",
        "csv", "txt", "ini", "xls", "xlsx", "wps", "rtf", "hlp", "pages", "numbers", "key", "md",
        "mdx", "py", "js", "java", "c", "cpp", "h", "php", "go", "ts", "sh", "cs", "kt", "html",
        "sql", "epub",
    ];
    const AURAL_EXTS: &[&str] = &[
        "wav", "flac", "ape", "alac", "wavpack", "wv", "mp3", "aac", "ogg", "vorbis", "opus",
    ];
    const VISUAL_EXTS: &[&str] = &[
        "jpg", "jpeg", "png", "tif", "gif", "bmp", "pcx", "tga", "exif", "fpx", "svg", "psd",
        "cdr", "pcd", "dxf", "ufo", "eps", "ai", "raw", "wmf", "webp", "avif", "apng", "icon",
        "ico", "mpg", "mpeg", "avi", "rm", "rmvb", "mov", "wmv", "asf", "dat", "asx", "wvx", "mpe",
        "mpa", "mp4", "mkv",
    ];
    if DOC_EXTS.contains(&ext) {
        "doc"
    } else if AURAL_EXTS.contains(&ext) {
        "aural"
    } else if VISUAL_EXTS.contains(&ext) {
        "visual"
    } else {
        "other"
    }
}

/// The stored `suffix`: the extension after the last dot, lowercased.
fn doc_suffix(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() && !ext.contains('/') => ext.to_lowercase(),
        _ => String::new(),
    }
}

/// The parsed query string as decoded `(name, value)` pairs (`getlist` order).
fn query_pairs(raw: Option<&str>) -> Vec<(String, String)> {
    raw.map(|raw| {
        url::form_urlencoded::parse(raw.as_bytes())
            .into_owned()
            .collect()
    })
    .unwrap_or_default()
}

fn q_first<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// `request.args.getlist(key)`, including the `key[]` spelling upstream reads too.
fn q_list(pairs: &[(String, String)], key: &str) -> Vec<String> {
    let bracketed = format!("{key}[]");
    pairs
        .iter()
        .filter(|(name, _)| *name == key || *name == bracketed)
        .map(|(_, value)| value.clone())
        .collect()
}

/// `int(q.get(key, 0))` — a malformed bound reads as "no bound".
fn q_int(pairs: &[(String, String)], key: &str) -> i64 {
    q_first(pairs, key)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// `validate_rest_api_page`: silent fallback to `DEFAULT_PAGE`.
fn validate_page(value: Option<&str>) -> usize {
    value
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|page| *page >= 1)
        .map(|page| page as usize)
        .unwrap_or(1)
}

/// `validate_rest_api_page_size`: silent fallback to `DEFAULT_PAGE_SIZE`; the
/// public maximum raises — upstream lets that exception reach the generic 500
/// handler instead of shaping a business error.
fn validate_page_size(value: Option<&str>) -> Result<usize, ()> {
    match value.and_then(|value| value.trim().parse::<i64>().ok()) {
        Some(size) if size > 100 => Err(()),
        Some(size) if size >= 1 => Ok(size as usize),
        _ => Ok(30),
    }
}

/// `_parse_run_status_filter`: `TaskStatus` names map onto their numeric
/// values; anything else must already be one of `"0".."5"`.
fn parse_run_filter(values: &[String]) -> Result<Vec<String>, Vec<String>> {
    let mut converted = Vec::new();
    let mut invalid: Vec<String> = Vec::new();
    for raw in values {
        let value = raw.trim();
        if value.is_empty() {
            continue;
        }
        let numeric = match value.to_uppercase().as_str() {
            "UNSTART" => "0",
            "RUNNING" => "1",
            "CANCEL" => "2",
            "DONE" => "3",
            "FAIL" => "4",
            "SCHEDULE" => "5",
            _ => value,
        }
        .to_string();
        if !matches!(numeric.as_str(), "0" | "1" | "2" | "3" | "4" | "5")
            && !invalid.contains(&numeric)
        {
            invalid.push(numeric.clone());
        }
        converted.push(numeric);
    }
    if invalid.is_empty() {
        Ok(converted)
    } else {
        Err(invalid)
    }
}

/// `VALID_FILE_TYPES` (api/db/__init__.py) — the `types` filter vocabulary.
const VALID_FILE_TYPES: &[&str] = &[
    "pdf", "doc", "visual", "aural", "virtual", "folder", "other",
];

/// Upstream's `str(vv)` rendering for numeric metadata values.
fn python_str_number(number: &serde_json::Number) -> String {
    if let Some(int) = number.as_i64() {
        return int.to_string();
    }
    if let Some(uint) = number.as_u64() {
        return uint.to_string();
    }
    match number.as_f64() {
        Some(float) if float.fract() == 0.0 => format!("{float:.1}"),
        Some(float) => float.to_string(),
        None => number.to_string(),
    }
}

/// `get_filter_by_kb_id`'s per-value expansion: scalars pass the non-blank
/// check, lists expand; anything richer is skipped (upstream would `str()` it).
fn metadata_value_strings(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(text) => {
            if text.trim().is_empty() {
                Vec::new()
            } else {
                vec![text.clone()]
            }
        }
        serde_json::Value::Number(number) => vec![python_str_number(number)],
        serde_json::Value::Bool(flag) => vec![if *flag { "True" } else { "False" }.to_string()],
        serde_json::Value::Array(items) => items.iter().flat_map(metadata_value_strings).collect(),
        _ => Vec::new(),
    }
}

/// The `chunk_method` names `UpdateDocumentReq` accepts
/// (`validation_utils.UpdateDocumentReq.validate_document_chunk_method`).
const VALID_CHUNK_METHODS: [&str; 14] = [
    "naive",
    "manual",
    "qa",
    "table",
    "paper",
    "book",
    "laws",
    "presentation",
    "picture",
    "one",
    "knowledge_graph",
    "email",
    "audio",
    "tag",
];

/// The document-scoped business errors: `get_error_data_result` shape — HTTP 200, two keys,
/// no `data` (round13 probes `upd2_name_ext` / `upd2_cc_wrong` / …).
fn update_data_error(message: &str) -> Response {
    api_error_result(StatusCode::OK, code::INVALID_OR_MISSING_DATA, message)
}

/// `format_validation_error_message` for one pydantic field error.
fn field_error(location: &str, message: &str, value: &serde_json::Value) -> String {
    format!(
        "Field: <{location}> - Message: <{message}> - Value: <{}>",
        py_value_str(value)
    )
}

/// `_normalize_legacy_raptor_config`: drop the RAPTOR fields the current schema removed and
/// lift a sub-512 `max_token` back to the floor. Runs on the request body in place.
fn normalize_legacy_raptor_config(request: &mut serde_json::Map<String, serde_json::Value>) {
    let Some(raptor) = request
        .get_mut("parser_config")
        .and_then(serde_json::Value::as_object_mut)
        .and_then(|config| config.get_mut("raptor"))
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    raptor.remove("threshold");
    raptor.remove("clustering_method");
    raptor.remove("tree_builder");
    let lift = raptor
        .get("max_token")
        .and_then(serde_json::Value::as_f64)
        .map(|number| number < 512.0)
        .unwrap_or(false);
    if lift {
        raptor.insert("max_token".into(), serde_json::json!(512));
    }
}

/// Lax integer coercion with pydantic's wording for the failure case
/// (`UpdateDocumentReq.enabled` / `chunk_count` / `token_count` are plain `int` fields).
fn coerce_int(value: &serde_json::Value) -> Result<i64, &'static str> {
    match value {
        serde_json::Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Ok(int)
            } else if let Some(float) = number.as_f64() {
                if float.fract() == 0.0 && float.abs() <= i64::MAX as f64 {
                    Ok(float as i64)
                } else {
                    Err("Input should be a valid integer, got a number with a fractional part")
                }
            } else {
                Ok(0)
            }
        }
        serde_json::Value::Bool(flag) => Ok(i64::from(*flag)),
        serde_json::Value::String(text) => text
            .trim()
            .parse::<i64>()
            .map_err(|_| "Input should be a valid integer"),
        _ => Err("Input should be a valid integer"),
    }
}

/// Lax float coercion (`UpdateDocumentReq.progress`, `ParserConfig.filename_embd_weight`).
fn coerce_float(value: &serde_json::Value) -> Result<f64, &'static str> {
    match value {
        serde_json::Value::Number(number) => {
            number.as_f64().ok_or("Input should be a valid number")
        }
        serde_json::Value::String(text) => text
            .trim()
            .parse::<f64>()
            .map_err(|_| "Input should be a valid number"),
        _ => Err("Input should be a valid number"),
    }
}

/// Python's `math.isclose` with the default tolerances (rel 1e-09, abs 0.0).
fn py_isclose(left: f64, right: f64) -> bool {
    (left - right).abs() <= 1e-9_f64 * left.abs().max(right.abs())
}

/// `pathlib.Path(name).suffix`: the last path component's final dot-segment, empty for
/// dotfiles, trailing dots and no-dot names. Callers lowercase first, as upstream does.
fn python_suffix(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or("");
    match base.rfind('.') {
        Some(index) if index > 0 && index + 1 < base.len() => base[index..].to_string(),
        _ => String::new(),
    }
}

/// `UpdateDocumentReq` validation in declaration order, joined the way
/// `format_validation_error_message` joins pydantic's report.
fn pydantic_update_errors(request: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    let mut errors = Vec::new();
    // name: `str | None`, `max_length=65535`.
    if let Some(value) = request.get("name")
        && !value.is_null()
    {
        match value.as_str() {
            None => errors.push(field_error("name", "Input should be a valid string", value)),
            Some(text) if text.chars().count() > 65535 => errors.push(field_error(
                "name",
                "String should have at most 65535 characters",
                value,
            )),
            Some(_) => {}
        }
    }
    // chunk_method: type, length, then the enum validator's wording
    // (`field_validator(mode="after")` with a PydanticCustomError — no "Value error, " prefix).
    if let Some(value) = request.get("chunk_method")
        && !value.is_null()
    {
        match value.as_str() {
            None => errors.push(field_error(
                "chunk_method",
                "Input should be a valid string",
                value,
            )),
            Some(method) => {
                if method.chars().count() > 65535 {
                    errors.push(field_error(
                        "chunk_method",
                        "String should have at most 65535 characters",
                        value,
                    ));
                }
                if !method.is_empty() && !VALID_CHUNK_METHODS.contains(&method) {
                    errors.push(format!(
                        "Field: <chunk_method> - Message: <`chunk_method` {method} doesn't exist> - Value: <{method}>"
                    ));
                }
            }
        }
    }
    // pipeline_id: `str | None`, `max_length=65535`.
    if let Some(value) = request.get("pipeline_id")
        && !value.is_null()
    {
        match value.as_str() {
            None => errors.push(field_error(
                "pipeline_id",
                "Input should be a valid string",
                value,
            )),
            Some(text) if text.chars().count() > 65535 => errors.push(field_error(
                "pipeline_id",
                "String should have at most 65535 characters",
                value,
            )),
            Some(_) => {}
        }
    }
    // enabled: int, 0..=1.
    if let Some(value) = request.get("enabled").filter(|value| !value.is_null()) {
        match coerce_int(value) {
            Err(message) => errors.push(field_error("enabled", message, value)),
            Ok(number) if number > 1 => errors.push(field_error(
                "enabled",
                "Input should be less than or equal to 1",
                value,
            )),
            Ok(number) if number < 0 => errors.push(field_error(
                "enabled",
                "Input should be greater than or equal to 0",
                value,
            )),
            Ok(_) => {}
        }
    }
    // chunk_count / token_count: int, ge=0.
    for field in ["chunk_count", "token_count"] {
        if let Some(value) = request.get(field).filter(|value| !value.is_null()) {
            match coerce_int(value) {
                Err(message) => errors.push(field_error(field, message, value)),
                Ok(number) if number < 0 => errors.push(field_error(
                    field,
                    "Input should be greater than or equal to 0",
                    value,
                )),
                Ok(_) => {}
            }
        }
    }
    // progress: float, 0..=1.
    if let Some(value) = request.get("progress").filter(|value| !value.is_null()) {
        match coerce_float(value) {
            Err(message) => errors.push(field_error("progress", message, value)),
            Ok(number) if number > 1.0 => errors.push(field_error(
                "progress",
                "Input should be less than or equal to 1",
                value,
            )),
            Ok(number) if number < 0.0 => errors.push(field_error(
                "progress",
                "Input should be greater than or equal to 0",
                value,
            )),
            Ok(_) => {}
        }
    }
    // parser_config: reject_ext first (model_validator before), then the strict nested checks.
    if let Some(value) = request
        .get("parser_config")
        .filter(|value| !value.is_null())
    {
        if let Some(config) = value.as_object() {
            if config.contains_key("ext") {
                errors.push(format!(
                    "Field: <parser_config> - Message: <Value error, parser_config.ext is not supported; send parser configuration fields directly> - Value: <{}>",
                    python_repr(value)
                ));
            } else {
                pydantic_parser_config_errors(config, &mut errors);
            }
        } else {
            errors.push(field_error(
                "parser_config",
                "Input should be a valid dictionary or instance of ParserConfig",
                value,
            ));
        }
    }
    // meta_fields: dict type first, then the value-shape validator (one error, first offender).
    if let Some(value) = request.get("meta_fields").filter(|value| !value.is_null()) {
        if let Some(fields) = value.as_object() {
            for field_value in fields.values() {
                if let Some(items) = field_value.as_array() {
                    let scalars = items
                        .iter()
                        .all(|item| item.is_string() || item.is_number() || item.is_boolean());
                    if !scalars {
                        errors.push(format!(
                            "Field: <meta_fields> - Message: <The type is not supported in list: {}> - Value: <{}>",
                            python_repr(field_value),
                            python_repr(value)
                        ));
                        break;
                    }
                } else if !(field_value.is_string()
                    || field_value.is_number()
                    || field_value.is_boolean())
                {
                    errors.push(format!(
                        "Field: <meta_fields> - Message: <The type is not supported: {}> - Value: <{}>",
                        python_repr(field_value),
                        python_repr(value)
                    ));
                    break;
                }
            }
        } else {
            errors.push(field_error(
                "meta_fields",
                "Input should be a valid dictionary",
                value,
            ));
        }
    }
    errors
}

/// The strict nested checks of `ParserConfig` (`extra="allow"`), in the model's declaration
/// order so multi-error reports keep pydantic's sequence.
fn pydantic_parser_config_errors(
    config: &serde_json::Map<String, serde_json::Value>,
    errors: &mut Vec<String>,
) {
    fn int_field(
        config: &serde_json::Map<String, serde_json::Value>,
        name: &str,
        min: i64,
        max: i64,
        errors: &mut Vec<String>,
    ) {
        let Some(value) = config.get(name) else {
            return;
        };
        let location = format!("parser_config.{name}");
        match value.as_i64() {
            Some(number) if number < min => errors.push(field_error(
                &location,
                &format!("Input should be greater than or equal to {min}"),
                value,
            )),
            Some(number) if number > max => errors.push(field_error(
                &location,
                &format!("Input should be less than or equal to {max}"),
                value,
            )),
            Some(_) => {}
            None => match value {
                serde_json::Value::Number(number) if number.as_f64().is_some() => {
                    errors.push(field_error(
                        &location,
                        "Input should be a valid integer, got a number with a fractional part",
                        value,
                    ))
                }
                _ => errors.push(field_error(
                    &location,
                    "Input should be a valid integer",
                    value,
                )),
            },
        }
    }

    int_field(config, "auto_keywords", 0, 32, errors);
    int_field(config, "auto_questions", 0, 10, errors);
    int_field(config, "chunk_token_num", 1, 2048, errors);
    if let Some(value) = config.get("delimiter") {
        match value.as_str() {
            None => errors.push(field_error(
                "parser_config.delimiter",
                "Input should be a valid string",
                value,
            )),
            Some("") => errors.push(field_error(
                "parser_config.delimiter",
                "String should have at least 1 character",
                value,
            )),
            Some(_) => {}
        }
    }
    for (name, instance) in [
        ("graphrag", "GraphragConfig"),
        ("parent_child", "ParentChildConfig"),
        ("raptor", "RaptorConfig"),
    ] {
        if let Some(value) = config.get(name)
            && !value.is_object()
        {
            errors.push(field_error(
                &format!("parser_config.{name}"),
                &format!("Input should be a valid dictionary or instance of {instance}"),
                value,
            ));
        }
    }
    if let Some(value) = config.get("html4excel").filter(|value| !value.is_boolean()) {
        errors.push(field_error(
            "parser_config.html4excel",
            "Input should be a valid boolean",
            value,
        ));
    }
    if let Some(value) = config.get("layout_recognize")
        && !value.is_string()
    {
        errors.push(field_error(
            "parser_config.layout_recognize",
            "Input should be a valid string",
            value,
        ));
    }
    if let Some(value) = config
        .get("enable_children")
        .filter(|value| !value.is_boolean())
    {
        errors.push(field_error(
            "parser_config.enable_children",
            "Input should be a valid boolean",
            value,
        ));
    }
    if let Some(value) = config.get("children_delimiter")
        && !value.is_string()
    {
        errors.push(field_error(
            "parser_config.children_delimiter",
            "Input should be a valid string",
            value,
        ));
    }
    int_field(config, "topn_tags", 1, 10, errors);
    if let Some(value) = config
        .get("filename_embd_weight")
        .filter(|value| !value.is_null())
    {
        match coerce_float(value) {
            Err(message) => errors.push(field_error(
                "parser_config.filename_embd_weight",
                message,
                value,
            )),
            Ok(number) if number > 1.0 => errors.push(field_error(
                "parser_config.filename_embd_weight",
                "Input should be less than or equal to 1",
                value,
            )),
            Ok(number) if number < 0.0 => errors.push(field_error(
                "parser_config.filename_embd_weight",
                "Input should be greater than or equal to 0",
                value,
            )),
            Ok(_) => {}
        }
    }
    if let Some(value) = config
        .get("task_page_size")
        .filter(|value| !value.is_null())
    {
        match value.as_i64() {
            Some(number) if number < 1 => errors.push(field_error(
                "parser_config.task_page_size",
                "Input should be greater than or equal to 1",
                value,
            )),
            Some(_) => {}
            None => errors.push(field_error(
                "parser_config.task_page_size",
                "Input should be a valid integer",
                value,
            )),
        }
    }
    if let Some(value) = config.get("pages").filter(|value| !value.is_null()) {
        if let Some(rows) = value.as_array() {
            for (row_index, row) in rows.iter().enumerate() {
                if let Some(cells) = row.as_array() {
                    for (cell_index, cell) in cells.iter().enumerate() {
                        if cell.as_i64().is_none() {
                            errors.push(field_error(
                                &format!("parser_config.pages.{row_index}.{cell_index}"),
                                "Input should be a valid integer",
                                cell,
                            ));
                        }
                    }
                } else {
                    errors.push(field_error(
                        &format!("parser_config.pages.{row_index}"),
                        "Input should be a valid list",
                        row,
                    ));
                }
            }
        } else {
            errors.push(field_error(
                "parser_config.pages",
                "Input should be a valid list",
                value,
            ));
        }
    }
    if let Some(value) = config.get("tag_kb_ids") {
        if let Some(items) = value.as_array() {
            for (index, item) in items.iter().enumerate() {
                if !item.is_string() {
                    errors.push(field_error(
                        &format!("parser_config.tag_kb_ids.{index}"),
                        "Input should be a valid string",
                        item,
                    ));
                }
            }
        } else {
            errors.push(field_error(
                "parser_config.tag_kb_ids",
                "Input should be a valid list",
                value,
            ));
        }
    }
    if let Some(value) = config
        .get("table_column_mode")
        .filter(|value| !value.is_null())
    {
        match value.as_str() {
            Some("auto") | Some("manual") => {}
            _ => errors.push(field_error(
                "parser_config.table_column_mode",
                "Input should be 'auto' or 'manual'",
                value,
            )),
        }
    }
    if let Some(value) = config
        .get("table_column_roles")
        .filter(|value| !value.is_null())
    {
        if let Some(roles) = value.as_object() {
            for (column, role) in roles {
                let normalized = match role.as_str() {
                    Some("vectorize") => Some("indexing"),
                    other => other,
                };
                if !matches!(
                    normalized,
                    Some("indexing") | Some("metadata") | Some("both")
                ) {
                    errors.push(field_error(
                        &format!("parser_config.table_column_roles.{column}"),
                        "Input should be 'indexing', 'metadata' or 'both'",
                        role,
                    ));
                }
            }
        } else {
            errors.push(field_error(
                "parser_config.table_column_roles",
                "Input should be a valid dictionary",
                value,
            ));
        }
    }
    if let Some(value) = config
        .get("table_column_names")
        .filter(|value| !value.is_null())
    {
        if let Some(items) = value.as_array() {
            for (index, item) in items.iter().enumerate() {
                if !item.is_string() {
                    errors.push(field_error(
                        &format!("parser_config.table_column_names.{index}"),
                        "Input should be a valid string",
                        item,
                    ));
                }
            }
        } else {
            errors.push(field_error(
                "parser_config.table_column_names",
                "Input should be a valid list",
                value,
            ));
        }
    }
}

/// `validate_document_update_fields`: immutable values, the rename rules and the chunk-method
/// restrictions — the first failure wins, with upstream's code (101 argument / 102 data).
fn validate_update_fields(
    state: &crate::server::AppState,
    request: &serde_json::Map<String, serde_json::Value>,
    doc: &DocRecord,
    current_token_count: i64,
) -> Option<(String, i32)> {
    if let Some(value) = request.get("chunk_count").filter(|value| !value.is_null())
        && let Ok(requested) = coerce_int(value)
        && requested != doc.chunk_count as i64
    {
        return Some((
            "can't change `chunk_count`".to_string(),
            code::INVALID_OR_MISSING_DATA,
        ));
    }
    if let Some(value) = request.get("token_count").filter(|value| !value.is_null())
        && let Ok(requested) = coerce_int(value)
        && requested != current_token_count
    {
        return Some((
            "can't change `token_count`".to_string(),
            code::INVALID_OR_MISSING_DATA,
        ));
    }
    if let Some(value) = request.get("progress").filter(|value| !value.is_null())
        && let Ok(requested) = coerce_float(value)
        && !py_isclose(requested, f64::from(doc.progress))
    {
        return Some((
            "can't change `progress`".to_string(),
            code::INVALID_OR_MISSING_DATA,
        ));
    }
    if let Some(serde_json::Value::String(new_name)) = request.get("name")
        && new_name != &doc.name
    {
        if new_name.len() > 255 {
            return Some((
                "File name must be 255 bytes or less.".to_string(),
                code::INVALID_ARGUMENT,
            ));
        }
        if python_suffix(&new_name.to_lowercase()) != python_suffix(&doc.name.to_lowercase()) {
            return Some((
                "The extension of file can't be changed".to_string(),
                code::INVALID_ARGUMENT,
            ));
        }
        let duplicated = state
            .docs
            .list(&doc.kb_id)
            .iter()
            .any(|existing| existing.name == *new_name);
        if duplicated {
            return Some((
                "Duplicated document name in the same dataset.".to_string(),
                code::INVALID_OR_MISSING_DATA,
            ));
        }
    }
    if request.contains_key("chunk_method") {
        let method = request
            .get("chunk_method")
            .and_then(serde_json::Value::as_str);
        if let Some(text) = method
            && text.is_empty()
        {
            return Some((
                "`chunk_method` (empty string) is not valid".to_string(),
                code::INVALID_OR_MISSING_DATA,
            ));
        }
        let file_type = filename_type(&doc.name);
        // The suffix patterns are case-sensitive upstream
        // (`re.search(r"\.(ppt|pptx|pages)$", doc.name)`).
        let presentation = doc.name.ends_with(".ppt")
            || doc.name.ends_with(".pptx")
            || doc.name.ends_with(".pages");
        let mail = doc.name.ends_with(".msg") || doc.name.ends_with(".eml");
        let wrong = |expected: &str| method != Some(expected);
        if (file_type == "visual" && wrong("picture"))
            || (file_type == "aural" && wrong("audio"))
            || (presentation && wrong("presentation"))
            || (mail && wrong("email"))
        {
            return Some((
                "the automatically detected parser type cannot be changed".to_string(),
                code::INVALID_OR_MISSING_DATA,
            ));
        }
    }
    None
}

/// The legacy `parser_id` guard that runs after the rename branch upstream.
fn legacy_parser_id_refused(name: &str, requested: &str) -> bool {
    let file_type = filename_type(name);
    // Case-sensitive, matching the upstream regex `re.search(r"\.(ppt|pptx|pages)$", doc.name)`.
    let presentation =
        name.ends_with(".ppt") || name.ends_with(".pptx") || name.ends_with(".pages");
    (file_type == "visual" && requested != "picture")
        || (presentation && requested != "presentation")
}

/// `strip_graphrag_raptor_config`: the two heavy-machinery keys never leave the server
/// (upstream removes them from every `parser_config` the API serializes).
fn strip_graphrag_raptor(parser_config: &serde_json::Value) -> serde_json::Value {
    let Some(object) = parser_config.as_object() else {
        return parser_config.clone();
    };
    serde_json::Value::Object(
        object
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "graphrag" | "raptor"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// The document's effective parser face: its own overrides when a PATCH set any, the
/// dataset's values otherwise (the upload-time behavior every existing document keeps).
fn document_parser_face(
    state: &crate::server::AppState,
    doc: &DocRecord,
) -> (String, serde_json::Value, serde_json::Value) {
    let kb = state.kbs.get(&doc.kb_id);
    let parser_config = doc
        .parser_config
        .clone()
        .or_else(|| {
            kb.as_ref()
                .and_then(|kb| serde_json::from_str(&kb.parser_config).ok())
        })
        .unwrap_or_else(|| serde_json::json!({}));
    let parser_config = strip_graphrag_raptor(&parser_config);
    let chunk_method = doc
        .parser_id
        .clone()
        .or_else(|| {
            parser_config
                .get("chunk_method")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "naive".to_string());
    let pipeline_id = match &doc.pipeline_id {
        Some(value) => serde_json::Value::String(value.clone()),
        None => serde_json::Value::Null,
    };
    (chunk_method, parser_config, pipeline_id)
}

/// `get_parser_config(method, None)` for the methods a PATCH may select: the deployment
/// defaults a document falls back to when a chunk method changes without a parser config.
fn default_parser_config(chunk_method: &str) -> serde_json::Value {
    let mut config = serde_json::json!({
        "table_context_size": 0,
        "image_context_size": 0,
    });
    let object = config.as_object_mut().unwrap();
    match chunk_method {
        "naive" => {
            object.insert("layout_recognize".into(), serde_json::json!("DeepDOC"));
            object.insert("chunk_token_num".into(), serde_json::json!(512));
            object.insert("delimiter".into(), serde_json::json!("\n"));
            object.insert("auto_keywords".into(), serde_json::json!(0));
            object.insert("auto_questions".into(), serde_json::json!(0));
            object.insert("html4excel".into(), serde_json::json!(false));
            object.insert("topn_tags".into(), serde_json::json!(3));
            object.insert(
                "parent_child".into(),
                serde_json::json!({"use_parent_child": false, "children_delimiter": "\n"}),
            );
            // `get_parser_config` flattens parent_child into children_delimiter.
            object.insert("children_delimiter".into(), serde_json::json!(""));
        }
        "knowledge_graph" => {
            object.insert("chunk_token_num".into(), serde_json::json!(8192));
            object.insert("delimiter".into(), serde_json::json!("\n"));
            object.insert(
                "entity_types".into(),
                serde_json::json!(["organization", "person", "location", "event", "time"]),
            );
        }
        _ => {}
    }
    config
}

/// `normalize_compilation_template_group_ids`: one id, a list of ids or nothing — trimmed,
/// string-only, deduplicated (`ParserConfig` before-validator).
fn normalize_compilation_template_group_ids(config: &mut serde_json::Value) {
    let Some(object) = config.as_object_mut() else {
        return;
    };
    if !object.contains_key("compilation_template_group_id") {
        return;
    }
    let raw: Vec<&serde_json::Value> = match object.get("compilation_template_group_id") {
        Some(serde_json::Value::String(_)) => object
            .get("compilation_template_group_id")
            .into_iter()
            .collect(),
        Some(serde_json::Value::Array(items)) => items.iter().collect(),
        _ => Vec::new(),
    };
    let mut ids: Vec<String> = Vec::new();
    for item in raw {
        let Some(text) = item.as_str() else {
            continue;
        };
        let text = text.trim();
        if !text.is_empty() && !ids.iter().any(|known| known == text) {
            ids.push(text.to_string());
        }
    }
    object.insert(
        "compilation_template_group_id".into(),
        serde_json::json!(ids),
    );
}

/// Summed `token_count` for one document (the same aggregation the list rows use).
fn document_token_count(state: &crate::server::AppState, kb_id: &str, doc_id: &str) -> usize {
    state
        .engine
        .read()
        .map(|engine| {
            let ids: HashSet<String> = std::iter::once(doc_id.to_string()).collect();
            engine
                .token_counts_for(kb_id, &ids)
                .remove(doc_id)
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// `reset_document_for_reparse`: back to an unparsed document — progress fields zeroed, the
/// document-store chunks and the knowledgebase counters released (the same steps the reparse
/// route already takes), and the parser face takes the new values when given.
fn reset_document_for_reparse(
    state: &crate::server::AppState,
    doc: &DocRecord,
    parser_id: Option<&str>,
    pipeline_id: &str,
) -> anyhow::Result<()> {
    if !state
        .docs
        .reset_for_reparse(&doc.id, parser_id, pipeline_id)?
    {
        anyhow::bail!("document not found");
    }
    match reparse_counter_release(&state.docs, &doc.id) {
        Ok(Some(release)) => {
            if let Err(error) = state
                .kbs
                .update_counts(&release.kb_id, release.chunk_delta, 0)
            {
                tracing::warn!(%error, doc_id = %doc.id, "reparse: KB chunk counter release failed");
            }
            if let Err(error) = state.docs.set_chunk_count(&doc.id, 0) {
                tracing::warn!(%error, doc_id = %doc.id, "reparse: document chunk count reset failed");
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, doc_id = %doc.id, "reparse counter release skipped");
        }
    }
    let _commit_guard = state.document_commit_lock.lock().unwrap();
    let mut engine = state.engine.write().unwrap();
    let previous = engine.to_vec();
    engine.remove_document(&doc.id);
    crate::server::persist_index_change(state, &mut engine, &previous)?;
    Ok(())
}

/// The document dict `get_result(data=…)` returns: `map_doc_keys`' key set in upstream's
/// jsonify order (alphabetical — round13 `upd2_cc_eq` measured all 26 keys).
fn document_update_entry(
    state: &crate::server::AppState,
    doc: &DocRecord,
    token_count: usize,
) -> serde_json::Value {
    let (chunk_method, parser_config, pipeline_id) = document_parser_face(state, doc);
    let owner_id = state
        .kbs
        .get(&doc.kb_id)
        .map(|kb| kb.owner_id.clone())
        .unwrap_or_default();
    serde_json::json!({
        "chunk_count": doc.chunk_count,
        "chunk_method": chunk_method,
        "content_hash": doc.content_hash,
        "create_date": crate::api::compat::ms_to_datetime_string(doc.created_at),
        "create_time": doc.created_at,
        "created_by": owner_id,
        "dataset_id": doc.kb_id,
        "id": doc.id,
        "location": doc.name,
        "name": doc.name,
        "parser_config": parser_config,
        "pipeline_id": pipeline_id,
        "process_begin_at": serde_json::Value::Null,
        "process_duration": serde_json::Value::Null,
        "progress": doc.progress,
        "progress_msg": doc.progress_msg,
        "run": run_api_text(&doc.run),
        "size": doc.size,
        "source_type": "local",
        "status": doc.status,
        "suffix": doc_suffix(&doc.name),
        "thumbnail": "",
        "token_count": token_count,
        "type": filename_type(&doc.name),
        "update_date": crate::api::compat::ms_to_datetime_string(doc.updated_at),
        "update_time": doc.updated_at,
    })
}

/// One document as the documents API reports it.
///
/// The list endpoint's row shape, shared with the upload response — upstream feeds both from
/// `map_doc_keys_with_run_status`, so a client moves a document between the two without
/// learning a second field set. The row speaks the upstream vocabulary: `run` in
/// `TaskStatus` names, `dataset_id` alongside the legacy `kb_id`, `suffix`/`type` derived
/// from the file name, and the parser face (`chunk_method` / `parser_config`) carried from
/// the dataset.
pub(crate) fn doc_list_entry(
    state: &crate::server::AppState,
    doc: &DocRecord,
    token_count: usize,
) -> serde_json::Value {
    let meta_fields = state
        .document_metadata
        .get(&doc.id, &doc.kb_id)
        .unwrap_or_default();
    let kb = state.kbs.get(&doc.kb_id);
    let (chunk_method, parser_config, pipeline_id) = document_parser_face(state, doc);
    // Document-level ownership is not recorded yet; the dataset owner is the
    // closest honest value (it is the uploader for every single-owner dataset).
    let owner_id = kb
        .as_ref()
        .map(|kb| kb.owner_id.clone())
        .unwrap_or_default();
    let nickname = if owner_id.is_empty() {
        serde_json::Value::Null
    } else {
        state
            .users
            .get_user_by_id(&owner_id)
            .map(|user| serde_json::Value::String(user.nickname))
            .unwrap_or(serde_json::Value::Null)
    };
    serde_json::json!({
        "id": doc.id,
        "name": doc.name,
        "kb_id": doc.kb_id,
        "dataset_id": doc.kb_id,
        "size": doc.size,
        "storage_name": doc.storage_name,
        "content_hash": doc.content_hash,
        "indexed_content_hash": doc.indexed_content_hash,
        // `_process_run_mapping`: the REST surface reports TaskStatus names.
        "run": run_api_text(&doc.run),
        "progress": doc.progress,
        "progress_msg": doc.progress_msg,
        "chunk_count": doc.chunk_count,
        "token_count": token_count,
        // `_process_key_mappings`: parser_id → chunk_method.
        "chunk_method": chunk_method,
        "suffix": doc_suffix(&doc.name),
        // `filename_type` value (pdf / doc / visual / aural / other).
        "type": filename_type(&doc.name),
        "status": doc.status,
        "meta_fields": meta_fields,
        "created_at": doc.created_at,
        "updated_at": doc.updated_at,
        "create_time": doc.created_at,
        "update_time": doc.updated_at,
        "create_date": crate::api::compat::ms_to_datetime_string(doc.created_at),
        "update_date": crate::api::compat::ms_to_datetime_string(doc.updated_at),
        "created_by": owner_id,
        "nickname": nickname,
        "parser_config": parser_config,
        // The pipeline columns exist upstream; without a pipeline flow they are null.
        "pipeline_id": pipeline_id,
        "pipeline_name": serde_json::Value::Null,
        // Parse timing is not tracked yet — the keys are present, honestly empty.
        "process_begin_at": serde_json::Value::Null,
        "process_duration": serde_json::Value::Null,
        // Thumbnails are generated on demand and not recorded on the row.
        "thumbnail": "",
    })
}

/// GET /api/v1/datasets/{kb_id}/documents — list docs in KB
///
/// The upstream query face (`_get_docs_with_request` / `_get_doc_filters_with_request`):
/// page/page_size (silent fallbacks, public max 100), orderby/desc, keywords,
/// suffix/types/run filters, id/name/ids addressing, the `create_time_from/to` window
/// applied to the returned page, and the `type=filter` aggregation branch.
pub async fn list_docs(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    RawQuery(raw): RawQuery,
) -> axum::response::Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            &format!("You don't own the dataset {kb_id}. "),
        );
    }
    let pairs = query_pairs(raw.as_deref());
    let page = validate_page(q_first(&pairs, "page"));
    let page_size = match validate_page_size(q_first(&pairs, "page_size")) {
        Ok(size) => size,
        Err(()) => {
            return api_error_result(
                StatusCode::INTERNAL_SERVER_ERROR,
                code::SERVER_ERROR,
                "page_size must be less than or equal to 100",
            );
        }
    };
    let orderby = q_first(&pairs, "orderby").unwrap_or("create_time");
    if !matches!(orderby, "create_time" | "update_time" | "name") {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_ARGUMENT,
            &format!("invalid orderby field: {orderby}"),
        );
    }
    let desc = q_first(&pairs, "desc")
        .map(|value| value.trim().to_lowercase() != "false")
        .unwrap_or(true);
    let keywords = q_first(&pairs, "keywords").unwrap_or("").to_string();
    let suffix = q_list(&pairs, "suffix");
    let types = q_list(&pairs, "types");
    if !types.is_empty() {
        let mut invalid: Vec<String> = Vec::new();
        for value in &types {
            if !VALID_FILE_TYPES.contains(&value.as_str()) && !invalid.contains(value) {
                invalid.push(value.clone());
            }
        }
        if !invalid.is_empty() {
            let plural = if invalid.len() > 1 { "s" } else { "" };
            return api_error_result(
                StatusCode::OK,
                code::INVALID_OR_MISSING_DATA,
                &format!(
                    "Invalid filter conditions: {} type{}",
                    invalid.join(", "),
                    plural
                ),
            );
        }
    }
    let run_values: Vec<String> = q_list(&pairs, "run")
        .into_iter()
        .chain(q_list(&pairs, "run_status"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect();
    let run_filter = match parse_run_filter(&run_values) {
        Ok(values) => values,
        Err(invalid) => {
            return api_error_result(
                StatusCode::OK,
                code::INVALID_OR_MISSING_DATA,
                &format!(
                    "Invalid filter run status conditions: {}",
                    invalid.join(", ")
                ),
            );
        }
    };
    // (`metadata` / `metadata_condition` / `return_empty_metadata` are not read here — the
    // metadata-condition filter family is deferred; unread parameters pass through.)
    let doc_id_param = q_first(&pairs, "id")
        .map(str::to_string)
        .filter(|value| !value.is_empty());
    let doc_name_param = q_first(&pairs, "name")
        .map(str::to_string)
        .filter(|value| !value.is_empty());
    let docs = state.docs.list(&kb_id);
    let mut doc_ids_filter: Option<Vec<String>> = None;
    if let Some(doc_id) = doc_id_param.as_deref() {
        if !docs.iter().any(|doc| doc.id == doc_id) {
            return api_error_result(
                StatusCode::OK,
                code::INVALID_OR_MISSING_DATA,
                &format!("you don't own the document {doc_id}"),
            );
        }
        doc_ids_filter = Some(vec![doc_id.to_string()]);
    }
    if let Some(doc_name) = doc_name_param.as_deref() {
        if !docs.iter().any(|doc| doc.name == doc_name) {
            return api_error_result(
                StatusCode::OK,
                code::INVALID_OR_MISSING_DATA,
                &format!("you don't own the document {doc_name}"),
            );
        }
    }
    let ids = q_list(&pairs, "ids");
    if ids.len() > 100 {
        return api_error_result(
            StatusCode::OK,
            code::INVALID_ARGUMENT,
            "ids must contain at most 100 IDs",
        );
    }
    if let Some(doc_id) = doc_id_param.as_deref() {
        if !ids.is_empty() {
            // Upstream renders the list with Python's `repr`: `['a', 'b']`.
            let repr = format!(
                "[{}]",
                ids.iter()
                    .map(|value| format!("'{value}'"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            return api_error_result(
                StatusCode::OK,
                code::INVALID_OR_MISSING_DATA,
                &format!("Should not provide both 'id':{doc_id} and 'ids'{repr}"),
            );
        }
    }
    if !ids.is_empty() {
        doc_ids_filter = Some(ids.clone());
    }
    let run_set: Option<HashSet<&str>> = if run_filter.is_empty() {
        None
    } else {
        Some(run_filter.iter().map(String::as_str).collect())
    };
    let type_set: Option<HashSet<&str>> = if types.is_empty() {
        None
    } else {
        Some(types.iter().map(String::as_str).collect())
    };
    let suffix_set: Option<HashSet<&str>> = if suffix.is_empty() {
        None
    } else {
        Some(suffix.iter().map(String::as_str).collect())
    };
    let keywords_lower = keywords.to_lowercase();
    let filtered: Vec<DocRecord> = docs
        .into_iter()
        .filter(|doc| {
            if let Some(ids) = doc_ids_filter.as_ref() {
                if !ids.iter().any(|id| id == &doc.id) {
                    return false;
                }
            }
            if let Some(name) = doc_name_param.as_deref() {
                if doc.name != name {
                    return false;
                }
            }
            if !keywords_lower.is_empty() && !doc.name.to_lowercase().contains(&keywords_lower) {
                return false;
            }
            if let Some(run_set) = run_set.as_ref() {
                match run_numeric(&doc.run) {
                    Some(numeric) if run_set.contains(numeric) => {}
                    _ => return false,
                }
            }
            if let Some(type_set) = type_set.as_ref() {
                if !type_set.contains(filename_type(&doc.name)) {
                    return false;
                }
            }
            if let Some(suffix_set) = suffix_set.as_ref() {
                let suffix = doc_suffix(&doc.name);
                if !suffix_set.contains(suffix.as_str()) {
                    return false;
                }
            }
            true
        })
        .collect();
    let total = filtered.len();
    if q_first(&pairs, "type") == Some("filter") {
        return list_docs_filter(&state, &kb_id, &filtered, total);
    }
    let mut sorted = filtered;
    match orderby {
        "name" => sorted.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id))),
        "update_time" => {
            sorted.sort_by(|a, b| {
                a.updated_at
                    .cmp(&b.updated_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        _ => {
            sorted.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
    }
    if desc {
        sorted.reverse();
    }
    let start = page.saturating_sub(1).saturating_mul(page_size);
    let mut page_docs: Vec<DocRecord> = sorted.into_iter().skip(start).take(page_size).collect();
    // Upstream applies the time window to the rows it is about to return (after
    // pagination), so an empty window narrows the page without re-filling it.
    let create_time_from = q_int(&pairs, "create_time_from");
    let create_time_to = q_int(&pairs, "create_time_to");
    if create_time_from != 0 || create_time_to != 0 {
        page_docs.retain(|doc| {
            let created = doc.created_at as i64;
            (create_time_from == 0 || created >= create_time_from)
                && (create_time_to == 0 || created <= create_time_to)
        });
    }
    let doc_ids: HashSet<String> = page_docs.iter().map(|doc| doc.id.clone()).collect();
    let mut token_counts = state
        .engine
        .read()
        .map(|engine| engine.token_counts_for(&kb_id, &doc_ids))
        .unwrap_or_default();
    let entries: Vec<serde_json::Value> = page_docs
        .iter()
        .map(|doc| {
            let tokens = token_counts.remove(&doc.id).unwrap_or(0);
            doc_list_entry(&state, doc, tokens)
        })
        .collect();
    // The API guide's shape for this endpoint is `data: {"docs": [...], "total_datasets": n}`.
    // A bare array looks equivalent but breaks every client written against the guide, which reads
    // `data.docs` and would silently get null.
    let mut data = serde_json::json!({ "docs": entries, "total": total, "total_datasets": total });
    crate::api::compat::enrich_documents(&mut data["docs"]);
    // Upstream `get_json_result(data=…)` — the list envelope carries `message: "success"`
    // (probes `doc_list_filter` / `doc_list_page_neg` / `doc_list_numeric_run`).
    Json(serde_json::json!({ "code": 0, "message": "success", "data": data })).into_response()
}

/// The `type=filter` branch of the list endpoint: `_get_doc_filters_with_request` over
/// `DocumentService.get_filter_by_kb_id` — suffix / run-status / metadata counters over
/// the filtered set, with the always-present `empty_metadata` bucket.
fn list_docs_filter(
    state: &crate::server::AppState,
    kb_id: &str,
    filtered: &[DocRecord],
    total: usize,
) -> axum::response::Response {
    let mut suffix_counter: BTreeMap<String, usize> = BTreeMap::new();
    let mut run_status_counter: BTreeMap<String, usize> = BTreeMap::new();
    let mut metadata_counter: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut empty_metadata_count = 0usize;
    for doc in filtered {
        *suffix_counter.entry(doc_suffix(&doc.name)).or_insert(0) += 1;
        if let Some(numeric) = run_numeric(&doc.run) {
            *run_status_counter.entry(numeric.to_string()).or_insert(0) += 1;
        }
        let meta_fields = state
            .document_metadata
            .get(&doc.id, kb_id)
            .unwrap_or_default();
        if meta_fields.is_empty() {
            empty_metadata_count += 1;
            continue;
        }
        let mut has_valid = false;
        for (key, value) in meta_fields.iter() {
            for text in metadata_value_strings(value) {
                *metadata_counter
                    .entry(key.clone())
                    .or_default()
                    .entry(text)
                    .or_insert(0) += 1;
                has_valid = true;
            }
        }
        if !has_valid {
            empty_metadata_count += 1;
        }
    }
    let mut metadata: serde_json::Map<String, serde_json::Value> = metadata_counter
        .into_iter()
        .map(|(key, values)| (key, serde_json::to_value(values).unwrap_or_default()))
        .collect();
    metadata.insert(
        "empty_metadata".to_string(),
        serde_json::json!({ "true": empty_metadata_count }),
    );
    let filter = serde_json::json!({
        "metadata": metadata,
        "run_status": run_status_counter,
        "suffix": suffix_counter,
    });
    Json(
        serde_json::json!({ "code": 0, "message": "success", "data": {
        "total": total,
        "filter": filter,
    } }),
    )
    .into_response()
}

/// GET /api/v1/datasets/{kb_id}/documents/{doc_id} — get doc status
pub async fn get_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !kb_accessible(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "Not found" }));
    }
    match state.docs.get(&doc_id).filter(|doc| doc.kb_id == kb_id) {
        Some(d) => {
            let meta_fields = state
                .document_metadata
                .get(&d.id, &kb_id)
                .unwrap_or_default();
            Json(serde_json::json!({ "code": 0, "data": {
                "id": d.id,
                "name": d.name,
                "kb_id": d.kb_id,
                "size": d.size,
                "storage_name": d.storage_name,
                "content_hash": d.content_hash,
                "indexed_content_hash": d.indexed_content_hash,
                "run": d.run,
                "progress": d.progress,
                "progress_msg": d.progress_msg,
                "chunk_count": d.chunk_count,
                "created_at": d.created_at,
                "updated_at": d.updated_at,
                "meta_fields": meta_fields,
            } }))
        }
        None => Json(serde_json::json!({ "code": 102, "message": "Not found" })),
    }
}

/// Wire a document into the async task executor (RAGFlow `task_executor.py`
/// port). Enqueues the doc into the persisted TaskQueue and hands execution to
/// the TaskExecutor worker pool; the upload and reparse flows share this path.
///
/// The returned `task_id` matches the RAGFlow `task_id` returned by
/// `/datasets/{kb}/documents` upload responses.
pub(crate) fn enqueue_document_processing(
    state: std::sync::Arc<crate::server::AppState>,
    owner_id: &str,
    doc: DocRecord,
    priority: i32,
) -> anyhow::Result<String> {
    crate::server::queue_document_processing(state, owner_id, doc, priority)
}

/// POST /api/v1/datasets/{kb_id}/documents/{doc_id}/reparse — re-parse
/// `api/db/services/document_counter_service.py::release_reparse_counters`
/// over the local stores.
///
/// A re-parse must start from zero: the document's chunk counter is released
/// and the same amount is subtracted from the owning knowledge base so the
/// totals never double-count. The caller performs both writes (the local
/// store has no single transaction); a missing document is an error, a zero
/// counter is a no-op — matching the upstream contract.
#[derive(Debug, Clone, PartialEq)]
pub struct ReparseCounterRelease {
    pub kb_id: String,
    /// Negative chunk delta to apply to the owning KB.
    pub chunk_delta: isize,
}

pub fn reparse_counter_release(
    docs: &DocStore,
    doc_id: &str,
) -> anyhow::Result<Option<ReparseCounterRelease>> {
    let Some(doc) = docs.get(doc_id) else {
        anyhow::bail!("Document not found: {doc_id}");
    };
    if doc.chunk_count == 0 {
        return Ok(None);
    }
    Ok(Some(ReparseCounterRelease {
        kb_id: doc.kb_id.clone(),
        chunk_delta: -(doc.chunk_count as isize),
    }))
}

pub async fn reparse_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "Not found" }));
    }
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 102, "message": "Not found" }));
    };
    if doc.kb_id != kb_id {
        return Json(serde_json::json!({ "code": 102, "message": "Not found" }));
    }
    // Upstream `release_reparse_counters`: zero the document counters and
    // subtract them from the owning KB before the new parse starts.
    match reparse_counter_release(&state.docs, &doc_id) {
        Ok(Some(release)) => {
            if let Err(error) = state
                .kbs
                .update_counts(&release.kb_id, release.chunk_delta, 0)
            {
                tracing::warn!(%error, doc_id = %doc_id, "reparse: KB chunk counter release failed");
            }
            if let Err(error) = state.docs.set_chunk_count(&doc_id, 0) {
                tracing::warn!(%error, doc_id = %doc_id, "reparse: document chunk count reset failed");
            }
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(%error, %doc_id, "reparse counter release skipped");
        }
    }
    match enqueue_document_processing(
        state,
        &auth.user_id,
        doc,
        crate::api::features::TASK_PRIORITY_HIGH,
    ) {
        Ok(task_id) => Json(serde_json::json!({
            "code": 0,
            "message": "Re-parse queued",
            "data": { "task_id": task_id }
        })),
        Err(error) => Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
    }
}

/// `POST /api/v1/datasets/{id}/documents/batch-update-status` — enable/disable many
/// documents at once (upstream `document_api.py::batch_update_document_status`).
///
/// The status is stored with the document, not in the browser: a disabled document
/// must stop taking part in retrieval for every client, which is what upstream means
/// by the switch. The answer mirrors upstream's per-document result map, and a partial
/// failure is reported as such instead of a blanket success.
pub async fn batch_update_document_status(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "you don't own the dataset" }));
    }
    let doc_ids: Vec<String> = body
        .get("doc_ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if doc_ids.is_empty() {
        return Json(serde_json::json!({
            "code": 101,
            "message": "\"doc_ids\" must be a non-empty list."
        }));
    }
    let status = match body.get("status") {
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::String(value)) => value.clone(),
        _ => "-1".to_string(),
    };
    if !matches!(status.as_str(), "0" | "1") {
        return Json(serde_json::json!({
            "code": 101,
            "message": format!("\"Status\" must be either 0 or 1:{status}!")
        }));
    }
    let mut result = serde_json::Map::new();
    let mut has_error = false;
    let mut accepted = Vec::new();
    for doc_id in &doc_ids {
        match state.docs.get(doc_id) {
            Some(doc) if doc.kb_id == kb_id => accepted.push(doc_id.clone()),
            Some(_) => {
                result.insert(
                    doc_id.clone(),
                    serde_json::json!({ "error": "Document not found in this dataset." }),
                );
                has_error = true;
            }
            None => {
                result.insert(
                    doc_id.clone(),
                    serde_json::json!({ "error": "Document not found" }),
                );
                has_error = true;
            }
        }
    }
    match state.docs.set_status(&accepted, &status) {
        Ok(changed) => {
            for doc_id in accepted {
                result.insert(doc_id, serde_json::json!({ "status": status }));
            }
            tracing::info!(
                kb_id = %kb_id,
                status = %status,
                requested = doc_ids.len(),
                changed = changed.len(),
                "Document status updated"
            );
        }
        Err(error) => {
            has_error = true;
            for doc_id in accepted {
                result.insert(
                    doc_id,
                    serde_json::json!({ "error": format!("Database error (Document update)! {error}") }),
                );
            }
        }
    }
    if has_error {
        return Json(serde_json::json!({
            "code": 500,
            "message": "Partial failure",
            "data": result,
        }));
    }
    // Upstream `get_json_result(data=result)` — the success envelope carries `message`.
    Json(serde_json::json!({ "code": 0, "message": "success", "data": result }))
}

/// `DELETE /api/v1/datasets/{id}/documents` — remove many documents (upstream
/// `document_api.py::delete_documents`).
///
/// Upstream accepts either `ids` or `delete_all` and refuses both at once; ids that do
/// not belong to the dataset are refused rather than silently skipped.
pub async fn delete_docs_bulk(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(
            serde_json::json!({ "code": 102, "message": format!("You don't own the dataset {kb_id}. ") }),
        );
    }
    let delete_all = body
        .get("delete_all")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let mut doc_ids: Vec<String> = body
        .get("ids")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if doc_ids.is_empty() && !delete_all {
        return Json(serde_json::json!({
            "code": 102,
            "message": format!("should either provide doc ids or set delete_all(true), dataset: {kb_id}. "),
        }));
    }
    if !doc_ids.is_empty() && delete_all {
        return Json(serde_json::json!({
            "code": 102,
            "message": format!("should not provide both doc ids and delete_all(true), dataset: {kb_id}. "),
        }));
    }
    let owned: std::collections::HashMap<String, DocRecord> = state
        .docs
        .list(&kb_id)
        .into_iter()
        .map(|doc| (doc.id.clone(), doc))
        .collect();
    if delete_all {
        doc_ids = owned.keys().cloned().collect();
    }
    let invalid: Vec<&String> = doc_ids
        .iter()
        .filter(|doc_id| !owned.contains_key(*doc_id))
        .collect();
    if !invalid.is_empty() {
        let list = invalid
            .iter()
            .map(|value| value.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Json(serde_json::json!({
            "code": 102,
            "message": format!(
                "These documents do not belong to dataset {kb_id} or Document not found: {list}"
            ),
        }));
    }
    let mut successes = 0usize;
    let mut errors: Vec<serde_json::Value> = Vec::new();
    for doc_id in &doc_ids {
        let Some(doc) = owned.get(doc_id) else {
            continue;
        };
        match crate::server::delete_document_data(&state, doc) {
            Ok(()) => successes += 1,
            Err(error) => errors
                .push(serde_json::json!({ "document_id": doc_id, "error": error.to_string() })),
        }
    }
    tracing::info!(kb_id = %kb_id, successes, failures = errors.len(), "Documents deleted");
    if errors.is_empty() {
        // Upstream answers `data {"deleted": n}`; `success_count` stays alongside it for
        // clients written against RayRAG's own spelling.
        return Json(serde_json::json!({
            "code": 0,
            "data": { "deleted": successes, "success_count": successes },
        }));
    }
    // Upstream `FileService.delete_docs` aggregates the per-document error texts and answers
    // `get_error_data_result(message=str(errors))`: business code 102, the texts concatenated
    // in `message`, and no `data` key at all.
    tracing::warn!(kb_id = %kb_id, ?errors, "document deletion had failures");
    let message = errors
        .iter()
        .map(|error| error["error"].as_str().unwrap_or_default())
        .collect::<String>();
    Json(serde_json::json!({
        "code": 102,
        "message": message,
    }))
}

/// `POST /api/v1/documents/ingest` — queue (or cancel) the parsing of many documents
/// (upstream `document_api.py::ingest`).
///
/// `run: "1"` starts parsing, `run: "2"` cancels it. `delete` clears the chunks the
/// document already has before re-parsing (upstream's `redo` checkbox) and `apply_kb`
/// re-applies the dataset's automatic metadata settings.
pub async fn ingest_documents(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    // Upstream `_run_sync` walks `req["doc_ids"]` directly: a missing key surfaces as
    // `KeyError('doc_ids')` (`ingest_none` probe), a JSON null as the TypeError Python
    // raises while iterating, and an empty list never reaches the accessibility check
    // (`ingest_empty` probe → success). Errors use `get_json_result(code, message)` —
    // the three-key envelope with `data: null`.
    let doc_ids: Vec<String> = match body.get("doc_ids") {
        None => return ingest_error(100, "KeyError('doc_ids')"),
        Some(serde_json::Value::Null) => {
            return ingest_error(100, "TypeError(\"'NoneType' object is not iterable\")");
        }
        Some(serde_json::Value::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str())
            .map(str::to_string)
            .collect(),
        // A scalar is not iterable; the closest observable is the first accessibility
        // refusal.
        Some(_) => return ingest_error(109, "no authorization"),
    };
    if doc_ids.is_empty() {
        return Json(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": true,
        }));
    }
    // First pass: every id must be accessible before any work happens (`ingest_fakeid`
    // probe → `no authorization`).
    for doc_id in &doc_ids {
        let accessible = state
            .docs
            .get(doc_id)
            .is_some_and(|doc| kb_accessible(&state, &doc.kb_id, &auth));
        if !accessible {
            return ingest_error(109, "no authorization");
        }
    }
    // `str(req["run"])` — the key is required (`KeyError('run')`), a JSON null becomes
    // the string "None", and the value is otherwise used raw.
    let run = match body.get("run") {
        None => return ingest_error(100, "KeyError('run')"),
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::String(value)) => value.clone(),
        Some(serde_json::Value::Null) => "None".to_string(),
        Some(serde_json::Value::Bool(true)) => "True".to_string(),
        Some(serde_json::Value::Bool(false)) => "False".to_string(),
        Some(other) => other.to_string(),
    };
    let clear_chunks = body
        .get("delete")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let accepted: Vec<_> = doc_ids
        .iter()
        .filter_map(|doc_id| state.docs.get(doc_id))
        .collect();
    if run == "2" {
        let mut cancelled = 0usize;
        for doc in &accepted {
            let current = state.docs.get(&doc.id).unwrap_or_else(|| doc.clone());
            match state.tasks.cancel_document(&doc.id) {
                Ok(count) => {
                    // Upstream cancels a RUNNING/CANCEL document or one with unfinished
                    // tasks; anything else is a DATA_ERROR.
                    let running = matches!(current.run.as_str(), "RUNNING" | "CANCELLED");
                    if count == 0 && !running {
                        return ingest_error(
                            102,
                            "Cannot cancel a task that is not in RUNNING status",
                        );
                    }
                    cancelled += count;
                }
                Err(error) => return ingest_error(500, &error.to_string()),
            }
            // The "stopped by user" marker is appended to `progress_msg` and the row's
            // progress resets to 0 (`info = {"run": …, "progress": 0}` + marker).
            let marker = format!(
                "{}\n{} Task stopped by user.",
                current.progress_msg,
                chrono::Local::now().format("%H:%M:%S")
            );
            if let Err(error) = state.docs.update_status(&doc.id, "CANCELLED", 0.0, &marker) {
                tracing::warn!(%error, doc_id = %doc.id, "cancel: document status write failed");
            }
        }
        tracing::info!(
            documents = accepted.len(),
            cancelled,
            "Document parsing cancelled"
        );
        // Upstream answers `get_json_result(data=True)` for both the queue and the cancel
        // paths: `{"code": 0, "message": "success", "data": true}`.
        return Json(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": true,
        }));
    }
    if run != "1" {
        // Any other `run` value still writes through (`update_by_id(info)` — status and
        // progress are persisted without queueing).
        let mapped = match run.as_str() {
            "0" => "UNSTARTED",
            "3" => "DONE",
            "4" => "FAILED",
            other => other,
        };
        for doc in &accepted {
            let current = state.docs.get(&doc.id).unwrap_or_else(|| doc.clone());
            if let Err(error) =
                state
                    .docs
                    .update_status(&doc.id, mapped, 0.0, &current.progress_msg)
            {
                tracing::warn!(%error, doc_id = %doc.id, "ingest: write-through failed");
            }
        }
        return Json(serde_json::json!({
            "code": 0,
            "message": "success",
            "data": true,
        }));
    }
    let mut queued = Vec::new();
    for doc in accepted {
        if clear_chunks {
            // Upstream's `redo` checkbox: drop the chunks this document already
            // contributed before parsing it again, so the new run cannot leave a
            // mixture of two parses behind.
            match crate::server::delete_document_data(&state, &doc) {
                Ok(()) => {
                    // The document row survives the chunk removal (its file is
                    // re-parsed from disk), so it is re-inserted with an empty index.
                    if let Err(error) = state.docs.set_chunk_count(&doc.id, 0) {
                        // The next commit computes its delta from this counter; leaving it
                        // stale makes the KB chunk count wrong for ever, silently.
                        tracing::error!(%error, doc_id = %doc.id, "redo: chunk count reset failed");
                    }
                }
                Err(error) => return ingest_error(100, &error.to_string()),
            }
        }
        match state
            .docs
            .get(&doc.id)
            .ok_or_else(|| anyhow::anyhow!("Document not found"))
            .and_then(|doc| {
                enqueue_document_processing(
                    state.clone(),
                    &auth.user_id,
                    doc,
                    crate::api::features::TASK_PRIORITY_HIGH,
                )
            }) {
            Ok(task_id) => {
                queued.push(serde_json::json!({ "document_id": doc.id, "task_id": task_id }))
            }
            // Upstream aborts the whole request on the first per-document failure
            // (`_run_sync` raises → EXCEPTION_ERROR with the message).
            Err(error) => return ingest_error(100, &error.to_string()),
        }
    }
    tracing::info!(queued = queued.len(), "Documents queued for parsing");
    // Upstream answers `get_json_result(data=True)` — the per-document task ids are not
    // part of the response.
    Json(serde_json::json!({ "code": 0, "message": "success", "data": true }))
}

/// `get_json_result(code, message)` — the ingest family's error envelope keeps the
/// `data` key (null), unlike the `get_error_data_result` family the document surface
/// otherwise uses.
fn ingest_error(code: i64, message: &str) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "code": code, "message": message, "data": null }))
}

/// The document selection upstream's `/documents/parse` and `/documents/stop` accept.
///
/// Upstream reads `document_ids` (`document_api.py`: "`document_ids` is required"); the
/// `doc_ids` spelling RayRAG's own `/documents/ingest` uses is accepted as a fallback so
/// one body shape does not silently produce an empty selection.
fn upstream_document_ids(body: &serde_json::Value) -> Vec<String> {
    ["document_ids", "doc_ids"]
        .iter()
        .find_map(|key| body.get(*key).and_then(|value| value.as_array()))
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve the selection against one dataset, upstream-style: ids that are not documents
/// of this dataset are reported together and the request fails when none is valid.
fn resolve_dataset_documents(
    state: &std::sync::Arc<AppState>,
    auth: &AuthContext,
    kb_id: &str,
    doc_ids: &[String],
) -> Result<Vec<DocRecord>, Vec<String>> {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for doc_id in doc_ids {
        match state.docs.get(doc_id) {
            Some(doc) if doc.kb_id == kb_id && kb_manageable(state, kb_id, auth) => found.push(doc),
            _ => missing.push(doc_id.clone()),
        }
    }
    if found.is_empty() && !missing.is_empty() {
        return Err(missing);
    }
    Ok(found)
}

/// `POST /api/v1/datasets/{dataset_id}/documents/parse` — upstream
/// `document_api.parse_documents`: run each selected document again and answer
/// `{"success_count": n}` (plus `errors` when some could not be queued).
pub async fn parse_documents(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(
            serde_json::json!({ "code": 102, "message": format!("You don't own the dataset {kb_id}.") }),
        );
    }
    let doc_ids = upstream_document_ids(&body);
    if doc_ids.is_empty() {
        return Json(serde_json::json!({ "code": 102, "message": "`document_ids` is required" }));
    }
    let documents = match resolve_dataset_documents(&state, &auth, &kb_id, &doc_ids) {
        Ok(documents) => documents,
        Err(missing) => {
            return Json(serde_json::json!({
                "code": 102,
                "message": format!("Documents not found: [{}]", missing.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ")),
            }));
        }
    };
    let mut success_count = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for doc in documents {
        let run = state
            .docs
            .get(&doc.id)
            .ok_or_else(|| anyhow::anyhow!("Document not found"))
            .and_then(|doc| {
                enqueue_document_processing(
                    state.clone(),
                    &auth.user_id,
                    doc,
                    crate::api::features::TASK_PRIORITY_HIGH,
                )
            });
        match run {
            Ok(_) => success_count += 1,
            Err(error) => errors.push(format!("{}: {error}", doc.id)),
        }
    }
    tracing::info!(dataset = %kb_id, success_count, failures = errors.len(), "Dataset documents queued for parsing");
    if errors.is_empty() {
        return Json(serde_json::json!({ "code": 0, "data": { "success_count": success_count } }));
    }
    Json(serde_json::json!({
        "code": 0,
        "data": { "success_count": success_count, "errors": errors },
    }))
}

/// `POST /api/v1/datasets/{dataset_id}/documents/stop` — upstream
/// `document_api.stop_parse_documents`: cancel the running parse of every selected
/// document and answer `{"success_count": n}`.
pub async fn stop_parse_documents(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(
            serde_json::json!({ "code": 102, "message": format!("You don't own the dataset {kb_id}.") }),
        );
    }
    let doc_ids = upstream_document_ids(&body);
    if doc_ids.is_empty() {
        return Json(serde_json::json!({ "code": 102, "message": "`document_ids` is required" }));
    }
    let documents = match resolve_dataset_documents(&state, &auth, &kb_id, &doc_ids) {
        Ok(documents) => documents,
        Err(missing) => {
            return Json(serde_json::json!({
                "code": 102,
                "message": format!("Documents not found: [{}]", missing.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(", ")),
            }));
        }
    };
    let mut success_count = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for doc in documents {
        if let Err(error) = state.tasks.cancel_document(&doc.id) {
            errors.push(format!("{}: {error}", doc.id));
            continue;
        }
        if let Err(error) =
            state
                .docs
                .update_status(&doc.id, "CANCELLED", 1.0, "Processing cancelled")
        {
            tracing::warn!(%error, doc_id = %doc.id, "stop-parse: status write failed");
        }
        success_count += 1;
    }
    tracing::info!(dataset = %kb_id, success_count, failures = errors.len(), "Dataset document parsing stopped");
    if errors.is_empty() {
        return Json(serde_json::json!({ "code": 0, "data": { "success_count": success_count } }));
    }
    Json(serde_json::json!({
        "code": 0,
        "data": { "success_count": success_count, "errors": errors },
    }))
}

/// DELETE /api/v1/datasets/{kb_id}/documents/{doc_id} — delete doc
/// PATCH /api/v1/datasets/{dataset_id}/documents/{document_id} — upstream
/// `document_api.update_document`: the route's whole field face, not just a rename.
///
/// Replicated from the v0.27.2 source and the round13 probes (`upd2_*` arms): an explicit
/// null name is a type error before ownership is checked; `UpdateDocumentReq`'s pydantic
/// wording is reproduced field by field (the names show no "Value error, " prefix for
/// PydanticCustomError); immutable values (`chunk_count` / `token_count` / `progress`)
/// refuse to change; and the update branches run in the service layer's order —
/// meta_fields → name → the legacy `parser_id` guard → parser_config → pipeline_id
/// (which wins over chunk_method) → enabled, each reset releasing the parse counters and
/// dropping the document-store chunks. The answer is the full document dict `get_result`
/// returns after the reload.
pub async fn update_document(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    // `get_request_json()`: an empty body normalises to `{}`, a malformed body and a
    // non-object payload surface as raised exceptions through the global handler
    // (round13 probes `upd2_nobody` / `upd2_body_bad` / `upd2_body_arr`).
    let request: serde_json::Value = if body.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(_) => return raised_error("<BadRequest '400: Bad Request'>".to_string()),
        }
    };
    let serde_json::Value::Object(mut request) = request else {
        return raised_error("TypeError('JSON payload must be an object.')".to_string());
    };
    normalize_legacy_raptor_config(&mut request);

    // An explicit null name is a type error, not an unset field.
    if matches!(request.get("name"), Some(serde_json::Value::Null)) {
        return update_data_error(
            "Field: <name> - Message: <Input should be a valid string> - Value: <None>",
        );
    }

    // Ownership and existence of dataset and document.
    if !kb_manageable(&state, &kb_id, &auth) {
        return update_data_error("you don't own the dataset");
    }
    let Some(doc) = state.docs.get(&doc_id).filter(|doc| doc.kb_id == kb_id) else {
        return update_data_error("the dataset doesn't own the document");
    };

    let current_token_count = document_token_count(&state, &kb_id, &doc_id) as i64;

    let pydantic_errors = pydantic_update_errors(&request);
    if !pydantic_errors.is_empty() {
        return update_data_error(&pydantic_errors.join("\n"));
    }
    if let Some((message, business_code)) =
        validate_update_fields(&state, &request, &doc, current_token_count)
    {
        return api_error_result(StatusCode::OK, business_code, &message);
    }

    // All validations passed — the updates, in the service layer's order.
    if request.contains_key("meta_fields")
        && let Some(serde_json::Value::Object(fields)) = request.get("meta_fields")
        && state
            .document_metadata
            .replace(&doc_id, &kb_id, fields.clone())
            .is_err()
    {
        return update_data_error("Failed to update metadata");
    }
    if let Some(serde_json::Value::String(new_name)) = request.get("name")
        && new_name != &doc.name
        && let Err(error) = state.docs.rename(&doc_id, new_name)
    {
        return update_data_error(&error.to_string());
    }
    if request.contains_key("parser_id") {
        let requested = request
            .get("parser_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if legacy_parser_id_refused(&doc.name, requested) {
            return update_data_error("Not supported yet!");
        }
    }
    if let Some(value) = request
        .get("parser_config")
        .filter(|value| !value.is_null())
    {
        let mut stored = value.clone();
        normalize_compilation_template_group_ids(&mut stored);
        if let Err(error) = state.docs.set_parser_config(&doc_id, stored) {
            return update_data_error(&error.to_string());
        }
    }
    if let Some(value) = request.get("pipeline_id") {
        let pipeline_id = value.as_str().unwrap_or("");
        if let Err(error) = reset_document_for_reparse(&state, &doc, None, pipeline_id) {
            return update_data_error(&error.to_string());
        }
    } else if let Some(method) = request
        .get("chunk_method")
        .and_then(serde_json::Value::as_str)
    {
        if !method.is_empty() {
            let (current_method, _, _) = document_parser_face(&state, &doc);
            if !current_method.eq_ignore_ascii_case(method) {
                if let Err(error) = reset_document_for_reparse(&state, &doc, Some(method), "") {
                    return update_data_error(&error.to_string());
                }
            } else if doc.pipeline_id.is_some()
                && let Err(error) = reset_document_for_reparse(&state, &doc, None, "")
            {
                return update_data_error(&error.to_string());
            }
            let parser_config_absent = request
                .get("parser_config")
                .map(|value| {
                    value.is_null()
                        || value
                            .as_object()
                            .map(|object| object.is_empty())
                            .unwrap_or(false)
                })
                .unwrap_or(true);
            if parser_config_absent {
                let defaults = default_parser_config(method);
                if let Err(error) = state.docs.set_parser_config(&doc_id, defaults) {
                    return update_data_error(&error.to_string());
                }
            }
        }
    }
    if let Some(value) = request.get("enabled").filter(|value| !value.is_null())
        && let Ok(requested) = coerce_int(value)
    {
        let status = requested.to_string();
        if doc.status != status
            && let Err(error) = state
                .docs
                .set_status(std::slice::from_ref(&doc_id), &status)
        {
            return update_data_error(&error.to_string());
        }
    }

    let Some(fresh) = state.docs.get(&doc_id) else {
        return update_data_error(&format!("Can not get document by id:{doc_id}"));
    };
    let token_count = document_token_count(&state, &kb_id, &doc_id);
    Json(serde_json::json!({
        "code": 0,
        "data": document_update_entry(&state, &fresh, token_count),
    }))
    .into_response()
}

/// Download the original file of a document (RAGFlow-compatible:
/// GET /api/v1/datasets/{dataset_id}/documents/{document_id}/download).
pub async fn download_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> axum::response::Response {
    if !kb_accessible(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    }
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    };
    if doc.kb_id != kb_id {
        return Json(serde_json::json!({
            "code": 102,
            "message": format!("The dataset not own the document {doc_id}.")
        }))
        .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let filename = doc.name.replace(['"', '\\', '/'], "_");
    let disposition = format!("attachment; filename=\"{filename}\"");
    crate::api::common::stream_stored_file(
        &physical,
        "application/octet-stream",
        &disposition,
        "This file is empty.",
    )
    .await
}

/// `GET /api/v1/documents/{doc_id}/preview` — upstream
/// `useGetDocumentUrl`'s document branch (`hooks/use-document-request.ts`):
/// the raw bytes of a document, addressed by document id alone and served
/// inline so the previewer can render it (a download keeps its `attachment`
/// disposition on the dataset route).
pub async fn preview_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let filename = doc.name.replace(['"', '\\', '/'], "_");
    let disposition = format!("inline; filename=\"{filename}\"");
    crate::api::common::stream_stored_file(
        &physical,
        mime_for_document(&doc.name),
        &disposition,
        "This file is empty.",
    )
    .await
}

/// `GET /api/v1/documents/{document_id}` — upstream
/// `api/apps/restful_apis/document_api.py::download_document`
/// (`@manager.route("/documents/<document_id>", methods=["GET"])`). It is the branch
/// `utils/file-util.ts::fetchPreviewBlob` takes for `resource === 'document'`, i.e. the
/// byte source behind `previewHtmlFile()` and `downloadDocument()`. Upstream answers
/// "document not found" for a document the caller cannot read, "This file is empty."
/// when the stored blob is missing, and serves the bytes with an `attachment`
/// disposition under the document's own MIME type.
pub async fn download_document(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let filename = doc.name.replace(['"', '\\', '/'], "_");
    let disposition = format!("attachment; filename=\"{filename}\"");
    crate::api::common::stream_stored_file(
        &physical,
        mime_for_document(&doc.name),
        &disposition,
        "This file is empty.",
    )
    .await
}

/// `GET /api/v1/documents/{doc_id}/preview/docx` — the Word previewer's
/// structural HTML. Upstream hands the bytes to `@extend-ai/react-docx` in the
/// browser; RayRAG renders the same document model with its Rust `docx-rs`
/// reader (`parser::docx::RAGFlowDocxParser::preview_html`) so the preview
/// works without an npm build. Paragraphs, headings, inline bold/italic,
/// line breaks and tables are rendered; pagination, themes, floating shapes
/// and embedded media are deliberately outside this partial surface.
pub async fn preview_docx(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    const MAX_BLOCKS: usize = 5000;
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let Ok(bytes) = tokio::fs::read(&physical).await else {
        return Json(serde_json::json!({ "code": 102, "message": "This file is empty." }))
            .into_response();
    };
    let (html, truncated) = match crate::parser::docx::RAGFlowDocxParser::new()
        .preview_html(&bytes, MAX_BLOCKS)
    {
        Ok(result) => result,
        Err(error) => {
            return Json(
                serde_json::json!({ "code": 500, "message": format!("Failed to read Word document: {error}") }),
            )
            .into_response();
        }
    };
    Json(serde_json::json!({
        "code": 0,
        "data": { "html": html, "truncated": truncated },
    }))
    .into_response()
}

/// `GET /api/v1/documents/{doc_id}/preview/sheets` — the workbook grid the
/// spreadsheet previewer renders. Upstream builds this in the browser through
/// `@js-preview/excel` (`components/document-preview/hooks.ts::useFetchExcel`,
/// which hands the raw arraybuffer to the library); RayRAG answers the same grid
/// from its own Rust reader (`parser::excel::read_xlsx_sheets`), so the preview
/// needs no npm dependency and stays inside the Rust implementation.
///
/// The payload is `{sheets:[{name, rows:[[cell,…],…]}], truncated:bool}`. Rows and
/// columns are capped so one huge workbook cannot bloat the response; the flag
/// tells the page to say so instead of silently cutting the sheet short.
pub async fn preview_sheets(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    const MAX_ROWS: usize = 500;
    const MAX_COLUMNS: usize = 64;
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let Ok(bytes) = tokio::fs::read(&physical).await else {
        return Json(serde_json::json!({ "code": 102, "message": "This file is empty." }))
            .into_response();
    };
    let sheets = match crate::parser::excel::read_xlsx_sheets(&bytes, MAX_ROWS) {
        Ok(sheets) => sheets,
        Err(error) => {
            return Json(
                serde_json::json!({ "code": 500, "message": format!("Failed to read workbook: {error}") }),
            )
            .into_response();
        }
    };
    let mut truncated = false;
    let payload: Vec<serde_json::Value> = sheets
        .into_iter()
        .map(|sheet| {
            truncated |= sheet.truncated;
            let rows: Vec<Vec<String>> = sheet
                .rows
                .into_iter()
                .map(|row| {
                    let mut cells = row;
                    if cells.len() > MAX_COLUMNS {
                        truncated = true;
                        cells.truncate(MAX_COLUMNS);
                    }
                    cells
                })
                .collect();
            serde_json::json!({ "name": sheet.name, "rows": rows })
        })
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": { "sheets": payload, "truncated": truncated },
    }))
    .into_response()
}

/// `GET /api/v1/documents/{doc_id}/preview/slides` — the deck text the slides
/// previewer renders. Upstream draws real slides in the browser with
/// `pptx-preview` (`components/document-preview/ppt-preview.tsx`); RayRAG returns
/// the per-slide text its own reader extracts (`parser::ppt::slide_texts`), which
/// is the part of a deck the chunker indexes. Shapes, theme and images are not
/// reproduced — the ledger keeps this branch `partial` for exactly that reason.
pub async fn preview_slides(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(doc_id): Path<String>,
) -> axum::response::Response {
    let Some(doc) = state.docs.get(&doc_id) else {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    };
    if !kb_accessible(&state, &doc.kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "document not found" }))
            .into_response();
    }
    let uploads = std::path::Path::new(&state.static_dir).join("../uploads");
    let physical = uploads.join(&doc.storage_name);
    let Ok(bytes) = tokio::fs::read(&physical).await else {
        return Json(serde_json::json!({ "code": 102, "message": "This file is empty." }))
            .into_response();
    };
    let slides = match crate::parser::ppt::slide_texts(&bytes) {
        Ok(slides) => slides,
        Err(error) => {
            return Json(
                serde_json::json!({ "code": 500, "message": format!("Failed to read presentation: {error}") }),
            )
            .into_response();
        }
    };
    Json(serde_json::json!({ "code": 0, "data": { "slides": slides } })).into_response()
}

/// Content type for a stored document, by extension. The previewer renders PDFs
/// through pdf.js, images and text natively, and everything else as a download.
fn mime_for_document(name: &str) -> &'static str {
    let extension = name
        .rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "txt" | "md" | "mdx" | "markdown" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

pub async fn delete_doc(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !kb_manageable(&state, &kb_id, &auth) {
        return Json(serde_json::json!({ "code": 102, "message": "Not found" }));
    }
    let Some(doc) = state.docs.get(&doc_id).filter(|doc| doc.kb_id == kb_id) else {
        return Json(serde_json::json!({ "code": 102, "message": "Not found" }));
    };
    match crate::server::delete_document_data(&state, &doc) {
        Ok(()) => Json(serde_json::json!({ "code": 0, "message": "Deleted" })),
        Err(error) => Json(serde_json::json!({ "code": 500, "message": error.to_string() })),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod store_tests {
    use super::*;

    #[test]
    fn reparse_counter_release_matches_upstream_semantics() {
        let path = std::env::temp_dir().join(format!(
            "rayrag-reparse-release-{}.json",
            uuid::Uuid::new_v4()
        ));
        let docs = DocStore::new(path.to_str().unwrap()).unwrap();
        let mut doc = sample_doc("doc-1");
        doc.chunk_count = 7;
        let kb_id = doc.kb_id.clone();
        docs.insert(doc).unwrap();

        let release = reparse_counter_release(&docs, "doc-1")
            .unwrap()
            .expect("counters released");
        assert_eq!(release.kb_id, kb_id);
        assert_eq!(release.chunk_delta, -7);

        // Apply exactly as the handler does, then the second call is a no-op.
        docs.set_chunk_count("doc-1", 0).unwrap();
        assert_eq!(docs.get("doc-1").unwrap().chunk_count, 0);
        assert!(reparse_counter_release(&docs, "doc-1").unwrap().is_none());

        // Missing document → error (upstream LookupError).
        assert!(reparse_counter_release(&docs, "missing").is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn python_suffix_follows_pathlib_semantics() {
        assert_eq!(python_suffix("a.tar.gz"), ".gz");
        assert_eq!(python_suffix("dir.with.dots/a.b"), ".b");
        assert_eq!(python_suffix("a."), "");
        assert_eq!(python_suffix(".bashrc"), "");
        assert_eq!(python_suffix("noext"), "");
        // The extension guard compares lowercased names on both sides.
        assert_eq!(python_suffix(&"A.PDF".to_lowercase()), ".pdf");
    }

    fn sample_doc(id: &str) -> DocRecord {
        DocRecord {
            id: id.into(),
            name: "sample.txt".into(),
            kb_id: "kb-1".into(),
            size: 6,
            storage_name: "sample.txt".into(),
            content_hash: "hash".into(),
            indexed_content_hash: String::new(),
            run: "UNSTARTED".into(),
            progress: 0.0,
            progress_msg: String::new(),
            chunk_count: 0,
            created_at: 1,
            updated_at: 1,
            status: enabled_status(),
            ..Default::default()
        }
    }

    #[test]
    fn failed_persistence_rolls_back_document_mutations() {
        let root =
            std::env::temp_dir().join(format!("rayrag-doc-rollback-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("docs.json");
        let store = DocStore::new(path.to_str().unwrap()).unwrap();
        store.insert(sample_doc("doc-1")).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        assert!(
            store
                .update_status("doc-1", "RUNNING", 0.2, "Parsing")
                .is_err()
        );
        let doc = store.get("doc-1").unwrap();
        assert_eq!(doc.run, "UNSTARTED");
        assert_eq!(doc.progress, 0.0);

        assert!(store.delete("doc-1").is_err());
        assert!(store.get("doc-1").is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn corrupt_document_json_fails_startup_instead_of_clearing_store() {
        let root =
            std::env::temp_dir().join(format!("rayrag-doc-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("docs.json");
        std::fs::write(&path, b"{not-json").unwrap();
        let error = match DocStore::new(path.to_str().unwrap()) {
            Err(error) => error,
            Ok(_) => panic!("corrupt document JSON must fail startup"),
        };
        assert!(
            error
                .to_string()
                .contains("Failed to parse document metadata")
        );
        std::fs::remove_dir_all(root).ok();
    }
}
