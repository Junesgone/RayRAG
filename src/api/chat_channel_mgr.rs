//! Tenant-scoped chat channel bot management.
//!
//! Mirrors the fixed RAGFlow v0.26.4 `chat_channel_api.py` wire contract:
//! `GET/POST /api/v1/chat-channels` and `GET/PATCH/DELETE
//! /api/v1/chat-channels/{id}`. Each record carries `{id, tenant_id, name,
//! channel, config, chat_id}` where `config.credential.*` holds the
//! channel-specific secrets. Runtime bot processes stay the responsibility of
//! the existing `channels` registry; this store only owns the configuration.

use crate::server::{AppState, AuthContext};
use anyhow::Context;
use axum::{
    Json,
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatChannelRecord {
    pub id: String,
    pub tenant_id: String,
    pub name: String,
    pub channel: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default)]
    pub updated_at: u64,
}

pub struct ChatChannelStore {
    rows: RwLock<Vec<ChatChannelRecord>>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
}

impl ChatChannelStore {
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let path = PathBuf::from(path);
        crate::persistence::restore_if_missing(&path)?;
        let rows =
            crate::persistence::load_json::<Vec<ChatChannelRecord>>(&path)?.unwrap_or_default();
        Ok(Self {
            rows: RwLock::new(rows),
            path: Some(path),
            save_lock: Mutex::new(()),
        })
    }

    pub fn in_memory() -> Self {
        Self {
            rows: RwLock::new(Vec::new()),
            path: None,
            save_lock: Mutex::new(()),
        }
    }

    pub fn list(&self, tenant_id: &str) -> Vec<ChatChannelRecord> {
        self.rows
            .read()
            .unwrap()
            .iter()
            .filter(|row| row.tenant_id == tenant_id)
            .cloned()
            .collect()
    }

    pub fn get(&self, tenant_id: &str, id: &str) -> Option<ChatChannelRecord> {
        self.rows
            .read()
            .unwrap()
            .iter()
            .find(|row| row.tenant_id == tenant_id && row.id == id)
            .cloned()
    }

    pub fn insert(&self, row: ChatChannelRecord) -> anyhow::Result<()> {
        {
            let mut rows = self.rows.write().unwrap();
            rows.retain(|existing| !(existing.tenant_id == row.tenant_id && existing.id == row.id));
            rows.push(row);
        }
        self.persist()
    }

    pub fn update(
        &self,
        tenant_id: &str,
        id: &str,
        row: ChatChannelRecord,
    ) -> anyhow::Result<bool> {
        let changed = {
            let mut rows = self.rows.write().unwrap();
            match rows
                .iter_mut()
                .find(|existing| existing.tenant_id == tenant_id && existing.id == id)
            {
                Some(slot) => {
                    *slot = row;
                    true
                }
                None => false,
            }
        };
        if changed {
            self.persist()?;
        }
        Ok(changed)
    }

    pub fn delete(&self, tenant_id: &str, id: &str) -> anyhow::Result<bool> {
        let removed = {
            let mut rows = self.rows.write().unwrap();
            let before = rows.len();
            rows.retain(|row| !(row.tenant_id == tenant_id && row.id == id));
            rows.len() != before
        };
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }

    fn persist(&self) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let _guard = self.save_lock.lock().unwrap();
        let rows: Vec<_> = self.rows.read().unwrap().clone();
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(&rows)?)
            .with_context(|| format!("Failed to persist chat channels '{}':", path.display()))
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateChatChannelRequest {
    pub name: String,
    pub channel: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub chat_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateChatChannelRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub chat_id: Option<String>,
}

fn channel_error(message: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "code": 102, "message": message })),
    )
        .into_response()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// GET `/api/v1/chat-channels` — the tenant's bot list.
