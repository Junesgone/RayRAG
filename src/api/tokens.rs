//! RAGFlow public API tokens — `api/apps/restful_apis/system_api.py`
//! (`list_token` / `new_token` / `rm`) and the `APIToken` row they persist.
//!
//! RAGFlow stores one row per issued token with a composite `(tenant_id, token)`
//! key plus the sibling `beta` secret:
//!
//! ```text
//! tenant_id, token, dialog_id, source, beta, create_time, create_date,
//! update_time, update_date
//! ```
//!
//! `beta` is the credential the embedded/share surfaces use: the embed URL
//! carries `?auth=<beta>`, `utils/request.ts` sends it as
//! `Authorization: Bearer <beta>`, and `login_required(auth_types=AUTH_BETA)`
//! resolves it back to the owning user through `APIToken.query(beta=...)`.
//!
//! Two upstream behaviours are load-bearing and reproduced here:
//! * `list_token` **backfills** a missing `beta` on every listed row (legacy
//!   rows predate the column) and persists it, because the frontend's embed
//!   flow refuses to open without a `beta` value;
//! * `list_token`/`new_token` resolve the **owner** tenant
//!   (`[tenant for tenant in tenants if tenant.role == "owner"][0]`), while
//!   `rm` uses `tenants[0]` — RayRAG reads the owner tenant in all three and
//!   records the deviation, since upstream's `tenants[0]` is an unordered pick
//!   from the same membership set and the two coincide for a personal account.

use crate::server::{AppState, AuthContext};
use anyhow::Context;
use axum::{
    Json,
    extract::{Extension, Path, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

/// `APIToken` row (`api/db/db_models.py`). `update_time`/`update_date` stay
/// null on a freshly issued token, exactly like the `new_token` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiToken {
    pub tenant_id: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub beta: String,
    pub create_time: u64,
    pub create_date: String,
    #[serde(default)]
    pub update_time: Option<u64>,
    #[serde(default)]
    pub update_date: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApiTokenSnapshot {
    api_tokens: Vec<ApiToken>,
}

pub struct ApiTokenStore {
    tokens: RwLock<Vec<ApiToken>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl ApiTokenStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = PathBuf::from(path);
        crate::persistence::restore_if_missing(&path)?;
        let tokens = if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("Failed to read API tokens '{}':", path.display()))?;
            serde_json::from_slice::<ApiTokenSnapshot>(&bytes)
                .with_context(|| format!("Failed to parse API tokens '{}':", path.display()))?
                .api_tokens
        } else {
            Vec::new()
        };
        let store = Self {
            tokens: RwLock::new(tokens),
            path: Some(path),
            save_lock: Mutex::new(()),
        };
        store.persist_current()?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            tokens: RwLock::new(Vec::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    fn persist(&self, tokens: &[ApiToken]) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        crate::persistence::atomic_write(
            path,
            &serde_json::to_vec_pretty(&ApiTokenSnapshot {
                api_tokens: tokens.to_vec(),
            })?,
        )
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let tokens = self.tokens.read().unwrap().clone();
        self.persist(&tokens)
    }

    /// Rows of one tenant in insertion order (upstream queries by `tenant_id`).
    pub fn list_for_tenant(&self, tenant_id: &str) -> Vec<ApiToken> {
        self.tokens
            .read()
            .unwrap()
            .iter()
            .filter(|token| token.tenant_id == tenant_id)
            .cloned()
            .collect()
    }

    /// `APIToken.query(beta=<value>)` — the AUTH_BETA lookup.
    pub fn tenant_for_beta(&self, beta: &str) -> Option<String> {
        if beta.is_empty() {
            return None;
        }
        self.tokens
            .read()
            .unwrap()
            .iter()
            .find(|token| token.beta == beta)
            .map(|token| token.tenant_id.clone())
    }

    pub fn insert(&self, token: ApiToken) -> anyhow::Result<ApiToken> {
        let mut guard = self.tokens.write().unwrap();
        guard.push(token.clone());
        let snapshot = guard.clone();
        drop(guard);
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(&snapshot)?;
        Ok(token)
    }

    /// `list_token` backfill: rewrite every row that has no `beta` yet.
    pub fn backfill_beta(&self, rows: &mut [ApiToken]) -> anyhow::Result<()> {
        let mut updated: Vec<ApiToken> = Vec::new();
        for row in rows.iter_mut() {
            if !row.beta.is_empty() {
                continue;
            }
            row.beta = new_beta();
            updated.push(row.clone());
        }
        if updated.is_empty() {
            return Ok(());
        }
        {
            let mut guard = self.tokens.write().unwrap();
            for row in &updated {
                if let Some(existing) = guard
                    .iter_mut()
                    .find(|token| token.tenant_id == row.tenant_id && token.token == row.token)
                {
                    existing.beta = row.beta.clone();
                }
            }
        }
        let snapshot = self.tokens.read().unwrap().clone();
        let _save_guard = self.save_lock.lock().unwrap();
        self.persist(&snapshot)
    }

    /// `APITokenService.filter_delete([tenant_id == t, token == token])`.
    pub fn delete(&self, tenant_id: &str, token: &str) -> anyhow::Result<bool> {
        let mut guard = self.tokens.write().unwrap();
        let before = guard.len();
        guard.retain(|row| !(row.tenant_id == tenant_id && row.token == token));
        let removed = guard.len() != before;
        let snapshot = guard.clone();
        drop(guard);
        if removed {
            let _save_guard = self.save_lock.lock().unwrap();
            self.persist(&snapshot)?;
        }
        Ok(removed)
    }
}

