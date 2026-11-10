//! File management — upload, folders, versioning, workspace commits.
//! Replaces RAGFlow's file_api.py, api/apps/services/file_api_service.py and the deprecated
//! /api/v1/file/* aliases in api/apps/backward_compat.py.
//!
//! The rules encoded here were read off the pinned v0.27.2 tree and re-checked against a live
//! instance:
//!
//! * rows have two shapes — File.to_json() for create/upload answers (nine keys, no timestamps)
//!   and the read-back dict for list/parent/ancestors answers (timestamps added);
//! * JSON bodies are pydantic-validated (strict models, extra="forbid") and every failure is
//!   reported as Field: <...> - Message: <...> - Value: <...> with code 101;
//! * service failures keep the upstream wording ("Folder not found!", "Source files not found!",
//!   "The extension of file can't be changed", ...) and use code 102;
//! * deleting reports per-id errors and a partial-success envelope.

use axum::{
    Json,
    extract::{Extension, Multipart, Query, Request, State},
    http::header::CONTENT_TYPE,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use crate::api::dataset_api::{ValidationIssue, format_issues};
use crate::server::{AppState, AuthContext};

/// One record in the file store (RAGFlow's file table, narrowed to what the manager needs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub owner_id: String,
    pub parent_id: String,
    pub size: usize,
    #[serde(default)]
    pub content_hash: String,
    /// RAGFlow File.type: folder, virtual, doc, pdf, visual, aural, other.
    pub file_type: String,
    pub created_at: u64,
    /// Object name inside the tenant's storage bucket (RAGFlow File.location). Stays at the
    /// name the file was uploaded under even after a rename.
    #[serde(default)]
    pub location: String,
    /// RAGFlow File.source_type; empty for local uploads.
    #[serde(default)]
    pub source_type: String,
    /// RAGFlow File.update_time; zero on records written before this field existed.
    #[serde(default)]
    pub updated_at: u64,
    /// Documents this file was converted into (File2Document relations).
    #[serde(default)]
    pub linked_documents: Vec<LinkedDocument>,
}

impl Default for FileRecord {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            owner_id: String::new(),
            parent_id: "root".into(),
            size: 0,
            content_hash: String::new(),
            file_type: "doc".into(),
            created_at: 0,
            location: String::new(),
            source_type: String::new(),
            updated_at: 0,
            linked_documents: Vec::new(),
        }
    }
}

/// One File2Document row: the dataset document a workspace file was converted into.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedDocument {
    pub kb_id: String,
    pub document_id: String,
}

pub struct FileStore {
    files: RwLock<HashMap<String, FileRecord>>,
    pub(crate) data_dir: String,
    metadata_path: String,
    save_lock: Mutex<()>,
}

impl FileStore {
    pub fn new(data_dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let metadata_path = std::path::Path::new(data_dir).join("files.json");
        crate::persistence::restore_if_missing(&metadata_path)?;
        let files = if metadata_path.exists() {
            let data = std::fs::read_to_string(&metadata_path)?;
            serde_json::from_str::<Vec<FileRecord>>(&data).map_err(|error| {
                anyhow::anyhow!(
                    "Failed to parse file metadata '{}': {error}",
                    metadata_path.display()
                )
            })?
        } else {
            Vec::new()
        }
        .into_iter()
        .map(|file| (file.id.clone(), file))
        .collect();
        Ok(Self {
            files: RwLock::new(files),
            data_dir: data_dir.into(),
            metadata_path: metadata_path.to_string_lossy().into_owned(),
            save_lock: Mutex::new(()),
        })
    }

    pub fn list_for(&self, owner_id: &str, is_admin: bool, parent_id: &str) -> Vec<FileRecord> {
        self.files
            .read()
            .unwrap()
            .values()
            .filter(|file| {
                file.parent_id == parent_id
                    && (file.owner_id == owner_id || (is_admin && file.owner_id.is_empty()))
            })
            .cloned()
            .collect()
    }

    /// One record by id, regardless of the folder it sits in.
    pub fn get(&self, id: &str) -> Option<FileRecord> {
        self.files.read().unwrap().get(id).cloned()
    }

    /// Every child of parent_id, regardless of owner (folder size / tree walks).
    pub fn children(&self, parent_id: &str) -> Vec<FileRecord> {
        self.files
            .read()
            .unwrap()
            .values()
            .filter(|file| file.parent_id == parent_id)
            .cloned()
            .collect()
    }

    /// Every record this caller may see, in any folder (the folder tree walks use it).
    pub fn all_for(&self, owner_id: &str, is_admin: bool) -> Vec<FileRecord> {
        self.files
            .read()
            .unwrap()
            .values()
            .filter(|file| file.owner_id == owner_id || (is_admin && file.owner_id.is_empty()))
            .cloned()
            .collect()
    }

    /// Re-parent (and optionally rename) one record, keeping the store's own bookkeeping.
    pub fn move_record(
        &self,
        id: &str,
        parent_id: &str,
        new_name: Option<&str>,
    ) -> anyhow::Result<FileRecord> {
        self.mutate(|files| {
            let Some(file) = files.get_mut(id) else {
                anyhow::bail!("File not found");
            };
            file.parent_id = parent_id.to_string();
            if let Some(name) = new_name.map(str::trim).filter(|name| !name.is_empty()) {
                file.name = name.to_string();
            }
            file.updated_at = now_ms();
            Ok(file.clone())
        })
    }

    /// Replace one record by id (used by move/link after the caller edited the clone).
    pub fn update(&self, record: FileRecord) -> anyhow::Result<()> {
        self.mutate(|files| {
            files.insert(record.id.clone(), record);
            Ok(())
        })
    }

    pub fn add(&self, f: FileRecord) -> anyhow::Result<()> {
        self.mutate(|files| {
            files.insert(f.id.clone(), f);
            Ok(())
        })
    }

    /// Insert a file while reserving a unique name for its owner and parent folder.
    pub fn add_unique(&self, mut file: FileRecord) -> anyhow::Result<FileRecord> {
        self.mutate(|files| {
            if files.contains_key(&file.id) {
                anyhow::bail!("File ID already exists");
            }
            file.name = crate::naming::duplicate_name(&file.name, |candidate| {
                files.values().any(|existing| {
                    existing.owner_id == file.owner_id
                        && existing.parent_id == file.parent_id
                        && existing.name == candidate
                })
            })?;
            files.insert(file.id.clone(), file.clone());
            Ok(file)
        })
    }

    pub fn remove(&self, id: &str) -> anyhow::Result<bool> {
        self.mutate_if_changed(|files| {
            let removed = files.remove(id).is_some();
            Ok((removed, removed))
        })
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, FileRecord>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.mutate_if_changed(|files| mutation(files).map(|value| (value, true)))
    }

