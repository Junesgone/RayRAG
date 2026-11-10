//! Common API semantics — ported from RAGFlow `api/common/`:
//!
//! - `exceptions.py`: the `AdminException` family of errors with RAGFlow's
//!   HTTP error-code convention (`{"code": <http status>, "message": ...}` on
//!   failure, `{"code": 0, "data": ...}` on success).
//! - `base64.py`: `encode_to_base64` UTF-8 → standard base64.
//! - `check_team_permission.py`: knowledge-base / file team-visibility rules
//!   used before cross-tenant access is granted.
//!
//! RayRAG previously answered every error with ad-hoc inline JSON; this module
//! is the single place that maps failures to the RAGFlow error vocabulary so
//! handlers and services can return a typed `ApiError`.

use axum::{
    Json,
    body::Body,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use std::path::Path;

/// Read size for a streamed file response.
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// Serve a stored file by **streaming** it, one chunk at a time.
///
/// Reading a whole upload into memory per request made a response as expensive as
/// the file: previewing a 2 GB video cost 2 GB of resident memory for a single
/// viewer, and concurrent viewers multiplied that. The bytes are now read as the
/// client consumes them, so memory stays flat regardless of file size.
///
/// A missing or empty file answers upstream's `This file is empty.` payload, the
/// same shape the buffered version returned.
pub async fn stream_stored_file(
    path: &Path,
    content_type: &str,
    disposition: &str,
    empty_message: &str,
) -> Response {
    let empty = match tokio::fs::metadata(path).await {
        Ok(metadata) if metadata.len() > 0 => false,
        Ok(_) => true,
        Err(_) => true,
    };
    if empty {
        return Json(serde_json::json!({ "code": 404, "message": empty_message })).into_response();
    }
    let Ok(file) = tokio::fs::File::open(path).await else {
        return Json(serde_json::json!({ "code": 404, "message": empty_message })).into_response();
    };
    let stream = futures_util::stream::unfold(file, |mut file| async move {
        let mut buffer = vec![0u8; STREAM_CHUNK_BYTES];
        match tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                Some((Ok::<_, std::io::Error>(buffer), file))
            }
            Err(error) => Some((Err(error), file)),
        }
    });
    (
        [
            (axum::http::header::CONTENT_TYPE, content_type.to_string()),
            (
                axum::http::header::CONTENT_DISPOSITION,
                disposition.to_string(),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

/// RAGFlow `api/common/exceptions.py` error families. Every variant carries
/// the HTTP code RAGFlow's `AdminException` subclass assigns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// `AdminException` — generic administrative error, default code 400.
    Admin,
    /// `UserNotFoundError` — 404.
    UserNotFound,
    /// `UserAlreadyExistsError` — 409.
    UserAlreadyExists,
    /// `CannotDeleteAdminError` — 403.
    CannotDeleteAdmin,
    /// `NotAdminError` — 403.
    NotAdmin,
}

impl ApiErrorKind {
    pub fn http_code(self) -> u16 {
        match self {
            Self::Admin => 400,
            Self::UserNotFound => 404,
            Self::UserAlreadyExists => 409,
            Self::CannotDeleteAdmin | Self::NotAdmin => 403,
        }
    }
}

/// Unified API error mirroring RAGFlow's `AdminException(message, code)`.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub kind: ApiErrorKind,
    pub code: u16,
    pub message: String,
}

impl ApiError {
    pub fn new(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            kind,
            code: kind.http_code(),
            message,
        }
    }

    /// `AdminException(message)` — default code 400.
    pub fn admin(message: impl Into<String>) -> Self {
        Self::new(ApiErrorKind::Admin, message)
    }

    /// `UserNotFoundError(username)` — `"User '{username}' not found"`, 404.
    pub fn user_not_found(username: &str) -> Self {
        Self::new(
            ApiErrorKind::UserNotFound,
            format!("User '{username}' not found"),
        )
    }

    /// `UserAlreadyExistsError(username)` — `"User '{username}' already exists"`, 409.
    pub fn user_already_exists(username: &str) -> Self {
        Self::new(
            ApiErrorKind::UserAlreadyExists,
            format!("User '{username}' already exists"),
        )
    }

    /// `CannotDeleteAdminError` — `"Cannot delete admin account"`, 403.
    pub fn cannot_delete_admin() -> Self {
        Self::new(
            ApiErrorKind::CannotDeleteAdmin,
            "Cannot delete admin account",
        )
    }

    /// `NotAdminError(username)` — `"User '{username}' is not admin"`, 403.
    pub fn not_admin(username: &str) -> Self {
        Self::new(
            ApiErrorKind::NotAdmin,
            format!("User '{username}' is not admin"),
        )
    }

    /// Internal failure — RAGFlow surfaces these as HTTP 500.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: ApiErrorKind::Admin,
            code: 500,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!("api error: {error:#}");
        Self::internal(error.to_string())
    }
}

