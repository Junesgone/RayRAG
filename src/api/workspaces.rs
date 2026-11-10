//! The workspace commit family: snapshots of a file workspace (a folder in the file store), the
//! files each snapshot changed, the diff between two snapshots, the uncommitted changes, the tree
//! as it looked at a snapshot, the content of one file at a snapshot, and a single file's version
//! history.
//!
//! Upstream RAGFlow serves these from `api/apps/restful_apis/file_commit_api.py`, which registers the
//! same eight handlers under several prefixes — `/workspaces/<workspace_id>`,
//! `/folders/<folder_id>` and `/datasets/<dataset_id>`. Two of those prefixes mean the same thing to
//! RayRAG:
//!
//! * a **workspace** is a folder in the file store (`"root"` or a record with `file_type: "folder"`),
//!   and its snapshot state is the files it contains;
//! * a **dataset** is a knowledge base, and its snapshot state is the documents it owns. Upstream
//!   resolves a dataset to a folder and documents that artifact commits are written with
//!   `folder_id = kb_id`, so RayRAG keeps the dataset id as its own commit scope and builds the state
//!   from the document store. The two scopes never mix: a commit records which kind of scope it
//!   belongs to.
//!
//! Nothing here is a stub: a commit is computed against the live state of its scope, stored, and
//! served back — including the per-file content a snapshot is expected to be able to reproduce.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::server::{AppState, AuthContext, api_error_code, code};
use crate::user_register::EXCEPTION_ERROR;

/// A snapshot of one scope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceCommit {
    pub id: String,
    /// The folder id, or the dataset id when the scope is a dataset.
    pub folder_id: String,
    /// What kind of scope this is: `"folder"` or `"dataset"`.
    #[serde(default)]
    pub scope: String,
    pub parent_id: Option<String>,
    pub message: String,
    pub author_id: String,
    pub file_count: usize,
    /// A JSON object (kept as a string, as upstream does) mapping file id to
    /// `{hash, location, name, size, status, parent_id}`.
    pub tree_state: String,
    /// The same snapshot in insertion order (upstream's Python dict order).
    #[serde(default)]
    pub tree_entries: Vec<TreeStateRow>,
    pub create_time: u64,
}

/// One file's change inside a snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitItem {
    pub id: String,
    pub commit_id: String,
    pub file_id: String,
    #[serde(default)]
    pub file_name: Option<String>,
    pub operation: String,
    #[serde(default)]
    pub old_hash: Option<String>,
    #[serde(default)]
    pub new_hash: Option<String>,
    #[serde(default)]
    pub old_location: Option<String>,
    #[serde(default)]
    pub new_location: Option<String>,
    #[serde(default)]
    pub old_name: Option<String>,
    #[serde(default)]
    pub new_name: Option<String>,
    /// The file's content when the caller supplied it; absent means the content lives in the store.
    #[serde(default)]
    pub content: Option<String>,
    /// FileCommitItem.create_time, the item's own timestamp.
    #[serde(default)]
    pub create_time: u64,
}

/// One ordered row of a commit snapshot: file id plus the entry upstream keeps under it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TreeStateRow {
    pub file_id: String,
    pub entry: StateEntry,
}

#[derive(Serialize, Deserialize, Default)]
struct CommitFile {
    commits: Vec<WorkspaceCommit>,
    items: Vec<CommitItem>,
}

pub struct WorkspaceCommitStore {
    commits: RwLock<Vec<WorkspaceCommit>>,
    items: RwLock<Vec<CommitItem>>,
    metadata_path: String,
    save_lock: Mutex<()>,
}

impl WorkspaceCommitStore {
    pub fn new(data_dir: &str) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let metadata_path = std::path::Path::new(data_dir).join("workspace_commits.json");
        crate::persistence::restore_if_missing(&metadata_path)?;
        let file: CommitFile = if metadata_path.exists() {
            let data = std::fs::read_to_string(&metadata_path)?;
            serde_json::from_str(&data).map_err(|error| {
                anyhow::anyhow!(
                    "Failed to parse workspace commits '{}': {error}",
                    metadata_path.display()
                )
            })?
        } else {
            CommitFile::default()
        };
        let mut commits = file.commits;
        for commit in &mut commits {
            if commit.tree_entries.is_empty() && !commit.tree_state.is_empty() {
                commit.tree_entries = parse_tree_rows(&commit.tree_state);
            }
        }
        Ok(Self {
            commits: RwLock::new(commits),
            items: RwLock::new(file.items),
            metadata_path: metadata_path.to_string_lossy().into_owned(),
            save_lock: Mutex::new(()),
        })
    }

    fn persist(&self) -> anyhow::Result<()> {
        let _guard = self.save_lock.lock().unwrap();
        let payload = CommitFile {
            commits: self.commits.read().unwrap().clone(),
            items: self.items.read().unwrap().clone(),
        };
        crate::persistence::save_json(std::path::Path::new(&self.metadata_path), &payload)
    }

    /// The newest commit in a scope, which the next commit records as its parent.
    pub fn head(&self, scope_id: &str) -> Option<WorkspaceCommit> {
        self.commits
            .read()
            .unwrap()
            .iter()
            .filter(|commit| commit.folder_id == scope_id)
            .max_by_key(|commit| (commit.create_time, commit.id.clone()))
            .cloned()
    }

    pub fn insert(&self, commit: WorkspaceCommit, items: Vec<CommitItem>) -> anyhow::Result<()> {
        self.commits.write().unwrap().push(commit);
        self.items.write().unwrap().extend(items);
        self.persist()
    }

    pub fn get(&self, scope_id: &str, commit_id: &str) -> Option<WorkspaceCommit> {
        self.commits
            .read()
            .unwrap()
            .iter()
            .find(|commit| commit.folder_id == scope_id && commit.id == commit_id)
            .cloned()
    }

    /// Any commit with this id, whichever scope it belongs to.
    ///
    /// Upstream looks a commit up by id first and compares its folder after, so "no such commit"
    /// and "commit belongs to another workspace" are different answers with different envelopes
    /// (round⑦ probes).
    pub fn find_any(&self, commit_id: &str) -> Option<WorkspaceCommit> {
        self.commits
            .read()
            .unwrap()
            .iter()
            .find(|commit| commit.id == commit_id)
            .cloned()
    }

    pub fn items_of(&self, commit_id: &str) -> Vec<CommitItem> {
        self.items
            .read()
            .unwrap()
            .iter()
            .filter(|item| item.commit_id == commit_id)
            .cloned()
            .collect()
    }

    /// `(commits, total)` for one scope, newest first unless `desc` is false.
    /// The most recent commits across every scope, newest first.
    ///
    /// `/api/v1/commits` needs a deployment-wide view; per-scope listing cannot answer it, and
    /// answering with a hard-coded empty array would report "no commits" while commits exist.
    pub fn recent(&self, limit: usize) -> Vec<WorkspaceCommit> {
        let mut commits: Vec<WorkspaceCommit> = self.commits.read().unwrap().clone();
        commits.sort_by(|left, right| right.create_time.cmp(&left.create_time));
        commits.truncate(limit.clamp(1, 500));
        commits
    }

    pub fn list(
        &self,
        scope_id: &str,
        order_by: &str,
        desc: bool,
    ) -> (Vec<WorkspaceCommit>, usize) {
        let mut commits: Vec<WorkspaceCommit> = self
            .commits
            .read()
            .unwrap()
            .iter()
            .filter(|commit| commit.folder_id == scope_id)
            .cloned()
            .collect();
        commits.sort_by(|left, right| match order_by {
            "file_count" => left.file_count.cmp(&right.file_count),
            "message" => left.message.cmp(&right.message),
            "author_id" => left.author_id.cmp(&right.author_id),
            "folder_id" => left.folder_id.cmp(&right.folder_id),
            "parent_id" => left.parent_id.cmp(&right.parent_id),
            "id" => left.id.cmp(&right.id),
            _ => left.create_time.cmp(&right.create_time),
        });
        if desc {
            commits.reverse();
        }
        let total = commits.len();
        (commits, total)
    }

    /// Every change recorded for one file, newest first — the version history.
    pub fn versions(&self, file_id: &str) -> Vec<(CommitItem, WorkspaceCommit)> {
        let commits = self.commits.read().unwrap().clone();
        let mut history: Vec<(CommitItem, WorkspaceCommit)> = self
            .items
            .read()
            .unwrap()
            .iter()
            .filter(|item| item.file_id == file_id)
            .filter_map(|item| {
                commits
                    .iter()
                    .find(|commit| commit.id == item.commit_id)
                    .map(|commit| (item.clone(), commit.clone()))
            })
            .collect();
        history.sort_by(|left, right| {
            right
                .1
                .create_time
                .cmp(&left.1.create_time)
                .then_with(|| right.1.id.cmp(&left.1.id))
        });
        history
    }
}