    fn mutate_if_changed<T>(
        &self,
        mutation: impl FnOnce(&mut HashMap<String, FileRecord>) -> anyhow::Result<(T, bool)>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().unwrap();
        let mut files = self.files.write().unwrap();
        let previous = files.clone();
        let (value, changed) = mutation(&mut files)?;
        if !changed {
            return Ok(value);
        }
        let snapshot: Vec<FileRecord> = files.values().cloned().collect();
        if let Err(error) = self.persist(&snapshot) {
            *files = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist(&self, files: &[FileRecord]) -> anyhow::Result<()> {
        let data = serde_json::to_vec_pretty(&files)?;
        crate::persistence::atomic_write(std::path::Path::new(&self.metadata_path), &data)
    }
}

// ============================================================================
// Upstream row shapes
// ============================================================================

const KNOWLEDGEBASE_FOLDER: &str = ".knowledgebase";
const SKILLS_FOLDER_NAME: &str = "skills";

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn now_ms_local() -> u64 {
    now_ms()
}

/// Python repr of a JSON value (pydantic's Value: <...> rendering).
fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".to_string(),
        Value::Bool(flag) => if *flag { "True" } else { "False" }.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => format!("'{}'", text.replace('\'', "\\'")),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(py_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => {
            let mut parts = Vec::new();
            for (key, item) in map {
                parts.push(format!("'{}': {}", key, py_repr(item)));
            }
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Python str(value) for the Value: <...> tail of a field error.
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => py_repr(other),
    }
}

fn today(ms: u64) -> String {
    crate::api::compat::ms_to_datetime_string(ms)
}

fn type_of(record: &FileRecord) -> &str {
    if record.file_type.is_empty() {
        "doc"
    } else {
        &record.file_type
    }
}

fn is_folder(record: &FileRecord) -> bool {
    record.file_type == "folder"
}

fn is_virtual(record: &FileRecord) -> bool {
    record.file_type == "virtual"
}

/// File.location: "" for folders/virtual files, the storage object name otherwise.
fn location_of(record: &FileRecord) -> String {
    if !record.location.is_empty() {
        return record.location.clone();
    }
    if is_folder(record) || is_virtual(record) {
        String::new()
    } else {
        record.name.clone()
    }
}

fn tenant_for(auth: &AuthContext, record: &FileRecord) -> String {
    if record.owner_id.is_empty() {
        auth.user_id.clone()
    } else {
        record.owner_id.clone()
    }
}

fn updated_at_of(record: &FileRecord) -> u64 {
    if record.updated_at == 0 {
        record.created_at
    } else {
        record.updated_at
    }
}

/// File.to_json() — the row a create/upload answer carries: raw constructor data, no
/// timestamps, no list-only decorations.
fn to_json_row(record: &FileRecord, tenant: &str) -> Value {
    json!({
        "created_by": tenant,
        "id": record.id,
        "location": location_of(record),
        "name": record.name,
        "parent_id": record.parent_id,
        "size": record.size,
        "source_type": record.source_type,
        "tenant_id": tenant,
        "type": type_of(record),
    })
}

/// A row read back from the table, with the create_date/update_date projections
/// FileService.get_by_pf_id adds. List rows also gain kbs_info (and folders a subtree size
/// plus has_child_folder).
fn full_row(state: &AppState, auth: &AuthContext, record: &FileRecord, list_fields: bool) -> Value {
    let tenant = tenant_for(auth, record);
    let mut row = json!({
        "create_date": today(record.created_at),
        "create_time": record.created_at,
        "created_by": tenant,
        "id": record.id,
        "location": location_of(record),
        "name": record.name,
        "parent_id": record.parent_id,
        "size": record.size,
        "source_type": record.source_type,
        "tenant_id": tenant,
        "type": type_of(record),
        "update_date": today(updated_at_of(record)),
        "update_time": updated_at_of(record),
    });
    if list_fields {
        row["kbs_info"] = json!([]);
        if is_folder(record) {
            row["size"] = json!(folder_size(state, &record.id));
            row["has_child_folder"] = json!(state.files.children(&record.id).iter().any(is_folder));
        }
    }
    row
}

/// FileService.get_folder_size: recursive byte sum of the subtree.
fn folder_size(state: &AppState, folder_id: &str) -> usize {
    let mut total = 0usize;
    let mut seen = std::collections::HashSet::new();
    let mut queue = vec![folder_id.to_string()];
    while let Some(current) = queue.pop() {
        for child in state.files.children(&current) {
            if !seen.insert(child.id.clone()) {
                continue;
            }
            total += child.size;
            if is_folder(&child) {
                queue.push(child.id);
            }
        }
    }
    total
}

/// The tenant root folder (FileService.get_root_folder): a folder whose parent is itself.
fn root_row(state: &AppState, auth: &AuthContext) -> Value {
    let created = state
        .users
        .get_user_by_id(&auth.user_id)
        .map(|user| user.created_at)
        .filter(|value| *value > 0)
        .or_else(|| {
            state
                .files
                .all_for(&auth.user_id, auth.is_admin)
                .iter()
                .map(|file| file.created_at)
                .min()
        })
        .unwrap_or_else(now_ms);
    json!({
        "create_date": today(created),
        "create_time": created,
        "created_by": auth.user_id,
        "id": "root",
        "location": "",
        "name": "/",
        "parent_id": "root",
        "size": 0,
        "source_type": "",
        "tenant_id": auth.user_id,
        "type": "folder",
        "update_date": today(created),
        "update_time": created,
    })
}

// ============================================================================
// Response helpers
// ============================================================================

fn success(data: Value) -> Response {
    Json(json!({ "code": 0, "data": data })).into_response()
}

fn success_with_message(data: Value) -> Response {
    Json(json!({ "code": 0, "message": "success", "data": data })).into_response()
}

fn data_error(message: &str) -> Response {
    Json(json!({ "code": 102, "message": message })).into_response()
}

fn argument_error(message: &str) -> Response {
    Json(json!({ "code": 101, "message": message })).into_response()
}

fn issue_error(field: &str, message: &str, value: String) -> Response {
    let issues = [ValidationIssue::new(field, message, value)];
    argument_error(&format_issues(&issues))
}

fn required_field(field: &str, body: &Value) -> Response {
    issue_error(field, "Field required", py_repr(body))
}

fn model_value_error(message: &str, body: &Value) -> Response {
    issue_error("", &format!("Value error, {message}"), py_repr(body))
}

// ============================================================================
// Request parsing / pydantic-equivalent validation
// ============================================================================

fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) => {
            if number.is_f64() {
                "float"
            } else {
                "int"
            }
        }
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// Read and validate the JSON body the way validate_and_parse_json_request does.
async fn json_object_body(request: Request) -> Result<Value, Response> {
    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = content_type.clone().unwrap_or_else(|| "None".to_string());
    let mimetype = content_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    if mimetype != "application/json" {
        return Err(argument_error(&format!(
            "Unsupported content type: Expected application/json, got {raw}"
        )));
    }
    let bytes = axum::body::to_bytes(request.into_body(), 4 * 1024 * 1024)
        .await
        .map_err(|_| {
            argument_error("Malformed JSON syntax: Missing commas/brackets or invalid encoding")
        })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        argument_error("Malformed JSON syntax: Missing commas/brackets or invalid encoding")
    })?;
    if !value.is_object() {
        return Err(argument_error(&format!(
            "Invalid request payload: expected object, got {}",
            python_type_name(&value)
        )));
    }
    Ok(value)
}

fn forbid_extra(body: &Value, allowed: &[&str]) -> Result<(), Response> {
    let Some(map) = body.as_object() else {
        return Ok(());
    };
    for (key, value) in map {
        if !allowed.contains(&key.as_str()) {
            return Err(issue_error(
                key,
                "Extra inputs are not permitted",
                value_text(value),
            ));
        }
    }
    Ok(())
}

fn strict_string_field(field: &str, value: Option<&Value>) -> Result<Option<String>, Response> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(other) => Err(issue_error(
            field,
            "Input should be a valid string",
            value_text(other),
        )),
    }
}

fn strict_string_list(field: &str, value: Option<&Value>) -> Result<Option<Vec<String>>, Response> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(items) = value.as_array() else {
        return Err(issue_error(
            field,
            "Input should be a valid list",
            value_text(value),
        ));
    };
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        match item {
            Value::String(text) => out.push(text.clone()),
            other => {
                return Err(issue_error(
                    &format!("{field}.{index}"),
                    "Input should be a valid string",
                    value_text(other),
                ));
            }
        }
    }
    Ok(Some(out))
}

