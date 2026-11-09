//! Dataset REST object mapping and request validation, aligned with RAGFlow
//! v0.27.2 `api/apps/restful_apis/dataset_api.py`, its
//! `api/apps/services/dataset_api_service.py`, and the pydantic request models
//! in `api/utils/validation_utils.py`.
//!
//! The live v0.27.2 instance answers every dataset business result with HTTP 200
//! and a body code (0/101/102); pydantic validation failures are code 101 with
//! lines of `Field: <field> - Message: <message> - Value: <value>`, and service
//! failures are code 102 with the service's own message.

use serde_json::{Map, Value, json};

use crate::kb::KnowledgeBase;

pub const DATASET_NAME_LIMIT: usize = 128;
pub const REST_API_MAX_PAGE_SIZE: usize = 100;
pub const REST_API_MAX_IDS: usize = 100;

pub const CHUNK_METHOD_LITERAL_ERROR: &str = "Input should be 'naive', 'book', 'email', 'laws', 'manual', 'one', 'paper', 'picture', 'presentation', 'qa', 'table', 'tag' or 'resume'";

pub const ALLOWED_CHUNK_METHODS: [&str; 13] = [
    "naive",
    "book",
    "email",
    "laws",
    "manual",
    "one",
    "paper",
    "picture",
    "presentation",
    "qa",
    "table",
    "tag",
    "resume",
];

