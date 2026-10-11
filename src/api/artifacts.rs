//! The sandbox artifacts a code execution produced, and the route that serves them.
//!
//! Upstream keeps collected artifacts in an object-storage bucket and serves
//! `GET /api/v1/documents/artifact/<filename>` from it after checking the name, the extension, and that
//! the artifact belongs to one of the caller's conversations. RayRAG parsed artifacts out of the sandbox
//! response (`code_exec::SandboxResult::artifacts`) and then dropped them: nothing was stored, so there
//! was nothing to serve. This module is the missing half — a store the execution path writes to and the
//! route reads back.
//!
//! Ownership follows upstream's rule as closely as RayRAG's model allows: an artifact belongs to the
//! authenticated user whose run produced it, and only that user (or an administrator) can fetch it.

use std::sync::{Mutex, OnceLock};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use serde_json::{Value, json};

use crate::server::{AppState, AuthContext, api_error_code, code};

/// One collected artifact.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ArtifactRecord {
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub owner_id: String,
    pub created_at: u64,
    /// Base64, as the sandbox reports it. Kept verbatim so what is served is exactly what ran.
    pub content_b64: String,
}

/// A stored artifact without its bytes, for listings.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ArtifactSummary {
    pub name: String,
    pub mime_type: String,
    pub size: u64,
    pub created_at: u64,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ArtifactFile {
    #[serde(default)]
    artifacts: Vec<ArtifactRecord>,
}

/// The process-wide artifact store. The sandbox executor has no handle on `AppState`, so this follows the
/// same shared-store pattern as the one-time-password and captcha stores: one instance per process,
/// persisted to a JSON file and mirrored into PostgreSQL like the other state files.
static DEFAULT_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
pub struct ArtifactStore {
    path: std::path::PathBuf,
    inner: Mutex<ArtifactFile>,
}

impl ArtifactStore {
    /// Cap on how much one artifact may hold, so a runaway script cannot fill the disk through the
    /// artifact path.
    pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
    /// Cap on how many artifacts are kept; the oldest go first.
    pub const MAX_ARTIFACTS: usize = 500;

    pub fn new(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::persistence::restore_if_missing(&path)?;
        let file = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ArtifactFile>(&bytes).ok())
            .unwrap_or_default();
        Ok(Self {
            path,
            inner: Mutex::new(file),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            path: std::path::PathBuf::new(),
            inner: Mutex::new(ArtifactFile::default()),
        }
    }

    /// Process-wide path for [`Self::shared`], so the executor and the API read one file. Set before the
    /// first `shared()` call; later calls are ignored, which keeps a second configuration from silently
    /// splitting the store in two.
    pub fn set_default_path(path: impl AsRef<std::path::Path>) {
        let _ = DEFAULT_PATH.set(path.as_ref().to_path_buf());
    }

    /// The process-wide store. The sandbox executor has no handle on the application state, so it writes
    /// here; the API holds the same instance, which is why this returns the `Arc` rather than a reference.
    pub fn shared() -> std::sync::Arc<ArtifactStore> {
        static STORE: OnceLock<std::sync::Arc<ArtifactStore>> = OnceLock::new();
        STORE
            .get_or_init(|| {
                let path = DEFAULT_PATH.get().cloned().unwrap_or_else(|| {
                    let static_dir =
                        std::env::var("RAYRAG_STATIC_DIR").unwrap_or_else(|_| "web/static".into());
                    std::path::PathBuf::from(static_dir)
                        .parent()
                        .map(|parent| parent.join("sandbox_artifacts.json"))
                        .unwrap_or_else(|| std::path::PathBuf::from("sandbox_artifacts.json"))
                });
                std::sync::Arc::new(
                    ArtifactStore::new(path).unwrap_or_else(|_| ArtifactStore::in_memory()),
                )
            })
            .clone()
    }

