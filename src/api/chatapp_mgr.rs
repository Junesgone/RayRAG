//! Chat-app store: RAGFlow 0.26.4 "Chat apps" (`/chats`) parity.
//!
//! RAGFlow models a chat application (name/icon/description/tenant/llm/kb
//! bindings) that owns conversations. RayRAG historically stored bare
//! conversations; this module adds the app layer. Conversations link back
//! through `Conversation::app_id`.

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatApp {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Emoji or avatar key; RAGFlow stores an icon string.
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub tenant_id: String,
    /// Knowledge-base ids bound to the app (RAGFlow `kb_ids`).
    #[serde(default)]
    pub kb_ids: Vec<String>,
    /// LLM model name for the app (optional; falls back to tenant default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_id: Option<String>,
    /// LLM generation settings (temperature/top_p/max_tokens/penalties);
    /// mirrors RAGFlow `llm_setting` in the chat-app settings form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_setting: Option<serde_json::Value>,
    /// Rerank model name (RAGFlow rerankFormSchema.rerank_id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_id: Option<String>,
    /// Number of chunks fed into the reranker (rerankFormSchema.top_k).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    /// Prompt configuration (system/prologue/cross_languages/quote/keyword/
    /// tts/toc_enhance/use_kg/reference_metadata/empty_response);
    /// mirrors RAGFlow `prompt_config` (chat-prompt-engine.tsx).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_config: Option<serde_json::Value>,
    #[serde(default)]
    pub created_at: u64,
}

pub struct ChatAppStore {
    apps: RwLock<HashMap<String, ChatApp>>,
    metadata_path: String,
    save_lock: Mutex<()>,
}

impl ChatAppStore {
    pub fn new(data_dir: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let metadata_path = std::path::Path::new(data_dir)
            .join("chat_apps.json")
            .to_string_lossy()
            .to_string();
        crate::persistence::restore_if_missing(std::path::Path::new(&metadata_path))?;
        let apps = if std::path::Path::new(&metadata_path).exists() {
            let content = std::fs::read_to_string(&metadata_path)?;
            serde_json::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self {
            apps: RwLock::new(apps),
            metadata_path,
            save_lock: Mutex::new(()),
        })
    }

    fn persist(&self) -> Result<()> {
        let _guard = self
            .save_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("save lock poisoned"))?;
        let apps = self
            .apps
            .read()
            .map_err(|_| anyhow::anyhow!("apps lock poisoned"))?;
        let json = serde_json::to_string_pretty(&apps.values().cloned().collect::<Vec<_>>())?;
        let tmp = format!("{}.tmp", self.metadata_path);
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.metadata_path)?;
        Ok(())
    }

    pub fn list(&self) -> Vec<ChatApp> {
        let apps = self.apps.read().expect("apps lock");
        let mut v: Vec<ChatApp> = apps.values().cloned().collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v
    }

    pub fn get(&self, id: &str) -> Option<ChatApp> {
        self.apps.read().expect("apps lock").get(id).cloned()
    }

    pub fn insert(&self, app: ChatApp) -> Result<()> {
        self.apps
            .write()
            .expect("apps lock")
            .insert(app.id.clone(), app);
        self.persist()
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<Option<ChatApp>> {
        let mut apps = self.apps.write().expect("apps lock");
        if let Some(app) = apps.get_mut(id) {
            app.name = name.to_string();
            let app = app.clone();
            drop(apps);
            self.persist()?;
            Ok(Some(app))
        } else {
            Ok(None)
        }
    }

    pub fn remove(&self, id: &str) -> Result<Option<ChatApp>> {
        let removed = self.apps.write().expect("apps lock").remove(id);
        if removed.is_some() {
            self.persist()?;
        }
        Ok(removed)
    }
}


// ---- HTTP handlers (RAGFlow /api/v1/chatapps parity) ----

#[derive(Debug, Deserialize)]
pub struct CreateChatAppRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub kb_ids: Vec<String>,
    #[serde(default)]
    pub llm_setting: Option<serde_json::Value>,
    #[serde(default)]
    pub prompt_config: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct RenameChatAppRequest {
    pub name: String,
}

/// Partial update for a chat app (RAGFlow `updateChat` parity): any subset of
/// the settings-form fields. `name` alone reproduces the old rename semantics.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateChatAppRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub kb_ids: Option<Vec<String>>,
    #[serde(default)]
    pub llm_id: Option<Option<String>>,
    #[serde(default)]
    pub llm_setting: Option<serde_json::Value>,
    #[serde(default)]
    pub prompt_config: Option<serde_json::Value>,
    #[serde(default)]
    pub rerank_id: Option<Option<String>>,
    #[serde(default)]
    pub top_k: Option<u32>,
}

pub async fn list_chat_apps(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .chat_apps
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "chat apps disabled".into()))?;
    Ok(Json(serde_json::json!({ "code": 0, "data": store.list() })))
}

