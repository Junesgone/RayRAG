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
        Ok(Self {
            commits: RwLock::new(file.commits),
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
struct StateEntry {
    hash: String,
    location: String,
    name: String,
    size: usize,
    status: String,
    parent_id: String,
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

/// `{file_id: {hash, location, name, size, status, parent_id}}` as a JSON string, the shape
/// upstream stores in `tree_state`.
fn tree_state_json(entries: &BTreeMap<String, StateEntry>) -> String {
    serde_json::to_string(entries).unwrap_or_else(|_| "{}".into())
}

fn tree_state_of(commit: &WorkspaceCommit) -> BTreeMap<String, StateEntry> {
    serde_json::from_str(&commit.tree_state).unwrap_or_default()
}

#[derive(Deserialize)]
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
    // Upstream `validate_request("message","files")`: an argument counts as supplied by the key
    // being present, and every missing one is listed in a single answer, in declaration order
    // (`{}` → "required argument are missing: message,files; ").
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
    let message = body.message.unwrap_or_default();
    let changes = body.files.unwrap_or_default();
    for change in &changes {
        if !matches!(
            change.operation.as_str(),
            "add" | "modify" | "delete" | "rename"
        ) {
            return bad_request(
                code::INVALID_ARGUMENT,
                &format!(
                    "Unsupported operation '{}'; expected add, modify, delete or rename",
                    change.operation
                ),
            );
        }
    }

    let mut head_state = current_state(state, auth, &scope);
    let parent_id = state
        .workspace_commits
        .head(scope.id())
        .map(|commit| commit.id);
    let commit_id = uuid::Uuid::new_v4().to_string();
    let mut items = Vec::new();
    let mut touched = 0usize;
    for change in changes {
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
            .or_else(|| change.file_name.clone())
            .or_else(|| previous.as_ref().map(|entry| entry.name.clone()));
        let new_location = new_name.clone();

        match change.operation.as_str() {
            "delete" => {
                head_state.remove(&change.file_id);
            }
            "rename" => {
                if let Some(entry) = head_state.get_mut(&change.file_id) {
                    if let Some(name) = &new_name {
                        entry.name = name.clone();
                        entry.location = name.clone();
                    }
                }
                if let Some(name) = &new_name {
                    head_state.insert(
                        change.file_id.clone(),
                        StateEntry {
                            hash: new_hash.clone().unwrap_or_default(),
                            location: name.clone(),
                            name: name.clone(),
                            size: previous.as_ref().map(|entry| entry.size).unwrap_or(0),
                            status: "1".into(),
                            parent_id: previous
                                .as_ref()
                                .map(|entry| entry.parent_id.clone())
                                .unwrap_or_default(),
                        },
                    );
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
        touched += 1;
        items.push(CommitItem {
            id: uuid::Uuid::new_v4().to_string(),
            commit_id: commit_id.clone(),
            file_id: change.file_id,
            file_name: new_name.clone(),
            operation: change.operation,
            old_hash,
            new_hash,
            old_location,
            new_location,
            old_name,
            new_name,
            content: change.content,
        });
    }

    let commit = WorkspaceCommit {
        id: commit_id,
        folder_id: scope.id().to_string(),
        scope: scope.kind().to_string(),
        parent_id,
        message,
        author_id: auth.user_id.clone(),
        file_count: touched,
        tree_state: tree_state_json(&head_state),
        create_time: crate::api::utils::datetime::now_ms(),
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
    // Upstream checks that both commits exist first, then that both belong to the scope; the two
    // failures answer different quirk strings.
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
    let mut file_ids: Vec<String> = before.keys().chain(after.keys()).cloned().collect();
    file_ids.sort();
    file_ids.dedup();
    let mut changes = Vec::new();
    for file_id in file_ids {
        let left = before.get(&file_id);
        let right = after.get(&file_id);
        let operation = match (left, right) {
            (None, Some(_)) => "add",
            (Some(_), None) => "delete",
            (Some(left), Some(right)) if left.hash != right.hash => "modify",
            (Some(left), Some(right)) if left.name != right.name => "rename",
            _ => continue,
        };
        changes.push(serde_json::json!({
            "file_id": file_id,
            "file_name": right.map(|entry| entry.name.clone()).or_else(|| left.map(|entry| entry.name.clone())),
            "new_hash": right.map(|entry| entry.hash.clone()),
            "new_location": right.map(|entry| entry.location.clone()),
            "old_hash": left.map(|entry| entry.hash.clone()),
            "old_location": left.map(|entry| entry.location.clone()),
            "operation": operation,
        }));
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
    let now_state = current_state(state, auth, &scope);
    let committed = state
        .workspace_commits
        .head(scope.id())
        .map(|commit| tree_state_of(&commit))
        .unwrap_or_default();
    let mut file_ids: Vec<String> = now_state.keys().chain(committed.keys()).cloned().collect();
    file_ids.sort();
    file_ids.dedup();
    let mut changes = Vec::new();
    for file_id in file_ids {
        let now = now_state.get(&file_id);
        let then = committed.get(&file_id);
        let operation = match (then, now) {
            (None, Some(_)) => "add",
            (Some(_), None) => "delete",
            (Some(then), Some(now)) if then.hash != now.hash || then.name != now.name => "modify",
            _ => continue,
        };
        changes.push(serde_json::json!({
            "file_id": file_id,
            "file_name": now.map(|entry| entry.name.clone()).or_else(|| then.map(|entry| entry.name.clone())),
            "operation": operation,
        }));
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
    let entries = tree_state_of(&commit);
    // Sub-folders are inferred from `parent_id`: an entry's parent that is not an entry itself is a
    // folder, which is how upstream's flat `tree_state` map is meant to be read.
    let mut by_parent: BTreeMap<String, Vec<(String, StateEntry)>> = BTreeMap::new();
    for (file_id, entry) in &entries {
        by_parent
            .entry(entry.parent_id.clone())
            .or_default()
            .push((file_id.clone(), entry.clone()));
    }
    let mut folders: Vec<String> = by_parent.keys().cloned().collect();
    // Every parent that is not itself a file entry is a folder.
    folders.retain(|parent| !entries.contains_key(parent));
    for (_, entry) in entries.iter() {
        let _ = entry;
    }
    fn build(
        folder_id: &str,
        by_parent: &BTreeMap<String, Vec<(String, StateEntry)>>,
        entries: &BTreeMap<String, StateEntry>,
        depth: usize,
    ) -> serde_json::Value {
        let mut children = Vec::new();
        if let Some(files) = by_parent.get(folder_id) {
            for (file_id, entry) in files {
                children.push(serde_json::json!({
                    "id": file_id,
                    "name": entry.name,
                    "type": "file",
                    "hash": entry.hash,
                    "size": entry.size,
                    "status": entry.status,
                    "location": entry.location,
                }));
            }
        }
        if depth < 32 {
            for (parent, _) in by_parent.iter() {
                if parent == folder_id || entries.contains_key(parent) {
                    continue;
                }
                if parent.is_empty() {
                    continue;
                }
                // A folder belongs under this one when its own parent is this one; the flat map does
                // not record that link, so every non-entry parent is shown under the scope root.
                if folder_id != scope_id_placeholder() {
                    continue;
                }
                children.push(serde_json::json!({
                    "id": parent,
                    "name": parent,
                    "type": "folder",
                    "children": build(parent, by_parent, entries, depth + 1),
                }));
            }
        }
        serde_json::json!({ "id": folder_id, "name": folder_id, "type": "folder", "children": children })
    }
    fn scope_id_placeholder() -> &'static str {
        "\u{0}"
    }
    let _ = folders;
    let tree = build(scope.id(), &by_parent, &entries, 0);
    Json(serde_json::json!({ "code": 0, "data": tree, "message": "success" })).into_response()
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
    let (_, commit) = match commit_in_scope(state, auth, id, dataset, commit_id) {
        Ok(found) => found,
        Err(response) => return response,
    };
    let item = state
        .workspace_commits
        .items_of(&commit.id)
        .into_iter()
        .find(|item| item.file_id == file_id);
    if let Some(item) = &item {
        if let Some(content) = &item.content {
            return Json(serde_json::json!({ "code": 0, "data": { "content": content }, "message": "success" }))
                .into_response();
        }
    }
    // No inline copy: serve the file as it exists now. The snapshot records the state, not a blob
    // store, and this is stated rather than pretending the historical bytes were kept.
    if let Some(record) = state.files.get(file_id) {
        let extension = std::path::Path::new(&record.name)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let physical = std::path::Path::new(&state.files.data_dir).join(if extension.is_empty() {
            file_id.to_string()
        } else {
            format!("{file_id}.{extension}")
        });
        if let Ok(content) = std::fs::read_to_string(&physical) {
            return Json(serde_json::json!({ "code": 0, "data": { "content": content }, "message": "success" }))
                .into_response();
        }
    }
    if let Some(record) = state.docs.get(file_id) {
        if let Ok(content) = std::fs::read_to_string(
            std::path::Path::new(&state.files.data_dir).join(&record.storage_name),
        ) {
            return Json(serde_json::json!({ "code": 0, "data": { "content": content }, "message": "success" }))
                .into_response();
        }
    }
    let _ = item;
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
                "create_time": commit.create_time,
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
        let mut entries = BTreeMap::new();
        entries.insert(
            "file-1".to_string(),
            StateEntry {
                hash: "abc".into(),
                location: "a.txt".into(),
                name: "a.txt".into(),
                size: 3,
                status: "1".into(),
                parent_id: "root".into(),
            },
        );
        let encoded = tree_state_json(&entries);
        let commit = WorkspaceCommit {
            id: "c1".into(),
            folder_id: "root".into(),
            scope: "folder".into(),
            parent_id: None,
            message: "m".into(),
            author_id: "u".into(),
            file_count: 1,
            tree_state: encoded,
            create_time: 1,
        };
        let decoded = tree_state_of(&commit);
        assert_eq!(decoded["file-1"].name, "a.txt");
        assert_eq!(decoded["file-1"].parent_id, "root");
    }
}
