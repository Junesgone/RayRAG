//! Reranker module — re-rank search results using a reranking model.
//!
//! Supports:
//! - Remote HTTP reranker (mxbai-rerank, Cohere API compatible)
//! - Local Candle reranker (planned)
//!
//! API: POST /v1/rerank with JSON body:
//!   {"query": "…", "documents": ["…"], "top_k": N, "return_documents": true}

use crate::Result;
use crate::search::{HybridSearchResult, SearchResult};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Re-ranked search result.
#[derive(Debug, Clone)]
pub struct RerankedResult {
    /// Cosine similarity score (pre-rerank)
    pub score: f32,
    /// Reranker relevance score (0.0–1.0)
    pub rerank_score: f32,
    /// Combined score (weighted average)
    pub combined_score: f32,
    /// The matched chunk text
    pub content: String,
    /// Document name
    pub doc_name: String,
    /// Chunk ID
    pub chunk_id: String,
    /// Rank position (0-based, post-rerank)
    pub rank: usize,
}

/// Reranker trait — compute relevance scores for (query, document) pairs.
#[async_trait::async_trait]
pub trait Reranker: Send + Sync {
    /// Re-rank a list of candidate documents against a query.
    /// Returns indices into `documents` sorted by relevance (most relevant first),
    /// along with relevance scores.
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<(usize, f32)>>;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RerankerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Model the endpoint should rerank with (`RERANK_MODEL`).
    ///
    /// The field was documented but never sent: an operator who set `RERANK_MODEL`
    /// (as the README instructs) got a silently different model, and endpoints that
    /// require a model name — TEI with several models loaded, vLLM, Xinference —
    /// answered with an error the user could not connect to their configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PublicRerankerConfig {
    pub enabled: bool,
    pub api_base: String,
    pub api_key_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

pub struct RerankerManager {
    path: Option<PathBuf>,
    config: RwLock<RerankerConfig>,
    current: RwLock<Option<Arc<dyn Reranker>>>,
}

impl RerankerManager {
    pub fn new(path: impl AsRef<Path>, fallback: RerankerConfig) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let config = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?)?
        } else {
            fallback
        };
        let current = build_reranker(&config)?;
        Ok(Self {
            path: Some(path),
            config: RwLock::new(config),
            current: RwLock::new(current),
        })
    }

    pub fn in_memory(current: Option<Arc<dyn Reranker>>) -> Self {
        Self {
            path: None,
            config: RwLock::new(RerankerConfig {
                enabled: current.is_some(),
                ..RerankerConfig::default()
            }),
            current: RwLock::new(current),
        }
    }

    pub fn current(&self) -> Option<Arc<dyn Reranker>> {
        self.current.read().unwrap().clone()
    }

    pub fn public_config(&self) -> PublicRerankerConfig {
        let config = self.config.read().unwrap();
        PublicRerankerConfig {
            enabled: config.enabled,
            api_base: config.api_base.clone(),
            api_key_configured: config.api_key.as_ref().is_some_and(|key| !key.is_empty()),
            model: config
                .model
                .clone()
                .filter(|model| !model.trim().is_empty()),
        }
    }

    pub fn update(&self, config: RerankerConfig) -> Result<PublicRerankerConfig> {
        let next = build_reranker(&config)?;
        if let Some(path) = &self.path {
            let data = serde_json::to_vec_pretty(&config)?;
            crate::persistence::atomic_write(path, &data)?;
            restrict_config_permissions(path)?;
        }
        *self.config.write().unwrap() = config;
        *self.current.write().unwrap() = next;
        Ok(self.public_config())
    }

    pub fn patch(
        &self,
        enabled: bool,
        api_base: String,
        api_key: Option<String>,
        clear_api_key: bool,
    ) -> Result<PublicRerankerConfig> {
        let current = self.config.read().unwrap().clone();
        let api_key = if clear_api_key {
            None
        } else {
            api_key.filter(|key| !key.is_empty()).or(current.api_key)
        };
        self.update(RerankerConfig {
            enabled,
            api_base,
            api_key,
            // The settings form does not edit the model, so a patch must not drop it.
            model: current.model,
        })
    }
}