/// StringConstraints(strip_whitespace=True, min_length=1, max_length=255).
fn constrained_name(field: &str, raw: &Value) -> Result<String, Response> {
    let text = match raw {
        Value::String(text) => text.clone(),
        other => {
            return Err(issue_error(
                field,
                "Input should be a valid string",
                value_text(other),
            ));
        }
    };
    let stripped = text.trim().to_string();
    if stripped.is_empty() {
        return Err(issue_error(
            field,
            "String should have at least 1 character",
            text,
        ));
    }
    if stripped.chars().count() > 255 {
        return Err(issue_error(
            field,
            "String should have at most 255 characters",
            text,
        ));
    }
    Ok(stripped)
}

struct CreateFolderRequest {
    name: String,
    parent_id: Option<String>,
    file_type: Option<String>,
}

fn validate_create_folder(body: &Value) -> Result<CreateFolderRequest, Response> {
    let name_value = body
        .get("name")
        .ok_or_else(|| required_field("name", body))?;
    let name = constrained_name("name", name_value)?;
    let parent_id = strict_string_field("parent_id", body.get("parent_id"))?;
    let file_type = strict_string_field("type", body.get("type"))?;
    forbid_extra(body, &["name", "parent_id", "type"])?;
    Ok(CreateFolderRequest {
        name,
        parent_id,
        file_type,
    })
}

struct MoveRequest {
    src_file_ids: Vec<String>,
    dest_file_id: Option<String>,
    new_name: Option<String>,
}

fn validate_move(body: &Value) -> Result<MoveRequest, Response> {
    let src_value = body
        .get("src_file_ids")
        .ok_or_else(|| required_field("src_file_ids", body))?;
    let src_file_ids =
        strict_string_list("src_file_ids", Some(src_value))?.expect("present value yields a list");
    if src_file_ids.is_empty() {
        return Err(issue_error(
            "src_file_ids",
            "List should have at least 1 item after validation, not 0",
            py_repr(src_value),
        ));
    }
    let dest_file_id = strict_string_field("dest_file_id", body.get("dest_file_id"))?;
    let new_name = match body.get("new_name") {
        None | Some(Value::Null) => None,
        Some(raw) => Some(constrained_name("new_name", raw)?),
    };
    forbid_extra(body, &["src_file_ids", "dest_file_id", "new_name"])?;
    let dest_present = dest_file_id
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    if !dest_present && new_name.is_none() {
        return Err(model_value_error(
            "At least one of dest_file_id or new_name must be provided",
            body,
        ));
    }
    if new_name.is_some() && src_file_ids.len() > 1 {
        return Err(model_value_error(
            "new_name can only be used with a single file",
            body,
        ));
    }
    Ok(MoveRequest {
        src_file_ids,
        dest_file_id,
        new_name,
    })
}

fn validate_delete(body: &Value) -> Result<Vec<String>, Response> {
    let ids_value = body.get("ids").ok_or_else(|| required_field("ids", body))?;
    let ids = strict_string_list("ids", Some(ids_value))?.expect("present value yields a list");
    if ids.is_empty() {
        return Err(issue_error(
            "ids",
            "List should have at least 1 item after validation, not 0",
            py_repr(ids_value),
        ));
    }
    forbid_extra(body, &["ids"])?;
    Ok(ids)
}

// ============================================================================
// Read handlers
// ============================================================================

/// GET /api/v1/files — the folder listing (file_api.list_files).
pub async fn list_files(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    // ListFileReq: unknown query parameters are rejected (extra="forbid").
    const ALLOWED: [&str; 6] = [
        "parent_id",
        "keywords",
        "page",
        "page_size",
        "orderby",
        "desc",
    ];
    for key in query.keys() {
        if !ALLOWED.contains(&key.as_str()) {
            return issue_error(key, "Extra inputs are not permitted", query[key].clone());
        }
    }
    let parent_value = query.get("parent_id").cloned().unwrap_or_default();
    let parent_id = if parent_value.is_empty() {
        "root".to_string()
    } else {
        parent_value
    };
    let keywords = query
        .get("keywords")
        .cloned()
        .unwrap_or_default()
        .to_lowercase();
    let page = match query.get("page") {
        None => 1i64,
        Some(raw) => match raw.trim().parse::<i64>() {
            Ok(value) if value >= 1 => value,
            Ok(value) => {
                return issue_error(
                    "page",
                    "Input should be greater than or equal to 1",
                    value.to_string(),
                );
            }
            Err(_) => {
                return issue_error(
                    "page",
                    "Input should be a valid integer, unable to parse string as an integer",
                    raw.clone(),
                );
            }
        },
    };
    let page_size = match query.get("page_size") {
        None => 15i64,
        Some(raw) => match raw.trim().parse::<i64>() {
            Ok(value) if value < 1 => {
                return issue_error(
                    "page_size",
                    "Input should be greater than or equal to 1",
                    value.to_string(),
                );
            }
            Ok(value) if value > 100 => {
                return issue_error(
                    "page_size",
                    "Value error, page_size must be less than or equal to 100",
                    value.to_string(),
                );
            }
            Ok(value) => value,
            Err(_) => {
                return issue_error(
                    "page_size",
                    "Input should be a valid integer, unable to parse string as an integer",
                    raw.clone(),
                );
            }
        },
    };
    let orderby = query
        .get("orderby")
        .cloned()
        .unwrap_or_else(|| "create_time".to_string());
    let descending = match query.get("desc") {
        None => true,
        Some(raw) => match parse_bool(raw) {
            Some(value) => value,
            None => {
                return issue_error(
                    "desc",
                    "Input should be a valid boolean, unable to interpret input",
                    raw.clone(),
                );
            }
        },
    };

    let root_listing = parent_id == "root";
    if !root_listing && state.files.get(&parent_id).is_none() {
        return data_error("Folder not found!");
    }

    let mut files: Vec<FileRecord> = if keywords.is_empty() {
        state
            .files
            .list_for(&auth.user_id, auth.is_admin, &parent_id)
            .into_iter()
            .filter(|file| file.id != parent_id)
            .collect()
    } else {
        // Keyword search covers the whole subtree under the folder.
        let mut subtree = vec![parent_id.clone()];
        let mut seen: std::collections::HashSet<String> = subtree.iter().cloned().collect();
        let mut cursor = 0usize;
        while cursor < subtree.len() {
            let current = subtree[cursor].clone();
            cursor += 1;
            for child in state.files.list_for(&auth.user_id, auth.is_admin, &current) {
                if seen.insert(child.id.clone()) {
                    subtree.push(child.id.clone());
                }
            }
        }
        subtree
            .into_iter()
            .filter(|id| id != &parent_id)
            .filter_map(|id| state.files.get(&id))
            .filter(|file| {
                file.owner_id == auth.user_id || (auth.is_admin && file.owner_id.is_empty())
            })
            .filter(|file| file.name.to_lowercase().contains(&keywords))
            .collect()
    };
    if root_listing {
        // exclude_skills in FileService.get_by_pf_id.
        files.retain(|file| file.name != SKILLS_FOLDER_NAME);
        files.retain(|file| file.name != KNOWLEDGEBASE_FOLDER);
    }
    if !sort_files(&mut files, &orderby, descending) {
        return data_error("Internal server error");
    }
    let total = files.len();
    let offset = (page - 1).saturating_mul(page_size) as usize;
    let rows: Vec<Value> = files
        .into_iter()
        .skip(offset)
        .take(page_size as usize)
        .map(|record| full_row(&state, &auth, &record, true))
        .collect();

    let parent_folder = if root_listing {
        root_row(&state, &auth)
    } else {
        let Some(parent) = state.files.get(&parent_id) else {
            return data_error("Folder not found!");
        };
        let parent_parent = parent.parent_id.clone();
        if parent_parent.is_empty() || parent_parent == "root" {
            root_row(&state, &auth)
        } else if let Some(row_record) = state.files.get(&parent_parent) {
            full_row(&state, &auth, &row_record, false)
        } else {
            return data_error("Internal server error");
        }
    };
    success(json!({ "files": rows, "parent_folder": parent_folder, "total": total }))
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.to_ascii_lowercase().as_str() {
        "1" | "on" | "t" | "true" | "y" | "yes" => Some(true),
        "0" | "off" | "f" | "false" | "n" | "no" => Some(false),
        _ => None,
    }
}