    fn persist(&self, file: &ArtifactFile) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        if let Err(error) = crate::persistence::save_json(&self.path, file) {
            tracing::warn!(%error, "the artifact index could not be persisted");
        }
    }

    /// Store one artifact. Returns `false` when it was refused: an unsafe name, an unsupported type, or
    /// more bytes than the cap allows. Refusals are named in the log — a dropped artifact must not look
    /// like a script that produced nothing.
    pub fn put(&self, owner_id: &str, name: &str, mime_type: &str, content_b64: &str) -> bool {
        let name = name.trim();
        if !crate::sandbox::artifact_name_is_safe(name) {
            tracing::warn!(%name, "artifact refused: unsafe name");
            return false;
        }
        if crate::sandbox::mime_type_for_artifact(name).is_none() {
            tracing::warn!(%name, "artifact refused: unsupported type");
            return false;
        }
        if content_b64.is_empty() {
            tracing::warn!(%name, "artifact refused: empty content");
            return false;
        }
        // Decode once here rather than estimating from the base64 length: padding makes the estimate
        // wrong (`YWJjZA==` is four bytes, not six), and a corrupt payload is better refused at the door
        // than discovered at download time.
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(content_b64) else {
            tracing::warn!(%name, "artifact refused: the content is not valid base64");
            return false;
        };
        let decoded_len = decoded.len();
        if decoded_len > Self::MAX_ARTIFACT_BYTES {
            tracing::warn!(%name, size = decoded_len, "artifact refused: too large");
            return false;
        }
        let mut file = self.inner.lock().unwrap();
        file.artifacts.retain(|existing| existing.name != name);
        file.artifacts.push(ArtifactRecord {
            name: name.to_string(),
            mime_type: if mime_type.trim().is_empty() {
                crate::sandbox::mime_type_for_artifact(name)
                    .unwrap_or("application/octet-stream")
                    .to_string()
            } else {
                mime_type.to_string()
            },
            size: decoded_len as u64,
            owner_id: owner_id.to_string(),
            created_at: crate::api::utils::datetime::now_ms(),
            content_b64: content_b64.to_string(),
        });
        // Keep the newest, drop the oldest beyond the cap.
        if file.artifacts.len() > Self::MAX_ARTIFACTS {
            let excess = file.artifacts.len() - Self::MAX_ARTIFACTS;
            file.artifacts.drain(0..excess);
        }
        self.persist(&file);
        true
    }

    /// Store every artifact a sandbox run reported. Returns how many were kept.
    pub fn put_all(&self, owner_id: &str, artifacts: &[Value]) -> usize {
        let mut kept = 0;
        for artifact in artifacts {
            let name = artifact
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let content_b64 = artifact
                .get("content_b64")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mime_type = artifact
                .get("mime_type")
                .and_then(Value::as_str)
                .unwrap_or("");
            if name.is_empty() || content_b64.is_empty() {
                continue;
            }
            if self.put(owner_id, name, mime_type, content_b64) {
                kept += 1;
            }
        }
        kept
    }

    /// Fetch one artifact for a caller. Administrators see every artifact; everyone else sees their own.
    pub fn get_for(&self, caller: &str, is_admin: bool, name: &str) -> Option<ArtifactRecord> {
        let file = self.inner.lock().unwrap();
        file.artifacts
            .iter()
            .find(|artifact| artifact.name == name && (is_admin || artifact.owner_id == caller))
            .cloned()
    }

    /// Delete one artifact. Returns whether anything was removed.
    pub fn delete_for(&self, caller: &str, is_admin: bool, name: &str) -> bool {
        let mut file = self.inner.lock().unwrap();
        let before = file.artifacts.len();
        file.artifacts.retain(|artifact| {
            !(artifact.name == name && (is_admin || artifact.owner_id == caller))
        });
        let removed = file.artifacts.len() != before;
        if removed {
            self.persist(&file);
        }
        removed
    }

    pub fn list_for(&self, caller: &str, is_admin: bool) -> Vec<ArtifactSummary> {
        let file = self.inner.lock().unwrap();
        let mut summaries: Vec<ArtifactSummary> = file
            .artifacts
            .iter()
            .filter(|artifact| is_admin || artifact.owner_id == caller)
            .map(|artifact| ArtifactSummary {
                name: artifact.name.clone(),
                mime_type: artifact.mime_type.clone(),
                size: artifact.size,
                created_at: artifact.created_at,
            })
            .collect();
        summaries.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        summaries
    }
}

impl Default for ArtifactStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// `GET /api/v1/documents/artifact/{filename}`.
///
/// The refusals are upstream's, in upstream's order: the name must be a bare filename, the extension must
/// be one of the served types, and the artifact must exist for this caller.
pub async fn get_artifact(
    State(state): State<std::sync::Arc<AppState>>,
    Path(filename): Path<String>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Response {
    let basename = std::path::Path::new(&filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if basename != filename || filename.contains('/') || filename.contains('\\') {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            "Invalid filename.",
        );
    }
    let Some(mime_type) = crate::sandbox::mime_type_for_artifact(&filename) else {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            "invalid file type",
        );
    };
    let Some(artifact) = state
        .artifacts
        .get_for(&auth.user_id, auth.is_admin, &filename)
    else {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            "Artifact not found.",
        );
    };
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&artifact.content_b64) else {
        // The stored copy is corrupt: say so rather than serving a truncated file.
        return api_error_code(
            StatusCode::INTERNAL_SERVER_ERROR,
            code::OPERATION_ERROR,
            "the stored artifact could not be decoded",
        );
    };
    // The name is sanitised for the header, exactly as upstream does, so a quote cannot break out of it.
    let safe_name: String = filename
        .chars()
        .map(|character| {
            if character.is_alphanumeric()
                || character == '.'
                || character == '-'
                || character == '_'
            {
                character
            } else {
                '_'
            }
        })
        .collect();
    let content_type = if artifact.mime_type.trim().is_empty() {
        mime_type
    } else {
        artifact.mime_type.as_str()
    };
    // A header map rather than a leak: the filename is per-request data.
    let mut headers = axum::http::HeaderMap::new();
    if let Ok(value) = axum::http::HeaderValue::from_str(content_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    if let Ok(value) =
        axum::http::HeaderValue::from_str(&format!("inline; filename=\"{safe_name}\""))
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    (StatusCode::OK, headers, bytes).into_response()
}