/// The upstream `ParserConfig` model defaults, keyed exactly as the API emits
/// them. `graphrag` and `raptor` are stripped before the response is sent
/// (`api_utils.strip_graphrag_raptor_config`).
fn parser_config_defaults() -> Map<String, Value> {
    json!({
        "auto_keywords": 0,
        "auto_questions": 0,
        "chunk_token_num": 512,
        "delimiter": "\n",
        "html4excel": false,
        "layout_recognize": "DeepDOC",
        "llm_id": "",
        "parent_child": { "use_parent_child": false, "children_delimiter": "\n" },
        "enable_children": false,
        "children_delimiter": "",
        "tag_kb_ids": [],
        "topn_tags": 1,
        "filename_embd_weight": 0.1,
        "task_page_size": Value::Null,
        "pages": Value::Null,
        "compilation_template_group_id": [],
        "table_column_mode": Value::Null,
        "table_column_roles": Value::Null,
        "table_column_names": Value::Null,
        "image_context_size": 0,
        "table_context_size": 0,
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// The subset `get_parser_config("naive", None)` produces when the request did
/// not carry a parser_config at all (upstream create with only a name). The
/// pydantic ParserConfig model defaults are NOT part of this shape.
fn parser_config_service_defaults() -> Map<String, Value> {
    json!({
        "table_context_size": 0,
        "image_context_size": 0,
        "layout_recognize": "DeepDOC",
        "chunk_token_num": 512,
        "delimiter": "@NL@",
        "auto_keywords": 0,
        "auto_questions": 0,
        "html4excel": false,
        "topn_tags": 3,
        "parent_child": { "use_parent_child": false, "children_delimiter": "@NL@" },
        "children_delimiter": "",
        "llm_id": "",
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// RayRAG-internal parser keys that are not RAGFlow parser-config fields; they
/// are dropped from the REST face instead of leaking into client payloads.
const INTERNAL_PARSER_KEYS: [&str; 8] = [
    "chunk_method",
    "parse_type",
    "overlapped_percent",
    "chunk_size",
    "overlap",
    "graphrag",
    "raptor",
    "parser_config_model",
];

/// `get_parser_config(chunk_method, None)` for the update path: naive keeps its
/// full default set, every other built-in method gets the base defaults only.
pub fn parser_config_for_method(method: &str) -> Value {
    let mut config = if method == "naive" {
        parser_config_service_defaults()
    } else {
        json!({ "table_context_size": 0, "image_context_size": 0 })
            .as_object()
            .cloned()
            .unwrap_or_default()
    };
    config.insert("chunk_method".into(), Value::String(method.to_string()));
    Value::Object(config)
}

/// The dataset's `chunk_method` (upstream `parser_id`): stored in the RayRAG
/// parser config as `chunk_method`, defaulting to `naive`.
pub fn dataset_chunk_method(kb: &KnowledgeBase) -> String {
    serde_json::from_str::<Value>(&kb.parser_config)
        .ok()
        .and_then(|value| {
            value
                .get("chunk_method")
                .or_else(|| value.get("parser_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|method| !method.trim().is_empty())
        .unwrap_or_else(|| "naive".to_string())
}

/// The dataset `parser_config` as the REST API reports it: the pydantic model
/// defaults merged with the stored config, internal keys stripped, and the
/// parent-child settings flattened exactly like upstream `get_parser_config`.
pub fn dataset_parser_config(kb: &KnowledgeBase) -> Value {
    let stored = serde_json::from_str::<Value>(&kb.parser_config)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    // Upstream either stored the service-side naive defaults (parser_config was
    // omitted on create) or a ParserConfig-model-normalized object (the request
    // carried parser_config). The marker records which face was stored.
    let model_normalized = stored
        .get("parser_config_model")
        .and_then(Value::as_str)
        .is_some_and(|flag| flag == "1");
    let mut merged = if model_normalized {
        parser_config_defaults()
    } else {
        parser_config_service_defaults()
    };
    for (key, value) in stored {
        if INTERNAL_PARSER_KEYS.contains(&key.as_str()) {
            continue;
        }
        if key == "parent_child" {
            if let Some(parent) = value.as_object() {
                let use_parent = parent
                    .get("use_parent_child")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let children = parent
                    .get("children_delimiter")
                    .and_then(Value::as_str)
                    .unwrap_or("\n");
                merged.insert(
                    "parent_child".into(),
                    json!({ "use_parent_child": use_parent, "children_delimiter": children }),
                );
                if model_normalized {
                    merged.insert("enable_children".into(), Value::Bool(use_parent));
                }
                merged.insert(
                    "children_delimiter".into(),
                    Value::String(if use_parent {
                        children.to_string()
                    } else {
                        String::new()
                    }),
                );
            }
            continue;
        }
        merged.insert(key, value);
    }
    // A stored config without parent_child still needs the flattened fields the
    // upstream response always carries.
    if let Some(parent) = merged.get("parent_child").and_then(Value::as_object) {
        let use_parent = parent
            .get("use_parent_child")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let children = parent
            .get("children_delimiter")
            .and_then(Value::as_str)
            .unwrap_or("\n")
            .to_string();
        if model_normalized {
            merged
                .entry("enable_children")
                .or_insert(Value::Bool(use_parent));
        }
        merged
            .entry("children_delimiter")
            .or_insert(Value::String(if use_parent {
                children
            } else {
                String::new()
            }));
    }
    Value::Object(merged)
}

/// One dataset row in the REST shape (upstream `remap_dictionary_keys(kb.to_dict())`).
///
/// The upstream key set is authoritative; RayRAG's own keys (`embd_id`,
/// `doc_count`, `owner_id`, `created_at`, `updated_at`, `tag_sets`,
/// `prompt_config`) stay alongside it for RayRAG's web UI. Endpoint-only extras
/// (`nickname`, `tenant_avatar`, `embedding_model_name`, parsing counts,
/// `size`, `connectors`) are added by the handlers.
pub fn dataset_entry(kb: &KnowledgeBase, token_num: u64) -> Value {
    let chunk_method = dataset_chunk_method(kb);
    let avatar = if kb.avatar.trim().is_empty() {
        Value::Null
    } else {
        Value::String(kb.avatar.clone())
    };
    let description = if kb.description.trim().is_empty() {
        Value::Null
    } else {
        Value::String(kb.description.clone())
    };
    let pipeline_id = kb
        .pipeline_id
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .map_or(Value::Null, |value| Value::String(value.clone()));
    let mut entry = json!({
        "avatar": avatar,
        "chunk_count": kb.chunk_count,
        "chunk_method": chunk_method,
        "create_date": crate::api::compat::ms_to_datetime_string(kb.created_at),
        "create_time": kb.created_at,
        "created_by": kb.owner_id,
        "description": description,
        "document_count": kb.doc_count,
        "embedding_model": kb.embd_id,
        "graphrag_task_finish_at": Value::Null,
        "graphrag_task_id": Value::Null,
        "id": kb.id,
        "language": kb.language,
        "mindmap_task_finish_at": Value::Null,
        "mindmap_task_id": Value::Null,
        "name": kb.name,
        "pagerank": kb.pagerank,
        "parser_config": dataset_parser_config(kb),
        "permission": kb.permission,
        "pipeline_id": pipeline_id,
        "raptor_task_finish_at": Value::Null,
        "raptor_task_id": Value::Null,
        "session_essence_task_finish_at": Value::Null,
        "session_essence_task_id": Value::Null,
        "session_graph_task_finish_at": Value::Null,
        "session_graph_task_id": Value::Null,
        "similarity_threshold": kb.similarity_threshold,
        "skill_task_finish_at": Value::Null,
        "skill_task_id": Value::Null,
        "status": "1",
        "structure_graph_task_finish_at": Value::Null,
        "structure_graph_task_id": Value::Null,
        "structure_mindmap_task_finish_at": Value::Null,
        "structure_mindmap_task_id": Value::Null,
        "structure_task_finish_at": Value::Null,
        "structure_task_id": Value::Null,
        "tenant_embd_id": Value::Null,
        "tenant_id": kb.owner_id,
        "timeline_task_finish_at": Value::Null,
        "timeline_task_id": Value::Null,
        "token_num": token_num,
        "update_date": crate::api::compat::ms_to_datetime_string(kb.updated_at),
        "update_time": kb.updated_at,
        "vector_similarity_weight": kb.vector_similarity_weight,
        "wiki_task_finish_at": Value::Null,
        "wiki_task_id": Value::Null,
        // RayRAG superset keys (web UI + previous clients).
        "embd_id": kb.embd_id,
        "doc_count": kb.doc_count,
        "owner_id": kb.owner_id,
        "created_at": kb.created_at,
        "updated_at": kb.updated_at,
        "tag_sets": kb.tag_sets,
        "prompt_config": kb.prompt_config,
    });
    // RayRAG's own client used to read `parser_config` as a JSON string; the
    // object face above is the RAGFlow contract, so the string is mirrored under
    // a non-conflicting key for old callers.
    entry["parser_config_json"] = Value::String(kb.parser_config.clone());
    entry
}

/// Per-run document counts as the list endpoint reports them with
/// `include_parsing_status=true`.
pub fn parsing_counts(runs: impl IntoIterator<Item = String>) -> Value {
    let mut unstart = 0u64;
    let mut running = 0u64;
    let mut cancel = 0u64;
    let mut done = 0u64;
    let mut fail = 0u64;
    for run in runs {
        match run.as_str() {
            "UNSTART" | "" => unstart += 1,
            "RUNNING" | "SCHEDULE" => running += 1,
            "CANCEL" | "CANCELLED" => cancel += 1,
            "DONE" => done += 1,
            "FAIL" => fail += 1,
            _ => unstart += 1,
        }
    }
    json!({
        "unstart_count": unstart,
        "running_count": running,
        "cancel_count": cancel,
        "done_count": done,
        "fail_count": fail,
    })
}

/// Validate a UUID-ish id the way upstream `validate_uuid1_hex` does for the
/// REST filters (any UUID version, hyphenated or 32-hex) and return the
/// 32-character lowercase form used for comparisons.
pub fn normalize_uuid(raw: &str) -> Option<String> {
    let text = raw.trim();
    let compact: String = if text.len() == 36 {
        let bytes = text.as_bytes();
        let valid = bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 8 | 13 | 18 | 23) && *byte == b'-'
                || (*byte as char).is_ascii_hexdigit() && !matches!(index, 8 | 13 | 18 | 23)
        });
        if !valid {
            return None;
        }
        text.chars().filter(|ch| *ch != '-').collect()
    } else if text.len() == 32 {
        text.to_string()
    } else {
        return None;
    };
    if compact.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(compact.to_ascii_lowercase())
    } else {
        None
    }
}

/// One pydantic-style validation issue.
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub field: String,
    pub message: String,
    pub value: String,
}

impl ValidationIssue {
    pub fn new(
        field: impl Into<String>,
        message: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            value: value.into(),
        }
    }
}

/// Render issues the way upstream `format_validation_error_message` does.
pub fn format_issues(issues: &[ValidationIssue]) -> String {
    issues
        .iter()
        .map(|issue| {
            let mut value = issue.value.clone();
            if value.chars().count() > 128 {
                value = format!("{}...", value.chars().take(125).collect::<String>());
            }
            format!(
                "Field: <{}> - Message: <{}> - Value: <{}>",
                issue.field, issue.message, value
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Python `repr`-ish rendering for the model-level dependency-error value.
pub fn python_repr(value: &Map<String, Value>) -> String {
    let mut parts = Vec::new();
    for (key, item) in value {
        parts.push(format!("'{}': {}", key, python_value(item)));
    }
    format!("{{{}}}", parts.join(", "))
}

fn python_value(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => format!("'{}'", text.replace('\'', "\\'")),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(python_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(map) => python_repr(map),
    }
}

/// Typed dataset create/update request after pydantic-equivalent validation.
#[derive(Debug, Clone, Default)]
pub struct DatasetWrite {
    pub name: Option<String>,
    pub avatar: Option<String>,
    pub description: Option<String>,
    pub embedding_model: Option<String>,
    pub permission: String,
    pub parse_type: Option<i64>,
    pub pipeline_id: Option<String>,
    pub chunk_method: Option<String>,
    pub language: Option<String>,
    pub parser_config: Option<Value>,
    pub pagerank: i64,
    pub connectors: Option<Vec<Value>>,
    /// Keys the client actually sent (pydantic `model_fields_set`).
    pub fields_set: Vec<String>,
}

impl DatasetWrite {
    pub fn provided(&self, field: &str) -> bool {
        self.fields_set.iter().any(|item| item == field)
    }
}

const CREATE_FIELDS: [&str; 11] = [
    "name",
    "avatar",
    "description",
    "embedding_model",
    "permission",
    "parse_type",
    "pipeline_id",
    "chunk_method",
    "language",
    "parser_config",
    "auto_metadata_config",
];

const UPDATE_ONLY_FIELDS: [&str; 2] = ["pagerank", "connectors"];

/// Render a JSON value the way pydantic's error formatter renders the input.
pub fn display_value(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(_) => python_value(value),
    }
}

fn issue(
    field: impl Into<String>,
    message: impl Into<String>,
    value: impl Into<String>,
) -> ValidationIssue {
    ValidationIssue::new(field, message, value)
}

fn character_suffix(count: usize) -> &'static str {
    if count == 1 {
        "character"
    } else {
        "characters"
    }
}

fn chars_len(text: &str) -> usize {
    text.chars().count()
}

fn string_field(
    issues: &mut Vec<ValidationIssue>,
    field: &str,
    value: Option<&Value>,
    max_len: Option<usize>,
    min_len: Option<usize>,
    strip: bool,
) -> Option<String> {
    let Some(value) = value else {
        return None;
    };
    if value.is_null() {
        return None;
    }
    let Some(raw) = value.as_str() else {
        issues.push(issue(
            field,
            "Input should be a valid string",
            display_value(value),
        ));
        return None;
    };
    let text = if strip {
        raw.trim().to_string()
    } else {
        raw.to_string()
    };
    if let Some(limit) = max_len {
        if chars_len(&text) > limit {
            issues.push(issue(
                field,
                format!(
                    "String should have at most {limit} {}",
                    character_suffix(limit)
                ),
                raw.to_string(),
            ));
            return Some(text);
        }
    }
    if let Some(limit) = min_len {
        if chars_len(&text) < limit {
            issues.push(issue(
                field,
                format!(
                    "String should have at least {limit} {}",
                    character_suffix(limit)
                ),
                raw.to_string(),
            ));
            return Some(text);
        }
    }
    Some(text)
}

fn strict_int_field(
    issues: &mut Vec<ValidationIssue>,
    field: &str,
    value: Option<&Value>,
    min: i64,
    max: i64,
) -> Option<i64> {
    let Some(value) = value else {
        return None;
    };
    if value.is_null() {
        return None;
    }
    let Some(number) = value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|float| float.fract() == 0.0)
            .map(|float| float as i64)
    }) else {
        issues.push(issue(
            field,
            "Input should be a valid integer",
            display_value(value),
        ));
        return None;
    };
    if number < min {
        issues.push(issue(
            field,
            format!("Input should be greater than or equal to {min}"),
            display_value(value),
        ));
        return Some(number);
    }
    if number > max {
        issues.push(issue(
            field,
            format!("Input should be less than or equal to {max}"),
            display_value(value),
        ));
        return Some(number);
    }
    Some(number)
}

fn validate_avatar(issues: &mut Vec<ValidationIssue>, value: Option<&Value>) -> Option<String> {
    let text = string_field(issues, "avatar", value, Some(65535), None, false)?;
    if text.is_empty() {
        return Some(text);
    }
    if let Some((prefix, _)) = text.split_once(',') {
        if !prefix.starts_with("data:") {
            issues.push(issue(
                "avatar",
                "invalid MIME prefix format. Must start with 'data:'",
                display_value(value.unwrap_or(&Value::Null)),
            ));
            return Some(text);
        }
        let mime = prefix[5..].split(';').next().unwrap_or("");
        if mime != "image/jpeg" && mime != "image/png" {
            issues.push(issue(
                "avatar",
                "Unsupported MIME type. Allowed: ['image/jpeg', 'image/png']",
                display_value(value.unwrap_or(&Value::Null)),
            ));
        }
        return Some(text);
    }
    issues.push(issue(
        "avatar",
        "missing MIME prefix. Expected format: data:<mime>;base64,<data>",
        display_value(value.unwrap_or(&Value::Null)),
    ));
    Some(text)
}

fn validate_embedding_model(
    issues: &mut Vec<ValidationIssue>,
    value: Option<&Value>,
) -> Option<String> {
    let Some(value) = value else {
        return None;
    };
    if value.is_null() {
        return None;
    }
    let Some(raw) = value.as_str() else {
        issues.push(issue(
            "embedding_model",
            "Input should be a valid string",
            display_value(value),
        ));
        return None;
    };
    let text = raw.trim();
    if text.is_empty() {
        return Some(String::new());
    }
    if text.len() == 32 && text.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Some(text.to_string());
    }
    if !text.contains('@') {
        issues.push(issue(
            "embedding_model",
            "embedding model identifier must follow <model_name>@<provider> format",
            raw.to_string(),
        ));
        return Some(text.to_string());
    }
    let mut parts = text.splitn(2, '@');
    let model_name = parts.next().unwrap_or("");
    let provider = parts.next().unwrap_or("");
    if parts.next().is_some() || model_name.is_empty() || provider.is_empty() {
        issues.push(issue(
            "embedding_model",
            "both model_name and provider must be non-empty strings",
            raw.to_string(),
        ));
        return Some(text.to_string());
    }
    if model_name.trim().is_empty() || provider.trim().is_empty() {
        issues.push(issue(
            "embedding_model",
            "Model name and provider cannot be whitespace-only strings",
            raw.to_string(),
        ));
    }
    Some(text.to_string())
}

fn validate_parser_config(
    issues: &mut Vec<ValidationIssue>,
    value: Option<&Value>,
) -> Option<Value> {
    let Some(value) = value else {
        return None;
    };
    if value.is_null() {
        return None;
    }
    let Some(object) = value.as_object() else {
        issues.push(issue(
            "parser_config",
            "Input should be a valid dictionary or instance of ParserConfig",
            display_value(value),
        ));
        return None;
    };
    if object.is_empty() {
        return None;
    }
    if object.contains_key("ext") {
        issues.push(issue(
            "parser_config",
            "Value error, parser_config.ext is not supported; send parser configuration fields directly",
            "{}",
        ));
    }
    let int_field = |issues: &mut Vec<ValidationIssue>, name: &str, min: i64, max: i64| {
        if let Some(raw) = object.get(name) {
            let field = format!("parser_config.{name}");
            if raw.is_null() {
                return;
            }
            let Some(number) = raw.as_i64() else {
                issues.push(issue(
                    &field,
                    "Input should be a valid integer",
                    display_value(raw),
                ));
                return;
            };
            if number < min {
                issues.push(issue(
                    &field,
                    format!("Input should be greater than or equal to {min}"),
                    display_value(raw),
                ));
            } else if number > max {
                issues.push(issue(
                    &field,
                    format!("Input should be less than or equal to {max}"),
                    display_value(raw),
                ));
            }
        }
    };
    int_field(issues, "auto_keywords", 0, 32);
    int_field(issues, "auto_questions", 0, 10);
    int_field(issues, "chunk_token_num", 1, 2048);
    int_field(issues, "topn_tags", 1, 10);
    if let Some(raw) = object.get("delimiter") {
        if !raw.is_string() {
            issues.push(issue(
                "parser_config.delimiter",
                "Input should be a valid string",
                display_value(raw),
            ));
        } else if raw.as_str().map(chars_len).unwrap_or(0) < 1 {
            issues.push(issue(
                "parser_config.delimiter",
                "String should have at least 1 character",
                display_value(raw),
            ));
        }
    }
    if let Some(raw) = object.get("html4excel") {
        if !raw.is_boolean() {
            issues.push(issue(
                "parser_config.html4excel",
                "Input should be a valid boolean",
                display_value(raw),
            ));
        }
    }
    if let Some(raw) = object.get("parent_child") {
        if !raw.is_object() {
            issues.push(issue(
                "parser_config.parent_child",
                "Input should be a valid dictionary or instance of ParentChildConfig",
                display_value(raw),
            ));
        }
    }
    if let Some(raw) = object.get("tag_kb_ids") {
        if !raw.is_array() {
            issues.push(issue(
                "parser_config.tag_kb_ids",
                "Input should be a valid list",
                display_value(raw),
            ));
        }
    }
    Some(value.clone())
}

fn unknown_fields(body: &Map<String, Value>, allowed: &[&str], issues: &mut Vec<ValidationIssue>) {
    for (key, value) in body {
        if !allowed.contains(&key.as_str()) {
            issues.push(issue(
                key,
                "Extra inputs are not permitted",
                display_value(value),
            ));
        }
    }
}

/// Validate a create (`update=false`) or update (`update=true`) dataset body.
pub fn validate_dataset_body(
    body: &Map<String, Value>,
    update: bool,
) -> Result<DatasetWrite, Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    let mut allowed: Vec<&str> = CREATE_FIELDS.to_vec();
    if update {
        allowed.extend(UPDATE_ONLY_FIELDS);
    }
    unknown_fields(body, &allowed, &mut issues);

    let fields_set: Vec<String> = body.keys().cloned().collect();
    let name = if update {
        if body.contains_key("name") {
            string_field(
                &mut issues,
                "name",
                body.get("name"),
                Some(DATASET_NAME_LIMIT),
                Some(1),
                true,
            )
        } else {
            None
        }
    } else if body.contains_key("name") {
        string_field(
            &mut issues,
            "name",
            body.get("name"),
            Some(DATASET_NAME_LIMIT),
            Some(1),
            true,
        )
    } else {
        issues.push(issue("name", "Field required", "None"));
        None
    };
    let avatar = validate_avatar(&mut issues, body.get("avatar"));
    let description = string_field(
        &mut issues,
        "description",
        body.get("description"),
        Some(65535),
        None,
        false,
    );
    let embedding_model = validate_embedding_model(&mut issues, body.get("embedding_model"));
    let permission = match body.get("permission") {
        None => "me".to_string(),
        Some(value) if value.is_null() => {
            issues.push(issue(
                "permission",
                "Input should be 'me' or 'team'",
                display_value(value),
            ));
            "me".to_string()
        }
        Some(Value::String(text)) if text == "me" || text == "team" => text.clone(),
        Some(value) => {
            issues.push(issue(
                "permission",
                "Input should be 'me' or 'team'",
                display_value(value),
            ));
            "me".to_string()
        }
    };
    let parse_type = strict_int_field(&mut issues, "parse_type", body.get("parse_type"), 0, 64);
    // `handle_pipeline_id`: parse_type == 1 selects direct-parser mode and drops
    // the pipeline before any other validation sees it.
    let requested_pipeline = if parse_type == Some(1) {
        None
    } else {
        match body.get("pipeline_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) if text.is_empty() => None,
            Some(Value::String(text)) => {
                if chars_len(text) < 32 {
                    issues.push(issue(
                        "pipeline_id",
                        "String should have at least 32 characters",
                        text.clone(),
                    ));
                } else if chars_len(text) > 32 {
                    issues.push(issue(
                        "pipeline_id",
                        "String should have at most 32 characters",
                        text.clone(),
                    ));
                } else if !text.chars().all(|ch| ch.is_ascii_hexdigit()) {
                    issues.push(issue(
                        "pipeline_id",
                        "pipeline_id must be hexadecimal",
                        text.clone(),
                    ));
                }
                Some(text.to_ascii_lowercase())
            }
            Some(value) => {
                issues.push(issue(
                    "pipeline_id",
                    "Input should be a valid string",
                    display_value(value),
                ));
                None
            }
        }
    };
    let mut chunk_method = match body.get("chunk_method") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if ALLOWED_CHUNK_METHODS.contains(&text.as_str()) => {
            Some(text.clone())
        }
        Some(value) => {
            let rendered = display_value(value);
            issues.push(issue("chunk_method", CHUNK_METHOD_LITERAL_ERROR, rendered));
            None
        }
    };
    let language = string_field(
        &mut issues,
        "language",
        body.get("language"),
        Some(32),
        Some(1),
        true,
    );
    let parser_config = validate_parser_config(&mut issues, body.get("parser_config"));
    // Model-level dependency validation runs only when every field passed
    // pydantic validation, like a pydantic v2 model_validator(mode="after").
    if issues.is_empty() {
        let pipeline_mode = requested_pipeline.is_some();
        if !body.contains_key("chunk_method") {
            if parse_type.is_none() && requested_pipeline.is_none() {
                chunk_method = Some("naive".to_string());
            } else if parse_type.is_none() || requested_pipeline.is_none() {
                let mut missing = Vec::new();
                if parse_type.is_none() {
                    missing.push("parse_type");
                }
                if requested_pipeline.is_none() {
                    missing.push("pipeline_id");
                }
                issues.push(issue(
                    "",
                    format!(
                        "parser_id omitted → required fields missing: {}",
                        missing.join(", ")
                    ),
                    python_repr(body),
                ));
            }
        } else {
            match body.get("chunk_method") {
                Some(Value::Null) => {
                    if !pipeline_mode {
                        issues.push(issue("chunk_method", CHUNK_METHOD_LITERAL_ERROR, "None"));
                    }
                }
                Some(Value::String(text)) if ALLOWED_CHUNK_METHODS.contains(&text.as_str()) => {
                    let mut invalid = Vec::new();
                    if !matches!(parse_type, None | Some(1)) {
                        invalid.push("parse_type");
                    }
                    if requested_pipeline.is_some() {
                        invalid.push("pipeline_id");
                    }
                    if !invalid.is_empty() {
                        issues.push(issue(
                            "",
                            format!(
                                "parser_id provided → disallowed fields present: {}",
                                invalid.join(", ")
                            ),
                            python_repr(body),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    let pagerank = if update {
        strict_int_field(&mut issues, "pagerank", body.get("pagerank"), 0, 100).unwrap_or(0)
    } else {
        0
    };
    let connectors = if update {
        match body.get("connectors") {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => {
                for (index, item) in items.iter().enumerate() {
                    if !item.is_object() {
                        issues.push(issue(
                            format!("connectors.{index}"),
                            "Input should be a valid dictionary",
                            display_value(item),
                        ));
                    }
                }
                Some(items.clone())
            }
            Some(value) => {
                issues.push(issue(
                    "connectors",
                    "Input should be a valid list",
                    display_value(value),
                ));
                None
            }
        }
    } else {
        None
    };
    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(DatasetWrite {
        name,
        avatar,
        description,
        embedding_model,
        permission,
        parse_type,
        pipeline_id: requested_pipeline,
        chunk_method,
        language,
        parser_config,
        pagerank,
        connectors,
        fields_set,
    })
}

/// Query parameters of `GET /api/v1/datasets`, after upstream validation.
#[derive(Debug, Clone)]
pub struct DatasetListParams {
    pub id: Option<String>,
    pub ids: Vec<String>,
    pub name: Option<String>,
    pub keywords: String,
    pub owner_ids: Vec<String>,
    pub parser_id: Option<String>,
    pub page: usize,
    pub page_size: usize,
    pub orderby: String,
    pub desc: bool,
    pub include_parsing_status: bool,
    pub filter_only: bool,
}

impl Default for DatasetListParams {
    fn default() -> Self {
        Self {
            id: None,
            ids: Vec::new(),
            name: None,
            keywords: String::new(),
            owner_ids: Vec::new(),
            parser_id: None,
            page: 1,
            page_size: 30,
            orderby: "create_time".to_string(),
            desc: true,
            include_parsing_status: false,
            filter_only: false,
        }
    }
}

fn parse_query_bool(issues: &mut Vec<ValidationIssue>, field: &str, raw: &str) -> bool {
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => true,
        "0" | "false" | "f" | "no" | "n" | "off" => false,
        _ => {
            issues.push(issue(
                field,
                "Input should be a valid boolean, unable to interpret input",
                raw.to_string(),
            ));
            true
        }
    }
}

/// Parse and validate the list query exactly like `ListDatasetReq`.
pub fn parse_dataset_list_query(
    pairs: &[(String, String)],
) -> Result<DatasetListParams, Vec<ValidationIssue>> {
    const KEYS: [&str; 12] = [
        "id",
        "ids",
        "name",
        "page",
        "page_size",
        "orderby",
        "desc",
        "keywords",
        "owner_ids",
        "parser_id",
        "include_parsing_status",
        "type",
    ];
    let mut issues = Vec::new();
    let mut params = DatasetListParams::default();
    let mut values: Vec<(String, String)> = Vec::new();
    for (key, value) in pairs {
        if !KEYS.contains(&key.as_str()) {
            issues.push(issue(key, "Extra inputs are not permitted", value.clone()));
            continue;
        }
        values.push((key.clone(), value.clone()));
    }
    let first = |name: &str| {
        values
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    if let Some(raw) = first("type") {
        if raw == "filter" {
            params.filter_only = true;
        } else {
            issues.push(issue("type", "Extra inputs are not permitted", raw));
        }
    }
    if params.filter_only {
        return if issues.is_empty() {
            Ok(params)
        } else {
            Err(issues)
        };
    }
    if let Some(raw) = first("id") {
        match normalize_uuid(&raw) {
            Some(id) => params.id = Some(id),
            None => issues.push(issue("id", "Invalid UUID format", raw)),
        }
    }
    let mut raw_ids: Vec<String> = Vec::new();
    for (key, value) in &values {
        if key == "ids" || key == "owner_ids" {
            for item in value.split(',') {
                let item = item.trim();
                if !item.is_empty() {
                    if key == "ids" {
                        raw_ids.push(item.to_string());
                    } else {
                        params.owner_ids.push(item.to_string());
                    }
                }
            }
        }
    }
    if raw_ids.len() > REST_API_MAX_IDS {
        issues.push(issue(
            "ids",
            format!("List should have at most {REST_API_MAX_IDS} items"),
            format!("{} items", raw_ids.len()),
        ));
    }
    for raw in &raw_ids {
        match normalize_uuid(raw) {
            Some(id) => params.ids.push(id),
            None => issues.push(issue("ids", "Invalid UUID format", format!("['{raw}']"))),
        }
    }
    params.name = first("name");
    params.keywords = first("keywords").unwrap_or_default();
    params.parser_id = first("parser_id");
    if let Some(raw) = first("page") {
        match raw.trim().parse::<i64>() {
            Ok(page) if page >= 1 => params.page = page as usize,
            Ok(_) => issues.push(issue(
                "page",
                "Input should be greater than or equal to 1",
                raw,
            )),
            Err(_) => issues.push(issue(
                "page",
                "Input should be a valid integer, unable to parse string as an integer",
                raw,
            )),
        }
    }
    if let Some(raw) = first("page_size") {
        match raw.trim().parse::<i64>() {
            Ok(size) if size < 1 => issues.push(issue(
                "page_size",
                "Input should be greater than or equal to 1",
                raw,
            )),
            Ok(size) if size as usize > REST_API_MAX_PAGE_SIZE => issues.push(issue(
                "page_size",
                format!(
                    "Value error, page_size must be less than or equal to {REST_API_MAX_PAGE_SIZE}"
                ),
                raw,
            )),
            Ok(size) => params.page_size = size as usize,
            Err(_) => issues.push(issue(
                "page_size",
                "Input should be a valid integer, unable to parse string as an integer",
                raw,
            )),
        }
    }
    if let Some(raw) = first("orderby") {
        if raw == "create_time" || raw == "update_time" {
            params.orderby = raw;
        } else {
            issues.push(issue(
                "orderby",
                "Input should be 'create_time' or 'update_time'",
                raw,
            ));
        }
    }
    if let Some(raw) = first("desc") {
        params.desc = parse_query_bool(&mut issues, "desc", &raw);
    }
    if let Some(raw) = first("include_parsing_status") {
        params.include_parsing_status =
            parse_query_bool(&mut issues, "include_parsing_status", &raw);
    }
    if issues.is_empty() {
        Ok(params)
    } else {
        Err(issues)
    }
}