/// Sort by one of the file columns; false when the column does not exist (upstream raises).
fn sort_files(files: &mut [FileRecord], orderby: &str, descending: bool) -> bool {
    match orderby {
        "create_time" => files.sort_by(|left, right| {
            ordered(left.created_at.cmp(&right.created_at), descending)
                .then_with(|| left.id.cmp(&right.id))
        }),
        "update_time" => files.sort_by(|left, right| {
            ordered(updated_at_of(left).cmp(&updated_at_of(right)), descending)
                .then_with(|| left.id.cmp(&right.id))
        }),
        "size" => files.sort_by(|left, right| {
            ordered(left.size.cmp(&right.size), descending).then_with(|| left.id.cmp(&right.id))
        }),
        "name" | "location" | "type" | "source_type" | "id" | "parent_id" | "tenant_id"
        | "created_by" => files.sort_by(|left, right| {
            let left_key = match orderby {
                "name" => left.name.clone(),
                "location" => location_of(left),
                "type" => type_of(left).to_string(),
                "source_type" => left.source_type.clone(),
                "id" => left.id.clone(),
                "parent_id" => left.parent_id.clone(),
                "tenant_id" | "created_by" => left.owner_id.clone(),
                _ => unreachable!(),
            };
            let right_key = match orderby {
                "name" => right.name.clone(),
                "location" => location_of(right),
                "type" => type_of(right).to_string(),
                "source_type" => right.source_type.clone(),
                "id" => right.id.clone(),
                "parent_id" => right.parent_id.clone(),
                "tenant_id" | "created_by" => right.owner_id.clone(),
                _ => unreachable!(),
            };
            ordered(left_key.cmp(&right_key), descending).then_with(|| left.id.cmp(&right.id))
        }),
        _ => return false,
    }
    true
}

fn ordered(ordering: std::cmp::Ordering, descending: bool) -> std::cmp::Ordering {
    if descending {
        ordering.reverse()
    } else {
        ordering
    }
}

/// GET /api/v1/files/{id}/parent — file_api_service.get_parent_folder.
pub async fn parent_folder(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(file_id): axum::extract::Path<String>,
) -> Response {
    let Some(record) = state.files.get(&file_id) else {
        return data_error("Folder not found!");
    };
    if record.owner_id != auth.user_id && !auth.is_admin {
        return data_error("no authorization");
    }
    let parent = if record.parent_id.is_empty() || record.parent_id == "root" {
        root_row(&state, &auth)
    } else {
        match state.files.get(&record.parent_id) {
            Some(parent) => full_row(&state, &auth, &parent, false),
            None => return data_error("Internal server error"),
        }
    };
    success(json!({ "parent_folder": parent }))
}

/// GET /api/v1/files/{id}/ancestors — get_all_parent_folders (self first, root last).
pub async fn file_ancestors(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(file_id): axum::extract::Path<String>,
) -> Response {
    let Some(record) = state.files.get(&file_id) else {
        return data_error("Folder not found!");
    };
    if record.owner_id != auth.user_id && !auth.is_admin {
        return data_error("no authorization");
    }
    let mut chain = Vec::new();
    let mut current = record;
    let mut guard = 0usize;
    loop {
        chain.push(full_row(&state, &auth, &current, false));
        if current.parent_id.is_empty() || current.parent_id == "root" {
            chain.push(root_row(&state, &auth));
            break;
        }
        match state.files.get(&current.parent_id) {
            Some(parent) => current = parent,
            None => return data_error("Internal server error"),
        }
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    success(json!({ "parent_folders": chain }))
}

/// Serve one stored file blob (file_api.download).
pub(crate) async fn serve_file_bytes(
    state: &AppState,
    auth: &AuthContext,
    file_id: &str,
) -> Response {
    let Some(record) = state.files.get(file_id) else {
        return data_error("document not found");
    };
    if record.owner_id != auth.user_id && !auth.is_admin {
        return data_error("no authorization");
    }
    let location = location_of(&record);
    if is_folder(&record) || is_virtual(&record) || location.is_empty() {
        // The upstream storage lookup receives an empty object name and raises; the API
        // answers its generic data error.
        return data_error("Internal server error");
    }
    let physical =
        crate::server::file_physical_path(&state.files.data_dir, &record.id, &record.name);
    if record.size == 0 || !physical.exists() {
        return data_error("This file is empty.");
    }
    let mime =
        crate::parser::mime_from_extension(&record.name).unwrap_or("application/octet-stream");
    crate::api::common::stream_stored_file(&physical, mime, "inline", "Internal server error").await
}

/// GET /api/v1/files/{file_id} and the deprecated GET /api/v1/file/get/{file_id}.
pub async fn get_file(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(file_id): axum::extract::Path<String>,
) -> Response {
    serve_file_bytes(&state, &auth, &file_id).await
}

/// GET /api/v1/files/{file_id}/download — RayRAG preview helper.
pub async fn download_file(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(file_id): axum::extract::Path<String>,
) -> Response {
    serve_file_bytes(&state, &auth, &file_id).await
}

/// DELETE /api/v1/files/{file_id} — RayRAG single-file extension.
pub async fn delete_file(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::extract::Path(file_id): axum::extract::Path<String>,
) -> Response {
    let mut errors = Vec::new();
    let count = match state.files.get(&file_id) {
        Some(record) if record.owner_id == auth.user_id || auth.is_admin => {
            delete_record_tree(&state, &auth, &record, &mut errors)
        }
        _ => 0,
    };
    if count == 0 && errors.is_empty() {
        return data_error("File not found");
    }
    if errors.is_empty() {
        return success(json!({ "success_count": count }));
    }
    Json(json!({
        "code": 102,
        "message": format!("Partially deleted {count} files with {} errors", errors.len()),
        "data": { "success_count": count, "errors": errors },
    }))
    .into_response()
}

// ============================================================================
// Write handlers
// ============================================================================

/// POST /api/v1/files — multipart upload or JSON folder creation.
pub async fn create_or_upload(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    request: Request,
) -> Response {
    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    if content_type
        .to_ascii_lowercase()
        .contains("multipart/form-data")
    {
        use axum::extract::FromRequest as _;
        return match Multipart::from_request(request, &()).await {
            Ok(multipart) => upload_files(State(state), Extension(auth), multipart).await,
            Err(_) => data_error("Internal server error"),
        };
    }
    let body = match json_object_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    create_folder_from_body(&state, &auth, &body)
}

/// RayRAG's own POST /api/v1/files/folder (JSON body).
pub async fn create_folder(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    create_folder_from_body(&state, &auth, &body)
}

fn create_folder_from_body(state: &AppState, auth: &AuthContext, body: &Value) -> Response {
    let request = match validate_create_folder(body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    if request.name.contains('/') {
        return data_error("Folder name cannot contain \"/\"");
    }
    let parent_id = request
        .parent_id
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "root".to_string());
    if parent_id != "root" && state.files.get(&parent_id).is_none() {
        return data_error("Parent Folder Doesn't Exist!");
    }
    if state
        .files
        .children(&parent_id)
        .iter()
        .any(|file| file.name == request.name)
    {
        return data_error("Duplicated folder name in the same folder.");
    }
    let folder = request
        .file_type
        .as_deref()
        .map(str::to_ascii_lowercase)
        .as_deref()
        == Some("folder");
    let record = FileRecord {
        id: uuid::Uuid::new_v4().to_string(),
        name: request.name,
        owner_id: auth.user_id.clone(),
        parent_id,
        size: 0,
        content_hash: String::new(),
        file_type: if folder { "folder" } else { "virtual" }.into(),
        created_at: now_ms(),
        location: String::new(),
        source_type: String::new(),
        updated_at: now_ms(),
        linked_documents: Vec::new(),
    };
    match state.files.add(record.clone()) {
        Ok(()) => success(to_json_row(&record, &auth.user_id)),
        Err(error) => {
            tracing::error!(%error, "Failed to create folder");
            data_error("Internal server error")
        }
    }
}

/// Uploaded file type (filename_type from api/utils/file_utils.py).
fn filename_type(name: &str) -> &'static str {
    let lower = name.trim().to_ascii_lowercase();
    let Some((_, extension)) = lower.rsplit_once('.') else {
        return "other";
    };
    match extension {
        "pdf" => "pdf",
        "msg" | "eml" | "doc" | "docx" | "ppt" | "pptx" | "yml" | "xml" | "htm" | "json"
        | "jsonl" | "ldjson" | "csv" | "txt" | "ini" | "xls" | "xlsx" | "wps" | "rtf" | "hlp"
        | "pages" | "numbers" | "key" | "md" | "mdx" | "py" | "js" | "java" | "c" | "cpp" | "h"
        | "php" | "go" | "ts" | "sh" | "cs" | "kt" | "html" | "sql" | "epub" => "doc",
        "wav" | "flac" | "ape" | "alac" | "wavpack" | "wv" | "mp3" | "aac" | "ogg" | "vorbis"
        | "opus" => "aural",
        "jpg" | "jpeg" | "png" | "tif" | "gif" | "bmp" | "pcx" | "tga" | "exif" | "fpx" | "svg"
        | "psd" | "cdr" | "pcd" | "dxf" | "ufo" | "eps" | "ai" | "raw" | "wmf" | "webp"
        | "avif" | "apng" | "icon" | "ico" | "mpg" | "mpeg" | "avi" | "rm" | "rmvb" | "mov"
        | "wmv" | "asf" | "dat" | "asx" | "wvx" | "mpe" | "mpa" | "mp4" | "mkv" => "visual",
        _ => "other",
    }
}

