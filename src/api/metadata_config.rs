//! Auto-metadata configuration: the field schema a dataset defines for its documents, and the
//! per-document override.
//!
//! Upstream keeps this in `dataset_api_service.get_auto_metadata` / `update_auto_metadata` and
//! `document_api.update_metadata_config`, validated by `validation_utils.AutoMetadataConfig`:
//!
//! ```text
//! { "metadata": [ {"key": …, "type": "string"|"list"|"time"|"number",
//!                  "description": …, "enum": […] } ],
//!   "built_in_metadata": [ … same shape … ] }
//! ```
//!
//! The difference between the two lists is kept exactly as upstream has it: `metadata` is what the
//! user configured, `built_in_metadata` is what the deployment offers on top (here: the keys the
//! extraction pipeline can fill in), and a client shows both while only the first is editable.
//!
//! Validation is real: a key must be non-empty and at most 255 characters after trimming, `type` must
//! be one of the four names, and every entry of `enum` must be a string. A rejected body is a `101`
//! with the field named — silently storing a malformed schema would break every later filter.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::Extension;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::server::{AppState, AuthContext, api_error_code, api_error_result, code};
use crate::user_register::EXCEPTION_ERROR;

/// One auto-metadata field.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MetadataField {
    pub key: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "enum")]
    pub enum_values: Option<Vec<String>>,
}

/// The configuration a dataset carries.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AutoMetadataConfig {
    #[serde(default)]
    pub metadata: Vec<MetadataField>,
    #[serde(default)]
    pub built_in_metadata: Vec<MetadataField>,
}

/// A per-document override of the dataset's schema.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DocumentMetadataConfig {
    #[serde(default)]
    pub metadata: Vec<MetadataField>,
    #[serde(default)]
    pub built_in_metadata: Vec<MetadataField>,
}

#[derive(Serialize, Deserialize, Default)]
struct ConfigFile {
    datasets: BTreeMap<String, AutoMetadataConfig>,
    documents: BTreeMap<String, DocumentMetadataConfig>,
}

pub struct MetadataConfigStore {
    datasets: RwLock<BTreeMap<String, AutoMetadataConfig>>,
    documents: RwLock<BTreeMap<String, DocumentMetadataConfig>>,
    metadata_path: String,
    save_lock: Mutex<()>,
}

/// The keys this deployment's extraction pipeline can fill in, offered as `built_in_metadata`.
///
/// They are named after the fields the parser already writes, so a client that shows the built-ins is
/// showing something that will really appear on a parsed document.
pub(crate) fn built_in_fields() -> Vec<MetadataField> {
    [
        ("author", "string", "Author recorded in the document"),
        ("tags", "list", "Tags extracted or assigned to the document"),
        (
            "url",
            "string",
            "Source URL, for documents fetched from the web",
        ),
        ("language", "string", "Language the document is written in"),
        (
            "published_at",
            "time",
            "Publication time, when the document states one",
        ),
    ]
    .into_iter()
    .map(|(key, field_type, description)| MetadataField {
        key: key.to_string(),
        field_type: field_type.to_string(),
        description: Some(description.to_string()),
        enum_values: None,
    })
    .collect()
}

