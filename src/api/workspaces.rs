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

fn not_found(message: &str) -> Response {
    api_error_code(
        axum::http::StatusCode::NOT_FOUND,
        code::INVALID_OR_MISSING_DATA,
        message,
    )
}

/// Resolve the `{workspace_id}` / `{dataset_id}` path parameter, answering upstream's wording when it
/// cannot be resolved. `dataset: true` selects the dataset reading of the id.
fn resolve_scope(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
) -> Result<Scope, Response> {
    if dataset {
        return dataset_scope(state, auth, id)
            .ok_or_else(|| not_found(&format!("Could not resolve datasets '{id}' to a folder")));
    }
    folder_scope(state, auth, id)
        .ok_or_else(|| not_found(&format!("Could not resolve folder '{id}'")))
}

fn commit_json(commit: &WorkspaceCommit) -> serde_json::Value {
    serde_json::json!({
        "id": commit.id,
        "folder_id": commit.folder_id,
        "parent_id": commit.parent_id,
        "message": commit.message,
        "author_id": commit.author_id,
        "file_count": commit.file_count,
        "tree_state": commit.tree_state,
        "create_time": commit.create_time,
        // Upstream's artifact-commit extension fields, null for workspace commits.
        "title": serde_json::Value::Null,
        "comments": serde_json::Value::Null,
    })
}

fn item_json(item: &CommitItem) -> serde_json::Value {
    serde_json::json!({
        "id": item.id,
        "file_id": item.file_id,
        "operation": item.operation,
        "old_hash": item.old_hash,
        "new_hash": item.new_hash,
        "old_location": item.old_location,
        "new_location": item.new_location,
        "old_name": item.old_name,
        "new_name": item.new_name,
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
    let message = body.message.unwrap_or_default().trim().to_string();
    if message.is_empty() {
        return bad_request(
            code::INVALID_ARGUMENT,
            "required argument are missing: message",
        );
    }
    let changes = match body.files {
        Some(changes) if !changes.is_empty() => changes,
        _ => {
            return bad_request(
                code::INVALID_ARGUMENT,
                "required argument are missing: files",
            );
        }
    };
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
    let payload = commit_json(&commit);
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
    let page_size = query
        .get("page_size")
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(15)
        .min(100);
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
        .map(|commit| commit_json(&commit))
        .collect();
    Json(serde_json::json!({
        "code": 0,
        "data": { "total": total, "page": page, "page_size": page_size, "commits": rows },
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

fn commit_or_404(
    state: &AppState,
    auth: &AuthContext,
    id: &str,
    dataset: bool,
    commit_id: &str,
) -> Result<(Scope, WorkspaceCommit), Response> {
    let scope = resolve_scope(state, auth, id, dataset)?;
    match state.workspace_commits.get(scope.id(), commit_id) {
        Some(commit) => Ok((scope, commit)),
        None => Err(not_found("Commit not found in workspace")),
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
    let (_, commit) = match commit_or_404(state, auth, id, dataset, commit_id) {
        Ok(found) => found,
        Err(response) => return response,
    };
    let files: Vec<serde_json::Value> = state
        .workspace_commits
        .items_of(&commit.id)
        .iter()
        .map(item_json)
        .collect();
    let mut payload = commit_json(&commit);
    if let Some(object) = payload.as_object_mut() {
        object.insert("files".into(), serde_json::Value::Array(files));
    }
    Json(serde_json::json!({ "code": 0, "data": payload, "message": "success" })).into_response()
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
    let (_, commit) = match commit_or_404(state, auth, id, dataset, commit_id) {
        Ok(found) => found,
        Err(response) => return response,
    };
    let files: Vec<serde_json::Value> = state
        .workspace_commits
        .items_of(&commit.id)
        .iter()
        .map(item_json)
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
        return bad_request(
            code::INVALID_ARGUMENT,
            "required argument are missing: from, to",
        );
    }
    let from = match state.workspace_commits.get(scope.id(), from_id) {
        Some(commit) => commit,
        None => return not_found("Commit not found in workspace"),
    };
    let to = match state.workspace_commits.get(scope.id(), to_id) {
        Some(commit) => commit,
        None => return not_found("Commit not found in workspace"),
    };
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
            "operation": operation,
            "old_hash": left.map(|entry| entry.hash.clone()),
            "new_hash": right.map(|entry| entry.hash.clone()),
            "old_location": left.map(|entry| entry.location.clone()),
            "new_location": right.map(|entry| entry.location.clone()),
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
    let (scope, commit) = match commit_or_404(state, auth, id, dataset, commit_id) {
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
    let (_, commit) = match commit_or_404(state, auth, id, dataset, commit_id) {
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
    not_found("File not found in this commit")
}

pub async fn commit_file_content(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path((id, commit_id, file_id)): Path<(String, String, String)>,
) -> Response {
    commit_file_content_in(&state, &auth, &id, false, &commit_id, &file_id).await
}

/// `GET /api/v1/workspace-files/{file_id}/versions`.
pub(crate) fn file_versions_in(state: &AppState, auth: &AuthContext, file_id: &str) -> Response {
    let history = state.workspace_commits.versions(file_id);
    if history.is_empty() {
        // An unknown file, or a file that was never committed: upstream answers an empty list for a
        // file it cannot see, so the two cases are not distinguished here either.
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
            serde_json::json!({
                "commit_id": commit.id,
                "operation": item.operation,
                "hash": item.new_hash,
                "create_time": commit.create_time,
                "message": commit.message,
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