/// One entry in a scope's snapshot state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateEntry {
    pub hash: String,
    pub location: String,
    pub name: String,
    pub size: usize,
    pub status: String,
    pub parent_id: String,
}

/// A content fingerprint. Upstream stores a sha256; this project only needs a stable, comparable
/// value (hashes are compared to decide add/modify/delete), and it is documented as such rather
/// than presented as a digest.
pub(crate) fn fingerprint(content: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Who may read or write a scope's history.
enum Scope {
    Folder(String),
    Dataset(String),
}

impl Scope {
    fn id(&self) -> &str {
        match self {
            Scope::Folder(id) | Scope::Dataset(id) => id,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Scope::Folder(_) => "folder",
            Scope::Dataset(_) => "dataset",
        }
    }
}

fn folder_scope(state: &AppState, auth: &AuthContext, id: &str) -> Option<Scope> {
    if id == "root" {
        return Some(Scope::Folder(id.to_string()));
    }
    let record = state.files.get(id)?;
    if record.file_type != "folder" {
        return None;
    }
    if record.owner_id == auth.user_id || auth.is_admin || record.owner_id.is_empty() {
        return Some(Scope::Folder(id.to_string()));
    }
    None
}

fn dataset_scope(state: &AppState, auth: &AuthContext, id: &str) -> Option<Scope> {
    let kb = state.kbs.get(id)?;
    if kb.owner_id == auth.user_id || auth.is_admin || kb.owner_id.is_empty() {
        return Some(Scope::Dataset(id.to_string()));
    }
    None
}

/// The recorded state of a scope as it is right now: the files in a folder, or the documents in a
/// dataset. Entries keep the parent so a snapshot tree can be rebuilt from the flat map.
fn current_state(
    state: &AppState,
    auth: &AuthContext,
    scope: &Scope,
) -> BTreeMap<String, StateEntry> {
    let mut entries = BTreeMap::new();
    match scope {
        Scope::Folder(folder_id) => {
            let mut pending = vec![folder_id.clone()];
            // Folders nest; a snapshot of a folder covers everything below it.
            while let Some(parent) = pending.pop() {
                for record in state.files.list_for(&auth.user_id, auth.is_admin, &parent) {
                    if record.file_type == "folder" {
                        pending.push(record.id.clone());
                        continue;
                    }
                    entries.insert(
                        record.id.clone(),
                        StateEntry {
                            hash: record.content_hash.clone(),
                            location: record.name.clone(),
                            name: record.name.clone(),
                            size: record.size,
                            status: "1".into(),
                            parent_id: record.parent_id.clone(),
                        },
                    );
                }
            }
        }
        Scope::Dataset(kb_id) => {
            for document in state.docs.list(kb_id) {
                entries.insert(
                    document.id.clone(),
                    StateEntry {
                        hash: document.content_hash.clone(),
                        location: document.storage_name.clone(),
                        name: document.name.clone(),
                        size: document.size,
                        status: "1".into(),
                        parent_id: kb_id.clone(),
                    },
                );
            }
        }
    }
    entries
}

/// SHA-256 hex, the digest upstream stores in tree_state hashes and object keys.
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn objects_dir(state: &AppState) -> std::path::PathBuf {
    std::path::Path::new(&state.files.data_dir).join(".objects")
}

/// Content-addressed put: .objects/<sha256> under the file store, like upstream's bucket object.
fn store_object(state: &AppState, hash: &str, bytes: &[u8]) -> anyhow::Result<String> {
    let dir = objects_dir(state);
    std::fs::create_dir_all(&dir)?;
    let location = format!(".objects/{hash}");
    let path = std::path::Path::new(&state.files.data_dir).join(&location);
    if !path.exists() {
        crate::persistence::atomic_write(&path, bytes)?;
    }
    Ok(location)
}

fn read_object(state: &AppState, hash: &str) -> Option<Vec<u8>> {
    std::fs::read(objects_dir(state).join(hash)).ok()
}

/// Python json.dumps(str, ensure_ascii=False) for one scalar.
fn py_json_str(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

/// json.dumps(tree_state, ensure_ascii=False): entries in insertion order, comma-space and
/// colon-space separators, and the six keys in the order upstream's create_commit writes them.
fn tree_state_string(rows: &[TreeStateRow]) -> String {
    let mut out = String::from("{");
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&py_json_str(&row.file_id));
        out.push_str(": {");
        let quote = '\u{22}';
        out.push_str(&format!(
            "{quote}hash{quote}: {}, {quote}location{quote}: {}, {quote}name{quote}: {}, {quote}size{quote}: {}, {quote}status{quote}: {}, {quote}parent_id{quote}: {}",
            py_json_str(&row.entry.hash),
            py_json_str(&row.entry.location),
            py_json_str(&row.entry.name),
            row.entry.size,
            py_json_str(&row.entry.status),
            py_json_str(&row.entry.parent_id)
        ));
        out.push('}');
    }
    out.push('}');
    out
}