impl MetadataConfigStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::persistence::restore_if_missing(std::path::Path::new(path))?;
        let file: ConfigFile = if std::path::Path::new(path).exists() {
            let data = std::fs::read_to_string(path)?;
            serde_json::from_str(&data).map_err(|error| {
                anyhow::anyhow!("Failed to parse metadata configuration '{path}': {error}")
            })?
        } else {
            ConfigFile::default()
        };
        Ok(Self {
            datasets: RwLock::new(file.datasets),
            documents: RwLock::new(file.documents),
            metadata_path: path.to_string(),
            save_lock: Mutex::new(()),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            datasets: RwLock::new(BTreeMap::new()),
            documents: RwLock::new(BTreeMap::new()),
            metadata_path: String::new(),
            save_lock: Mutex::new(()),
        }
    }

    fn persist(&self) -> anyhow::Result<()> {
        if self.metadata_path.is_empty() {
            return Ok(());
        }
        let _guard = self.save_lock.lock().unwrap();
        let file = ConfigFile {
            datasets: self.datasets.read().unwrap().clone(),
            documents: self.documents.read().unwrap().clone(),
        };
        crate::persistence::save_json(std::path::Path::new(&self.metadata_path), &file)
    }

    /// The dataset's configuration with `built_in_metadata` filled in.
    pub fn get(&self, kb_id: &str) -> AutoMetadataConfig {
        let mut config = self
            .datasets
            .read()
            .unwrap()
            .get(kb_id)
            .cloned()
            .unwrap_or_default();
        config.built_in_metadata = built_in_fields();
        config
    }

    pub fn replace(&self, kb_id: &str, mut config: AutoMetadataConfig) -> anyhow::Result<()> {
        // The built-ins are the deployment's, not the client's: whatever the body says is replaced.
        config.built_in_metadata = built_in_fields();
        self.datasets
            .write()
            .unwrap()
            .insert(kb_id.to_string(), config);
        self.persist()
    }

    pub fn get_document(&self, doc_id: &str) -> Option<DocumentMetadataConfig> {
        self.documents.read().unwrap().get(doc_id).cloned()
    }

    pub fn replace_document(
        &self,
        doc_id: &str,
        config: DocumentMetadataConfig,
    ) -> anyhow::Result<()> {
        self.documents
            .write()
            .unwrap()
            .insert(doc_id.to_string(), config);
        self.persist()
    }

    /// Remove everything recorded for a dataset (its own schema and its documents').
    pub fn drop_dataset(&self, kb_id: &str, document_ids: &[String]) -> anyhow::Result<()> {
        self.datasets.write().unwrap().remove(kb_id);
        if !document_ids.is_empty() {
            let mut documents = self.documents.write().unwrap();
            for doc_id in document_ids {
                documents.remove(doc_id);
            }
        }
        self.persist()
    }
}

/// Validate one field list, returning the first problem in the guide's wording.
pub(crate) fn validate_fields(fields: &[MetadataField]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for field in fields {
        let key = field.key.trim();
        if key.is_empty() {
            return Err("Field: <key> - Message: <String should have at least 1 character>".into());
        }
        if key.chars().count() > 255 {
            return Err(format!(
                "Field: <key> - Message: <String should have at most 255 characters> - Value: <{key}>"
            ));
        }
        if !matches!(
            field.field_type.as_str(),
            "string" | "list" | "time" | "number"
        ) {
            return Err(format!(
                "Field: <type> - Message: <Input should be 'string', 'list', 'time' or 'number'> - Value: <{}>",
                field.field_type
            ));
        }
        if !seen.insert(key.to_string()) {
            return Err(format!(
                "Field: <key> - Message: <Duplicated metadata key> - Value: <{key}>"
            ));
        }
        if let Some(values) = &field.enum_values {
            if values.iter().any(|value| value.is_empty()) {
                return Err(format!(
                    "Field: <enum> - Message: <Enum values must not be empty> - Value: <{key}>"
                ));
            }
        }
    }
    Ok(())
}

/// A dataset a reader may see, or `None`.
fn visible_kb(state: &AppState, auth: &AuthContext, kb_id: &str) -> Option<()> {
    let kb = state.kbs.get(kb_id)?;
    if kb.owner_id == auth.user_id || auth.is_admin || kb.owner_id.is_empty() {
        Some(())
    } else {
        None
    }
}

/// `get_error_argument_result`: `{code:101, message}` at HTTP 200 and no `data` key — the
/// metadata family's argument errors (round12 probes `mcfg_put_empty` / `meta_put_empty_ds`).
fn argument_result(message: &str) -> Response {
    api_error_result(axum::http::StatusCode::OK, code::INVALID_ARGUMENT, message)
}

/// The document-scoped config route's business errors, `get_error_data_result` shape: HTTP 200
/// with `{code:102, message}` and **no** `data` key (`mcfg_fake` probe).
fn data_error(message: &str) -> Response {
    api_error_result(
        axum::http::StatusCode::OK,
        code::INVALID_OR_MISSING_DATA,
        message,
    )
}