/// RAGFlow failure envelope: `{"code": <http status>, "message": <text>}`.
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (
            status,
            Json(serde_json::json!({
                "code": self.code,
                "message": self.message,
            })),
        )
            .into_response()
    }
}

/// RAGFlow success envelope: `{"code": 0, "data": ...}`.
pub fn ok_json<T: Serialize>(data: T) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "code": 0, "data": data }))
}

/// `common/base64.py::encode_to_base64` — UTF-8 → standard base64 (RFC 4648).
pub fn encode_to_base64(input: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(input.as_bytes())
}

/// `api/db/__init__.py::TenantPermission` — `me` / `team`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamPermission {
    Me,
    Team,
}

impl TeamPermission {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Me => "me",
            Self::Team => "team",
        }
    }
}

impl From<&str> for TeamPermission {
    fn from(value: &str) -> Self {
        match value {
            "team" => Self::Team,
            _ => Self::Me,
        }
    }
}

/// `check_team_permission.py::check_kb_team_permission` — a knowledge base is
/// visible to `other` when it is owned by `other`, or when it is `team`
/// permissioned and `other` has joined the owning tenant.
///
/// `is_joined_tenant(tenant_id)` decides membership in the owning tenant
/// (RayRAG resolves it from its tenant-membership store; RAGFlow calls
/// `TenantService.get_joined_tenants_by_user_id`).
pub fn check_kb_team_permission(
    kb_tenant_id: &str,
    kb_permission: &str,
    other_user_id: &str,
    is_joined_tenant: impl Fn(&str) -> bool,
) -> bool {
    if kb_tenant_id == other_user_id {
        return true;
    }
    if kb_permission != TeamPermission::Team.as_str() {
        return false;
    }
    is_joined_tenant(kb_tenant_id)
}

/// `check_team_permission.py::check_file_team_permission` — a file is visible
/// to `other` when it lives in `other`'s tenant, or when any knowledge base it
/// is linked to passes [`check_kb_team_permission`].
///
/// `kb_ids_for_file(file_id)` returns the knowledge-base ids linked to the
/// file (`FileService.get_kb_id_by_file_id`), and `kb_info(kb_id)` returns
/// `(tenant_id, permission)` for a knowledge base.
pub fn check_file_team_permission(
    file_tenant_id: &str,
    file_id: &str,
    other_user_id: &str,
    kb_ids_for_file: impl Fn(&str) -> Vec<String>,
    kb_info: impl Fn(&str) -> Option<(String, String)>,
    is_joined_tenant: impl Fn(&str) -> bool,
) -> bool {
    if file_tenant_id == other_user_id {
        return true;
    }
    for kb_id in kb_ids_for_file(file_id) {
        let Some((kb_tenant_id, kb_permission)) = kb_info(&kb_id) else {
            continue;
        };
        if check_kb_team_permission(
            &kb_tenant_id,
            &kb_permission,
            other_user_id,
            &is_joined_tenant,
        ) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// RAGFlow constants re-exports (common/constants.py + api/constants.py)
// ---------------------------------------------------------------------------
// The canonical definitions live in `crate::common::constants`; these
// re-exports give API handlers the same short names RAGFlow uses without
// re-defining the tables here. `FILE_NAME_LEN_LIMIT` / `IMG_BASE64_PREFIX`
// already live in `api::utils` (see its coverage map).

pub use crate::common::constants::{
    API_VERSION, ActiveEnum, DATASET_NAME_LIMIT, MEMORY_NAME_LIMIT, MEMORY_SIZE_LIMIT,
    NAME_LENGTH_LIMIT, REQUEST_MAX_WAIT_SEC, REQUEST_WAIT_SEC, RetCode, StatusEnum, TaskStatus,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// A streamed response must deliver every byte of a file larger than the read
    /// chunk, and must answer the upstream empty-file payload when there is nothing
    /// to send. The previous buffered version had no such failure mode; the
    /// streaming one does, so it is pinned here.
    #[tokio::test]
    async fn stream_stored_file_delivers_large_files_and_reports_empty_ones() {
        let dir = std::env::temp_dir().join(format!("rayrag-stream-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let large = dir.join("large.bin");
        let size = STREAM_CHUNK_BYTES * 3 + 1234;
        let payload: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
        std::fs::write(&large, &payload).unwrap();

        let response =
            stream_stored_file(&large, "application/octet-stream", "inline", "empty").await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .unwrap(),
            "application/octet-stream"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.len(), size, "every chunk must arrive");
        assert_eq!(
            &body[..],
            &payload[..],
            "streamed bytes must match the file"
        );

        let empty = dir.join("empty.bin");
        std::fs::write(&empty, b"").unwrap();
        let response =
            stream_stored_file(&empty, "text/plain", "inline", "This file is empty.").await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["message"], "This file is empty.");

        let missing = dir.join("missing.bin");
        let response =
            stream_stored_file(&missing, "text/plain", "inline", "This file is empty.").await;
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["message"], "This file is empty.");
        std::fs::remove_dir_all(dir).ok();
    }
}