/// Parse a rendered tree_state string into rows (legacy commits, or upstream-shaped input).
fn parse_tree_rows(text: &str) -> Vec<TreeStateRow> {
    serde_json::from_str::<BTreeMap<String, StateEntry>>(text)
        .map(|map| {
            map.into_iter()
                .map(|(file_id, entry)| TreeStateRow { file_id, entry })
                .collect()
        })
        .unwrap_or_default()
}

fn tree_rows_of(commit: &WorkspaceCommit) -> Vec<TreeStateRow> {
    if !commit.tree_entries.is_empty() {
        commit.tree_entries.clone()
    } else {
        parse_tree_rows(&commit.tree_state)
    }
}

fn tree_state_of(commit: &WorkspaceCommit) -> BTreeMap<String, StateEntry> {
    tree_rows_of(commit)
        .into_iter()
        .map(|row| (row.file_id, row.entry))
        .collect()
}

fn upsert_tree_row(rows: &mut Vec<TreeStateRow>, file_id: String, entry: StateEntry) {
    if let Some(existing) = rows.iter_mut().find(|row| row.file_id == file_id) {
        existing.entry = entry;
    } else {
        rows.push(TreeStateRow { file_id, entry });
    }
}

/// The hash of a file's current bytes: the object key when it points into .objects/, otherwise
/// the sha256 of the stored blob (upstream recomputes it from storage the same way).
fn live_hash(state: &AppState, record: &crate::api::file_mgr::FileRecord) -> Option<String> {
    if let Some(hash) = record.location.strip_prefix(".objects/")
        && !hash.is_empty()
        && read_object(state, hash).is_some()
    {
        return Some(hash.to_string());
    }
    let physical =
        crate::server::file_physical_path(&state.files.data_dir, &record.id, &record.name);
    std::fs::read(physical).ok().map(|bytes| sha256_hex(&bytes))
}

/// Every non-folder file below a folder, ordered by creation: upstream's uncommitted-changes walk
/// answers in query order and the live probe pins that order.
fn live_files_under(
    state: &AppState,
    auth: &AuthContext,
    folder_id: &str,
) -> Vec<crate::api::file_mgr::FileRecord> {
    let mut files = Vec::new();
    let mut pending = vec![folder_id.to_string()];
    while let Some(parent) = pending.pop() {
        let mut children = state.files.list_for(&auth.user_id, auth.is_admin, &parent);
        children.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        for record in children {
            if record.file_type == "folder" {
                pending.push(record.id);
            } else {
                files.push(record);
            }
        }
    }
    files
}

/// Upstream FileService.get_by_id(folder_id).name, with the tenant root answered as "/".
fn folder_display_name(state: &AppState, folder_id: &str) -> String {
    if folder_id == "root" {
        return "/".to_string();
    }
    state
        .files
        .get(folder_id)
        .map(|record| record.name)
        .unwrap_or_else(|| folder_id.to_string())
}

/// Apply a committed add/modify/rename to the live File row, like upstream's File.update(...).
fn update_file_for_commit(
    state: &AppState,
    file_id: &str,
    location: Option<&str>,
    size: Option<usize>,
    name: Option<&str>,
) -> Option<crate::api::file_mgr::FileRecord> {
    let mut record = state.files.get(file_id)?;
    if let Some(location) = location {
        record.location = location.to_string();
        record.content_hash = location
            .strip_prefix(".objects/")
            .unwrap_or(location)
            .to_string();
    }
    if let Some(size) = size {
        record.size = size;
    }
    if let Some(name) = name {
        record.name = name.to_string();
    }
    record.updated_at = crate::api::utils::datetime::now_ms();
    let _ = state.files.update(record.clone());
    Some(record)
}

/// A blank commit item in upstream's column order; callers fill exactly the fields their
/// operation records (add/modify/delete/rename each write a different subset).
fn new_commit_item(commit_id: &str, file_id: &str, operation: &str, now: u64) -> CommitItem {
    CommitItem {
        id: uuid::Uuid::new_v4().to_string(),
        commit_id: commit_id.to_string(),
        file_id: file_id.to_string(),
        file_name: None,
        operation: operation.to_string(),
        old_hash: None,
        new_hash: None,
        old_location: None,
        new_location: None,
        old_name: None,
        new_name: None,
        content: None,
        create_time: now,
    }
}

#[derive(Clone, Deserialize)]
pub struct CommitFileChange {
    pub file_id: String,
    #[serde(default)]
    pub file_name: Option<String>,
    pub operation: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub old_name: Option<String>,
    #[serde(default)]
    pub new_name: Option<String>,
}

#[derive(Deserialize)]
pub struct CreateCommitRequest {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub files: Option<Vec<CommitFileChange>>,
}

fn bad_request(code_value: i32, message: &str) -> Response {
    api_error_code(axum::http::StatusCode::BAD_REQUEST, code_value, message)
}

/// The file-commit family's error answer, replicated character-for-character from upstream.
///
/// Every failing route in this family answers through `get_data_error_result("<message>")`, whose
/// signature is `(code, message)`: the string lands in the `code` field and `message` keeps its
/// default. The round⑦ probes of v0.27.2 captured the result — HTTP 200 with
/// `{"code":"<message>","message":"Sorry! Data missing!"}` (`fc_fake_commit`, `fc_fake_commit_fls`,
/// `fc_diff_missing`, `wf_fake_versions`) — and the 2026-10-12 parity decision is to replicate the
/// quirk rather than normalise it (the ledger's "aligned means audited behavior" reading).
fn string_code_error(message: &str) -> Response {
    Json(serde_json::json!({ "code": message, "message": "Sorry! Data missing!" })).into_response()
}

/// An exception answered through `server_error_response`, verbatim: HTTP 200 with
/// `{"code":100,"data":null,"message":"ValueError(\"…\")"}`, `message` being the Python `repr` of
/// the exception (`fc_fake_dataset` probe).
fn raised_value_error(python_repr: String) -> Response {
    Json(serde_json::json!({
        "code": EXCEPTION_ERROR,
        "data": serde_json::Value::Null,
        "message": python_repr,
    }))
    .into_response()
}

/// Upstream `validate_request` lists every missing argument in one answer:
/// `"required argument are missing: {}; ".format(",".join(no_arguments))` — answered as
/// `{"code":101,"data":null,"message":"…"}` (`fc_create_missing` probe).
fn missing_arguments(no_arguments: &[&str]) -> Response {
    Json(serde_json::json!({
        "code": code::INVALID_ARGUMENT,
        "data": serde_json::Value::Null,
        "message": format!("required argument are missing: {}; ", no_arguments.join(",")),
    }))
    .into_response()
}