/// An unknown or foreign dataset: upstream's service answers
/// `User '<tenant>' lacks permission for dataset '<id>'` as a `102` (`meta_get_fake` probe).
fn permission_error(auth: &AuthContext, kb_id: &str) -> Response {
    api_error_result(
        axum::http::StatusCode::OK,
        code::INVALID_OR_MISSING_DATA,
        &format!(
            "User '{}' lacks permission for dataset '{kb_id}'",
            auth.user_id
        ),
    )
}

/// A raised exception answered by the global handler: `{code:100, data:null, message}` at
/// HTTP 200 (round12 probes `mcfg_put_bad` / `mcfg_put_arr`; round13 `upd2_body_bad` /
/// `upd2_body_arr` for the document route).
pub(crate) fn raised_error(message: String) -> Response {
    Json(serde_json::json!({
        "code": EXCEPTION_ERROR,
        "data": serde_json::Value::Null,
        "message": message,
    }))
    .into_response()
}

/// Python's `repr` for the value shapes that appear in the validation messages below.
pub(crate) fn python_repr(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "None".to_string(),
        serde_json::Value::Bool(true) => "True".to_string(),
        serde_json::Value::Bool(false) => "False".to_string(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => {
            format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
        }
        serde_json::Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_repr).collect::<Vec<_>>().join(", ")
        ),
        serde_json::Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, value)| format!("'{key}': {}", python_repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Python's `str(value)` as `format_validation_error_message` renders the input value:
/// a top-level string is its own text (no quotes), every other shape goes through the repr
/// above (so nested strings inside lists/dicts keep their single quotes).
pub(crate) fn py_value_str(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => python_repr(other),
    }
}

/// Python's `type(value).__name__`, for the payload-shape message.
fn python_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "NoneType",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(number) if number.is_i64() || number.is_u64() => "int",
        serde_json::Value::Number(_) => "float",
        serde_json::Value::String(_) => "str",
        serde_json::Value::Array(_) => "list",
        serde_json::Value::Object(_) => "dict",
    }
}

const AUTO_METADATA_FIELD_KEYS: [&str; 4] = ["key", "type", "description", "enum"];

type AutoMetadataIssue = (String, String, String);

fn chars_len(text: &str) -> usize {
    text.chars().count()
}

fn auto_metadata_error(
    issues: &mut Vec<AutoMetadataIssue>,
    field: &str,
    message: impl Into<String>,
    value: &serde_json::Value,
) {
    // format_validation_error_message truncates the rendered input at 128 characters.
    let rendered = py_value_str(value);
    let rendered = if rendered.chars().count() > 128 {
        rendered.chars().take(125).collect::<String>() + "..."
    } else {
        rendered
    };
    issues.push((field.to_string(), message.into(), rendered));
}

