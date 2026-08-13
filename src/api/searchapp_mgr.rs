//! Search app store (RAGFlow `searchapps` parity): persist named search
//! configurations (bound knowledge bases + retrieval knobs) so a saved
//! retrieval setup can be reopened from the Search page.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchAppRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub owner_id: String,
    /// Emoji icon (RAGFlow HomeCard avatar).
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub kb_ids: Vec<String>,
    #[serde(default)]
    pub hybrid: bool,
    #[serde(default)]
    pub rerank: bool,
    #[serde(default)]
    pub top_k: u32,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SearchAppCreate {
    pub name: String,
    #[serde(default)]
    pub kb_ids: Vec<String>,
    #[serde(default)]
    pub hybrid: bool,
    #[serde(default)]
    pub rerank: bool,
    #[serde(default)]
    pub top_k: u32,
}

pub struct SearchAppStore {
    apps: RwLock<HashMap<String, SearchAppRecord>>,
    metadata_path: String,
    save_lock: Mutex<()>,
}

impl SearchAppStore {
    pub fn new(data_dir: &str) -> Result<Self> {
        std::fs::create_dir_all(data_dir)?;
        let metadata_path = std::path::Path::new(data_dir)
            .join("search_apps.json")
            .to_string_lossy()
            .to_string();
        let apps = if std::path::Path::new(&metadata_path).exists() {
            serde_json::from_str(
                &std::fs::read_to_string(&metadata_path).context("read search_apps.json")?,
            )
            .unwrap_or_default()
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
        let _guard = self.save_lock.lock().unwrap();
        let json = serde_json::to_string_pretty(&*self.apps.read().unwrap())
            .context("serialize search_apps")?;
        std::fs::write(&self.metadata_path, json).context("write search_apps.json")
    }

    pub fn list(&self) -> Vec<SearchAppRecord> {
        let mut v: Vec<_> = self.apps.read().unwrap().values().cloned().collect();
        v.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        v
    }

    pub fn get(&self, id: &str) -> Option<SearchAppRecord> {
        self.apps.read().unwrap().get(id).cloned()
    }

    /// Insert or replace a record (used by the partial-update handler).
    pub fn upsert(&self, record: SearchAppRecord) -> Result<()> {
        self.apps
            .write()
            .unwrap()
            .insert(record.id.clone(), record);
        self.persist()
    }

    pub fn create(&self, owner_id: &str, req: SearchAppCreate) -> Result<SearchAppRecord> {
        if req.name.trim().is_empty() {
            bail!("Search app name cannot be empty");
        }
        let record = SearchAppRecord {
            id: uuid_like(),
            name: req.name.trim().to_string(),
            owner_id: owner_id.to_string(),
            icon: String::new(),
            description: String::new(),
            kb_ids: req.kb_ids,
            hybrid: req.hybrid,
            rerank: req.rerank,
            top_k: req.top_k,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        self.apps
            .write()
            .unwrap()
            .insert(record.id.clone(), record.clone());
        self.persist()?;
        Ok(record)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let removed = self.apps.write().unwrap().remove(id).is_some();
        if removed {
            self.persist()?;
        }
        Ok(removed)
    }
}

fn uuid_like() -> String {
    // 16 hex chars + timestamp suffix — unique enough for local app ids.
    let mut s = String::new();
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    s.push_str(&format!("{:016x}{:06x}", t, std::process::id()));
    s
}

// ---- HTTP handlers ----

use axum::{
    Extension, Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};

fn json(v: serde_json::Value) -> Response {
    Json(v).into_response()
}

pub async fn list_search_apps(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Extension(auth): Extension<crate::server::AuthContext>,
) -> Response {
    let Some(store) = &state.search_apps else {
        return json(serde_json::json!({"code": 404, "message": "Search apps disabled"}));
    };
    let apps = store.list();
    let visible: Vec<_> = apps
        .into_iter()
        .filter(|a| a.owner_id == auth.user_id || auth.is_admin)
        .collect();
    json(serde_json::json!({"code": 0, "data": visible}))
}

pub async fn create_search_app(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Extension(auth): Extension<crate::server::AuthContext>,
    Json(req): Json<SearchAppCreate>,
) -> Response {
    let Some(store) = &state.search_apps else {
        return json(serde_json::json!({"code": 404, "message": "Search apps disabled"}));
    };
    match store.create(&auth.user_id, req) {
        Ok(app) => json(serde_json::json!({"code": 0, "message": "Created", "data": app})),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"code": 400, "message": e.to_string()})),
        )
            .into_response(),
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateSearchAppRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub kb_ids: Option<Vec<String>>,
    #[serde(default)]
    pub hybrid: Option<bool>,
    #[serde(default)]
    pub rerank: Option<bool>,
    #[serde(default)]
    pub top_k: Option<u32>,
}