/// Resolve the `{workspace_id}` / `{dataset_id}` path parameter. When it cannot be resolved,
/// upstream raises `ValueError` from the resolver and answers `server_error_response(e)`; the
/// envelope (including the Python `repr` carried in `message`) is replicated here.
/// `dataset: true` selects the dataset reading of the id.
fn resolve_scope(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
) -> Result<Scope, Response> {
    if dataset {
        return dataset_scope(state, auth, id).ok_or_else(|| {
            raised_value_error(format!(
                "ValueError(\"Could not resolve datasets '{id}' to a folder\")"
            ))
        });
    }
    folder_scope(state, auth, id).ok_or_else(|| {
        raised_value_error(format!("ValueError(\"Could not resolve folder '{id}'\")"))
    })
}

/// The create-commit answer's commit object (upstream's `create_commit` key set, no `title`/
/// `comments`). Keys are written in the alphabetical order upstream's jsonify emits.
fn commit_created_json(commit: &WorkspaceCommit) -> serde_json::Value {
    serde_json::json!({
        "author_id": commit.author_id,
        "create_time": commit.create_time,
        "file_count": commit.file_count,
        "folder_id": commit.folder_id,
        "id": commit.id,
        "message": commit.message,
        "parent_id": commit.parent_id,
        "tree_state": commit.tree_state,
    })
}

/// The list answer's row (upstream's `list_commits` key set — no `tree_state`, plus the
/// artifact-commit extension fields, null for workspace commits).
fn commit_row_json(commit: &WorkspaceCommit) -> serde_json::Value {
    serde_json::json!({
        "author_id": commit.author_id,
        "comments": serde_json::Value::Null,
        "create_time": commit.create_time,
        "file_count": commit.file_count,
        "folder_id": commit.folder_id,
        "id": commit.id,
        "message": commit.message,
        "parent_id": commit.parent_id,
        "title": serde_json::Value::Null,
    })
}

/// The single-commit detail body (upstream's `get_commit`: no `tree_state`, no `title`).
fn commit_detail_json(
    commit: &WorkspaceCommit,
    files: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "author_id": commit.author_id,
        "create_time": commit.create_time,
        "file_count": commit.file_count,
        "files": files,
        "folder_id": commit.folder_id,
        "id": commit.id,
        "message": commit.message,
        "parent_id": commit.parent_id,
    })
}

/// A file change as the single-commit detail lists it (upstream maps six fields there).
fn detail_item_json(item: &CommitItem) -> serde_json::Value {
    serde_json::json!({
        "file_id": item.file_id,
        "new_hash": item.new_hash,
        "new_name": item.new_name,
        "old_hash": item.old_hash,
        "old_name": item.old_name,
        "operation": item.operation,
    })
}

/// A file change as `/commits/{id}/files` lists it (upstream's full nine-field mapping).
fn list_item_json(item: &CommitItem) -> serde_json::Value {
    serde_json::json!({
        "file_id": item.file_id,
        "id": item.id,
        "new_hash": item.new_hash,
        "new_location": item.new_location,
        "new_name": item.new_name,
        "old_hash": item.old_hash,
        "old_location": item.old_location,
        "old_name": item.old_name,
        "operation": item.operation,
    })
}

/// `POST /api/v1/workspaces/{workspace_id}/commits` and `POST /api/v1/datasets/{dataset_id}/commits`.
pub async fn create_commit(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<CreateCommitRequest>,
) -> Response {
    create_commit_in(&state, &auth, &id, false, body).await
}

pub(crate) async fn create_commit_in(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    body: CreateCommitRequest,
) -> Response {
    let scope = match resolve_scope(state, auth, id, dataset) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let mut missing: Vec<&str> = Vec::new();
    if body.message.is_none() {
        missing.push("message");
    }
    if body.files.is_none() {
        missing.push("files");
    }
    if !missing.is_empty() {
        return missing_arguments(&missing);
    }
    let message = body.message.clone().unwrap_or_default();
    let changes = body.files.clone().unwrap_or_default();
    if dataset {
        return create_dataset_scope_commit(state, auth, &scope, message, changes);
    }
    let quote = '\u{22}';
    if changes.iter().any(|change| change.operation == "delete") {
        // Upstream v0.27.2 writes the soft delete through File.status, a column its File model
        // does not have: the whole transaction rolls back and the route answers the AttributeError.
        return raised_value_error(format!(
            "AttributeError({quote}type object 'File' has no attribute 'status'{quote})"
        ));
    }
    let head = state.workspace_commits.head(scope.id());
    let mut rows = head.as_ref().map(tree_rows_of).unwrap_or_default();
    for row in &mut rows {
        if row.entry.parent_id.is_empty()
            && let Some(record) = state.files.get(&row.file_id)
        {
            row.entry.parent_id = record.parent_id;
        }
    }
    let commit_id = uuid::Uuid::new_v4().to_string();
    let now = crate::api::utils::datetime::now_ms();
    let mut items = Vec::new();
    for change in &changes {
        let bytes = change.content.clone().unwrap_or_default().into_bytes();
        let operation = change.operation.clone();
        let mut item = new_commit_item(&commit_id, &change.file_id, &operation, now);
        match operation.as_str() {
            "add" | "modify" => {
                let hash = sha256_hex(&bytes);
                let location = match store_object(state, &hash, &bytes) {
                    Ok(location) => location,
                    Err(error) => {
                        return api_error_code(
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            code::OPERATION_ERROR,
                            &error.to_string(),
                        );
                    }
                };
                let previous = rows
                    .iter()
                    .find(|row| row.file_id == change.file_id)
                    .map(|row| row.entry.clone());
                if operation == "modify"
                    && let Some(previous) = &previous
                    && !previous.hash.is_empty()
                {
                    item.old_hash = Some(previous.hash.clone());
                    item.old_location = Some(previous.location.clone());
                }
                let live = update_file_for_commit(
                    state,
                    &change.file_id,
                    Some(&location),
                    Some(bytes.len()),
                    None,
                );
                let name = change
                    .file_name
                    .clone()
                    .or_else(|| previous.as_ref().map(|entry| entry.name.clone()))
                    .unwrap_or_default();
                let parent_id = live
                    .as_ref()
                    .map(|record| record.parent_id.clone())
                    .or_else(|| {
                        state
                            .files
                            .get(&change.file_id)
                            .map(|record| record.parent_id)
                    })
                    .unwrap_or_default();
                upsert_tree_row(
                    &mut rows,
                    change.file_id.clone(),
                    StateEntry {
                        hash: hash.clone(),
                        location: location.clone(),
                        name,
                        size: bytes.len(),
                        status: "1".into(),
                        parent_id,
                    },
                );
                item.new_hash = Some(hash);
                item.new_location = Some(location);
            }
            "rename" => {
                let old_name = change.old_name.clone().unwrap_or_default();
                let new_name = change.new_name.clone().unwrap_or_default();
                let _ = update_file_for_commit(state, &change.file_id, None, None, Some(&new_name));
                if let Some(row) = rows.iter_mut().find(|row| row.file_id == change.file_id) {
                    row.entry.name = new_name.clone();
                }
                item.old_name = Some(old_name);
                item.new_name = Some(new_name);
            }
            // Any other operation (including misspellings) still records the item and leaves the
            // tree untouched, which is what the live probe observed for operation "nope".
            _ => {}
        }
        items.push(item);
    }
    let file_count = changes.len();
    let commit = WorkspaceCommit {
        id: commit_id,
        folder_id: scope.id().to_string(),
        scope: "folder".to_string(),
        parent_id: head.map(|commit| commit.id),
        message,
        author_id: auth.user_id.clone(),
        file_count,
        tree_state: tree_state_string(&rows),
        tree_entries: rows,
        create_time: now,
    };
    let payload = commit_created_json(&commit);
    if let Err(error) = state.workspace_commits.insert(commit, items) {
        return api_error_code(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        );
    }
    Json(serde_json::json!({ "code": 0, "data": payload, "message": "success" })).into_response()
}