pub async fn list_chat_channels(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Response {
    Json(serde_json::json!({
        "code": 0,
        "data": state.chat_channels.list(&auth.user_id)
    }))
    .into_response()
}

/// POST `/api/v1/chat-channels` — create a bot for the tenant.
pub async fn create_chat_channel(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<CreateChatChannelRequest>,
) -> Response {
    let name = request.name.trim();
    if name.is_empty() {
        return channel_error("Name is required");
    }
    let channel = request.channel.trim();
    if channel.is_empty() {
        return channel_error("Channel is required");
    }
    let now = now_ms();
    let row = ChatChannelRecord {
        id: uuid::Uuid::new_v4().to_string(),
        tenant_id: auth.user_id.clone(),
        name: name.to_string(),
        channel: channel.to_string(),
        config: request.config,
        chat_id: request.chat_id,
        created_at: now,
        updated_at: now,
    };
    match state.chat_channels.insert(row.clone()) {
        Ok(()) => Json(serde_json::json!({ "code": 0, "data": row })).into_response(),
        Err(error) => channel_error(&error.to_string()),
    }
}

/// GET `/api/v1/chat-channels/{id}`.
pub async fn get_chat_channel(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    match state.chat_channels.get(&auth.user_id, &id) {
        Some(row) => Json(serde_json::json!({ "code": 0, "data": row })).into_response(),
        None => channel_error(&format!("Can't find this chat channel! ({id})")),
    }
}

/// GET `/api/v1/chat-channels/{id}/runtime` — live runtime metadata for a
/// running chat channel (upstream `get_chat_channel_runtime`): WhatsApp-only,
/// `waiting` status with no QR until a runtime session exists (RayRAG does not
/// host the WhatsApp websocket session, so the snapshot is the fixed
/// not-started shape).
pub async fn get_chat_channel_runtime(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    match state.chat_channels.get(&auth.user_id, &id) {
        Some(row) if row.channel == "whatsapp" => Json(serde_json::json!({
            "code": 0,
            "data": {
                "account_id": row.id,
                "session_key": row.id,
                "status": "waiting",
                "connected_at": null,
                "qr_updated_at": null,
                "qr_data_url": null,
                "last_error": null,
                "session_id": null,
                "last_snapshot_at": null
            }
        })).into_response(),
        Some(_) => channel_error("Runtime snapshot is only available for WhatsApp."),
        None => channel_error(&format!("Can't find this chat channel! ({id})")),
    }
}

/// PATCH `/api/v1/chat-channels/{id}` — name/config/chat_id.
pub async fn update_chat_channel(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(request): Json<UpdateChatChannelRequest>,
) -> Response {
    let Some(existing) = state.chat_channels.get(&auth.user_id, &id) else {
        return channel_error(&format!("Can't find this chat channel! ({id})"));
    };
    let name = request
        .name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or(existing.name);
    let row = ChatChannelRecord {
        id: id.clone(),
        tenant_id: existing.tenant_id,
        name,
        channel: existing.channel,
        config: request.config.unwrap_or(existing.config),
        chat_id: match request.chat_id {
            Some(chat_id) if !chat_id.is_empty() => Some(chat_id),
            _ => existing.chat_id,
        },
        created_at: existing.created_at,
        updated_at: now_ms(),
    };
    match state.chat_channels.update(&auth.user_id, &id, row.clone()) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": row })).into_response(),
        Ok(false) => channel_error(&format!("Can't find this chat channel! ({id})")),
        Err(error) => channel_error(&error.to_string()),
    }
}

/// DELETE `/api/v1/chat-channels/{id}`.
pub async fn delete_chat_channel(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> Response {
    match state.chat_channels.delete(&auth.user_id, &id) {
        Ok(true) => Json(serde_json::json!({ "code": 0, "data": true })).into_response(),
        Ok(false) => channel_error(&format!("Can't find this chat channel! ({id})")),
        Err(error) => channel_error(&error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_is_tenant_scoped_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("channels.json")
            .to_string_lossy()
            .into_owned();
        let store = ChatChannelStore::new(&path).unwrap();
        let row = ChatChannelRecord {
            id: "cc-1".into(),
            tenant_id: "tenant-a".into(),
            name: "Telegram Bot".into(),
            channel: "telegram".into(),
            config: serde_json::json!({ "credential": { "token": "tok" } }),
            chat_id: None,
            created_at: 1,
            updated_at: 1,
        };
        store.insert(row).unwrap();
        assert_eq!(store.list("tenant-a").len(), 1);
        assert!(store.list("tenant-b").is_empty());
        assert!(
            ChatChannelStore::new(&path)
                .unwrap()
                .get("tenant-a", "cc-1")
                .is_some()
        );
        assert!(store.delete("tenant-a", "cc-1").unwrap());
        assert!(
            ChatChannelStore::new(&path)
                .unwrap()
                .list("tenant-a")
                .is_empty()
        );
    }
}