/// Split a multipart filename into the folder path upstream creates for it.
fn upload_path_segments(filename: &str) -> Vec<String> {
    filename
        .split('/')
        .map(str::trim)
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .map(str::to_string)
        .collect()
}

/// POST /api/v1/files multipart branch (file_api_service.upload_file).
pub async fn upload_files(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    mut multipart: Multipart,
) -> Response {
    let mut parent_id: Option<String> = None;
    let mut uploads: Vec<(String, Vec<u8>)> = Vec::new();
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(_) => return data_error("Internal server error"),
        };
        let name = field.name().unwrap_or("").to_string();
        let mut field = field;
        match name.as_str() {
            "file" => {
                let filename = field.file_name().unwrap_or("").to_string();
                if filename.is_empty() {
                    return argument_error("No file part!");
                }
                let mut bytes = Vec::new();
                loop {
                    match field.chunk().await {
                        Ok(Some(chunk)) => {
                            if bytes.len().saturating_add(chunk.len()) > state.max_upload_bytes {
                                return data_error(
                                    "Uploaded file exceeds the configured size limit",
                                );
                            }
                            bytes.extend_from_slice(&chunk);
                        }
                        Ok(None) => break,
                        Err(_) => return data_error("Internal server error"),
                    }
                }
                uploads.push((filename, bytes));
            }
            "parent_id" => {
                parent_id = Some(field.text().await.unwrap_or_default());
            }
            _ => {
                let _ = field.bytes().await;
            }
        }
    }
    if uploads.is_empty() {
        return argument_error("No file part!");
    }
    let base_parent = parent_id
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "root".to_string());
    if base_parent != "root" && state.files.get(&base_parent).is_none() {
        return data_error("Can't find this folder!");
    }
    let mut rows = Vec::new();
    for (filename, bytes) in uploads {
        let segments = upload_path_segments(&filename);
        let Some((leaf, parents)) = segments.split_last() else {
            return argument_error("No file part!");
        };
        let mut current_parent = base_parent.clone();
        for segment in parents {
            let existing = state
                .files
                .children(&current_parent)
                .into_iter()
                .find(|file| file.name == *segment);
            current_parent = match existing {
                Some(folder) => folder.id,
                None => {
                    let folder = FileRecord {
                        id: uuid::Uuid::new_v4().to_string(),
                        name: segment.clone(),
                        owner_id: auth.user_id.clone(),
                        parent_id: current_parent.clone(),
                        size: 0,
                        content_hash: String::new(),
                        file_type: "folder".into(),
                        created_at: now_ms(),
                        location: String::new(),
                        source_type: String::new(),
                        updated_at: now_ms(),
                        linked_documents: Vec::new(),
                    };
                    if let Err(error) = state.files.add(folder.clone()) {
                        tracing::error!(%error, "Failed to create upload folder");
                        return data_error("Internal server error");
                    }
                    folder.id
                }
            };
        }
        let mut location = leaf.clone();
        let siblings = state.files.children(&current_parent);
        while siblings
            .iter()
            .any(|file| file.id != current_parent && location_of(file) == location)
        {
            location.push('_');
        }
        let id = uuid::Uuid::new_v4().to_string();
        let physical = crate::server::file_physical_path(&state.files.data_dir, &id, leaf);
        if let Some(directory) = physical.parent()
            && let Err(error) = std::fs::create_dir_all(directory)
        {
            tracing::error!(%error, "Failed to create the upload directory");
            return data_error("Internal server error");
        }
        if let Err(error) = std::fs::write(&physical, &bytes) {
            tracing::error!(%error, "Failed to store the uploaded file");
            return data_error("Internal server error");
        }
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        hasher.update(&bytes);
        let record = FileRecord {
            id,
            name: leaf.clone(),
            owner_id: auth.user_id.clone(),
            parent_id: current_parent,
            size: bytes.len(),
            content_hash: format!("{:032x}", hasher.digest128()),
            file_type: filename_type(leaf).into(),
            created_at: now_ms(),
            location,
            source_type: String::new(),
            updated_at: now_ms(),
            linked_documents: Vec::new(),
        };
        match state.files.add_unique(record) {
            Ok(record) => rows.push(to_json_row(&record, &auth.user_id)),
            Err(error) => {
                tracing::error!(%error, "Failed to store uploaded file");
                return data_error("Internal server error");
            }
        }
    }
    success(Value::Array(rows))
}

/// POST /api/v1/files/upload (single-file convenience route).
pub async fn upload_file(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    multipart: Multipart,
) -> Response {
    upload_files(State(state), Extension(auth), multipart).await
}

/// POST /api/v1/files/move — move and/or rename (file_api_service.move_files).
pub async fn move_files(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    request: Request,
) -> Response {
    let body = match json_object_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let request = match validate_move(&body) {
        Ok(request) => request,
        Err(response) => return response,
    };
    let dest = request.dest_file_id.filter(|value| !value.is_empty());
    match move_selection(
        &state,
        &auth,
        &request.src_file_ids,
        dest.as_deref(),
        request.new_name.as_deref(),
    ) {
        Ok(()) => success(json!(true)),
        Err(message) => data_error(&message),
    }
}