/// The dataset-scoped commit keeps RayRAG's document snapshot as a documented replacement for
/// upstream's artifact-page commits; only the persisted row shape changed (ordered rows plus the
/// rendered string).
fn create_dataset_scope_commit(
    state: &AppState,
    auth: &AuthContext,
    scope: &Scope,
    message: String,
    changes: Vec<CommitFileChange>,
) -> Response {
    let mut head_state = current_state(state, auth, scope);
    let parent_id = state
        .workspace_commits
        .head(scope.id())
        .map(|commit| commit.id);
    let commit_id = uuid::Uuid::new_v4().to_string();
    let now = crate::api::utils::datetime::now_ms();
    let mut items = Vec::new();
    for change in &changes {
        let previous = head_state.get(&change.file_id).cloned();
        let old_hash = previous.as_ref().map(|entry| entry.hash.clone());
        let old_location = previous.as_ref().map(|entry| entry.location.clone());
        let old_name = previous
            .as_ref()
            .map(|entry| entry.name.clone())
            .or(change.old_name.clone());
        let new_hash = change
            .content
            .as_ref()
            .map(|content| fingerprint(content))
            .or_else(|| {
                if change.operation == "delete" {
                    None
                } else {
                    previous.as_ref().map(|entry| entry.hash.clone())
                }
            });
        let new_name = change
            .new_name
            .clone()
            .or(change.file_name.clone())
            .or_else(|| previous.as_ref().map(|entry| entry.name.clone()));
        let new_location = new_name.clone();
        match change.operation.as_str() {
            "delete" => {
                head_state.remove(&change.file_id);
            }
            "rename" => {
                if let Some(entry) = head_state.get_mut(&change.file_id)
                    && let Some(name) = &new_name
                {
                    entry.name = name.clone();
                }
            }
            _ => {
                head_state.insert(
                    change.file_id.clone(),
                    StateEntry {
                        hash: new_hash.clone().unwrap_or_default(),
                        location: new_location.clone().unwrap_or_default(),
                        name: new_name.clone().unwrap_or_else(|| change.file_id.clone()),
                        size: change
                            .content
                            .as_ref()
                            .map(|content| content.len())
                            .or_else(|| previous.as_ref().map(|entry| entry.size))
                            .unwrap_or(0),
                        status: "1".into(),
                        parent_id: previous
                            .as_ref()
                            .map(|entry| entry.parent_id.clone())
                            .or_else(|| Some(scope.id().to_string()))
                            .unwrap_or_default(),
                    },
                );
            }
        }
        items.push(CommitItem {
            id: uuid::Uuid::new_v4().to_string(),
            commit_id: commit_id.clone(),
            file_id: change.file_id.clone(),
            file_name: new_name.clone(),
            operation: change.operation.clone(),
            old_hash,
            new_hash,
            old_location,
            new_location,
            old_name,
            new_name,
            content: change.content.clone(),
            create_time: now,
        });
    }
    let rows: Vec<TreeStateRow> = head_state
        .into_iter()
        .map(|(file_id, entry)| TreeStateRow { file_id, entry })
        .collect();
    let file_count = changes.len();
    let commit = WorkspaceCommit {
        id: commit_id,
        folder_id: scope.id().to_string(),
        scope: "dataset".to_string(),
        parent_id,
        message,
        author_id: auth.user_id.clone(),
        file_count,
        tree_state: tree_state_string(&rows),
        tree_entries: rows,
        create_time: now,
    };
    let payload = commit_created_json(&commit);
    if let Err(error) = state.workspace_commits.insert(commit, items) {
        return api_error_code(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            &error.to_string(),
        );
    }
    Json(serde_json::json!({ "code": 0, "data": payload, "message": "success" })).into_response()
}

/// `GET .../commits` — the paginated list.
pub(crate) fn list_commits_in(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    query: &HashMap<String, String>,
) -> Response {
    let scope = match resolve_scope(state, auth, id, dataset) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let page = query
        .get("page")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(1);
    // Upstream `validate_rest_api_page_size`: junk and non-positive values fall back to the
    // default (30) silently; above the public maximum it raises, and the route answers the repr.
    let page_size = match query
        .get("page_size")
        .and_then(|value| value.trim().parse::<i64>().ok())
    {
        Some(value) if value > 100 => {
            return raised_value_error(
                "ValueError('page_size must be less than or equal to 100')".to_string(),
            );
        }
        Some(value) if value >= 1 => value as usize,
        _ => 30,
    };
    let order_by = query
        .get("order_by")
        .map(String::as_str)
        .unwrap_or("create_time");
    let known_columns = [
        "id",
        "folder_id",
        "parent_id",
        "message",
        "author_id",
        "file_count",
        "create_time",
    ];
    if !known_columns.contains(&order_by) {
        let quote = '\u{22}';
        return raised_value_error(format!(
            "AttributeError({quote}type object 'FileCommit' has no attribute '{order_by}'{quote})"
        ));
    }
    let desc = query
        .get("desc")
        .map(|value| value != "false")
        .unwrap_or(true);
    let (commits, total) = state.workspace_commits.list(scope.id(), order_by, desc);
    let start = (page - 1) * page_size;
    let rows: Vec<serde_json::Value> = commits
        .into_iter()
        .skip(start)
        .take(page_size)
        .map(|commit| commit_row_json(&commit))
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": { "commits": rows, "page": page, "page_size": page_size, "total": total },
        "message": "success",
    }))
    .into_response()
}

