//! Skill search/index replacement for RAGFlow's Go `SkillIndexerService` and
//! `SkillSearchService`.
//!
//! RayRAG does not deploy Infinity.  The exact Infinity physical rows are
//! stored inside an atomic JSON snapshot (and mirrored by the existing
//! optional PostgreSQL snapshot layer); vectors are also mirrored to an
//! isolated zvec collection tree when the native backend is enabled, while
//! keyword/vector fusion runs in Rust. Skill bundle upload and the
//! file-system-backed space lifecycle are intentionally outside this module.

use axum::{
    Json,
    extract::{Extension, Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{Mutex, RwLock},
};

use crate::server::{AppState, AuthContext};
use crate::{search::IndexedChunk, store::OnlineVectorMirror};

fn default_name_weight() -> f64 {
    3.0
}

fn default_tags_weight() -> f64 {
    2.0
}

fn default_description_weight() -> f64 {
    1.0
}

fn default_content_weight() -> f64 {
    0.5
}

fn default_vector_weight() -> f64 {
    0.3
}

fn default_similarity_threshold() -> f64 {
    0.2
}

fn default_top_k() -> usize {
    10
}

fn default_index_version() -> String {
    "1.0.0".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillFieldWeight {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillFieldConfig {
    #[serde(default = "default_name_field")]
    pub name: SkillFieldWeight,
    #[serde(default = "default_tags_field")]
    pub tags: SkillFieldWeight,
    #[serde(default = "default_description_field")]
    pub description: SkillFieldWeight,
    #[serde(default = "default_content_field")]
    pub content: SkillFieldWeight,
}

fn default_name_field() -> SkillFieldWeight {
    SkillFieldWeight {
        enabled: true,
        weight: default_name_weight(),
    }
}

fn default_tags_field() -> SkillFieldWeight {
    SkillFieldWeight {
        enabled: true,
        weight: default_tags_weight(),
    }
}

fn default_description_field() -> SkillFieldWeight {
    SkillFieldWeight {
        enabled: true,
        weight: default_description_weight(),
    }
}

fn default_content_field() -> SkillFieldWeight {
    SkillFieldWeight {
        enabled: false,
        weight: default_content_weight(),
    }
}

impl Default for SkillFieldConfig {
    fn default() -> Self {
        Self {
            name: default_name_field(),
            tags: default_tags_field(),
            description: default_description_field(),
            content: default_content_field(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillSearchConfig {
    #[serde(default)]
    pub id: String,
    pub tenant_id: String,
    #[serde(default = "default_space_id")]
    pub space_id: String,
    #[serde(default)]
    pub embd_id: String,
    #[serde(default = "default_vector_weight")]
    pub vector_similarity_weight: f64,
    #[serde(default = "default_similarity_threshold")]
    pub similarity_threshold: f64,
    #[serde(default)]
    pub field_config: SkillFieldConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_id: Option<String>,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default = "default_index_version")]
    pub index_version: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default)]
    pub create_time: i64,
    #[serde(default)]
    pub update_time: i64,
}

fn default_space_id() -> String {
    "default".to_string()
}

fn default_status() -> String {
    "1".to_string()
}

impl SkillSearchConfig {
    fn defaults(tenant_id: &str, space_id: &str, embd_id: &str) -> Self {
        Self {
            id: String::new(),
            tenant_id: tenant_id.to_string(),
            space_id: normalize_space_id(space_id),
            embd_id: embd_id.to_string(),
            vector_similarity_weight: default_vector_weight(),
            similarity_threshold: default_similarity_threshold(),
            field_config: SkillFieldConfig::default(),
            rerank_id: None,
            top_k: default_top_k(),
            index_version: default_index_version(),
            status: default_status(),
            create_time: 0,
            update_time: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillInfo {
    pub id: String,
    #[serde(default)]
    pub folder_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkillSearchResult {
    pub skill_id: String,
    pub folder_id: String,
    pub name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub score: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub bm25_score: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub vector_score: f64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub index_version: String,
    pub create_time: i64,
    pub version: String,
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkillSearchResponse {
    pub skills: Vec<SkillSearchResult>,
    pub total: usize,
    pub query: String,
    pub search_type: String,
}

#[derive(Debug, Clone, PartialEq)]
struct SkillIndexRecord {
    tenant_id: String,
    skill_id: String,
    space_id: String,
    folder_id: String,
    name: String,
    tags: Vec<String>,
    description: String,
    content: String,
    version: String,
    status: String,
    create_time: i64,
    update_time: i64,
    embedding: Vec<f32>,
}

impl SkillIndexRecord {
    fn info(&self) -> SkillInfo {
        SkillInfo {
            id: self.skill_id.clone(),
            folder_id: self.folder_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            tags: self.tags.clone(),
            content: self.content.clone(),
            version: self.version.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct SkillIndexState {
    records: Vec<SkillIndexRecord>,
    configs: Vec<SkillSearchConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct StoredSkillSnapshot {
    #[serde(default)]
    indexes: Vec<StoredSkillPartition>,
    #[serde(default)]
    configs: Vec<SkillSearchConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSkillPartition {
    tenant_id: String,
    space_id: String,
    rows: Vec<Value>,
}

pub struct SkillIndexStore {
    state: RwLock<SkillIndexState>,
    path: Option<PathBuf>,
    save_lock: Mutex<()>,
    vector_mirror: OnlineVectorMirror,
}

impl SkillIndexStore {
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        crate::persistence::restore_if_missing(&path)?;
        let state = if path.exists() {
            let snapshot: StoredSkillSnapshot = serde_json::from_slice(&std::fs::read(&path)?)?;
            decode_snapshot(snapshot)?
        } else {
            SkillIndexState::default()
        };
        let vector_mirror = OnlineVectorMirror::for_skill_index()?;
        let store = Self {
            state: RwLock::new(state),
            path: Some(path),
            save_lock: Mutex::new(()),
            vector_mirror,
        };
        if store.path.as_ref().is_some_and(|path| !path.exists()) {
            store.persist_current()?;
        }
        store.vector_mirror.reconcile_snapshot(&skill_chunks(
            &store.state.read().expect("skill index lock poisoned"),
        ))?;
        Ok(store)
    }

    pub fn in_memory() -> Self {
        Self {
            state: RwLock::new(SkillIndexState::default()),
            path: None,
            save_lock: Mutex::new(()),
            vector_mirror: OnlineVectorMirror::disabled(),
        }
    }

    pub fn get_config(
        &self,
        tenant_id: &str,
        space_id: &str,
        embd_id: Option<&str>,
    ) -> SkillSearchConfig {
        let space_id = normalize_space_id(space_id);
        let state = self.state.read().expect("skill index lock poisoned");
        state
            .configs
            .iter()
            .find(|config| {
                config.tenant_id == tenant_id
                    && config.space_id == space_id
                    && embd_id.is_none_or(|value| config.embd_id == value)
            })
            .cloned()
            .unwrap_or_else(|| {
                SkillSearchConfig::defaults(tenant_id, &space_id, embd_id.unwrap_or_default())
            })
    }

    pub fn update_config(
        &self,
        tenant_id: &str,
        request: UpdateSkillSearchConfigRequest,
    ) -> anyhow::Result<SkillSearchConfig> {
        validate_config_request(&request)?;
        let space_id = normalize_space_id(&request.space_id);
        let now = unix_ms_i64();
        self.mutate(|state| {
            let config = if let Some(config) = state
                .configs
                .iter_mut()
                .find(|config| config.tenant_id == tenant_id && config.space_id == space_id)
            {
                config
            } else {
                state.configs.push(SkillSearchConfig {
                    id: uuid::Uuid::new_v4().simple().to_string(),
                    create_time: now,
                    update_time: now,
                    ..SkillSearchConfig::defaults(tenant_id, &space_id, &request.embd_id)
                });
                state.configs.last_mut().expect("config was inserted")
            };
            config.embd_id = request.embd_id;
            config.vector_similarity_weight = request.vector_similarity_weight;
            config.similarity_threshold = request.similarity_threshold;
            config.field_config = request.field_config;
            config.rerank_id = request.rerank_id.filter(|value| !value.trim().is_empty());
            config.top_k = request.top_k;
            config.update_time = now;
            Ok(config.clone())
        })
    }

    pub fn index_skills(
        &self,
        tenant_id: &str,
        space_id: &str,
        skills: Vec<SkillInfo>,
        embeddings: Vec<Vec<f32>>,
    ) -> anyhow::Result<usize> {
        if skills.is_empty() {
            return Ok(0);
        }
        if skills.len() != embeddings.len() {
            anyhow::bail!("Embedding result count does not match skill count");
        }
        let dimension = embeddings.first().map(Vec::len).unwrap_or(0);
        if dimension == 0
            || embeddings.iter().any(|embedding| {
                embedding.len() != dimension || embedding.iter().any(|value| !value.is_finite())
            })
        {
            anyhow::bail!("Skill embeddings must be finite vectors with one shared dimension");
        }
        let space_id = normalize_space_id(space_id);
        let now = unix_ms_i64();
        let mut records = Vec::with_capacity(skills.len());
        for (skill, embedding) in skills.into_iter().zip(embeddings) {
            let skill_id = skill.id.trim();
            let name = skill.name.trim();
            if skill_id.is_empty() || name.is_empty() {
                anyhow::bail!("Skill id and name are required");
            }
            let mut seen = HashSet::new();
            let tags = skill
                .tags
                .into_iter()
                .map(|tag| tag.trim().to_string())
                .filter(|tag| !tag.is_empty() && seen.insert(tag.clone()))
                .collect();
            records.push(SkillIndexRecord {
                tenant_id: tenant_id.to_string(),
                skill_id: skill_id.replace('/', "_"),
                space_id: space_id.clone(),
                folder_id: skill.folder_id.trim().to_string(),
                name: name.to_string(),
                tags,
                description: skill.description,
                content: skill.content,
                version: if skill.version.trim().is_empty() {
                    default_index_version()
                } else {
                    skill.version.trim().to_string()
                },
                status: default_status(),
                create_time: now,
                update_time: now,
                embedding,
            });
        }
        let count = records.len();
        self.mutate(|state| {
            for record in records {
                state.records.retain(|existing| {
                    existing.tenant_id != tenant_id
                        || existing.space_id != space_id
                        || (existing.name != record.name && existing.skill_id != record.skill_id)
                });
                state.records.push(record);
            }
            Ok(count)
        })
    }

    pub fn delete_skill(
        &self,
        tenant_id: &str,
        space_id: &str,
        skill_id: &str,
    ) -> anyhow::Result<bool> {
        let space_id = normalize_space_id(space_id);
        let skill_id = skill_id.replace('/', "_");
        self.mutate(|state| {
            let before = state.records.len();
            state.records.retain(|record| {
                record.tenant_id != tenant_id
                    || record.space_id != space_id
                    || record.skill_id != skill_id
            });
            Ok(before != state.records.len())
        })
    }

    pub fn skill_infos(&self, tenant_id: &str, space_id: &str) -> Vec<SkillInfo> {
        let space_id = normalize_space_id(space_id);
        self.state
            .read()
            .expect("skill index lock poisoned")
            .records
            .iter()
            .filter(|record| record.tenant_id == tenant_id && record.space_id == space_id)
            .map(SkillIndexRecord::info)
            .collect()
    }

    pub fn replace_embeddings_and_bump_version(
        &self,
        tenant_id: &str,
        space_id: &str,
        embd_id: &str,
        embeddings: Vec<Vec<f32>>,
    ) -> anyhow::Result<(usize, String)> {
        let space_id = normalize_space_id(space_id);
        let expected = self
            .state
            .read()
            .expect("skill index lock poisoned")
            .records
            .iter()
            .filter(|record| record.tenant_id == tenant_id && record.space_id == space_id)
            .count();
        if expected != embeddings.len() {
            anyhow::bail!("Embedding result count does not match indexed skills");
        }
        if let Some(first) = embeddings.first()
            && (first.is_empty()
                || embeddings.iter().any(|embedding| {
                    embedding.len() != first.len()
                        || embedding.iter().any(|value| !value.is_finite())
                }))
        {
            anyhow::bail!("Skill embeddings must share one finite non-empty dimension");
        }
        let now = unix_ms_i64();
        self.mutate(|state| {
            for (record, embedding) in state
                .records
                .iter_mut()
                .filter(|record| record.tenant_id == tenant_id && record.space_id == space_id)
                .zip(embeddings)
            {
                record.embedding = embedding;
                record.update_time = now;
            }
            let config = if let Some(config) = state
                .configs
                .iter_mut()
                .find(|config| config.tenant_id == tenant_id && config.space_id == space_id)
            {
                config
            } else {
                state.configs.push(SkillSearchConfig {
                    id: uuid::Uuid::new_v4().simple().to_string(),
                    create_time: now,
                    update_time: now,
                    ..SkillSearchConfig::defaults(tenant_id, &space_id, embd_id)
                });
                state.configs.last_mut().expect("config was inserted")
            };
            config.embd_id = embd_id.to_string();
            config.index_version = increment_semantic_version(&config.index_version);
            config.update_time = now;
            Ok((expected, config.index_version.clone()))
        })
    }

    pub fn search(
        &self,
        tenant_id: &str,
        request: &SkillSearchRequest,
        config: &SkillSearchConfig,
        query_vector: Option<&[f32]>,
    ) -> SkillSearchResponse {
        let space_id = normalize_space_id(&request.space_id);
        let records: Vec<_> = self
            .state
            .read()
            .expect("skill index lock poisoned")
            .records
            .iter()
            .filter(|record| record.tenant_id == tenant_id && record.space_id == space_id)
            .cloned()
            .collect();
        search_records(records, request, config, query_vector)
    }

    fn mutate<T>(
        &self,
        mutation: impl FnOnce(&mut SkillIndexState) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _save_guard = self.save_lock.lock().expect("skill save lock poisoned");
        let mut state = self.state.write().expect("skill index lock poisoned");
        let previous = state.clone();
        let value = mutation(&mut state)?;
        let previous_chunks = skill_chunks(&previous);
        let current_chunks = skill_chunks(&state);
        if let Err(error) = self
            .vector_mirror
            .sync_snapshot(&previous_chunks, &current_chunks)
        {
            *state = previous;
            return Err(error);
        }
        if let Err(error) = self.persist(&state) {
            if let Err(rollback_error) = self
                .vector_mirror
                .sync_snapshot(&current_chunks, &previous_chunks)
            {
                tracing::error!(
                    %rollback_error,
                    "Failed to roll back zvec Skill index after snapshot failure"
                );
            }
            *state = previous;
            return Err(error);
        }
        Ok(value)
    }

    fn persist_current(&self) -> anyhow::Result<()> {
        let _save_guard = self.save_lock.lock().expect("skill save lock poisoned");
        self.persist(&self.state.read().expect("skill index lock poisoned"))
    }

    fn persist(&self, state: &SkillIndexState) -> anyhow::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let snapshot = encode_snapshot(state)?;
        crate::persistence::atomic_write(path, &serde_json::to_vec_pretty(&snapshot)?)
    }
}

fn skill_chunks(state: &SkillIndexState) -> Vec<IndexedChunk> {
    state
        .records
        .iter()
        .map(|record| {
            let partition_key = format!("{}\0{}", record.tenant_id, record.space_id);
            let collection_id = format!(
                "skill_{:032x}",
                xxhash_rust::xxh3::xxh3_128(partition_key.as_bytes())
            );
            IndexedChunk {
                id: record.skill_id.clone(),
                doc_name: record.name.clone(),
                content: [
                    record.name.as_str(),
                    &record.tags.join(", "),
                    record.description.as_str(),
                    record.content.as_str(),
                ]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
                embedding: record.embedding.clone(),
                token_count: 0,
                position: 0,
                metadata: std::collections::HashMap::from([
                    ("kb_id".into(), collection_id),
                    ("tenant_id".into(), record.tenant_id.clone()),
                    ("space_id".into(), record.space_id.clone()),
                    ("skill_id".into(), record.skill_id.clone()),
                    ("status".into(), record.status.clone()),
                    ("content_type".into(), "skill".into()),
                ]),
            }
        })
        .collect()
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSkillSearchConfigRequest {
    #[serde(default)]
    pub space_id: String,
    pub embd_id: String,
    pub vector_similarity_weight: f64,
    pub similarity_threshold: f64,
    #[serde(default)]
    pub field_config: SkillFieldConfig,
    #[serde(default)]
    pub rerank_id: Option<String>,
    pub top_k: usize,
}

#[derive(Debug, Deserialize, Default)]
pub struct GetSkillConfigQuery {
    pub space_id: Option<String>,
    pub embd_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IndexSkillsRequest {
    pub skills: Vec<SkillInfo>,
    #[serde(default)]
    pub space_id: String,
    #[serde(default)]
    pub embd_id: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct DeleteSkillIndexQuery {
    pub skill_id: Option<String>,
    pub space_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SkillSearchRequest {
    #[serde(default)]
    pub space_id: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub page: usize,
    #[serde(default)]
    pub page_size: usize,
    #[serde(default)]
    pub sort_by: String,
    #[serde(default)]
    pub sort_order: String,
}

#[derive(Debug, Deserialize)]
pub struct ReindexSkillsRequest {
    pub space_id: String,
    #[serde(default)]
    pub embd_id: String,
}

pub async fn get_skill_config(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<GetSkillConfigQuery>,
) -> axum::response::Response {
    let space_id = query.space_id.as_deref().unwrap_or_default();
    let config = state
        .skill_index
        .get_config(&auth.user_id, space_id, query.embd_id.as_deref());
    Json(serde_json::json!({ "code": 0, "data": config, "message": "success" })).into_response()
}

pub async fn update_skill_config(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<UpdateSkillSearchConfigRequest>,
) -> axum::response::Response {
    match state.skill_index.update_config(&auth.user_id, request) {
        Ok(config) => Json(serde_json::json!({ "code": 0, "data": config, "message": "success" }))
            .into_response(),
        Err(error) => skill_bad_request(error),
    }
}

pub async fn index_skills(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<IndexSkillsRequest>,
) -> axum::response::Response {
    if request.skills.is_empty() {
        return skill_bad_request(anyhow::anyhow!("skills is required"));
    }
    let space_id = normalize_space_id(&request.space_id);
    let config = state.skill_index.get_config(&auth.user_id, &space_id, None);
    let embd_id = if request.embd_id.trim().is_empty() {
        config.embd_id.clone()
    } else {
        request.embd_id.trim().to_string()
    };
    if embd_id.is_empty() {
        return skill_bad_request(anyhow::anyhow!(
            "no embedding model configured in skill search config"
        ));
    }
    let embedder = match skill_embedder_for(&state, &auth.user_id, &embd_id) {
        Ok(embedder) => embedder,
        Err(error) => return skill_bad_request(error),
    };
    let texts: Vec<_> = request
        .skills
        .iter()
        .map(|skill| build_vector_text(skill, &config.field_config))
        .collect();
    let text_refs: Vec<_> = texts.iter().map(String::as_str).collect();
    let embeddings = match embedder.embed(&text_refs).await {
        Ok(embeddings) => embeddings,
        Err(error) => return skill_bad_request(anyhow::anyhow!(error.to_string())),
    };
    match state
        .skill_index
        .index_skills(&auth.user_id, &space_id, request.skills, embeddings)
    {
        Ok(indexed_count) => Json(serde_json::json!({
            "code": 0,
            "data": { "indexed_count": indexed_count },
            "message": "success"
        }))
        .into_response(),
        Err(error) => skill_bad_request(error),
    }
}

pub async fn delete_skill_index(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Query(query): Query<DeleteSkillIndexQuery>,
) -> axum::response::Response {
    let skill_id = query.skill_id.as_deref().unwrap_or_default().trim();
    if skill_id.is_empty() {
        return skill_bad_request(anyhow::anyhow!("skill_id is required"));
    }
    match state.skill_index.delete_skill(
        &auth.user_id,
        query.space_id.as_deref().unwrap_or_default(),
        skill_id,
    ) {
        Ok(_) => Json(serde_json::json!({
            "code": 0,
            "data": true,
            "message": "success"
        }))
        .into_response(),
        Err(error) => skill_bad_request(error),
    }
}

pub async fn search_skills(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<SkillSearchRequest>,
) -> axum::response::Response {
    let config = state
        .skill_index
        .get_config(&auth.user_id, &request.space_id, None);
    let mut query_vector = None;
    if !request.query.trim().is_empty()
        && config.vector_similarity_weight > 0.0
        && !config.embd_id.is_empty()
        && let Ok(embedder) = skill_embedder_for(&state, &auth.user_id, &config.embd_id)
        && let Ok(mut embeddings) = embedder.embed(&[request.query.as_str()]).await
        && embeddings.len() == 1
        && !embeddings[0].is_empty()
    {
        query_vector = embeddings.pop();
    }
    let response =
        state
            .skill_index
            .search(&auth.user_id, &request, &config, query_vector.as_deref());
    Json(serde_json::json!({ "code": 0, "data": response, "message": "success" })).into_response()
}

pub async fn reindex_skills(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(request): Json<ReindexSkillsRequest>,
) -> axum::response::Response {
    let config = state
        .skill_index
        .get_config(&auth.user_id, &request.space_id, None);
    let embd_id = if request.embd_id.trim().is_empty() {
        config.embd_id
    } else {
        request.embd_id.trim().to_string()
    };
    if embd_id.is_empty() {
        return skill_bad_request(anyhow::anyhow!(
            "no embedding model configured in skill search config"
        ));
    }
    let skills = state
        .skill_index
        .skill_infos(&auth.user_id, &request.space_id);
    let texts: Vec<_> = skills
        .iter()
        .map(|skill| build_vector_text(skill, &config.field_config))
        .collect();
    let text_refs: Vec<_> = texts.iter().map(String::as_str).collect();
    let embedder = match skill_embedder_for(&state, &auth.user_id, &embd_id) {
        Ok(embedder) => embedder,
        Err(error) => return skill_bad_request(error),
    };
    let embeddings = match embedder.embed(&text_refs).await {
        Ok(embeddings) => embeddings,
        Err(error) => return skill_bad_request(anyhow::anyhow!(error.to_string())),
    };
    match state.skill_index.replace_embeddings_and_bump_version(
        &auth.user_id,
        &request.space_id,
        &embd_id,
        embeddings,
    ) {
        Ok((indexed_count, version)) => Json(serde_json::json!({
            "code": 0,
            "data": {
                "indexed_count": indexed_count,
                "total_skills": indexed_count,
                "version": version,
                "failed_count": 0
            },
            "message": "success"
        }))
        .into_response(),
        Err(error) => skill_bad_request(error),
    }
}

fn skill_bad_request(error: anyhow::Error) -> axum::response::Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "code": 400,
            "data": Value::Null,
            "message": error.to_string()
        })),
    )
        .into_response()
}

fn skill_embedder_for(
    state: &AppState,
    tenant_id: &str,
    selector: &str,
) -> anyhow::Result<crate::embed::SharedEmbedder> {
    if selector == "default" {
        return state
            .embedder
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Embedding is not configured"));
    }
    state
        .tenant_models
        .resolve(
            &state.providers,
            tenant_id,
            crate::api::tenant_models::ModelCapability::Embedding,
            Some(selector),
        )?
        .map(|model| model.embedder())
        .ok_or_else(|| anyhow::anyhow!("Embedding is not configured: {selector}"))
}

fn validate_config_request(request: &UpdateSkillSearchConfigRequest) -> anyhow::Result<()> {
    if request.embd_id.trim().is_empty() {
        anyhow::bail!("embd_id is required");
    }
    if !request.vector_similarity_weight.is_finite()
        || !(0.0..=1.0).contains(&request.vector_similarity_weight)
    {
        anyhow::bail!("vector_similarity_weight must be between 0 and 1");
    }
    if !request.similarity_threshold.is_finite()
        || !(0.0..=1.0).contains(&request.similarity_threshold)
    {
        anyhow::bail!("similarity_threshold must be between 0 and 1");
    }
    if request.top_k == 0 {
        anyhow::bail!("top_k must be positive");
    }
    for field in [
        &request.field_config.name,
        &request.field_config.tags,
        &request.field_config.description,
        &request.field_config.content,
    ] {
        if !field.weight.is_finite() {
            anyhow::bail!("field weights must be finite");
        }
    }
    Ok(())
}

pub fn build_vector_text(skill: &SkillInfo, fields: &SkillFieldConfig) -> String {
    let mut parts = Vec::new();
    if fields.name.enabled && !skill.name.is_empty() {
        parts.push(skill.name.clone());
    }
    if fields.tags.enabled && !skill.tags.is_empty() {
        parts.push(skill.tags.join(" "));
    }
    if fields.description.enabled && !skill.description.is_empty() {
        parts.push(skill.description.clone());
    }
    if fields.content.enabled && !skill.content.is_empty() {
        parts.push(skill.content.clone());
    }
    parts.join("\n\n")
}

/// Build the exact Elasticsearch document shape produced by the fixed Go
/// `SkillIndexerService`. The returned first value is the ES document ID
/// (`/` is not legal there); `skill_id` in `_source` deliberately keeps the
/// original wire value. RayRAG stores the equivalent logical record in its
/// Infinity-compatible JSON snapshot and isolated zvec collection.
pub fn build_skill_es_document(
    skill: &SkillInfo,
    space_id: &str,
    fields: &SkillFieldConfig,
    embedding: Option<&[f32]>,
    timestamp: i64,
) -> anyhow::Result<(String, Value)> {
    let skill_id = skill.id.trim();
    let name = skill.name.trim();
    if skill_id.is_empty() || name.is_empty() {
        anyhow::bail!("Skill id and name are required");
    }
    if embedding
        .is_some_and(|vector| vector.is_empty() || vector.iter().any(|value| !value.is_finite()))
    {
        anyhow::bail!("Skill embedding must be a finite non-empty vector");
    }

    let mut source = Map::new();
    source.insert("skill_id".into(), Value::String(skill_id.to_string()));
    source.insert(
        "space_id".into(),
        Value::String(normalize_space_id(space_id)),
    );
    source.insert("folder_id".into(), Value::String(skill.folder_id.clone()));
    source.insert("name".into(), Value::String(name.to_string()));
    source.insert("tags".into(), Value::String(skill.tags.join(", ")));
    source.insert(
        "description".into(),
        Value::String(skill.description.clone()),
    );
    source.insert("content".into(), Value::String(skill.content.clone()));
    source.insert(
        "version".into(),
        Value::String(if skill.version.is_empty() {
            default_index_version()
        } else {
            skill.version.clone()
        }),
    );
    source.insert("status".into(), Value::String(default_status()));
    source.insert("create_time".into(), Value::from(timestamp));
    source.insert("update_time".into(), Value::from(timestamp));

    // Go always tokenizes name/tags. Description/content are present only
    // when their indexing switches are enabled.
    source.insert(
        "name_tks".into(),
        Value::String(crate::nlp::rag_tokenize(&skill.name)),
    );
    source.insert(
        "tags_tks".into(),
        Value::String(crate::nlp::rag_tokenize(&skill.tags.join(" "))),
    );
    if fields.description.enabled {
        source.insert(
            "description_tks".into(),
            Value::String(crate::nlp::rag_tokenize(&skill.description)),
        );
    }
    if fields.content.enabled {
        source.insert(
            "content_tks".into(),
            Value::String(crate::nlp::rag_tokenize(&skill.content)),
        );
    }
    if let Some(embedding) = embedding {
        source.insert(
            format!("q_{}_vec", embedding.len()),
            serde_json::to_value(embedding)?,
        );
    }

    Ok((skill_id.replace('/', "_"), Value::Object(source)))
}

fn normalize_space_id(space_id: &str) -> String {
    let value = space_id.trim();
    if value.is_empty() {
        default_space_id()
    } else {
        value.to_string()
    }
}

fn unix_ms_i64() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn increment_semantic_version(version: &str) -> String {
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3 {
        return default_index_version();
    }
    let (Ok(mut major), Ok(mut minor), Ok(mut patch)) = (
        parts[0].parse::<u64>(),
        parts[1].parse::<u64>(),
        parts[2].parse::<u64>(),
    ) else {
        return default_index_version();
    };
    patch += 1;
    if patch > 999 {
        patch = 0;
        minor += 1;
        if minor > 999 {
            minor = 0;
            major += 1;
        }
    }
    format!("{major}.{minor}.{patch}")
}

fn field_tokens(value: &str) -> Vec<String> {
    crate::nlp::rag_fine_grained_tokenize(&crate::nlp::rag_tokenize(value))
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

fn keyword_score(
    query_tokens: &[String],
    record: &SkillIndexRecord,
    fields: &SkillFieldConfig,
) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let computer = crate::nlp::TermWeightComputer::new();
    let mut indexed_fields = vec![
        (field_tokens(&record.name), 10.0),
        (field_tokens(&record.tags.join(" ")), 5.0),
    ];
    if fields.description.enabled {
        indexed_fields.push((field_tokens(&record.description), 3.0));
    }
    if fields.content.enabled {
        indexed_fields.push((field_tokens(&record.content), 1.0));
    }
    let mut score = 0.0;
    for (tokens, weight) in indexed_fields {
        score += computer.token_similarity(query_tokens, &[tokens])[0] * weight;
    }
    score / 19.0
}

fn cosine(left: &[f32], right: &[f32]) -> f64 {
    if left.is_empty() || left.len() != right.len() {
        return 0.0;
    }
    let (mut dot, mut left_norm, mut right_norm) = (0.0, 0.0, 0.0);
    for (left, right) in left.iter().zip(right) {
        let left = *left as f64;
        let right = *right as f64;
        dot += left * right;
        left_norm += left * left;
        right_norm += right * right;
    }
    if left_norm == 0.0 || right_norm == 0.0 {
        0.0
    } else {
        dot / (left_norm.sqrt() * right_norm.sqrt())
    }
}

fn search_records(
    records: Vec<SkillIndexRecord>,
    request: &SkillSearchRequest,
    config: &SkillSearchConfig,
    query_vector: Option<&[f32]>,
) -> SkillSearchResponse {
    let query = request.query.trim();
    let query_tokens = field_tokens(query);
    let use_vector = !query.is_empty()
        && config.vector_similarity_weight > 0.0
        && query_vector.is_some_and(|vector| !vector.is_empty());
    let mut search_type = if use_vector {
        if config.vector_similarity_weight >= 1.0 {
            "vector"
        } else {
            "hybrid"
        }
    } else {
        "keyword"
    };

    let rank = |mode: &str| {
        let mut ranked: Vec<_> = records
            .iter()
            .filter(|record| mode == "keyword" || record.status == "1")
            .map(|record| {
                let bm25_score = keyword_score(&query_tokens, record, &config.field_config);
                let vector_score = query_vector
                    .map(|vector| cosine(vector, &record.embedding))
                    .unwrap_or(0.0);
                let score = match mode {
                    "vector" => vector_score,
                    "hybrid" => {
                        bm25_score * (1.0 - config.vector_similarity_weight)
                            + vector_score * config.vector_similarity_weight
                    }
                    _ => bm25_score,
                };
                (record, score, bm25_score, vector_score)
            })
            .filter(|(_, score, _, _)| query.is_empty() || *score >= config.similarity_threshold)
            .collect();
        if query.is_empty() || mode == "keyword" {
            sort_keyword_results(&mut ranked, request, query.is_empty());
        } else {
            ranked.sort_by(|left, right| {
                right
                    .1
                    .total_cmp(&left.1)
                    .then_with(|| right.0.update_time.cmp(&left.0.update_time))
            });
            ranked.truncate(config.top_k);
        }
        ranked
    };

    let mut ranked = rank(search_type);
    if use_vector && ranked.is_empty() {
        search_type = "keyword";
        ranked = rank(search_type);
    }
    ranked.truncate(100);
    let total = ranked.len();
    let page = request.page.max(1);
    let page_size = if request.page_size == 0 {
        10
    } else {
        request.page_size
    };
    let start = page.saturating_sub(1).saturating_mul(page_size).min(total);
    let skills = ranked
        .into_iter()
        .skip(start)
        .take(page_size)
        .map(
            |(record, score, bm25_score, vector_score)| SkillSearchResult {
                skill_id: record.skill_id.clone(),
                folder_id: record.folder_id.clone(),
                name: record.name.clone(),
                description: record.description.clone(),
                tags: record.tags.clone(),
                score,
                bm25_score,
                vector_score,
                index_version: String::new(),
                create_time: record.create_time,
                version: record.version.clone(),
            },
        )
        .collect();
    SkillSearchResponse {
        skills,
        total,
        query: request.query.clone(),
        search_type: search_type.to_string(),
    }
}

fn sort_keyword_results(
    ranked: &mut [(&SkillIndexRecord, f64, f64, f64)],
    request: &SkillSearchRequest,
    empty_query: bool,
) {
    let sort_by = if request.sort_by.is_empty() && empty_query {
        "update_time"
    } else {
        request.sort_by.as_str()
    };
    let descending = match request.sort_order.to_ascii_lowercase().as_str() {
        "asc" => false,
        "desc" => true,
        _ => sort_by != "name",
    };
    ranked.sort_by(|left, right| {
        let order = match sort_by {
            "name" => left.0.name.cmp(&right.0.name),
            "create_time" | "createTime" | "created_at" => {
                left.0.create_time.cmp(&right.0.create_time)
            }
            "update_time" | "updateTime" | "updated_at" => {
                left.0.update_time.cmp(&right.0.update_time)
            }
            "relevance" | "" => left.1.total_cmp(&right.1),
            _ => left.1.total_cmp(&right.1),
        };
        if descending { order.reverse() } else { order }
    });
}

fn encode_snapshot(state: &SkillIndexState) -> anyhow::Result<StoredSkillSnapshot> {
    let mut partitions: BTreeMap<(String, String), Vec<&SkillIndexRecord>> = BTreeMap::new();
    for record in &state.records {
        partitions
            .entry((record.tenant_id.clone(), record.space_id.clone()))
            .or_default()
            .push(record);
    }
    let indexes = partitions
        .into_iter()
        .map(|((tenant_id, space_id), mut records)| {
            records.sort_by(|left, right| {
                left.name
                    .cmp(&right.name)
                    .then_with(|| left.skill_id.cmp(&right.skill_id))
            });
            let rows = records
                .into_iter()
                .map(physical_row)
                .collect::<anyhow::Result<_>>()?;
            Ok(StoredSkillPartition {
                tenant_id,
                space_id,
                rows,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    let mut configs = state.configs.clone();
    configs.sort_by(|left, right| {
        left.tenant_id
            .cmp(&right.tenant_id)
            .then_with(|| left.space_id.cmp(&right.space_id))
    });
    Ok(StoredSkillSnapshot { indexes, configs })
}

fn decode_snapshot(snapshot: StoredSkillSnapshot) -> anyhow::Result<SkillIndexState> {
    let mut records = Vec::new();
    let mut partitions = HashSet::new();
    for partition in snapshot.indexes {
        let space_id = normalize_space_id(&partition.space_id);
        if partition.tenant_id.trim().is_empty()
            || !partitions.insert((partition.tenant_id.clone(), space_id.clone()))
        {
            anyhow::bail!("Invalid or duplicate skill index partition");
        }
        for row in partition.rows {
            let record = decode_physical_row(&partition.tenant_id, row)?;
            if record.space_id != space_id {
                anyhow::bail!("Skill row space_id does not match its partition");
            }
            records.push(record);
        }
    }
    let mut configs_seen = HashSet::new();
    for config in &snapshot.configs {
        if config.tenant_id.trim().is_empty()
            || !configs_seen.insert((config.tenant_id.clone(), config.space_id.clone()))
        {
            anyhow::bail!("Invalid or duplicate skill search config");
        }
    }
    Ok(SkillIndexState {
        records,
        configs: snapshot.configs,
    })
}

fn physical_row(record: &SkillIndexRecord) -> anyhow::Result<Value> {
    let mut row = Map::new();
    row.insert("skill_id".into(), Value::String(record.skill_id.clone()));
    row.insert("space_id".into(), Value::String(record.space_id.clone()));
    row.insert("folder_id".into(), Value::String(record.folder_id.clone()));
    row.insert("name".into(), Value::String(record.name.clone()));
    row.insert("tags".into(), Value::String(record.tags.join(", ")));
    row.insert(
        "description".into(),
        Value::String(record.description.clone()),
    );
    row.insert("content".into(), Value::String(record.content.clone()));
    row.insert("version".into(), Value::String(record.version.clone()));
    row.insert("status".into(), Value::String(record.status.clone()));
    row.insert("create_time".into(), Value::from(record.create_time));
    row.insert("update_time".into(), Value::from(record.update_time));
    row.insert(
        format!("q_{}_vec", record.embedding.len()),
        serde_json::to_value(&record.embedding)?,
    );
    Ok(Value::Object(row))
}

fn decode_physical_row(tenant_id: &str, row: Value) -> anyhow::Result<SkillIndexRecord> {
    let mut row = row
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Skill physical row must be an object"))?;
    let skill_id = take_string(&mut row, "skill_id")?;
    let space_id = normalize_space_id(&take_string(&mut row, "space_id")?);
    let folder_id = take_string(&mut row, "folder_id")?;
    let name = take_string(&mut row, "name")?;
    let tags = take_string(&mut row, "tags")?
        .split(',')
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect();
    let description = take_string(&mut row, "description")?;
    let content = take_string(&mut row, "content")?;
    let version = take_string(&mut row, "version")?;
    let status = take_string(&mut row, "status")?;
    let create_time = take_i64(&mut row, "create_time")?;
    let update_time = take_i64(&mut row, "update_time")?;
    let vector_keys: Vec<_> = row
        .keys()
        .filter(|key| {
            key.strip_prefix("q_")
                .and_then(|value| value.strip_suffix("_vec"))
                .is_some_and(|dimension| dimension.parse::<usize>().is_ok())
        })
        .cloned()
        .collect();
    if vector_keys.len() != 1 {
        anyhow::bail!("Skill physical row must contain exactly one q_<dimension>_vec field");
    }
    let vector_key = &vector_keys[0];
    let dimension = vector_key[2..vector_key.len() - 4].parse::<usize>()?;
    let embedding: Vec<f32> = serde_json::from_value(
        row.remove(vector_key)
            .ok_or_else(|| anyhow::anyhow!("Missing skill vector"))?,
    )?;
    if embedding.len() != dimension
        || embedding.is_empty()
        || embedding.iter().any(|value| !value.is_finite())
    {
        anyhow::bail!("Skill vector dimension or values are invalid");
    }
    if !row.is_empty() {
        anyhow::bail!("Unexpected skill physical fields: {:?}", row.keys());
    }
    if skill_id.is_empty() || name.is_empty() {
        anyhow::bail!("Skill id and name are required");
    }
    Ok(SkillIndexRecord {
        tenant_id: tenant_id.to_string(),
        skill_id,
        space_id,
        folder_id,
        name,
        tags,
        description,
        content,
        version,
        status,
        create_time,
        update_time,
        embedding,
    })
}

fn take_string(row: &mut Map<String, Value>, key: &str) -> anyhow::Result<String> {
    row.remove(key)
        .and_then(|value| value.as_str().map(str::to_string))
        .ok_or_else(|| anyhow::anyhow!("Skill field {key} must be a string"))
}

fn take_i64(row: &mut Map<String, Value>, key: &str) -> anyhow::Result<i64> {
    row.remove(key)
        .and_then(|value| value.as_i64())
        .ok_or_else(|| anyhow::anyhow!("Skill field {key} must be an integer"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!("rayrag-skill-index-{}.json", uuid::Uuid::new_v4()))
    }

    fn skill(id: &str, name: &str, description: &str) -> SkillInfo {
        SkillInfo {
            id: id.into(),
            folder_id: format!("folder-{id}"),
            name: name.into(),
            description: description.into(),
            tags: vec!["rust".into(), "search".into()],
            content: format!("{description} implementation guide"),
            version: "2.1.0".into(),
        }
    }

    #[test]
    fn physical_snapshot_uses_exact_infinity_fields_and_restores() {
        let path = temp_path();
        let store = SkillIndexStore::new(&path).unwrap();
        store
            .index_skills(
                "tenant-a",
                "Space-A",
                vec![skill("skill/a", "Rust Search", "hybrid retrieval")],
                vec![vec![1.0, 0.0, 0.5]],
            )
            .unwrap();
        let snapshot: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let row = snapshot["indexes"][0]["rows"][0].as_object().unwrap();
        let expected: HashSet<_> = [
            "skill_id",
            "space_id",
            "folder_id",
            "name",
            "tags",
            "description",
            "content",
            "version",
            "status",
            "create_time",
            "update_time",
            "q_3_vec",
        ]
        .into_iter()
        .collect();
        assert_eq!(
            row.keys().map(String::as_str).collect::<HashSet<_>>(),
            expected
        );
        assert_eq!(row["skill_id"], "skill_a");
        assert_eq!(row["space_id"], "Space-A");
        assert_eq!(row["tags"], "rust, search");
        assert_eq!(row["version"], "2.1.0");
        assert_eq!(row["status"], "1");
        assert_eq!(row["q_3_vec"], serde_json::json!([1.0, 0.0, 0.5]));
        drop(store);

        let restored = SkillIndexStore::new(&path).unwrap();
        let infos = restored.skill_infos("tenant-a", "Space-A");
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].name, "Rust Search");
        assert!(restored.skill_infos("tenant-b", "Space-A").is_empty());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn config_upsert_name_replacement_delete_and_search_are_scoped() {
        let store = SkillIndexStore::in_memory();
        let config = store
            .update_config(
                "tenant-a",
                UpdateSkillSearchConfigRequest {
                    space_id: "space".into(),
                    embd_id: "default".into(),
                    vector_similarity_weight: 0.5,
                    similarity_threshold: 0.05,
                    field_config: SkillFieldConfig::default(),
                    rerank_id: None,
                    top_k: 10,
                },
            )
            .unwrap();
        assert_eq!(config.field_config.name.weight, 3.0);
        store
            .index_skills(
                "tenant-a",
                "space",
                vec![
                    skill("first", "Rust Search", "hybrid retrieval"),
                    skill("second", "Cooking", "noodles and soup"),
                ],
                vec![vec![1.0, 0.0], vec![0.0, 1.0]],
            )
            .unwrap();
        store
            .index_skills(
                "tenant-a",
                "space",
                vec![skill("new-id", "Rust Search", "updated retrieval")],
                vec![vec![1.0, 0.0]],
            )
            .unwrap();
        assert_eq!(store.skill_infos("tenant-a", "space").len(), 2);

        let response = store.search(
            "tenant-a",
            &SkillSearchRequest {
                space_id: "space".into(),
                query: "hybrid retrieval".into(),
                page: 1,
                page_size: 10,
                ..Default::default()
            },
            &config,
            Some(&[1.0, 0.0]),
        );
        assert_eq!(response.search_type, "hybrid");
        assert_eq!(response.skills[0].skill_id, "new-id");
        assert!(
            store
                .search(
                    "tenant-b",
                    &SkillSearchRequest {
                        space_id: "space".into(),
                        ..Default::default()
                    },
                    &SkillSearchConfig::defaults("tenant-b", "space", ""),
                    None,
                )
                .skills
                .is_empty()
        );
        assert!(store.delete_skill("tenant-a", "space", "new-id").unwrap());
        assert!(!store.delete_skill("tenant-a", "space", "new-id").unwrap());
    }

    #[test]
    fn vector_text_defaults_and_semver_match_go_contract() {
        let value = skill("id", "Name", "Description");
        assert_eq!(
            build_vector_text(&value, &SkillFieldConfig::default()),
            "Name\n\nrust search\n\nDescription"
        );
        assert_eq!(increment_semantic_version("1.0.0"), "1.0.1");
        assert_eq!(increment_semantic_version("1.0.999"), "1.1.0");
        assert_eq!(increment_semantic_version("bad"), "1.0.0");
    }

    #[test]
    fn es_document_uses_raw_skill_id_and_configured_token_fields() {
        let mut value = skill("skill/a", "Rust 搜索", "semantic cache");
        value.version.clear();
        let fields = SkillFieldConfig::default();
        let (document_id, source) =
            build_skill_es_document(&value, "", &fields, Some(&[1.0, 0.0, 0.5]), 1234).unwrap();
        let source = source.as_object().unwrap();

        assert_eq!(document_id, "skill_a");
        assert_eq!(source["skill_id"], "skill/a");
        assert_eq!(source["space_id"], "default");
        assert_eq!(source["tags"], "rust, search");
        assert_eq!(source["version"], "1.0.0");
        assert_eq!(source["status"], "1");
        assert_eq!(source["create_time"], 1234);
        assert_eq!(source["update_time"], 1234);
        assert_eq!(source["name_tks"], "rust 搜 索 搜索");
        assert_eq!(source["tags_tks"], "rust search");
        assert_eq!(source["description_tks"], "semantic cache");
        assert!(source.get("content_tks").is_none());
        assert_eq!(source["q_3_vec"], serde_json::json!([1.0, 0.0, 0.5]));
        assert_eq!(source.len(), 15);

        let record = SkillIndexRecord {
            tenant_id: "tenant".into(),
            skill_id: "skill_a".into(),
            space_id: "default".into(),
            folder_id: value.folder_id,
            name: value.name,
            tags: value.tags,
            description: value.description,
            content: value.content,
            version: "1.0.0".into(),
            status: "1".into(),
            create_time: 1234,
            update_time: 1234,
            embedding: vec![1.0, 0.0, 0.5],
        };
        let content_query = field_tokens("implementation guide");
        assert!(keyword_score(&content_query, &record, &fields) < 1e-8);
        let mut content_fields = fields;
        content_fields.content.enabled = true;
        assert!(keyword_score(&content_query, &record, &content_fields) > 0.0);
    }

    #[test]
    fn infinity_status_filter_applies_only_to_vector_and_hybrid_search() {
        let store = SkillIndexStore::in_memory();
        store
            .index_skills(
                "tenant-a",
                "space",
                vec![
                    skill("hidden", "Hidden Skill", "inactive result"),
                    skill("active", "Active Skill", "enabled result"),
                ],
                vec![vec![1.0, 0.0], vec![0.0, 1.0]],
            )
            .unwrap();
        store
            .state
            .write()
            .expect("skill index lock poisoned")
            .records
            .iter_mut()
            .find(|record| record.skill_id == "hidden")
            .unwrap()
            .status = "0".into();

        let request = SkillSearchRequest {
            space_id: "space".into(),
            query: "Hidden Skill".into(),
            page: 1,
            page_size: 10,
            ..Default::default()
        };
        let mut config = SkillSearchConfig::defaults("tenant-a", "space", "default");
        config.vector_similarity_weight = 0.0;
        config.similarity_threshold = 0.0;
        let keyword = store.search("tenant-a", &request, &config, None);
        assert_eq!(keyword.search_type, "keyword");
        assert!(
            keyword
                .skills
                .iter()
                .any(|skill| skill.skill_id == "hidden")
        );

        let listing = store.search(
            "tenant-a",
            &SkillSearchRequest {
                space_id: "space".into(),
                page: 1,
                page_size: 10,
                ..Default::default()
            },
            &config,
            None,
        );
        assert_eq!(listing.total, 2);

        config.vector_similarity_weight = 1.0;
        let vector = store.search("tenant-a", &request, &config, Some(&[1.0, 0.0]));
        assert_eq!(vector.search_type, "vector");
        assert!(vector.skills.iter().all(|skill| skill.skill_id != "hidden"));

        config.vector_similarity_weight = 0.5;
        let hybrid = store.search("tenant-a", &request, &config, Some(&[1.0, 0.0]));
        assert_eq!(hybrid.search_type, "hybrid");
        assert!(hybrid.skills.iter().all(|skill| skill.skill_id != "hidden"));
    }
}