fn validate_auto_metadata_field(
    issues: &mut Vec<AutoMetadataIssue>,
    base: &str,
    item: &serde_json::Value,
) {
    let Some(object) = item.as_object() else {
        auto_metadata_error(
            issues,
            base,
            "Input should be a valid dictionary or instance of AutoMetadataField",
            item,
        );
        return;
    };
    match object.get("key") {
        None => issues.push((
            format!("{base}.key"),
            "Field required".to_string(),
            python_repr(item),
        )),
        Some(value) => {
            if !value.is_string() {
                auto_metadata_error(
                    issues,
                    &format!("{base}.key"),
                    "Input should be a valid string",
                    value,
                );
            } else {
                let text = value.as_str().unwrap_or("").trim();
                if chars_len(text) < 1 {
                    auto_metadata_error(
                        issues,
                        &format!("{base}.key"),
                        "String should have at least 1 character",
                        value,
                    );
                } else if chars_len(text) > 255 {
                    auto_metadata_error(
                        issues,
                        &format!("{base}.key"),
                        "String should have at most 255 characters",
                        value,
                    );
                }
            }
        }
    }
    match object.get("type") {
        None => issues.push((
            format!("{base}.type"),
            "Field required".to_string(),
            python_repr(item),
        )),
        Some(value) => {
            let valid = value
                .as_str()
                .is_some_and(|text| matches!(text, "string" | "list" | "time" | "number"));
            if !valid {
                auto_metadata_error(
                    issues,
                    &format!("{base}.type"),
                    "Input should be 'string', 'list', 'time' or 'number'",
                    value,
                );
            }
        }
    }
    if let Some(value) = object.get("description")
        && !value.is_null()
    {
        if !value.is_string() {
            auto_metadata_error(
                issues,
                &format!("{base}.description"),
                "Input should be a valid string",
                value,
            );
        } else if chars_len(value.as_str().unwrap_or("")) > 65535 {
            auto_metadata_error(
                issues,
                &format!("{base}.description"),
                "String should have at most 65535 characters",
                value,
            );
        }
    }
    if let Some(value) = object.get("enum")
        && !value.is_null()
    {
        match value.as_array() {
            Some(items) => {
                for (index, item) in items.iter().enumerate() {
                    if !item.is_string() {
                        auto_metadata_error(
                            issues,
                            &format!("{base}.enum.{index}"),
                            "Input should be a valid string",
                            item,
                        );
                    }
                }
            }
            None => auto_metadata_error(
                issues,
                &format!("{base}.enum"),
                "Input should be a valid list",
                value,
            ),
        }
    }
    for (key, value) in object {
        if !AUTO_METADATA_FIELD_KEYS.contains(&key.as_str()) {
            auto_metadata_error(
                issues,
                &format!("{base}.{key}"),
                "Extra inputs are not permitted",
                value,
            );
        }
    }
}