impl Default for ApiTokenStore {
    fn default() -> Self {
        Self::in_memory()
    }
}

/// `generate_confirmation_token()` — upstream mints `ragflow-<urlsafe token>`.
fn new_token() -> String {
    format!("ragflow-{}", uuid::Uuid::new_v4().simple())
}

/// `generate_confirmation_token().replace("ragflow-", "")[:32]` — the beta
/// secret is the first 32 characters of the token without its prefix.
fn new_beta() -> String {
    let digest = uuid::Uuid::new_v4().simple().to_string().replace('-', "");
    digest.repeat(2).chars().take(32).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or_default()
}

fn now_date() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `get_data_error_result(message="Tenant not found!")`.
fn tenant_not_found() -> Response {
    Json(serde_json::json!({
        "code": 102,
        "message": "Tenant not found!",
        "data": null,
    }))
    .into_response()
}

/// The **owner** tenant of the calling user, mirroring
/// `[tenant for tenant in tenants if tenant.role == "owner"][0]`. RayRAG's
/// `TenantStore::list_for_user` always publishes the personal tenant, so this
/// only fails when a caller has no membership at all.
fn owner_tenant(state: &AppState, user_id: &str) -> Option<String> {
    let memberships = state.tenants.list_for_user(user_id);
    memberships
        .iter()
        .find(|membership| membership.role == crate::kb::TenantRole::Owner)
        .or_else(|| memberships.first())
        .map(|membership| membership.tenant_id.clone())
}

/// `GET /api/v1/system/tokens` — the caller tenant's API tokens, with the
/// upstream `beta` backfill for rows created before the column existed.
pub async fn list_tokens(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    let Some(tenant_id) = owner_tenant(&state, &auth.user_id) else {
        return tenant_not_found();
    };
    let mut rows = state.api_tokens.list_for_tenant(&tenant_id);
    if let Err(error) = state.api_tokens.backfill_beta(&mut rows) {
        return Json(serde_json::json!({
            "code": 500,
            "message": error.to_string(),
            "data": null,
        }))
        .into_response();
    }
    Json(serde_json::json!({ "code": 0, "data": rows })).into_response()
}