pub async fn create_chat_app(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Json(body): Json<CreateChatAppRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .chat_apps
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "chat apps disabled".into()))?;
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name is required".into()));
    }
    if name.chars().count() > 128 {
        return Err((StatusCode::BAD_REQUEST, "name too long (max 128 chars)".into()));
    }
    let app = ChatApp {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        description: body.description.trim().to_string(),
        icon: if body.icon.trim().is_empty() {
            "🤖".into()
        } else {
            body.icon.trim().to_string()
        },
        tenant_id: String::new(),
        kb_ids: body.kb_ids,
        llm_id: None,
        llm_setting: body.llm_setting,
        prompt_config: body.prompt_config,
        rerank_id: None,
        top_k: None,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    };
    let id = app.id.clone();
    store.insert(app).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(serde_json::json!({ "code": 0, "data": { "id": id } })))
}

pub async fn update_chat_app(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<UpdateChatAppRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .chat_apps
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "chat apps disabled".into()))?;
    let mut app = store
        .get(&id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "chat app not found".into()))?;
    let mut changed = false;
    if let Some(name) = body.name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "name is required".into()));
        }
        if name.chars().count() > 128 {
            return Err((StatusCode::BAD_REQUEST, "name too long (max 128 chars)".into()));
        }
        if name != app.name {
            app.name = name;
            changed = true;
        }
    }
    if let Some(description) = body.description {
        let description = description.trim().to_string();
        if description != app.description {
            app.description = description;
            changed = true;
        }
    }
    if let Some(icon) = body.icon {
        let icon = icon.trim().to_string();
        if icon != app.icon {
            app.icon = icon;
            changed = true;
        }
    }
    if let Some(kb_ids) = body.kb_ids {
        if kb_ids != app.kb_ids {
            app.kb_ids = kb_ids;
            changed = true;
        }
    }
    if let Some(llm_id) = body.llm_id {
        if llm_id != app.llm_id {
            app.llm_id = llm_id;
            changed = true;
        }
    }
    if let Some(llm_setting) = body.llm_setting {
        app.llm_setting = Some(llm_setting);
        changed = true;
    }
    if let Some(prompt_config) = body.prompt_config {
        app.prompt_config = Some(prompt_config);
        changed = true;
    }
    if let Some(rerank_id) = body.rerank_id {
        if rerank_id != app.rerank_id {
            app.rerank_id = rerank_id;
            changed = true;
        }
    }
    if let Some(top_k) = body.top_k {
        if Some(top_k) != app.top_k {
            app.top_k = Some(top_k);
            changed = true;
        }
    }
    if changed {
        store
            .insert(app)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    Ok(Json(serde_json::json!({ "code": 0, "message": "Updated" })))
}

pub async fn delete_chat_app(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let store = state
        .chat_apps
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_IMPLEMENTED, "chat apps disabled".into()))?;
    let removed = store
        .remove(&id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    if removed.is_none() {
        return Err((StatusCode::NOT_FOUND, "chat app not found".into()));
    }
    Ok(Json(serde_json::json!({ "code": 0, "message": "Deleted", "data": null })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip_rename_remove() {
        let dir = std::env::temp_dir().join(format!("rayrag-chatapp-{}", uuid::Uuid::new_v4()));
        let store = ChatAppStore::new(dir.to_str().unwrap()).unwrap();
        let app = ChatApp {
            id: "a1".into(),
            name: "客服助手".into(),
            description: "FAQ bot".into(),
            icon: "🤖".into(),
            tenant_id: "t1".into(),
            kb_ids: vec!["kb1".into()],
            llm_id: None,
            llm_setting: None,
            prompt_config: None,
            rerank_id: None,
            top_k: None,
            created_at: 1,
        };
        store.insert(app).unwrap();
        assert_eq!(store.list().len(), 1);
        assert!(store.rename("a1", "新名字").unwrap().is_some());
        assert_eq!(store.get("a1").unwrap().name, "新名字");
        assert!(store.remove("a1").unwrap().is_some());
        assert!(store.list().is_empty());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn update_request_name_only_keeps_rename_semantics() {
        let req: UpdateChatAppRequest =
            serde_json::from_str(r#"{"name":"新名"}"#).unwrap();
        assert_eq!(req.name.as_deref(), Some("新名"));
        assert!(req.kb_ids.is_none());
        assert!(req.prompt_config.is_none());
    }

    #[test]
    fn update_request_accepts_prompt_config_and_llm_setting() {
        let req: UpdateChatAppRequest = serde_json::from_str(
            r#"{"prompt_config":{"system":"你是客服","quote":true},"llm_setting":{"temperature":0.3},"llm_id":null}"#,
        )
        .unwrap();
        assert_eq!(
            req.prompt_config.unwrap()["quote"],
            serde_json::json!(true)
        );
        assert!(req.llm_id.is_none()); // null coalesces to None (serde default)
        assert!(req.llm_setting.is_some());
    }
}