fn move_selection(
    state: &AppState,
    auth: &AuthContext,
    src_file_ids: &[String],
    dest_file_id: Option<&str>,
    new_name: Option<&str>,
) -> Result<(), String> {
    let mut records = Vec::new();
    for id in src_file_ids {
        if let Some(record) = state.files.get(id) {
            records.push(record);
        }
    }
    if records.is_empty() {
        return Err("Source files not found!".to_string());
    }
    for id in src_file_ids {
        let Some(record) = records.iter().find(|record| &record.id == id) else {
            return Err("File or folder not found!".to_string());
        };
        if record.owner_id.is_empty() {
            return Err("Tenant not found!".to_string());
        }
        if record.owner_id != auth.user_id && !auth.is_admin {
            return Err("no authorization".to_string());
        }
    }
    // "root" is the tenant root folder; upstream stores it as a regular row, RayRAG keeps a
    // sentinel id, so the lookup has to accept it explicitly.
    let dest = match dest_file_id {
        None => None,
        Some("root") => Some(FileRecord {
            id: "root".to_string(),
            name: "/".to_string(),
            parent_id: "root".to_string(),
            file_type: "folder".to_string(),
            ..Default::default()
        }),
        Some(id) => match state.files.get(id) {
            Some(record) => Some(record),
            None => return Err("Parent folder not found!".to_string()),
        },
    };
    if let Some(name) = new_name {
        let first = &records[0];
        if name.contains('/') {
            return Err("Name cannot contain \"/\"".to_string());
        }
        if !is_folder(first)
            && crate::api::document::python_suffix(name)
                != crate::api::document::python_suffix(&first.name)
        {
            return Err("The extension of file can't be changed".to_string());
        }
        let target_parent = dest
            .as_ref()
            .map(|folder| folder.id.clone())
            .unwrap_or_else(|| first.parent_id.clone());
        if state
            .files
            .children(&target_parent)
            .iter()
            .any(|file| file.name == *name)
        {
            return Err("Duplicated file name in the same folder.".to_string());
        }
    }
    if let Some(folder) = dest.as_ref() {
        for record in &records {
            if is_folder(record) && record.id == folder.id {
                return Err("Cannot move a folder to itself.".to_string());
            }
        }
        let dest_ancestors = ancestor_ids(state, &folder.id);
        for record in &records {
            if is_folder(record) && dest_ancestors.contains(&record.id) {
                return Err("Cannot move a folder into its own subfolder.".to_string());
            }
        }
    }
    for record in &records {
        match dest.as_ref() {
            Some(folder) => {
                move_entry(state, record, &folder.id, new_name)
                    .map_err(|error| error.to_string())?;
            }
            None => {
                // Pure rename: no storage operation.
                let mut updated = record.clone();
                if let Some(name) = new_name {
                    updated.name = name.to_string();
                    updated.updated_at = now_ms();
                    rename_linked_documents(state, &updated);
                }
                state
                    .files
                    .update(updated)
                    .map_err(|error| error.to_string())?;
            }
        }
    }
    Ok(())
}

/// Walk the destination's ancestor chain (self included), like get_all_parent_folders.
fn ancestor_ids(state: &AppState, start: &str) -> Vec<String> {
    let mut ids = Vec::new();
    let mut current = start.to_string();
    let mut guard = 0usize;
    loop {
        ids.push(current.clone());
        let Some(record) = state.files.get(&current) else {
            break;
        };
        if record.parent_id.is_empty() || record.parent_id == current {
            break;
        }
        current = record.parent_id;
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    ids
}

fn rename_linked_documents(state: &AppState, record: &FileRecord) {
    for link in &record.linked_documents {
        if let Err(error) = state.docs.rename(&link.document_id, &record.name) {
            tracing::warn!(%error, doc_id = %link.document_id, "Failed to rename the linked document");
        }
    }
}

/// _move_entry_recursive: re-parent one entry, merging a folder into a same-named sibling.
fn move_entry(
    state: &AppState,
    source: &FileRecord,
    dest_folder_id: &str,
    override_name: Option<&str>,
) -> anyhow::Result<()> {
    if source.id == dest_folder_id {
        anyhow::bail!("The folder is already in the target location. There is no need to move it.");
    }
    let effective_name = override_name.unwrap_or(&source.name).to_string();
    if is_folder(source) {
        let existing = state
            .files
            .children(dest_folder_id)
            .into_iter()
            .find(|file| file.name == effective_name);
        let target = match existing {
            Some(folder) => {
                if folder.id == source.id {
                    anyhow::bail!(
                        "The folder is already in the target location. There is no need to move it."
                    );
                }
                folder
            }
            None => {
                let folder = FileRecord {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: effective_name.clone(),
                    owner_id: source.owner_id.clone(),
                    parent_id: dest_folder_id.to_string(),
                    size: 0,
                    content_hash: String::new(),
                    file_type: "folder".into(),
                    created_at: now_ms(),
                    location: String::new(),
                    source_type: source.source_type.clone(),
                    updated_at: now_ms(),
                    linked_documents: Vec::new(),
                };
                state.files.add(folder.clone())?;
                folder
            }
        };
        for child in state.files.children(&source.id) {
            move_entry(state, &child, &target.id, None)?;
        }
        state.files.remove(&source.id)?;
        return Ok(());
    }
    let mut updated = source.clone();
    if dest_folder_id != source.parent_id {
        updated.parent_id = dest_folder_id.to_string();
        let mut location = effective_name.clone();
        while state
            .files
            .children(dest_folder_id)
            .iter()
            .any(|file| file.id != source.id && location_of(file) == location)
        {
            location.push('_');
        }
        updated.location = location;
    }
    if override_name.is_some() {
        updated.name = effective_name;
        rename_linked_documents(state, &updated);
    }
    updated.updated_at = now_ms();
    state.files.update(updated)
}

/// DELETE /api/v1/files — bulk delete by ids (file_api_service.delete_files).
pub async fn delete_files(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    request: Request,
) -> Response {
    let body = match json_object_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let ids = match validate_delete(&body) {
        Ok(ids) => ids,
        Err(response) => return response,
    };
    let mut errors: Vec<String> = Vec::new();
    let mut success_count = 0usize;
    for id in &ids {
        let Some(record) = state.files.get(id) else {
            errors.push(format!("File or Folder not found: {id}"));
            continue;
        };
        if record.owner_id.is_empty() {
            errors.push(format!("Tenant not found for file {id}"));
            continue;
        }
        if record.source_type == "knowledgebase" || record.source_type == "skill_space" {
            continue;
        }
        if record.owner_id != auth.user_id && !auth.is_admin {
            errors.push(format!("No authorization for file {id}"));
            continue;
        }
        success_count += delete_record_tree(&state, &auth, &record, &mut errors);
    }
    if errors.is_empty() {
        return success(json!({ "success_count": success_count }));
    }
    let message = if success_count > 0 {
        format!(
            "Partially deleted {success_count} files with {} errors",
            errors.len()
        )
    } else {
        format!("Deleted files failed with {} errors", errors.len())
    };
    Json(json!({
        "code": 102,
        "message": message,
        "data": { "success_count": success_count, "errors": errors },
    }))
    .into_response()
}

/// Delete one record (recursing into folders) and count the records removed.
fn delete_record_tree(
    state: &std::sync::Arc<AppState>,
    auth: &AuthContext,
    record: &FileRecord,
    errors: &mut Vec<String>,
) -> usize {
    let mut deleted = 0usize;
    if is_folder(record) {
        for child in state.files.children(&record.id) {
            if child.owner_id == auth.user_id || auth.is_admin {
                deleted += delete_record_tree(state, auth, &child, errors);
            }
        }
    } else {
        let location = location_of(record);
        if !location.is_empty() {
            let physical =
                crate::server::file_physical_path(&state.files.data_dir, &record.id, &record.name);
            if physical.exists()
                && let Err(error) = std::fs::remove_file(&physical)
            {
                errors.push(format!(
                    "Failed to remove object {}/{}: {error}",
                    record.parent_id, location
                ));
            }
        }
    }
    for link in &record.linked_documents {
        if let Some(doc) = state.docs.get(&link.document_id)
            && let Err(error) = crate::server::delete_document_data(state, &doc)
        {
            errors.push(format!(
                "Failed to remove document {} for file {}: {error}",
                link.document_id, record.id
            ));
        }
    }
    match state.files.remove(&record.id) {
        Ok(true) => deleted + 1,
        Ok(false) => deleted,
        Err(error) => {
            errors.push(format!(
                "Failed to delete file record {}: {error}",
                record.id
            ));
            deleted
        }
    }
}

// ============================================================================
// Deprecated /api/v1/file/* aliases (backward_compat.py)
// ============================================================================

fn file_id_query(query: &HashMap<String, String>) -> Result<String, Response> {
    match query.get("file_id") {
        Some(id) if !id.is_empty() => Ok(id.clone()),
        _ => Err(data_error("`file_id` query parameter is required")),
    }
}

/// GET /api/v1/file/parent_folder?file_id=...
pub async fn parent_folder_by_query(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    match file_id_query(&query) {
        Ok(file_id) => {
            parent_folder(State(state), Extension(auth), axum::extract::Path(file_id)).await
        }
        Err(response) => response,
    }
}

/// GET /api/v1/file/all_parent_folder?file_id=...
pub async fn file_ancestors_by_query(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    match file_id_query(&query) {
        Ok(file_id) => {
            file_ancestors(State(state), Extension(auth), axum::extract::Path(file_id)).await
        }
        Err(response) => response,
    }
}

/// POST /api/v1/file/rename — the old file_id/name body, translated to a move.
pub async fn legacy_file_rename(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    request: Request,
) -> Response {
    let body = match json_object_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let src_file_ids = match body.get("file_id") {
        Some(Value::String(id)) => vec![id.clone()],
        Some(Value::Null) | None => vec!["None".to_string()],
        Some(other) => vec![value_text(other)],
    };
    let new_name = match body.get("name") {
        Some(Value::String(name)) => Some(name.clone()),
        Some(Value::Null) | None => None,
        Some(_) => {
            return data_error("Internal server error");
        }
    };
    match move_selection(&state, &auth, &src_file_ids, None, new_name.as_deref()) {
        Ok(()) => success_with_message(json!(true)),
        Err(message) => data_error(&message),
    }
}

// ============================================================================
// DocFileCache — RAGFlow `rag/svr/cache_file_svr.py` port.
//
// RAGFlow runs a dedicated worker that polls the *ongoing* document tasks
// (`TaskService.get_ongoing_doc_name()`), fetches each `(kb_id, location)`
// file binary from object storage and caches it in Redis with a short TTL
// (`12 * 60` seconds). The cache is populated *before* the parser needs the
// file, so chunking workers read from the warm cache instead of hitting
// object storage directly.
//
// This port keeps the same contract against RayRAG's local uploads dir:
//   - key      = `"{kb_id}/{location}"` (same key layout as RAGFlow Redis);
//   - `cache_ongoing`   = `cache_file_svr.main()` poll step;
//   - `get`             = Redis `exist` + read (expired entries miss);
//   - `sweep_expired`   = Redis TTL eviction (expired entries + files).
//
// Like Redis, the cache is volatile: `DocFileCache::new` starts empty and
// drops stale cache files from a previous run.
// ============================================================================

/// One cached file entry (`cache_file_svr` Redis value + TTL bookkeeping).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// `"{kb_id}/{location}"` — identical key layout to the RAGFlow worker.
    pub key: String,
    /// Physical file name inside the cache directory.
    pub file_name: String,
    pub size: usize,
    /// Epoch millis when the entry was cached.
    pub cached_at: u64,
    /// Epoch millis after which the entry is considered expired (Redis TTL).
    pub expires_at: u64,
}