#[cfg(unix)]
fn restrict_config_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_config_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn build_reranker(config: &RerankerConfig) -> Result<Option<Arc<dyn Reranker>>> {
    if !config.enabled {
        return Ok(None);
    }
    let api_base = config.api_base.trim();
    let url = reqwest::Url::parse(api_base)?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("Reranker API base must use http or https");
    }
    let reranker = RemoteReranker::new(api_base);
    let reranker = match config.api_key.as_deref().filter(|key| !key.is_empty()) {
        Some(key) => reranker.with_api_key(key),
        None => reranker,
    };
    let reranker = match config
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        Some(model) => reranker.with_model(model),
        None => reranker,
    };
    Ok(Some(Arc::new(reranker)))
}

/// Remote HTTP reranker (mxbai-rerank / Cohere API compatible).
///
/// Sends POST to `{base_url}/v1/rerank` with:
/// ```json
/// {"query": "...", "documents": [...], "top_k": N, "model": "bge-reranker-v2-m3"}
/// ```
///
/// `model` is sent only when `RERANK_MODEL` (or the provider's rerank model) names
/// one: a single-model endpoint rejects an unknown name, and omitting the field keeps
/// those deployments working.
///
/// Expects response:
/// ```json
/// {"results": [{"index": 0, "relevance_score": 0.99}, ...]}
/// ```
pub struct RemoteReranker {
    /// Reranker API base URL (e.g., http://127.0.0.1:8899)
    base_url: String,
    /// Optional API key
    api_key: Option<String>,
    /// Optional model name sent as `model` in the request body
    model: Option<String>,
    /// HTTP client
    client: reqwest::Client,
}

impl RemoteReranker {
    /// Create a new remote reranker.
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: None,
            model: None,
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("valid reranker HTTP client configuration"),
        }
    }

    /// Set an API key for the reranker.
    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = Some(key.to_string());
        self
    }

    /// Name the model the endpoint should rerank with (`RERANK_MODEL`).
    pub fn with_model(mut self, model: &str) -> Self {
        let model = model.trim();
        self.model = (!model.is_empty()).then(|| model.to_string());
        self
    }

    fn endpoint(&self) -> String {
        if self.base_url.ends_with("/v1") {
            format!("{}/rerank", self.base_url)
        } else {
            format!("{}/v1/rerank", self.base_url)
        }
    }
}

#[async_trait::async_trait]
impl Reranker for RemoteReranker {
    async fn rerank(
        &self,
        query: &str,
        documents: &[String],
        top_k: usize,
    ) -> Result<Vec<(usize, f32)>> {
        let url = self.endpoint();

        let mut body = serde_json::json!({
            "query": query,
            "documents": documents,
            "top_k": top_k,
        });
        if let Some(model) = &self.model {
            body["model"] = serde_json::Value::String(model.clone());
        }

        let mut req = self.client.post(&url).json(&body);
        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await?;
        let status = resp.status();
        let json: serde_json::Value = resp.json().await?;

        if !status.is_success() {
            let err_msg = json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            anyhow::bail!("Reranker API error ({}): {}", status, err_msg);
        }

        let results = json["results"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid reranker response: missing 'results' array"))?;

        let mut ranked: Vec<(usize, f32)> = Vec::with_capacity(results.len());
        let mut seen = HashSet::new();
        for r in results {
            let index = r["index"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("Invalid reranker response: missing index"))?
                as usize;
            let score = r["relevance_score"].as_f64().ok_or_else(|| {
                anyhow::anyhow!("Invalid reranker response: missing relevance_score")
            })? as f32;
            if index >= documents.len() || !score.is_finite() || !seen.insert(index) {
                anyhow::bail!("Invalid reranker response entry: index={index}, score={score}");
            }
            ranked.push((index, score));
        }
        if ranked.len() != documents.len().min(top_k) {
            anyhow::bail!(
                "Invalid reranker response: expected {} results, got {}",
                documents.len().min(top_k),
                ranked.len()
            );
        }

        Ok(ranked)
    }
}

/// Blend model relevance with lexical similarity using RAGFlow's formula.
pub fn apply_hybrid_rerank(
    candidates: Vec<HybridSearchResult>,
    reranked: &[(usize, f32)],
    vector_weight: f32,
) -> Result<Vec<HybridSearchResult>> {
    let vector_weight = vector_weight.clamp(0.0, 1.0);
    let term_weight = 1.0 - vector_weight;
    let mut seen = HashSet::new();
    let mut output = Vec::with_capacity(reranked.len());

    for &(index, model_score) in reranked {
        if index >= candidates.len() || !model_score.is_finite() || !seen.insert(index) {
            anyhow::bail!("Invalid reranker result: index={index}, score={model_score}");
        }
        let mut result = candidates[index].clone();
        result.term_score = result.model_term_score;
        result.vector_score = model_score.clamp(0.0, 1.0);
        result.score = term_weight * result.term_score
            + vector_weight * result.vector_score
            + result.rank_feature_score;
        output.push(result);
    }

    output.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.chunk.id.cmp(&b.chunk.id))
    });
    for (rank, result) in output.iter_mut().enumerate() {
        result.rank = rank;
    }
    Ok(output)
}