pub async fn list_commits(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    list_commits_in(&state, &auth, &id, false, &query)
}

fn commit_in_scope(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    commit_id: &str,
) -> Result<(Scope, WorkspaceCommit), Response> {
    let scope = resolve_scope(state, auth, id, dataset)?;
    match state.workspace_commits.find_any(commit_id) {
        None => Err(string_code_error("Commit not found")),
        Some(commit) if commit.folder_id != scope.id() => {
            Err(string_code_error("Commit not found in workspace"))
        }
        Some(commit) => Ok((scope, commit)),
    }
}

/// `GET .../commits/{commit_id}` — the commit with its file changes.
pub(crate) fn get_commit_in(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    commit_id: &str,
) -> Response {
    let (_, commit) = match commit_in_scope(state, auth, id, dataset, commit_id) {
        Ok(found) => found,
        Err(response) => return response,
    };
    let files: Vec<serde_json::Value> = state
        .workspace_commits
        .items_of(&commit.id)
        .iter()
        .map(detail_item_json)
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": commit_detail_json(&commit, files),
        "message": "success",
    }))
    .into_response()
}

pub async fn get_commit(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id)): Path<(String, String)>,
) -> Response {
    get_commit_in(&state, &auth, &id, false, &commit_id)
}

/// `GET .../commits/{commit_id}/files`.
pub(crate) fn list_commit_files_in(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    commit_id: &str,
) -> Response {
    let (_, commit) = match commit_in_scope(state, auth, id, dataset, commit_id) {
        Ok(found) => found,
        Err(response) => return response,
    };
    let files: Vec<serde_json::Value> = state
        .workspace_commits
        .items_of(&commit.id)
        .iter()
        .map(list_item_json)
        .collect();
    Json(serde_json::json!({ "code": 0, "data": files, "message": "success" })).into_response()
}

pub async fn list_commit_files(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id)): Path<(String, String)>,
) -> Response {
    list_commit_files_in(&state, &auth, &id, false, &commit_id)
}

/// `GET .../commits/diff?from=&to=`.
pub(crate) fn diff_commits_in(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    query: &HashMap<String, String>,
) -> Response {
    let scope = match resolve_scope(state, auth, id, dataset) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let from_id = query.get("from").map(String::as_str).unwrap_or("");
    let to_id = query.get("to").map(String::as_str).unwrap_or("");
    if from_id.is_empty() || to_id.is_empty() {
        return string_code_error("'from' and 'to' parameters are required");
    }
    let (from, to) = match (
        state.workspace_commits.find_any(from_id),
        state.workspace_commits.find_any(to_id),
    ) {
        (Some(from), Some(to)) => (from, to),
        _ => return string_code_error("Commit not found"),
    };
    if from.folder_id != scope.id() || to.folder_id != scope.id() {
        return string_code_error("Commit not found in workspace");
    }
    let before = tree_state_of(&from);
    let after = tree_state_of(&to);
    let from_items: HashMap<String, CommitItem> = state
        .workspace_commits
        .items_of(&from.id)
        .into_iter()
        .map(|item| (item.file_id.clone(), item))
        .collect();
    let to_items: HashMap<String, CommitItem> = state
        .workspace_commits
        .items_of(&to.id)
        .into_iter()
        .map(|item| (item.file_id.clone(), item))
        .collect();
    let mut file_ids: Vec<String> = before.keys().chain(after.keys()).cloned().collect();
    file_ids.sort();
    file_ids.dedup();
    let pick = |value: String, fallback: Option<&CommitItem>, field: bool| -> Option<String> {
        if !value.is_empty() {
            return Some(value);
        }
        fallback.and_then(|item| {
            if field {
                item.new_hash.clone()
            } else {
                item.new_location.clone()
            }
        })
    };
    let mut changes = Vec::new();
    for file_id in file_ids {
        let left = before.get(&file_id);
        let right = after.get(&file_id);
        let from_item = from_items.get(&file_id);
        let to_item = to_items.get(&file_id);
        match (left, right) {
            (Some(left), None) => {
                changes.push(serde_json::json!({
                    "file_id": file_id,
                    "file_name": left.name,
                    "operation": "delete",
                    "old_hash": pick(left.hash.clone(), from_item, true),
                    "old_location": Some(left.location.clone()),
                    "new_hash": serde_json::Value::Null,
                    "new_location": serde_json::Value::Null,
                }));
            }
            (None, Some(right)) => {
                changes.push(serde_json::json!({
                    "file_id": file_id,
                    "file_name": right.name,
                    "operation": "add",
                    "old_hash": serde_json::Value::Null,
                    "old_location": serde_json::Value::Null,
                    "new_hash": pick(right.hash.clone(), to_item, true),
                    "new_location": Some(right.location.clone()),
                }));
            }
            (Some(left), Some(right)) => {
                let mut changed = false;
                let mut operation = "modify";
                if left.hash != right.hash {
                    changed = true;
                }
                if left.status != right.status {
                    changed = true;
                    operation = if right.status == "0" { "delete" } else { "add" };
                }
                if left.name != right.name {
                    changed = true;
                    operation = "rename";
                }
                if changed {
                    let file_name = if right.name.is_empty() {
                        left.name.clone()
                    } else {
                        right.name.clone()
                    };
                    changes.push(serde_json::json!({
                        "file_id": file_id,
                        "file_name": file_name,
                        "operation": operation,
                        "old_hash": pick(left.hash.clone(), from_item, true),
                        "old_location": pick(left.location.clone(), from_item, false),
                        "new_hash": pick(right.hash.clone(), to_item, true),
                        "new_location": pick(right.location.clone(), to_item, false),
                    }));
                }
            }
            (None, None) => {}
        }
    }
    Json(serde_json::json!({ "code": 0, "data": changes, "message": "success" })).into_response()
}

pub async fn diff_commits(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    diff_commits_in(&state, &auth, &id, false, &query)
}