/// PUT /api/v1/searchapps/{id} — partial update (rename + settings).
pub async fn update_search_app(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<UpdateSearchAppRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let store = state
        .search_apps
        .as_ref()
        .ok_or_else(|| (axum::http::StatusCode::NOT_IMPLEMENTED, "search apps disabled".into()))?;
    let app = store
        .get(&id)
        .ok_or_else(|| (axum::http::StatusCode::NOT_FOUND, "search app not found".into()))?;
    let mut next = app.clone();
    let mut changed = false;
    if let Some(name) = body.name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err((axum::http::StatusCode::BAD_REQUEST, "name is required".into()));
        }
        if name != next.name { next.name = name; changed = true; }
    }
    if let Some(description) = body.description {
        if description != next.description { next.description = description; changed = true; }
    }
    if let Some(icon) = body.icon {
        let icon = icon.trim().to_string();
        if icon != next.icon { next.icon = icon; changed = true; }
    }
    if let Some(kb_ids) = body.kb_ids {
        if kb_ids != next.kb_ids { next.kb_ids = kb_ids; changed = true; }
    }
    if let Some(hybrid) = body.hybrid {
        if hybrid != next.hybrid { next.hybrid = hybrid; changed = true; }
    }
    if let Some(rerank) = body.rerank {
        if rerank != next.rerank { next.rerank = rerank; changed = true; }
    }
    if let Some(top_k) = body.top_k {
        if top_k != next.top_k { next.top_k = top_k; changed = true; }
    }
    if changed {
        store
            .upsert(next)
            .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }
    Ok(Json(serde_json::json!({ "code": 0, "message": "Updated" })))
}

pub async fn delete_search_app(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Extension(auth): Extension<crate::server::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let Some(store) = &state.search_apps else {
        return json(serde_json::json!({"code": 404, "message": "Search apps disabled"}));
    };
    if !store
        .get(&id)
        .map(|a| a.owner_id == auth.user_id || auth.is_admin)
        .unwrap_or(false)
    {
        return json(serde_json::json!({"code": 404, "message": "Not found"}));
    }
    match store.delete(&id) {
        Ok(true) => json(serde_json::json!({"code": 0, "data": true})),
        _ => json(serde_json::json!({"code": 404, "message": "Not found"})),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_existing_record() {
        let dir = std::env::temp_dir().join(format!("rayrag-searchapp-{}", uuid::Uuid::new_v4()));
        let store = SearchAppStore::new(dir.to_str().unwrap()).unwrap();
        let rec = store
            .create(
                "u1",
                SearchAppCreate {
                    name: "检索A".into(),
                    kb_ids: vec!["kb1".into()],
                    hybrid: false,
                    rerank: false,
                    top_k: 8,
                },
            )
            .unwrap();
        let mut next = rec.clone();
        next.rerank = true;
        next.top_k = 16;
        next.icon = "🔎".into();
        store.upsert(next).unwrap();
        let got = store.get(&rec.id).unwrap();
        assert!(got.rerank);
        assert_eq!(got.top_k, 16);
        assert_eq!(got.icon, "🔎");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn update_request_parses_partial_fields() {
        let req: UpdateSearchAppRequest =
            serde_json::from_str(r#"{"name":"新名","rerank":true,"top_k":12}"#).unwrap();
        assert_eq!(req.name.as_deref(), Some("新名"));
        assert!(req.rerank == Some(true));
        assert_eq!(req.top_k, Some(12));
        assert!(req.hybrid.is_none());
    }
}
