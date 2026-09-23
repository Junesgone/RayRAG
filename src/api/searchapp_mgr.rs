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
    pub rerank_id: String,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,
    #[serde(default = "default_vector_similarity_weight")]
    pub vector_similarity_weight: f64,
    /// Upstream `search_config.doc_ids` — an explicit document allow-list.
    #[serde(default)]
    pub doc_ids: Vec<String>,
    /// Upstream `search_config.chat_id` — the tenant model instance used by the
    /// related-search / AI-summary chat calls.
    #[serde(default)]
    pub chat_id: String,
    /// Upstream `search_config.llm_setting` — generation overrides for those calls.
    #[serde(default)]
    pub llm_setting: serde_json::Value,
    /// Upstream `search_config.meta_data_filter` — the app-level metadata filter
    /// (`manual` / `semi_auto` / `auto`) that `apply_meta_data_filter` resolves
    /// when the running search does not send one of its own.
    #[serde(default)]
    pub meta_data_filter: serde_json::Value,
    /// Upstream `search_config.chat_settingcross_languages` (sic).
    #[serde(default)]
    pub cross_languages: Vec<String>,
    /// Upstream `search_config.use_kg`.
    #[serde(default)]
    pub use_kg: bool,
    /// Upstream `search_config.highlight` — render server-side `<em>` highlights.
    #[serde(default)]
    pub highlight: bool,
    /// Upstream `search_config.keyword` — keyword-only retrieval.
    #[serde(default)]
    pub keyword: bool,
    /// Upstream `search_config.web_search`.
    #[serde(default)]
    pub web_search: bool,
    /// Upstream `search_config.related_search` — fetch `chat/recommendation`
    /// questions after each search.
    #[serde(default)]
    pub related_search: bool,
    /// Upstream `search_config.query_mindmap`.
    #[serde(default)]
    pub query_mindmap: bool,
    /// Upstream `search_config.summary` — stream an AI summary answer.
    #[serde(default)]
    pub summary: bool,
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
    pub rerank_id: String,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,
    #[serde(default = "default_vector_similarity_weight")]
    pub vector_similarity_weight: f64,
    #[serde(default)]
    pub doc_ids: Vec<String>,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default)]
    pub llm_setting: serde_json::Value,
    #[serde(default)]
    pub meta_data_filter: serde_json::Value,
    #[serde(default)]
    pub cross_languages: Vec<String>,
    #[serde(default)]
    pub use_kg: bool,
    #[serde(default)]
    pub highlight: bool,
    #[serde(default)]
    pub keyword: bool,
    #[serde(default)]
    pub web_search: bool,
    #[serde(default)]
    pub related_search: bool,
    #[serde(default)]
    pub query_mindmap: bool,
    #[serde(default)]
    pub summary: bool,
}

impl Default for SearchAppCreate {
    /// Upstream `Search.search_config` defaults, so a partially specified
    /// create request still lands on the documented configuration.
    fn default() -> Self {
        Self {
            name: String::new(),
            kb_ids: Vec::new(),
            hybrid: false,
            rerank: false,
            rerank_id: String::new(),
            top_k: default_top_k(),
            similarity_threshold: default_similarity_threshold(),
            vector_similarity_weight: default_vector_similarity_weight(),
            doc_ids: Vec::new(),
            chat_id: String::new(),
            llm_setting: serde_json::Value::Null,
            meta_data_filter: serde_json::Value::Null,
            cross_languages: Vec::new(),
            use_kg: false,
            highlight: false,
            keyword: false,
            web_search: false,
            related_search: false,
            query_mindmap: false,
            summary: false,
        }
    }
}

fn default_top_k() -> u32 {
    // Upstream `Search.search_config.top_k` default.
    1024
}

fn default_similarity_threshold() -> f64 {
    0.2
}