/// `GET .../changes` — what changed since the last commit.
pub(crate) fn uncommitted_changes_in(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
) -> Response {
    let scope = match resolve_scope(state, auth, id, dataset) {
        Ok(scope) => scope,
        Err(response) => return response,
    };
    let mut changes = Vec::new();
    match &scope {
        Scope::Folder(folder_id) => {
            let committed = state
                .workspace_commits
                .head(scope.id())
                .map(|commit| tree_rows_of(&commit))
                .unwrap_or_default();
            let live = live_files_under(state, auth, folder_id);
            let mut processed = std::collections::HashSet::new();
            for row in &committed {
                processed.insert(row.file_id.clone());
                if row.entry.status == "0" {
                    continue;
                }
                match live.iter().find(|record| record.id == row.file_id) {
                    Some(record) => {
                        if let Some(hash) = live_hash(state, record)
                            && hash != row.entry.hash
                        {
                            changes.push(serde_json::json!({
                                "file_id": row.file_id,
                                "file_name": row.entry.name,
                                "operation": "modify",
                            }));
                        }
                    }
                    None => {
                        if state.files.get(&row.file_id).is_none() {
                            changes.push(serde_json::json!({
                                "file_id": row.file_id,
                                "file_name": row.entry.name,
                                "operation": "delete",
                            }));
                        }
                    }
                }
            }
            for record in &live {
                if !processed.contains(&record.id) {
                    changes.push(serde_json::json!({
                        "file_id": record.id,
                        "file_name": record.name,
                        "operation": "add",
                    }));
                }
            }
        }
        Scope::Dataset(_) => {
            let now_state = current_state(state, auth, &scope);
            let committed = state
                .workspace_commits
                .head(scope.id())
                .map(|commit| tree_state_of(&commit))
                .unwrap_or_default();
            let mut file_ids: Vec<String> =
                now_state.keys().chain(committed.keys()).cloned().collect();
            file_ids.sort();
            file_ids.dedup();
            for file_id in file_ids {
                let now = now_state.get(&file_id);
                let then = committed.get(&file_id);
                let operation = match (then, now) {
                    (None, Some(_)) => "add",
                    (Some(_), None) => "delete",
                    (Some(then), Some(now)) if then.hash != now.hash || then.name != now.name => {
                        "modify"
                    }
                    _ => continue,
                };
                changes.push(serde_json::json!({
                    "file_id": file_id,
                    "file_name": now
                        .map(|entry| entry.name.clone())
                        .or_else(|| then.map(|entry| entry.name.clone())),
                    "operation": operation,
                }));
            }
        }
    }
    Json(serde_json::json!({ "code": 0, "data": changes, "message": "success" })).into_response()
}

pub async fn uncommitted_changes(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    uncommitted_changes_in(&state, &auth, &id, false)
}

/// `GET .../commits/{commit_id}/tree` — the folder tree as it looked at that commit.
pub(crate) fn commit_tree_in(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    commit_id: &str,
) -> Response {
    let (scope, commit) = match commit_in_scope(state, auth, id, dataset, commit_id) {
        Ok(found) => found,
        Err(response) => return response,
    };
    let rows = tree_rows_of(&commit);
    let root_id = scope.id().to_string();
    let root_name = if dataset {
        state
            .kbs
            .get(&root_id)
            .map(|kb| kb.name.clone())
            .unwrap_or_else(|| root_id.clone())
    } else {
        folder_display_name(state, &root_id)
    };
    let mut folder_ids: Vec<String> = vec![root_id.clone()];
    let mut files_by_parent: HashMap<String, Vec<TreeStateRow>> = HashMap::new();
    for row in &rows {
        let parent = if row.entry.parent_id.is_empty() {
            root_id.clone()
        } else {
            row.entry.parent_id.clone()
        };
        if !folder_ids.contains(&parent) {
            folder_ids.push(parent.clone());
        }
        files_by_parent.entry(parent).or_default().push(row.clone());
    }
    let mut children_by_folder: HashMap<String, Vec<String>> = HashMap::new();
    for folder_id in &folder_ids {
        if folder_id == &root_id {
            continue;
        }
        if let Some(record) = state.files.get(folder_id) {
            children_by_folder
                .entry(record.parent_id.clone())
                .or_default()
                .push(folder_id.clone());
        }
    }
    let tree = commit_tree_node(
        state,
        &root_id,
        &root_name,
        &files_by_parent,
        &children_by_folder,
        0,
    );
    Json(serde_json::json!({ "code": 0, "data": tree, "message": "success" })).into_response()
}

/// Upstream _build_node: files first, then sub-folders, each node keyed id/name/type/children.
fn commit_tree_node(
    state: &AppState,
    node_id: &str,
    root_name: &str,
    files_by_parent: &HashMap<String, Vec<TreeStateRow>>,
    children_by_folder: &HashMap<String, Vec<String>>,
    depth: usize,
) -> serde_json::Value {
    let mut children = Vec::new();
    if let Some(rows) = files_by_parent.get(node_id) {
        for row in rows {
            let mut file = serde_json::json!({
                "id": row.file_id,
                "name": row.entry.name,
                "type": "file",
                "hash": row.entry.hash,
                "size": row.entry.size,
                "status": row.entry.status,
            });
            if !row.entry.location.is_empty() {
                file["location"] = serde_json::json!(row.entry.location);
            }
            children.push(file);
        }
    }
    if depth < 32
        && let Some(folders) = children_by_folder.get(node_id)
    {
        for folder_id in folders {
            children.push(commit_tree_node(
                state,
                folder_id,
                root_name,
                files_by_parent,
                children_by_folder,
                depth + 1,
            ));
        }
    }
    let name = folder_display_name(state, node_id);
    serde_json::json!({ "id": node_id, "name": name, "type": "folder", "children": children })
}

pub async fn commit_tree(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id)): Path<(String, String)>,
) -> Response {
    commit_tree_in(&state, &auth, &id, false, &commit_id)
}

/// `GET .../commits/{commit_id}/files/{file_id}/content`.
pub(crate) async fn commit_file_content_in(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    commit_id: &str,
    file_id: &str,
) -> Response {
    let (_scope, commit) = match commit_in_scope(state, auth, id, dataset, commit_id) {
        Ok(found) => found,
        Err(response) => return response,
    };
    // 1. The snapshot's own hash, read from the content-addressed object store.
    if let Some(row) = tree_rows_of(&commit)
        .into_iter()
        .find(|row| row.file_id == file_id)
        && !row.entry.hash.is_empty()
        && let Some(bytes) = read_object(state, &row.entry.hash)
    {
        return Json(serde_json::json!({
            "code": 0,
            "data": { "content": String::from_utf8_lossy(&bytes) },
            "message": "success",
        }))
        .into_response();
    }
    // 2. Walk the parent chain until a commit item recorded this file's bytes.
    let mut current = Some(commit.clone());
    let mut guard = 0usize;
    while let Some(candidate) = current {
        if let Some(item) = state
            .workspace_commits
            .items_of(&candidate.id)
            .into_iter()
            .find(|item| item.file_id == file_id)
            && let Some(hash) = item.new_hash.clone().filter(|hash| !hash.is_empty())
            && let Some(bytes) = read_object(state, &hash)
        {
            return Json(serde_json::json!({
                "code": 0,
                "data": { "content": String::from_utf8_lossy(&bytes) },
                "message": "success",
            }))
            .into_response();
        }
        current = candidate
            .parent_id
            .and_then(|parent| state.workspace_commits.find_any(&parent));
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    // 3. Fall back to the file's current bytes (RayRAG's documented substitute for a full blob
    // snapshot; upstream keeps every historic blob in its bucket).
    if let Some(record) = state.files.get(file_id) {
        let physical =
            crate::server::file_physical_path(&state.files.data_dir, &record.id, &record.name);
        if let Ok(content) = std::fs::read_to_string(&physical) {
            return Json(serde_json::json!({ "code": 0, "data": { "content": content }, "message": "success" }))
                .into_response();
        }
    }
    if let Some(record) = state.docs.get(file_id)
        && let Ok(content) = std::fs::read_to_string(
            std::path::Path::new(&state.files.data_dir).join(&record.storage_name),
        )
    {
        return Json(
            serde_json::json!({ "code": 0, "data": { "content": content }, "message": "success" }),
        )
        .into_response();
    }
    string_code_error("File not found in this commit")
}

pub async fn commit_file_content(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id, file_id)): Path<(String, String, String)>,
) -> Response {
    commit_file_content_in(&state, &auth, &id, false, &commit_id, &file_id).await
}

