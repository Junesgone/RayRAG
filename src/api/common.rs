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
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

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
    use base64::Engine as _;

    #[test]
    fn api_constants_re_export_ragflow_semantics() {
        assert_eq!(RetCode::ArgumentError.as_i32(), 101);
        assert_eq!(TaskStatus::Done.as_str(), "3");
        assert_eq!(StatusEnum::Valid.as_str(), "1");
        assert_eq!(ActiveEnum::Active.as_str(), "1");
        assert_eq!(API_VERSION, "v1");
        assert_eq!(NAME_LENGTH_LIMIT, 1024);
        assert_eq!(DATASET_NAME_LIMIT, 128);
        assert_eq!(REQUEST_WAIT_SEC, 2);
        assert_eq!(REQUEST_MAX_WAIT_SEC, 300);
        assert_eq!(MEMORY_SIZE_LIMIT, 10 * 1024 * 1024);
    }

    #[test]
    fn admin_exception_family_maps_to_ragflow_codes() {
        use base64::Engine as _;
        assert_eq!(ApiError::admin("boom").code, 400);
        assert_eq!(ApiError::user_not_found("alice").code, 404);
        assert_eq!(
            ApiError::user_not_found("alice").message,
            "User 'alice' not found"
        );
        assert_eq!(ApiError::user_already_exists("alice").code, 409);
        assert_eq!(
            ApiError::user_already_exists("alice").message,
            "User 'alice' already exists"
        );
        assert_eq!(ApiError::cannot_delete_admin().code, 403);
        assert_eq!(ApiError::not_admin("alice").code, 403);
        assert_eq!(
            ApiError::not_admin("alice").message,
            "User 'alice' is not admin"
        );
    }

    #[test]
    fn base64_encode_matches_ragflow_semantics() {
        assert_eq!(encode_to_base64("hello"), "aGVsbG8=");
        assert_eq!(encode_to_base64(""), "");
        assert_eq!(encode_to_base64("中文"), "5Lit5paH");
        // UTF-8 input round-trips through standard base64 decoding.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encode_to_base64("RAGFlow 知识库"))
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "RAGFlow 知识库");
    }

    #[test]
    fn kb_team_permission_rules() {
        // user-x has joined tenant-a; user-y has not joined anything.
        let joined_x = |tenant: &str| tenant == "tenant-a";
        // Owner always passes.
        assert!(check_kb_team_permission(
            "tenant-a", "team", "tenant-a", joined_x
        ));
        assert!(check_kb_team_permission(
            "tenant-a", "me", "tenant-a", joined_x
        ));
        // Non-team permission blocks cross-tenant access even when joined.
        assert!(!check_kb_team_permission(
            "tenant-a", "me", "user-x", joined_x
        ));
        // Team permission + joined owning tenant passes.
        assert!(check_kb_team_permission(
            "tenant-a", "team", "user-x", joined_x
        ));
        // Team permission without membership is denied.
        assert!(!check_kb_team_permission(
            "tenant-a",
            "team",
            "user-y",
            |_| false
        ));
    }

    #[test]
    fn file_team_permission_follows_linked_kbs() {
        let kbs = |file_id: &str| match file_id {
            "file-1" => vec!["kb-a".into()],
            "file-2" => vec!["kb-a".into(), "kb-b".into()],
            _ => vec![],
        };
        let kb_info = |kb_id: &str| match kb_id {
            "kb-a" => Some(("tenant-a".into(), "me".into())),
            "kb-b" => Some(("tenant-b".into(), "team".into())),
            _ => None,
        };
        let joined = |tenant: &str| tenant == "tenant-b";
        // Owner passes regardless of KB permissions.
        assert!(check_file_team_permission(
            "tenant-a", "file-1", "tenant-a", &kbs, &kb_info, joined
        ));
        // Cross-tenant: only a team-permissioned linked KB grants access.
        assert!(!check_file_team_permission(
            "tenant-a", "file-1", "user-x", &kbs, &kb_info, joined
        ));
        assert!(check_file_team_permission(
            "tenant-a", "file-2", "user-x", &kbs, &kb_info, joined
        ));
        // No linked KBs → denied.
        assert!(!check_file_team_permission(
            "tenant-a", "file-3", "user-x", &kbs, &kb_info, joined
        ));
    }
}