/// Volatile on-disk cache for ongoing-task files (`cache_file_svr` port).
pub struct DocFileCache {
    entries: RwLock<HashMap<String, CacheEntry>>,
    cache_dir: String,
}

impl DocFileCache {
    /// Create an empty cache rooted at `cache_dir`. Stale files from a
    /// previous run are removed (the cache is volatile, like Redis).
    pub fn new(cache_dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(cache_dir)?;
        for entry in std::fs::read_dir(cache_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.file_name().to_string_lossy().starts_with("cache-")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        Ok(Self {
            entries: RwLock::new(HashMap::new()),
            cache_dir: cache_dir.into(),
        })
    }

    /// `"{kb_id}/{location}"` cache key (RAGFlow `"{}/{}".format(kb_id, loc)`).
    pub fn key(kb_id: &str, location: &str) -> String {
        format!("{kb_id}/{location}")
    }

    /// Path of the cached file if a non-expired entry exists.
    pub fn get(&self, kb_id: &str, location: &str, now: u64) -> Option<std::path::PathBuf> {
        let key = Self::key(kb_id, location);
        let entries = self.entries.read().unwrap();
        let entry = entries.get(&key)?;
        if entry.expires_at <= now {
            return None; // Redis `exist` after TTL eviction == false
        }
        let path = std::path::Path::new(&self.cache_dir).join(&entry.file_name);
        path.is_file().then_some(path)
    }

    /// `cache_file_svr.main()` poll step: for each `(kb_id, location)` of an
    /// ongoing task, cache the source file (read from `source_dir`) unless a
    /// valid cache entry already exists. Returns the number of newly cached
    /// files (the RAGFlow `CACHE: <loc>` log lines). Missing source files are
    /// skipped, mirroring the `STORAGE_IMPL.get` error path.
    pub fn cache_ongoing(
        &self,
        locations: &[(String, String)],
        source_dir: &std::path::Path,
        ttl: std::time::Duration,
    ) -> anyhow::Result<usize> {
        let now = now_ms();
        let mut cached = 0usize;
        for (kb_id, location) in locations {
            let key = Self::key(kb_id, location);
            {
                let entries = self.entries.read().unwrap();
                if entries
                    .get(&key)
                    .is_some_and(|entry| entry.expires_at > now)
                {
                    continue;
                }
            }
            let Ok(data) = std::fs::read(source_dir.join(location)) else {
                continue; // RAGFlow logs the storage error and keeps polling
            };
            let file_name = format!("cache-{}.bin", uuid::Uuid::new_v4());
            let path = std::path::Path::new(&self.cache_dir).join(&file_name);
            std::fs::write(&path, &data)?;
            let entry = CacheEntry {
                key: key.clone(),
                file_name,
                size: data.len(),
                cached_at: now,
                expires_at: now.saturating_add(ttl.as_millis() as u64),
            };
            self.entries.write().unwrap().insert(key, entry);
            cached += 1;
        }
        Ok(cached)
    }

    /// Evict expired entries and delete their physical cache files
    /// (Redis TTL eviction). Returns the number of evicted entries.
    pub fn sweep_expired(&self, now: u64) -> usize {
        let expired: Vec<CacheEntry> = self
            .entries
            .read()
            .unwrap()
            .values()
            .filter(|entry| entry.expires_at <= now)
            .cloned()
            .collect();
        if expired.is_empty() {
            return 0;
        }
        let mut entries = self.entries.write().unwrap();
        for entry in &expired {
            entries.remove(&entry.key);
            let _ =
                std::fs::remove_file(std::path::Path::new(&self.cache_dir).join(&entry.file_name));
        }
        expired.len()
    }

    /// Number of entries currently held (expired entries are counted until
    /// swept, matching Redis semantics where keys disappear on access).
    pub fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str) -> FileRecord {
        FileRecord {
            id: id.into(),
            name: format!("{id}.txt"),
            owner_id: "owner-1".into(),
            parent_id: "root".into(),
            size: 1,
            content_hash: String::new(),
            file_type: "txt".into(),
            created_at: 1,
            ..Default::default()
        }
    }