/// `GET /api/v1/documents/artifacts` — the caller's collected artifacts.
///
/// Upstream has no listing route for these; RayRAG needs one so the UI can offer a download button per
/// artifact, and inventing a *route* is better than inventing *data*: the list is exactly what the store
/// holds for this caller.
pub async fn list_artifacts(
    State(state): State<std::sync::Arc<AppState>>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Response {
    let items = state.artifacts.list_for(&auth.user_id, auth.is_admin);
    Json(json!({
        "code": 0,
        "data": { "artifacts": items, "total": items.len() },
        "message": "success",
    }))
    .into_response()
}

/// `DELETE /api/v1/documents/artifact/{filename}` — remove one of the caller's artifacts.
pub async fn delete_artifact(
    State(state): State<std::sync::Arc<AppState>>,
    Path(filename): Path<String>,
    axum::Extension(auth): axum::Extension<AuthContext>,
) -> Response {
    if !crate::sandbox::artifact_name_is_safe(&filename) {
        return api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            "Invalid filename.",
        );
    }
    if state
        .artifacts
        .delete_for(&auth.user_id, auth.is_admin, &filename)
    {
        Json(json!({ "code": 0, "data": true, "message": "success" })).into_response()
    } else {
        api_error_code(
            StatusCode::OK,
            code::INVALID_OR_MISSING_DATA,
            "Artifact not found.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> ArtifactStore {
        ArtifactStore::in_memory()
    }

    #[test]
    fn an_artifact_is_stored_read_back_and_scoped_to_its_owner() {
        let store = store();
        assert!(store.put("alice", "chart.png", "image/png", "aGVsbG8="));
        assert_eq!(store.put_all(
            "alice",
            &[
                json!({"name": "report.csv", "mime_type": "text/csv", "content_b64": "YSxi"}),
                json!({"name": "no-bytes.pdf", "mime_type": "application/pdf", "content_b64": ""}),
            ]
        ), 1, "the artifact without bytes is skipped, not stored empty");

        // The owner reads them back with their bytes intact.
        let chart = store.get_for("alice", false, "chart.png").unwrap();
        assert_eq!(chart.content_b64, "aGVsbG8=");
        assert_eq!(chart.mime_type, "image/png");
        assert_eq!(store.list_for("alice", false).len(), 2);

        // Another user does not see them, and an administrator does.
        assert!(store.get_for("bob", false, "chart.png").is_none());
        assert!(store.get_for("bob", true, "chart.png").is_some());
        assert!(store.list_for("bob", false).is_empty());
        assert_eq!(store.list_for("bob", true).len(), 2);

        // Deleting is scoped the same way.
        assert!(!store.delete_for("bob", false, "chart.png"));
        assert!(store.get_for("alice", false, "chart.png").is_some());
        assert!(store.delete_for("alice", false, "chart.png"));
        assert!(store.get_for("alice", false, "chart.png").is_none());
        assert_eq!(store.list_for("alice", false).len(), 1);

        // Storing the same name again replaces it rather than building a duplicate.
        assert!(store.put("alice", "report.csv", "text/csv", "bmV3"));
        let listed = store.list_for("alice", false);
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(
            store
                .get_for("alice", false, "report.csv")
                .unwrap()
                .content_b64,
            "bmV3"
        );
    }

    #[test]
    fn unsafe_names_and_unsupported_types_are_refused() {
        let store = store();
        for name in ["../escape.png", "dir/file.png", "..\\windows.png", ""] {
            assert!(
                !store.put("alice", name, "image/png", "aGVsbG8="),
                "{name:?} must be refused"
            );
        }
        // A safe name with a type RayRAG does not serve is refused too, as upstream refuses it.
        assert!(!store.put("alice", "script.sh", "text/x-sh", "aGVsbG8="));
        assert!(!store.put("alice", "archive.zip", "application/zip", "aGVsbG8="));
        assert!(store.list_for("alice", false).is_empty());
    }

    #[test]
    fn the_stored_size_is_the_decoded_size() {
        let store = store();
        assert!(store.put("alice", "data.json", "application/json", "YWJjZA=="));
        assert_eq!(store.get_for("alice", false, "data.json").unwrap().size, 4);
    }
}