/// `POST /api/v1/system/tokens` — issue a token plus its beta secret. Upstream
/// reads no request body here (the frontend's optional `{canvasId}` payload is
/// ignored in this release), so neither does RayRAG.
pub async fn create_token(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    let Some(tenant_id) = owner_tenant(&state, &auth.user_id) else {
        return tenant_not_found();
    };
    let token = ApiToken {
        tenant_id,
        token: new_token(),
        dialog_id: None,
        source: None,
        beta: new_beta(),
        create_time: now_ms(),
        create_date: now_date(),
        update_time: None,
        update_date: None,
    };
    match state.api_tokens.insert(token) {
        // `if not APITokenService.save(**obj): return get_data_error_result(message="Fail to new a dialog!")`
        Ok(row) => Json(serde_json::json!({ "code": 0, "data": row })).into_response(),
        Err(error) => Json(serde_json::json!({
            "code": 102,
            "message": format!("Fail to new a dialog! {error}"),
            "data": null,
        }))
        .into_response(),
    }
}

/// `DELETE /api/v1/system/tokens/{token}` — remove one of the caller tenant's
/// tokens and answer `true`.
pub async fn delete_token(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(token): Path<String>,
) -> Response {
    let Some(tenant_id) = owner_tenant(&state, &auth.user_id) else {
        return tenant_not_found();
    };
    match state.api_tokens.delete(&tenant_id, &token) {
        Ok(_) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Err(error) => Json(serde_json::json!({
            "code": 500,
            "message": error.to_string(),
            "data": null,
        }))
        .into_response(),
    }
}

/// Request body of the personal-API-key fixture (`POST /system/api_keys`).
/// Upstream's token endpoint has no body; RayRAG accepts this shape only for the
/// legacy alias so older clients keep working.
#[derive(Debug, Default, Deserialize)]
pub struct LegacyApiKeyRequest {
    #[serde(default)]
    pub name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_secret_matches_the_upstream_shape() {
        let beta = new_beta();
        assert_eq!(beta.len(), 32);
        assert!(beta.chars().all(|value| value.is_ascii_hexdigit()));

        let token = new_token();
        assert!(token.starts_with("ragflow-"));
        assert_eq!(token.len(), "ragflow-".len() + 32);
    }

    #[test]
    fn backfill_generates_and_persists_a_missing_beta() {
        let store = ApiTokenStore::in_memory();
        store
            .insert(ApiToken {
                tenant_id: "tenant-a".into(),
                token: "ragflow-legacy".into(),
                dialog_id: None,
                source: None,
                beta: String::new(),
                create_time: 1,
                create_date: "2026-01-01 00:00:00".into(),
                update_time: None,
                update_date: None,
            })
            .unwrap();
        let mut rows = store.list_for_tenant("tenant-a");
        assert_eq!(rows[0].beta, "");
        store.backfill_beta(&mut rows).unwrap();
        assert_eq!(rows[0].beta.len(), 32);
        // The backfill is persisted, so the beta survives a fresh read and the
        // AUTH_BETA lookup resolves the tenant.
        let stored = store.list_for_tenant("tenant-a");
        assert_eq!(stored[0].beta, rows[0].beta);
        assert_eq!(
            store.tenant_for_beta(&rows[0].beta).as_deref(),
            Some("tenant-a")
        );
    }

    #[test]
    fn tokens_are_scoped_to_their_tenant() {
        let store = ApiTokenStore::in_memory();
        let row = |tenant: &str, token: &str, beta: &str| ApiToken {
            tenant_id: tenant.into(),
            token: token.into(),
            dialog_id: None,
            source: None,
            beta: beta.into(),
            create_time: 1,
            create_date: "2026-01-01 00:00:00".into(),
            update_time: None,
            update_date: None,
        };
        store.insert(row("tenant-a", "token-a", "beta-a")).unwrap();
        store.insert(row("tenant-b", "token-b", "beta-b")).unwrap();
        assert_eq!(store.list_for_tenant("tenant-a").len(), 1);
        assert_eq!(store.tenant_for_beta("beta-b").as_deref(), Some("tenant-b"));
        assert!(store.tenant_for_beta("").is_none());
        assert!(store.tenant_for_beta("unknown").is_none());
        // Deleting through the wrong tenant leaves the row in place.
        assert!(!store.delete("tenant-b", "token-a").unwrap());
        assert_eq!(store.list_for_tenant("tenant-a").len(), 1);
        assert!(store.delete("tenant-a", "token-a").unwrap());
        assert!(store.list_for_tenant("tenant-a").is_empty());
    }
}