fn default_vector_similarity_weight() -> f64 {
    0.3
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
        self.apps.write().unwrap().insert(record.id.clone(), record);
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
            rerank_id: req.rerank_id,
            top_k: req.top_k,
            similarity_threshold: req.similarity_threshold,
            vector_similarity_weight: req.vector_similarity_weight,
            doc_ids: req.doc_ids,
            chat_id: req.chat_id,
            llm_setting: req.llm_setting,
            meta_data_filter: req.meta_data_filter,
            cross_languages: req.cross_languages,
            use_kg: req.use_kg,
            highlight: req.highlight,
            keyword: req.keyword,
            web_search: req.web_search,
            related_search: req.related_search,
            query_mindmap: req.query_mindmap,
            summary: req.summary,
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

/// `GET /api/v1/searches/{search_id}` — upstream `search_api.detail`.
///
/// The upstream web client reads the search app back through this path before
/// opening the settings drawer (`getSearchDetail`), and the route used to be
/// missing here: `/api/v1/searchapps/{id}` only answered `PUT`/`DELETE`, so a
/// `GET` returned 405 with an empty body and the drawer had nothing to load.
pub async fn get_search_app(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    Extension(auth): Extension<crate::server::AuthContext>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let Some(store) = &state.search_apps else {
        return json(serde_json::json!({"code": 404, "message": "Search apps disabled"}));
    };
    let Some(app) = store.get(&id) else {
        // Upstream `get_data_error_result(message="Can't find this Search App!")`.
        return json(serde_json::json!({"code": 102, "message": "Can't find this Search App!"}));
    };
    if app.owner_id != auth.user_id && !auth.is_admin {
        // Upstream `RetCode.OPERATING_ERROR` for a tenant that does not own it.
        return json(serde_json::json!({
            "code": 103,
            "message": "Has no permission for this operation."
        }));
    }
    json(serde_json::json!({"code": 0, "data": app}))
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
    pub rerank_id: Option<String>,
    #[serde(default)]
    pub similarity_threshold: Option<f64>,
    #[serde(default)]
    pub vector_similarity_weight: Option<f64>,
    #[serde(default)]
    pub top_k: Option<u32>,
    #[serde(default)]
    pub doc_ids: Option<Vec<String>>,
    #[serde(default)]
    pub chat_id: Option<String>,
    #[serde(default)]
    pub llm_setting: Option<serde_json::Value>,
    #[serde(default)]
    pub meta_data_filter: Option<serde_json::Value>,
    #[serde(default)]
    pub cross_languages: Option<Vec<String>>,
    #[serde(default)]
    pub use_kg: Option<bool>,
    #[serde(default)]
    pub highlight: Option<bool>,
    #[serde(default)]
    pub keyword: Option<bool>,
    #[serde(default)]
    pub web_search: Option<bool>,
    #[serde(default)]
    pub related_search: Option<bool>,
    #[serde(default)]
    pub query_mindmap: Option<bool>,
    #[serde(default)]
    pub summary: Option<bool>,
}

/// PUT /api/v1/searchapps/{id} — partial update (rename + settings).
pub async fn update_search_app(
    State(state): State<std::sync::Arc<crate::server::AppState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(body): Json<UpdateSearchAppRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let store = state.search_apps.as_ref().ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_IMPLEMENTED,
            "search apps disabled".into(),
        )
    })?;
    let app = store.get(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "search app not found".into(),
        )
    })?;
    let mut next = app.clone();
    let mut changed = false;
    if let Some(name) = body.name {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                "name is required".into(),
            ));
        }
        if name != next.name {
            next.name = name;
            changed = true;
        }
    }
    if let Some(description) = body.description
        && description != next.description
    {
        next.description = description;
        changed = true;
    }
    if let Some(icon) = body.icon {
        let icon = icon.trim().to_string();
        if icon != next.icon {
            next.icon = icon;
            changed = true;
        }
    }
    if let Some(kb_ids) = body.kb_ids
        && kb_ids != next.kb_ids
    {
        next.kb_ids = kb_ids;
        changed = true;
    }
    if let Some(hybrid) = body.hybrid
        && hybrid != next.hybrid
    {
        next.hybrid = hybrid;
        changed = true;
    }
    if let Some(rerank) = body.rerank
        && rerank != next.rerank
    {
        next.rerank = rerank;
        changed = true;
    }
    if let Some(rerank_id) = body.rerank_id
        && rerank_id != next.rerank_id
    {
        next.rerank_id = rerank_id;
        changed = true;
    }
    if let Some(top_k) = body.top_k
        && top_k != next.top_k
    {
        next.top_k = top_k;
        changed = true;
    }
    if let Some(similarity_threshold) = body.similarity_threshold
        && similarity_threshold != next.similarity_threshold
    {
        next.similarity_threshold = similarity_threshold.clamp(0.0, 1.0);
        changed = true;
    }
    if let Some(vector_similarity_weight) = body.vector_similarity_weight
        && vector_similarity_weight != next.vector_similarity_weight
    {
        next.vector_similarity_weight = vector_similarity_weight.clamp(0.0, 1.0);
        changed = true;
    }
    if let Some(doc_ids) = body.doc_ids
        && doc_ids != next.doc_ids
    {
        next.doc_ids = doc_ids;
        changed = true;
    }
    if let Some(chat_id) = body.chat_id {
        let chat_id = chat_id.trim().to_string();
        if chat_id != next.chat_id {
            next.chat_id = chat_id;
            changed = true;
        }
    }
    if let Some(llm_setting) = body.llm_setting
        && llm_setting != next.llm_setting
    {
        next.llm_setting = llm_setting;
        changed = true;
    }
    if let Some(meta_data_filter) = body.meta_data_filter
        && meta_data_filter != next.meta_data_filter
    {
        next.meta_data_filter = meta_data_filter;
        changed = true;
    }
    if let Some(cross_languages) = body.cross_languages
        && cross_languages != next.cross_languages
    {
        next.cross_languages = cross_languages;
        changed = true;
    }
    // Upstream `search_config` boolean switches.
    for (incoming, current) in [
        (body.use_kg, &mut next.use_kg),
        (body.highlight, &mut next.highlight),
        (body.keyword, &mut next.keyword),
        (body.web_search, &mut next.web_search),
        (body.related_search, &mut next.related_search),
        (body.query_mindmap, &mut next.query_mindmap),
        (body.summary, &mut next.summary),
    ] {
        if let Some(value) = incoming
            && value != *current
        {
            *current = value;
            changed = true;
        }
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
    fn create_persists_rerank_id() {
        let dir = std::env::temp_dir().join(format!("rayrag-searchapp-{}", uuid::Uuid::new_v4()));
        let store = SearchAppStore::new(dir.to_str().unwrap()).unwrap();
        let rec = store
            .create(
                "u1",
                SearchAppCreate {
                    name: "rerank-app".into(),
                    kb_ids: vec![],
                    hybrid: false,
                    rerank: true,
                    rerank_id: "prov/inst/bge-reranker-v2-m3".into(),
                    top_k: 8,
                    similarity_threshold: 0.2,
                    vector_similarity_weight: 0.3,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(rec.rerank_id, "prov/inst/bge-reranker-v2-m3");
        let got = store.get(&rec.id).unwrap();
        assert_eq!(got.rerank_id, "prov/inst/bge-reranker-v2-m3");
        assert_eq!(got.similarity_threshold, 0.2);
        assert_eq!(got.vector_similarity_weight, 0.3);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn search_config_switch_fields_round_trip() {
        let dir = std::env::temp_dir().join(format!("rayrag-searchapp-{}", uuid::Uuid::new_v4()));
        let store = SearchAppStore::new(dir.to_str().unwrap()).unwrap();
        let rec = store
            .create(
                "u1",
                SearchAppCreate {
                    name: "related".into(),
                    kb_ids: vec!["kb1".into()],
                    hybrid: false,
                    rerank: false,
                    rerank_id: String::new(),
                    top_k: 1024,
                    similarity_threshold: 0.2,
                    vector_similarity_weight: 0.3,
                    doc_ids: vec!["doc1".into()],
                    chat_id: "prov/inst/chat".into(),
                    llm_setting: serde_json::json!({ "temperature": 0.5 }),
                    meta_data_filter: serde_json::Value::Null,
                    cross_languages: vec!["English".into()],
                    use_kg: true,
                    highlight: true,
                    keyword: true,
                    web_search: false,
                    related_search: true,
                    query_mindmap: true,
                    summary: true,
                },
            )
            .unwrap();
        assert!(rec.related_search && rec.query_mindmap && rec.summary);
        assert_eq!(rec.chat_id, "prov/inst/chat");
        assert_eq!(rec.doc_ids, vec!["doc1".to_string()]);
        assert_eq!(rec.llm_setting["temperature"], 0.5);
        // Reload from disk so the JSON round-trip is covered, not just memory.
        let reloaded = SearchAppStore::new(dir.to_str().unwrap()).unwrap();
        let got = reloaded.get(&rec.id).expect("persisted app");
        assert!(got.related_search && got.use_kg && got.highlight && got.keyword);
        assert_eq!(got.cross_languages, vec!["English".to_string()]);
        // Legacy records without the new keys deserialize with upstream defaults.
        let legacy: SearchAppRecord = serde_json::from_value(serde_json::json!({
            "id": "old",
            "name": "legacy",
            "created_at": "2026-01-01T00:00:00Z",
        }))
        .expect("legacy record");
        assert_eq!(legacy.top_k, 1024);
        assert!(!legacy.related_search);
        assert_eq!(legacy.llm_setting, serde_json::Value::Null);
        std::fs::remove_dir_all(dir).ok();
    }

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
                    rerank_id: String::new(),
                    top_k: 8,
                    similarity_threshold: 0.2,
                    vector_similarity_weight: 0.3,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut next = rec.clone();
        next.rerank = true;
        next.top_k = 16;
        next.rerank_id = "prov/inst/bge-reranker-v2-m3".into();
        next.similarity_threshold = 0.6;
        next.vector_similarity_weight = 0.4;
        next.icon = "🔎".into();
        store.upsert(next).unwrap();
        let got = store.get(&rec.id).unwrap();
        assert!(got.rerank);
        assert_eq!(got.top_k, 16);
        assert_eq!(got.icon, "🔎");
        assert_eq!(got.rerank_id, "prov/inst/bge-reranker-v2-m3");
        assert_eq!(got.similarity_threshold, 0.6);
        assert_eq!(got.vector_similarity_weight, 0.4);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn update_request_parses_partial_fields() {
        let req: UpdateSearchAppRequest = serde_json::from_str(
            r#"{"name":"新名","rerank":true,"rerank_id":"prov/inst/m3","top_k":12,"similarity_threshold":0.6,"vector_similarity_weight":0.4}"#,
        )
        .unwrap();
        assert_eq!(req.name.as_deref(), Some("新名"));
        assert!(req.rerank == Some(true));
        assert_eq!(req.top_k, Some(12));
        assert!(req.hybrid.is_none());
        assert_eq!(req.rerank_id.as_deref(), Some("prov/inst/m3"));
        assert_eq!(req.similarity_threshold, Some(0.6));
        assert_eq!(req.vector_similarity_weight, Some(0.4));
    }
}