/// `GET /api/v1/workspace-files/{file_id}/versions`.
/// Whether the caller can see the file a version history is asked about. Workspace files are the
/// file records; documents stand in for upstream's file rows under a dataset, so a document id is
/// visible when its dataset is.
fn file_or_document_visible(state: &AppState, auth: &AuthContext, file_id: &str) -> bool {
    if let Some(record) = state.files.get(file_id) {
        if record.owner_id == auth.user_id || auth.is_admin || record.owner_id.is_empty() {
            return true;
        }
    }
    if let Some(document) = state.docs.get(file_id) {
        return dataset_scope(state, auth, &document.kb_id).is_some();
    }
    false
}

pub(crate) fn file_versions_in(state: &AppState, auth: &AuthContext, file_id: &str) -> Response {
    // Upstream resolves the file first and answers the string-code quirk `"File not found"` for a
    // file that is not there or not this caller's (probe `wf_fake_versions`); a file it can see but
    // that was never committed answers an empty list.
    if !file_or_document_visible(state, auth, file_id) {
        return string_code_error("File not found");
    }
    let history = state.workspace_commits.versions(file_id);
    if history.is_empty() {
        return Json(serde_json::json!({ "code": 0, "data": [], "message": "success" }))
            .into_response();
    }
    let visible: Vec<serde_json::Value> = history
        .into_iter()
        .filter(|(_, commit)| {
            // A version is visible when the reader can see the scope it belongs to.
            match commit.scope.as_str() {
                "dataset" => dataset_scope(state, auth, &commit.folder_id).is_some(),
                _ => folder_scope(state, auth, &commit.folder_id).is_some(),
            }
        })
        .map(|(item, commit)| {
            // Upstream `hash = item.new_hash or item.old_hash or ""` — Python truthiness, so an
            // empty string falls through as well.
            let hash = item
                .new_hash
                .clone()
                .filter(|value| !value.is_empty())
                .or_else(|| item.old_hash.clone().filter(|value| !value.is_empty()))
                .unwrap_or_default();
            serde_json::json!({
                "commit_id": commit.id,
                "create_time": if item.create_time == 0 {
                    commit.create_time
                } else {
                    item.create_time
                },
                "hash": hash,
                "message": commit.message,
                "operation": item.operation,
            })
        })
        .collect();
    Json(serde_json::json!({ "code": 0, "data": visible, "message": "success" })).into_response()
}

pub async fn file_versions(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(file_id): Path<String>,
) -> Response {
    file_versions_in(&state, &auth, &file_id)
}

// ── dataset-scoped handlers ──────────────────────────────────────────────────────────────────────
// The same eight operations under `/api/v1/datasets/{dataset_id}/commits`, as upstream registers
// them. RayRAG's dataset scope is the knowledge base itself (upstream writes artifact commits with
// `folder_id = kb_id`), so these differ from the workspace routes only in how the id is resolved.

pub async fn create_dataset_commit(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<CreateCommitRequest>,
) -> Response {
    create_commit_in(&state, &auth, &id, true, body).await
}

pub async fn list_dataset_commits(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    list_commits_in(&state, &auth, &id, true, &query)
}

pub async fn get_dataset_commit(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id)): Path<(String, String)>,
) -> Response {
    get_commit_in(&state, &auth, &id, true, &commit_id)
}

pub async fn list_dataset_commit_files(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id)): Path<(String, String)>,
) -> Response {
    list_commit_files_in(&state, &auth, &id, true, &commit_id)
}

pub async fn diff_dataset_commits(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    diff_commits_in(&state, &auth, &id, true, &query)
}

pub async fn dataset_uncommitted_changes(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    uncommitted_changes_in(&state, &auth, &id, true)
}

pub async fn dataset_commit_tree(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id)): Path<(String, String)>,
) -> Response {
    commit_tree_in(&state, &auth, &id, true, &commit_id)
}

pub async fn dataset_commit_file_content(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id, file_id)): Path<(String, String, String)>,
) -> Response {
    commit_file_content_in(&state, &auth, &id, true, &commit_id, &file_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fingerprint_is_stable_and_content_dependent() {
        assert_eq!(fingerprint("hello"), fingerprint("hello"));
        assert_ne!(fingerprint("hello"), fingerprint("hello "));
        assert_eq!(fingerprint("").len(), 16);
    }

    #[test]
    fn a_tree_state_round_trips_through_its_json_string() {
        let rows = vec![TreeStateRow {
            file_id: "file-1".to_string(),
            entry: StateEntry {
                hash: "abc".into(),
                location: ".objects/abc".into(),
                name: "a.txt".into(),
                size: 3,
                status: "1".into(),
                parent_id: "root".into(),
            },
        }];
        let encoded = tree_state_string(&rows);
        // Python json.dumps(ensure_ascii=False) separators and key order.
        assert_eq!(
            encoded,
            "{\"file-1\": {\"hash\": \"abc\", \"location\": \".objects/abc\", \"name\": \"a.txt\", \"size\": 3, \"status\": \"1\", \"parent_id\": \"root\"}}"
        );
        let commit = WorkspaceCommit {
            id: "c1".into(),
            folder_id: "root".into(),
            scope: "folder".into(),
            parent_id: None,
            message: "m".into(),
            author_id: "u".into(),
            file_count: 1,
            tree_state: encoded,
            tree_entries: rows,
            create_time: 1,
        };
        let decoded = tree_state_of(&commit);
        assert_eq!(decoded["file-1"].name, "a.txt");
        assert_eq!(decoded["file-1"].parent_id, "root");
    }
}