fn normalized_auto_metadata_field(item: &serde_json::Value) -> serde_json::Value {
    let object = item.as_object().cloned().unwrap_or_default();
    let key = object
        .get("key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let field_type = object
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let description = object
        .get("description")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let enum_values = object
        .get("enum")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    serde_json::json!({
        "key": key,
        "type": field_type,
        "description": description,
        "enum": enum_values,
    })
}

/// Every pydantic-style issue in an AutoMetadataConfig body, in validation order.
pub fn auto_metadata_issues(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Vec<AutoMetadataIssue> {
    let mut issues: Vec<AutoMetadataIssue> = Vec::new();
    for name in ["metadata", "built_in_metadata"] {
        let items = match payload.get(name) {
            None => Vec::new(),
            Some(value) => match value.as_array() {
                Some(items) => items.clone(),
                None => {
                    auto_metadata_error(&mut issues, name, "Input should be a valid list", value);
                    Vec::new()
                }
            },
        };
        for (index, item) in items.iter().enumerate() {
            validate_auto_metadata_field(&mut issues, &format!("{name}.{index}"), item);
        }
    }
    for (key, value) in payload {
        if key != "metadata" && key != "built_in_metadata" {
            auto_metadata_error(&mut issues, key, "Extra inputs are not permitted", value);
        }
    }
    issues
}

/// Validate and normalize an AutoMetadataConfig body. Returns the pydantic model_dump
/// shape (both lists present, every field carrying explicit description/enum keys), or
/// the joined format_validation_error_message lines.
pub fn validate_auto_metadata_payload(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let issues = auto_metadata_issues(payload);
    if !issues.is_empty() {
        let lines: Vec<String> = issues
            .iter()
            .map(|(field, message, value)| {
                format!("Field: <{field}> - Message: <{message}> - Value: <{value}>")
            })
            .collect();
        return Err(lines.join("\n"));
    }
    let mut normalized: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for name in ["metadata", "built_in_metadata"] {
        let items = payload
            .get(name)
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        normalized.insert(
            name.into(),
            serde_json::Value::Array(items.iter().map(normalized_auto_metadata_field).collect()),
        );
    }
    Ok(serde_json::Value::Object(normalized))
}

/// The get_auto_metadata / update_auto_metadata payload read out of a stored parser config:
/// both lists pass through verbatim (upstream returns what is stored, or an empty list).
pub fn dataset_metadata_config(parser_config: &str) -> serde_json::Value {
    let stored = serde_json::from_str::<serde_json::Value>(parser_config)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let list = |name: &str| match stored.get(name) {
        Some(serde_json::Value::Array(items)) => serde_json::Value::Array(items.clone()),
        _ => serde_json::Value::Array(Vec::new()),
    };
    serde_json::json!({
        "metadata": list("metadata"),
        "built_in_metadata": list("built_in_metadata"),
    })
}

/// Grow one field list out of a request value.
///
/// The shapes `validate_fields` cannot see — a list that is not a list, items that are not
/// field objects, fields of the wrong type — answer in the catalogue's pydantic wording, the
/// same `Field: <…> - Message: <…> - Value: <…>` form `format_validation_error_message`
/// produces upstream.
fn fields_from_value(
    list_name: &str,
    value: &serde_json::Value,
) -> Result<Vec<MetadataField>, String> {
    let Some(items) = value.as_array() else {
        return Err(format!(
            "Field: <{list_name}> - Message: <Input should be a valid list> - Value: <{}>",
            python_repr(value)
        ));
    };
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            return Err(format!(
                "Field: <{list_name}.{index}> - Message: <Input should be a valid dictionary or instance of AutoMetadataField> - Value: <{}>",
                python_repr(item)
            ));
        };
        match object.get("key") {
            None => {
                return Err(format!(
                    "Field: <{list_name}.{index}.key> - Message: <Field required> - Value: <{}>",
                    python_repr(item)
                ));
            }
            Some(value) if !value.is_string() => {
                return Err(format!(
                    "Field: <{list_name}.{index}.key> - Message: <Input should be a valid string> - Value: <{}>",
                    python_repr(value)
                ));
            }
            Some(_) => {}
        }
        match object.get("type") {
            None => {
                return Err(format!(
                    "Field: <{list_name}.{index}.type> - Message: <Field required> - Value: <{}>",
                    python_repr(item)
                ));
            }
            Some(serde_json::Value::String(_)) => {}
            Some(value) => {
                return Err(format!(
                    "Field: <{list_name}.{index}.type> - Message: <Input should be 'string', 'list', 'time' or 'number'> - Value: <{}>",
                    python_repr(value)
                ));
            }
        }
        if let Some(value) = object.get("description")
            && !value.is_null()
            && !value.is_string()
        {
            return Err(format!(
                "Field: <{list_name}.{index}.description> - Message: <Input should be a valid string> - Value: <{}>",
                python_repr(value)
            ));
        }
        if let Some(serde_json::Value::Array(values)) = object.get("enum") {
            for (position, value) in values.iter().enumerate() {
                if !value.is_string() {
                    return Err(format!(
                        "Field: <{list_name}.{index}.enum.{position}> - Message: <Input should be a valid string> - Value: <{}>",
                        python_repr(value)
                    ));
                }
            }
        } else if let Some(value) = object.get("enum")
            && !value.is_null()
        {
            return Err(format!(
                "Field: <{list_name}.{index}.enum> - Message: <Input should be a valid list> - Value: <{}>",
                python_repr(value)
            ));
        }
    }
    serde_json::from_value(value.clone()).map_err(|_| {
        format!(
            "Field: <{list_name}> - Message: <Input should be a valid list> - Value: <{}>",
            python_repr(value)
        )
    })
}

fn config_json(config: &AutoMetadataConfig) -> serde_json::Value {
    // Key order follows upstream's jsonify output (alphabetical; `mcfg_get_real` measured
    // `{"built_in_metadata": …, "metadata": …}`).
    serde_json::json!({
        "built_in_metadata": config.built_in_metadata,
        "metadata": config.metadata,
    })
}