    #[test]
    fn failed_persistence_rolls_back_file_mutations() {
        let root =
            std::env::temp_dir().join(format!("rayrag-file-rollback-{}", uuid::Uuid::new_v4()));
        let store = FileStore::new(root.to_str().unwrap()).unwrap();
        store.add(record("file-1")).unwrap();
        let metadata_path = root.join("files.json");
        std::fs::remove_file(&metadata_path).unwrap();
        std::fs::create_dir(&metadata_path).unwrap();

        assert!(store.add(record("file-2")).is_err());
        assert_eq!(store.list_for("owner-1", false, "root").len(), 1);
        assert!(store.remove("file-1").is_err());
        assert_eq!(store.list_for("owner-1", false, "root").len(), 1);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn corrupt_file_json_fails_startup_instead_of_clearing_store() {
        let root =
            std::env::temp_dir().join(format!("rayrag-file-corrupt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("files.json"), b"{not-json").unwrap();
        let error = match FileStore::new(root.to_str().unwrap()) {
            Err(error) => error,
            Ok(_) => panic!("corrupt file JSON must fail startup"),
        };
        assert!(error.to_string().contains("Failed to parse file metadata"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn doc_file_cache_caches_ongoing_files_with_ttl() {
        let root = std::env::temp_dir().join(format!("rayrag-filecache-{}", uuid::Uuid::new_v4()));
        let cache = DocFileCache::new(root.join("cache").to_str().unwrap()).unwrap();
        let source_dir = root.join("uploads");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("a.pdf"), b"pdf-bytes-a").unwrap();
        std::fs::write(source_dir.join("b.txt"), b"text-b").unwrap();

        let now = now_ms();
        let ttl = std::time::Duration::from_secs(60);
        let locations = vec![
            ("kb-1".to_string(), "a.pdf".to_string()),
            ("kb-1".to_string(), "b.txt".to_string()),
            // Missing source file is skipped, not fatal (RAGFlow storage-error path).
            ("kb-1".to_string(), "missing.pdf".to_string()),
        ];
        let cached = cache.cache_ongoing(&locations, &source_dir, ttl).unwrap();
        assert_eq!(cached, 2);
        assert_eq!(cache.len(), 2);

        // Warm read returns the cached file path with the right bytes.
        let path = cache.get("kb-1", "a.pdf", now).expect("cached file");
        assert_eq!(std::fs::read(path).unwrap(), b"pdf-bytes-a");
        // Unknown key misses.
        assert!(cache.get("kb-2", "a.pdf", now).is_none());

        // Re-polling the same locations does not re-cache (Redis `exist` skip).
        let cached = cache.cache_ongoing(&locations, &source_dir, ttl).unwrap();
        assert_eq!(cached, 0);
        assert_eq!(cache.len(), 2);

        // After TTL the entry expires: get() misses (Redis TTL eviction)...
        let later = now + 61_000;
        assert!(cache.get("kb-1", "a.pdf", later).is_none());
        assert_eq!(cache.len(), 2, "expired entries linger until swept");
        // ...and sweep_expired removes the entry and its physical file.
        assert_eq!(cache.sweep_expired(later), 2);
        assert_eq!(cache.len(), 0);
        let cache_dir = root.join("cache");
        let leftover: Vec<_> = std::fs::read_dir(&cache_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(leftover.is_empty(), "cache files must be deleted on sweep");

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn doc_file_cache_is_volatile_across_restarts() {
        let root = std::env::temp_dir().join(format!("rayrag-filecache2-{}", uuid::Uuid::new_v4()));
        let cache_dir = root.join("cache");
        let source_dir = root.join("uploads");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::write(source_dir.join("c.md"), b"hello").unwrap();

        let ttl = std::time::Duration::from_secs(60);
        let cache = DocFileCache::new(cache_dir.to_str().unwrap()).unwrap();
        cache
            .cache_ongoing(
                &[("kb-1".to_string(), "c.md".to_string())],
                &source_dir,
                ttl,
            )
            .unwrap();
        assert_eq!(cache.len(), 1);

        // A new DocFileCache (worker restart) drops stale files: Redis-like
        // volatility — the cache is a warm-up, not durable storage.
        let restarted = DocFileCache::new(cache_dir.to_str().unwrap()).unwrap();
        assert_eq!(restarted.len(), 0);
        assert!(restarted.get("kb-1", "c.md", 1_000_000u64).is_none());

        std::fs::remove_dir_all(root).ok();
    }

    fn row_keys(value: &Value) -> Vec<String> {
        let mut keys: Vec<String> = value
            .as_object()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();
        keys
    }

    fn response_body(response: Response) -> Value {
        let bytes = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(axum::body::to_bytes(response.into_body(), usize::MAX))
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn to_json_row_matches_upstream_constructor_data() {
        let mut folder = record("folder");
        folder.file_type = "folder".into();
        let row = to_json_row(&folder, "tenant");
        assert_eq!(
            row_keys(&row),
            vec![
                "created_by",
                "id",
                "location",
                "name",
                "parent_id",
                "size",
                "source_type",
                "tenant_id",
                "type",
            ]
        );
        assert_eq!(row["created_by"], "tenant");
        assert_eq!(row["type"], "folder");
        assert_eq!(row["location"], "");
    }

    #[test]
    fn created_folder_validation_matches_upstream_wording() {
        let missing = validate_create_folder(&json!({"type": "folder"}));
        let body = response_body(missing.err().expect("missing name"));
        assert_eq!(body["code"], 101);
        assert_eq!(
            body["message"],
            "Field: <name> - Message: <Field required> - Value: <{'type': 'folder'}>"
        );

        let extra_field = validate_create_folder(&json!({"name": "x", "nope": 1}));
        let body = response_body(extra_field.err().expect("extra field"));
        assert_eq!(
            body["message"],
            "Field: <nope> - Message: <Extra inputs are not permitted> - Value: <1>"
        );

        let too_long = validate_create_folder(&json!({"name": "x".repeat(256)}));
        let body = response_body(too_long.err().expect("too long"));
        let message = body["message"].as_str().unwrap();
        assert!(message.starts_with(
            "Field: <name> - Message: <String should have at most 255 characters> - Value: <"
        ));
    }

    #[test]
    fn move_validation_matches_upstream_model_validator() {
        let none = validate_move(&json!({"src_file_ids": ["a"]}));
        let body = response_body(none.err().expect("model error"));
        assert_eq!(
            body["message"],
            "Field: <> - Message: <Value error, At least one of dest_file_id or new_name must be provided> - Value: <{'src_file_ids': ['a']}>"
        );

        let multi = validate_move(&json!({
            "src_file_ids": ["a", "b"],
            "new_name": "nope.txt"
        }));
        let body = response_body(multi.err().expect("model error"));
        assert_eq!(
            body["message"],
            "Field: <> - Message: <Value error, new_name can only be used with a single file> - Value: <{'src_file_ids': ['a', 'b'], 'new_name': 'nope.txt'}>"
        );

        let empty = validate_move(&json!({"src_file_ids": []}));
        let body = response_body(empty.err().expect("empty list"));
        assert_eq!(
            body["message"],
            "Field: <src_file_ids> - Message: <List should have at least 1 item after validation, not 0> - Value: <[]>"
        );
    }

    #[test]
    fn upload_filenames_map_to_upstream_file_types() {
        assert_eq!(filename_type("a.pdf"), "pdf");
        assert_eq!(filename_type("notes.TXT"), "doc");
        assert_eq!(filename_type("voice.mp3"), "aural");
        assert_eq!(filename_type("photo.JPEG"), "visual");
        assert_eq!(filename_type("archive.zip"), "other");
        assert_eq!(filename_type("noext"), "other");
        assert_eq!(
            upload_path_segments("docs/2026/report.txt"),
            vec!["docs", "2026", "report.txt"]
        );
        assert!(
            upload_path_segments("../etc/passwd")
                .iter()
                .all(|segment| segment != "..")
        );
    }
}