/// Candidate window used for stable block-based pagination.
pub fn rerank_window(page_size: usize, top: Option<usize>) -> usize {
    if page_size <= 1 {
        return top.map_or(30, |limit| limit.min(30)).max(1);
    }
    let mut window = 64_usize.div_ceil(page_size) * page_size;
    if let Some(limit) = top.filter(|limit| *limit > 0) {
        window = window.min(limit.div_ceil(page_size) * page_size);
    }
    window.max(page_size)
}

/// Convenience: create the default reranker (mxbai-rerank on localhost:8899).
pub fn default_reranker() -> RemoteReranker {
    RemoteReranker::new("http://127.0.0.1:8899")
}

/// Re-rank search results (sync wrapper — use in CLI for now).
pub fn apply_rerank(results: Vec<SearchResult>, reranked: &[(usize, f32)]) -> Vec<RerankedResult> {
    let alpha = 0.3; // Weight for cosine similarity vs rerank score

    let mut output = Vec::with_capacity(reranked.len());
    let mut seen = HashSet::new();
    for (idx, rerank_score) in reranked {
        if *idx >= results.len() || !rerank_score.is_finite() || !seen.insert(*idx) {
            continue;
        }
        let sr = &results[*idx];
        let combined = alpha * sr.score + (1.0 - alpha) * rerank_score;
        output.push(RerankedResult {
            score: sr.score,
            rerank_score: *rerank_score,
            combined_score: combined,
            content: sr.chunk.content.clone(),
            doc_name: sr.chunk.doc_name.clone(),
            chunk_id: sr.chunk.id.clone(),
            rank: 0,
        });
    }
    // Sort by combined score descending
    output.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (i, r) in output.iter_mut().enumerate() {
        r.rank = i;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::IndexedChunk;
    use std::collections::HashMap;

    fn candidate(id: &str, term_score: f32) -> HybridSearchResult {
        HybridSearchResult {
            chunk: IndexedChunk {
                id: id.into(),
                doc_name: "doc".into(),
                content: id.into(),
                embedding: Vec::new(),
                token_count: 1,
                position: 0,
                metadata: HashMap::new(),
            },
            score: term_score,
            vector_score: 0.0,
            term_score,
            model_term_score: term_score,
            rank_feature_score: 0.0,
            rank: 0,
        }
    }

    #[test]
    fn hybrid_rerank_uses_ragflow_weights() {
        let candidates = vec![candidate("term", 1.0), candidate("model", 0.0)];
        let results = apply_hybrid_rerank(candidates, &[(0, 0.0), (1, 1.0)], 0.3).unwrap();
        assert_eq!(results[0].chunk.id, "term");
        assert!((results[0].score - 0.7).abs() < 1e-6);
        assert!((results[1].score - 0.3).abs() < 1e-6);
    }

    #[test]
    fn hybrid_rerank_rejects_duplicate_or_out_of_range_indices() {
        let candidates = vec![candidate("one", 1.0)];
        assert!(apply_hybrid_rerank(candidates.clone(), &[(0, 0.5), (0, 0.4)], 0.3).is_err());
        assert!(apply_hybrid_rerank(candidates, &[(1, 0.5)], 0.3).is_err());
    }

    #[test]
    fn hybrid_rerank_uses_model_branch_term_score() {
        let mut lexical = candidate("lexical", 1.0);
        lexical.model_term_score = 0.0;
        let mut model = candidate("model", 0.0);
        model.model_term_score = 1.0;

        let results =
            apply_hybrid_rerank(vec![lexical, model], &[(0, 0.0), (1, 0.0)], 0.3).unwrap();
        assert_eq!(results[0].chunk.id, "model");
        assert_eq!(results[0].term_score, 1.0);
    }

    #[test]
    fn hybrid_rerank_preserves_rank_feature_boost() {
        let mut boosted = candidate("boosted", 0.0);
        boosted.rank_feature_score = 2.0;
        let plain = candidate("plain", 0.0);

        let results =
            apply_hybrid_rerank(vec![boosted, plain], &[(0, 0.0), (1, 1.0)], 0.3).unwrap();
        assert_eq!(results[0].chunk.id, "boosted");
        assert!((results[0].score - 2.0).abs() < 1e-6);
        assert!((results[1].score - 0.3).abs() < 1e-6);
    }

    #[test]
    fn rerank_window_is_page_aligned_and_bounded() {
        assert_eq!(rerank_window(10, None), 70);
        assert_eq!(rerank_window(10, Some(25)), 30);
        assert_eq!(rerank_window(1, Some(20)), 20);
        assert_eq!(rerank_window(0, None), 30);
    }

    #[test]
    fn reranker_endpoint_does_not_duplicate_openai_v1_prefix() {
        assert_eq!(
            RemoteReranker::new("http://127.0.0.1:8899").endpoint(),
            "http://127.0.0.1:8899/v1/rerank"
        );
        assert_eq!(
            RemoteReranker::new("http://127.0.0.1:8899/v1/").endpoint(),
            "http://127.0.0.1:8899/v1/rerank"
        );
    }

    /// `RERANK_MODEL` used to be documented and never sent. A body without the model
    /// is what a single-model endpoint wants, so it must stay omitted when unset.
    #[tokio::test]
    async fn the_request_names_the_model_only_when_one_is_configured() {
        use axum::{Router, routing::post};
        let captured: std::sync::Arc<tokio::sync::Mutex<Vec<serde_json::Value>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let sink = captured.clone();
        let app = Router::new().route(
            "/v1/rerank",
            post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let sink = sink.clone();
                async move {
                    sink.lock().await.push(body);
                    // Every document needs a score: the client rejects a short answer
                    // rather than silently dropping candidates.
                    axum::Json(serde_json::json!({
                        "results": [
                            {"index": 0, "relevance_score": 0.9},
                            {"index": 1, "relevance_score": 0.1}
                        ]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let documents = vec!["alpha".to_string(), "beta".to_string()];
        let with_model = RemoteReranker::new(&format!("http://{addr}"))
            .with_api_key("secret")
            .with_model("bge-reranker-v2-m3");
        with_model.rerank("question", &documents, 2).await.unwrap();
        let without_model = RemoteReranker::new(&format!("http://{addr}"));
        without_model
            .rerank("question", &documents, 2)
            .await
            .unwrap();

        let requests = captured.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["model"], "bge-reranker-v2-m3");
        assert_eq!(requests[0]["query"], "question");
        assert_eq!(requests[0]["top_k"], 2);
        assert!(
            requests[1].get("model").is_none(),
            "an unset RERANK_MODEL must not send a model name: {}",
            requests[1]
        );

        // The manager builds the same client from the config, and the provider path
        // no longer discards the instance's model.
        let manager = RerankerManager::in_memory(None);
        manager
            .update(RerankerConfig {
                enabled: true,
                api_base: format!("http://{addr}"),
                api_key: Some("secret".into()),
                model: Some("  ".into()),
            })
            .unwrap();
        assert_eq!(manager.public_config().model, None, "blank is not a model");
    }

    #[test]
    fn reranker_manager_persists_config_without_exposing_secret() {
        let root = std::env::temp_dir().join(format!("rayrag-reranker-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("reranker.json");
        let manager = RerankerManager::new(&path, RerankerConfig::default()).unwrap();
        let public = manager
            .update(RerankerConfig {
                enabled: true,
                api_base: "http://127.0.0.1:8899".into(),
                api_key: Some("secret".into()),
                model: Some("bge-reranker-v2-m3".into()),
            })
            .unwrap();
        assert!(public.enabled);
        assert!(public.api_key_configured);
        assert_eq!(public.model.as_deref(), Some("bge-reranker-v2-m3"));
        assert!(manager.current().is_some());

        let reloaded = RerankerManager::new(&path, RerankerConfig::default()).unwrap();
        assert_eq!(reloaded.public_config(), public);
        let serialized = serde_json::to_value(public).unwrap();
        assert!(serialized.get("api_key").is_none());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reranker_manager_rejects_invalid_enabled_url_without_mutation() {
        let manager = RerankerManager::in_memory(None);
        assert!(
            manager
                .update(RerankerConfig {
                    enabled: true,
                    api_base: "file:///tmp/reranker".into(),
                    api_key: None,
                    model: None,
                })
                .is_err()
        );
        assert!(!manager.public_config().enabled);
        assert!(manager.current().is_none());
    }
}