/// GET /api/v1/datasets/{dataset_id}/metadata/config.
pub async fn get_dataset_metadata_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
) -> Response {
    if visible_kb(&state, &auth, &kb_id).is_none() {
        return permission_error(&auth, &kb_id);
    }
    let Some(kb) = state.kbs.get(&kb_id) else {
        return permission_error(&auth, &kb_id);
    };
    // Upstream get_auto_metadata reads parser_config.metadata / parser_config.built_in_metadata
    // and returns an empty list when the key is absent; a fresh dataset therefore answers
    // {"metadata": [], "built_in_metadata": []} even though the UI may offer its own built-ins.
    let config = dataset_metadata_config(&kb.parser_config);
    Json(serde_json::json!({ "code": 0, "data": config })).into_response()
}

/// PUT /api/v1/datasets/{dataset_id}/metadata/config.
pub async fn update_dataset_metadata_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(kb_id): Path<String>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Response {
    if visible_kb(&state, &auth, &kb_id).is_none() {
        return permission_error(&auth, &kb_id);
    }
    // Upstream update_auto_metadata runs validate_and_parse_json_request: content type
    // first, then JSON syntax, then payload shape (round15 probes: empty JSON body is the
    // malformed-JSON message, a text/plain body names the content type).
    let content_type = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let mime = content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase());
    if mime.as_deref() != Some("application/json") {
        return argument_result(&format!(
            "Unsupported content type: Expected application/json, got {}",
            content_type.unwrap_or("None")
        ));
    }
    if body.is_empty() {
        return argument_result(
            "Malformed JSON syntax: Missing commas/brackets or invalid encoding",
        );
    }
    let parsed: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(_) => {
            return argument_result(
                "Malformed JSON syntax: Missing commas/brackets or invalid encoding",
            );
        }
    };
    // Python: payload = await request.get_json() or {}. Every falsy JSON value (None,
    // False, 0, "", [], {}) becomes an empty object, so "[]" is a valid empty config.
    let falsy = parsed.is_null()
        || matches!(&parsed, serde_json::Value::Bool(false))
        || matches!(&parsed, serde_json::Value::String(text) if text.is_empty())
        || matches!(&parsed, serde_json::Value::Array(items) if items.is_empty())
        || matches!(&parsed, serde_json::Value::Object(map) if map.is_empty())
        || parsed.as_f64() == Some(0.0);
    let payload = if falsy {
        serde_json::Map::new()
    } else if let Some(object) = parsed.as_object() {
        object.clone()
    } else {
        return argument_result(&format!(
            "Invalid request payload: expected object, got {}",
            python_type_name(&parsed)
        ));
    };
    let normalized = match validate_auto_metadata_payload(&payload) {
        Ok(value) => value,
        Err(message) => return argument_result(&message),
    };
    let Some(kb) = state.kbs.get(&kb_id) else {
        return permission_error(&auth, &kb_id);
    };
    let mut stored = serde_json::from_str::<serde_json::Value>(&kb.parser_config)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    stored.insert("metadata".into(), normalized["metadata"].clone());
    stored.insert(
        "built_in_metadata".into(),
        normalized["built_in_metadata"].clone(),
    );
    let serialized = serde_json::Value::Object(stored).to_string();
    match state
        .kbs
        .update_config(&kb_id, None, None, Some(&serialized), None)
    {
        Ok(Some(_)) => {}
        Ok(None) => return permission_error(&auth, &kb_id),
        Err(error) => {
            return api_error_code(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                code::OPERATION_ERROR,
                &error.to_string(),
            );
        }
    }
    Json(serde_json::json!({ "code": 0, "data": normalized })).into_response()
}

/// `PUT /api/v1/datasets/{dataset_id}/documents/{document_id}/metadata/config`.
pub async fn update_document_metadata_config(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((kb_id, doc_id)): Path<(String, String)>,
    body: Bytes,
) -> Response {
    if visible_kb(&state, &auth, &kb_id).is_none() {
        return data_error("you don't own the dataset");
    }
    match state.docs.get(&doc_id) {
        Some(doc) if doc.kb_id == kb_id => {}
        _ => return data_error(&format!("document {doc_id} not found in dataset {kb_id}")),
    }
    // Upstream reads the body through `get_request_json()`: an empty body normalises to `{}`
    // and the route requires the `metadata` key — `101 "metadata is required"` (round12
    // probes `mcfg_put_empty` / `mcfg_put_obj` / `mcfg_put_noct`). A malformed JSON body and
    // a non-object payload surface as raised exceptions through the global handler
    // (`mcfg_put_bad` / `mcfg_put_arr`).
    let parsed: serde_json::Value = if body.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_slice(&body) {
            Ok(value) => value,
            Err(_) => return raised_error("<BadRequest '400: Bad Request'>".to_string()),
        }
    };
    if !parsed.is_object() {
        return raised_error("TypeError('JSON payload must be an object.')".to_string());
    }
    let Some(metadata_value) = parsed.get("metadata") else {
        return argument_result("metadata is required");
    };
    // Only `metadata` reaches the store, as upstream's route does; the doc-level override
    // keeps no built-in list.
    let metadata = match fields_from_value("metadata", metadata_value) {
        Ok(fields) => fields,
        Err(message) => return argument_result(&message),
    };
    if let Err(message) = validate_fields(&metadata) {
        return argument_result(&message);
    }
    let body = DocumentMetadataConfig {
        metadata,
        built_in_metadata: Vec::new(),
    };
    if let Err(error) = state.metadata_config.replace_document(&doc_id, body) {
        return api_error_code(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        );
    }
    let stored = state
        .metadata_config
        .get_document(&doc_id)
        .unwrap_or_default();
    Json(serde_json::json!({
        "code": 0,
        "data": { "built_in_metadata": stored.built_in_metadata, "metadata": stored.metadata },
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str, field_type: &str) -> MetadataField {
        MetadataField {
            key: key.into(),
            field_type: field_type.into(),
            description: None,
            enum_values: None,
        }
    }

    #[test]
    fn a_valid_schema_passes_and_every_rule_has_a_message() {
        assert!(validate_fields(&[field("author", "string"), field("tags", "list")]).is_ok());
        // An empty key.
        let error = validate_fields(&[field("   ", "string")]).unwrap_err();
        assert!(error.contains("at least 1 character"), "{error}");
        // An unknown type, named as the guide names it.
        let error = validate_fields(&[field("author", "text")]).unwrap_err();
        assert!(
            error.contains("'string', 'list', 'time' or 'number'"),
            "{error}"
        );
        // A duplicate key would make every later filter ambiguous.
        let error =
            validate_fields(&[field("author", "string"), field("author", "list")]).unwrap_err();
        assert!(error.contains("Duplicated metadata key"), "{error}");
        // A key past the documented limit.
        let long = "k".repeat(256);
        assert!(
            validate_fields(&[field(&long, "string")])
                .unwrap_err()
                .contains("255")
        );
    }

    #[test]
    fn the_built_ins_are_the_deployments_and_the_user_list_is_the_clients() {
        let store = MetadataConfigStore::in_memory();
        store
            .replace(
                "kb-1",
                AutoMetadataConfig {
                    metadata: vec![field("author", "string")],
                    // A body that tries to set the built-ins is overridden.
                    built_in_metadata: vec![field("mine", "string")],
                },
            )
            .unwrap();
        let config = store.get("kb-1");
        assert_eq!(config.metadata.len(), 1);
        assert_eq!(config.metadata[0].key, "author");
        assert!(
            config.built_in_metadata.iter().any(|f| f.key == "author"),
            "the deployment's built-ins are offered: {config:?}"
        );
        assert!(
            !config.built_in_metadata.iter().any(|f| f.key == "mine"),
            "a client cannot define the built-ins"
        );
    }

    #[test]
    fn a_document_override_is_stored_per_document() {
        let store = MetadataConfigStore::in_memory();
        store
            .replace_document(
                "doc-1",
                DocumentMetadataConfig {
                    metadata: vec![field("author", "string")],
                    built_in_metadata: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(
            store.get_document("doc-1").unwrap().metadata[0].key,
            "author"
        );
        assert!(store.get_document("doc-2").is_none());
    }
}
