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

const RERANKER_GUIDANCE: &str = "Check the reranker API base URL, credentials, model name and service status, then retry retrieval. 请检查重排服务地址、凭据、模型和运行状态后重试检索。";

/// Remote HTTP reranker (mxbai-rerank / Cohere API compatible).
///
/// Provider protocol shaping the request and response. RAGFlow proxies every
/// provider through `similarity()`: it asks for `top_n = len(documents)`,
/// normalizes out-of-range scores, sorts and only then applies top-k.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RerankProtocol {
    /// OpenAI-compatible rerank (TEI, vLLM, Xinference, Cohere, Jina, MWS, ...):
    /// POST with `query`/`documents`/`top_n` and a `results` array.
    OpenAi,
    /// NVIDIA retrieval reranking: `query: {text}`, `passages: [{text}]`,
    /// response `rankings: [{index, logit}]`.
    Nvidia,
}

/// Sends POST to the provider endpoint, asking it to score **every** document
/// with `top_n = documents.len()`; the caller sorts and truncates to top-k.
/// Out-of-range scores (NVIDIA logits, raw cosine similarities) are min-max
/// normalized exactly like RAGFlow's `Base._normalize_rank`.
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
            client: if crate::common::cmd_timeout::is_local_model_endpoint(base_url) {
                // A local reranker (llama.cpp / vLLM / a GPU box on the LAN) can be
                // slow; like local chat models it must not be cut off by a timeout.
                crate::common::cmd_timeout::model_client_for(base_url)
            } else {
                reqwest::Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(5))
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                    .expect("valid reranker HTTP client configuration")
            },
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

    fn protocol(&self) -> RerankProtocol {
        let base = self.base_url.to_ascii_lowercase();
        let model = self
            .model
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        if base.contains("ai.api.nvidia.com")
            || base.contains("/retrieval/nvidia")
            || model.starts_with("nvidia/")
            || model.starts_with("nv-")
        {
            RerankProtocol::Nvidia
        } else {
            RerankProtocol::OpenAi
        }
    }

    fn endpoint(&self) -> String {
        if self.protocol() == RerankProtocol::Nvidia {
            return self.nvidia_endpoint();
        }
        if self.base_url.ends_with("/rerank") {
            return self.base_url.clone();
        }
        if self.base_url.ends_with("/v1") {
            format!("{}/rerank", self.base_url)
        } else {
            format!("{}/v1/rerank", self.base_url)
        }
    }

    /// Mirrors RAGFlow `NvidiaRerank.__init__`: a full `/reranking` endpoint is
    /// kept, otherwise the model-specific path is appended to the base URL.
    fn nvidia_endpoint(&self) -> String {
        if self.base_url.ends_with("/reranking") || self.base_url.ends_with("/rerank") {
            return self.base_url.clone();
        }
        let base = if self.base_url.is_empty() {
            "https://ai.api.nvidia.com/v1/retrieval/nvidia/".to_string()
        } else {
            format!("{}/", self.base_url.trim_end_matches('/'))
        };
        match self.model.as_deref() {
            Some("nvidia/nv-rerankqa-mistral-4b-v3") => {
                format!("{base}nv-rerankqa-mistral-4b-v3/reranking")
            }
            _ => format!("{base}reranking"),
        }
    }

    fn nvidia_model(&self) -> Option<String> {
        self.model.as_deref().map(|model| {
            if model == "nvidia/rerank-qa-mistral-4b" {
                "nv-rerank-qa-mistral-4b:1".to_string()
            } else {
                model.to_string()
            }
        })
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
        if documents.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }
        let protocol = self.protocol();
        let url = self.endpoint();
        let mut body = match protocol {
            RerankProtocol::OpenAi => serde_json::json!({
                "query": query,
                "documents": documents,
                // RAGFlow asks every provider for scores covering all input
                // documents, then sorts and takes top-k locally.
                "top_n": documents.len(),
            }),
            RerankProtocol::Nvidia => serde_json::json!({
                "model": self.nvidia_model().unwrap_or_default(),
                "query": {"text": query},
                "passages": documents
                    .iter()
                    .map(|text| serde_json::json!({"text": text}))
                    .collect::<Vec<_>>(),
                "top_n": documents.len(),
            }),
        };
        if protocol == RerankProtocol::OpenAi
            && let Some(model) = &self.model
        {
            body["model"] = serde_json::Value::String(model.clone());
        }

        let mut req = self.client.post(&url).json(&body);
        if let Some(ref key) = self.api_key {
            req = req.bearer_auth(key);
        }

        let resp = req.send().await.map_err(|error| {
            anyhow::anyhow!(
                "Reranker API request failed: {}. {RERANKER_GUIDANCE}",
                error.without_url()
            )
        })?;
        let status = resp.status();
        let json: serde_json::Value = crate::common::cmd_timeout::read_json_limited(
            resp,
            crate::common::cmd_timeout::body_limit_bytes(),
            "Reranker API",
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!(
                "Reranker API response failed (HTTP {status}): {error}. {RERANKER_GUIDANCE}"
            )
        })?;

        if !status.is_success() {
            let err_msg = json
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("Unknown error");
            anyhow::bail!("Reranker API error ({status}): {err_msg}. {RERANKER_GUIDANCE}");
        }

        let mut ranked = match protocol {
            RerankProtocol::OpenAi => parse_rerank_results(&json, documents.len())?,
            RerankProtocol::Nvidia => parse_nvidia_rankings(&json, documents.len())?,
        };
        normalize_rerank_scores(&mut ranked);
        ranked.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked.truncate(top_k.min(documents.len()));
        Ok(ranked)
    }
}

fn parse_rerank_results(json: &serde_json::Value, expected: usize) -> Result<Vec<(usize, f32)>> {
    let results = json["results"].as_array().ok_or_else(|| {
        anyhow::anyhow!("Invalid reranker response: missing 'results' array. {RERANKER_GUIDANCE}")
    })?;
    parse_ranked_entries(results, expected, "relevance_score", "results")
}

fn parse_nvidia_rankings(json: &serde_json::Value, expected: usize) -> Result<Vec<(usize, f32)>> {
    let rankings = json["rankings"].as_array().ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid NVIDIA reranker response: missing 'rankings' array. {RERANKER_GUIDANCE}"
        )
    })?;
    parse_ranked_entries(rankings, expected, "logit", "rankings")
}

fn parse_ranked_entries(
    entries: &[serde_json::Value],
    expected: usize,
    score_field: &str,
    label: &str,
) -> Result<Vec<(usize, f32)>> {
    if entries.len() != expected {
        anyhow::bail!(
            "Invalid reranker response: expected {expected} scores in '{label}', got {}. Ask the endpoint to score every submitted document (top_n), or check the model/provider configuration. {RERANKER_GUIDANCE}",
            entries.len()
        );
    }
    let mut ranked = Vec::with_capacity(entries.len());
    let mut seen = HashSet::new();
    for entry in entries {
        let index = entry["index"].as_u64().ok_or_else(|| {
            anyhow::anyhow!("Invalid reranker response: missing index in '{label}'")
        })? as usize;
        let score = entry[score_field].as_f64().ok_or_else(|| {
            anyhow::anyhow!("Invalid reranker response: missing {score_field} in '{label}'")
        })? as f32;
        if index >= expected || !score.is_finite() || !seen.insert(index) {
            anyhow::bail!(
                "Invalid reranker response entry in '{label}': index={index}, score={score}. {RERANKER_GUIDANCE}"
            );
        }
        ranked.push((index, score));
    }
    Ok(ranked)
}

/// RAGFlow \`Base._normalize_rank\`: calibrated [0,1] scores are preserved, while
/// out-of-range provider output (NVIDIA logits, raw cosine similarities) is
/// min-max mapped. A spreadless batch is clamped instead of being zeroed.
fn normalize_rerank_scores(ranked: &mut [(usize, f32)]) {
    if ranked.is_empty() {
        return;
    }
    let min = ranked
        .iter()
        .map(|entry| entry.1)
        .fold(f32::INFINITY, f32::min);
    let max = ranked
        .iter()
        .map(|entry| entry.1)
        .fold(f32::NEG_INFINITY, f32::max);
    if min >= 0.0 && max <= 1.0 {
        return;
    }
    let span = max - min;
    if span < 1e-3 {
        for entry in ranked.iter_mut() {
            entry.1 = entry.1.clamp(0.0, 1.0);
        }
    } else {
        for entry in ranked.iter_mut() {
            entry.1 = (entry.1 - min) / span;
        }
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
    #[test]
    fn reranker_accepts_a_full_endpoint_and_builds_nvidia_paths() {
        assert_eq!(
            RemoteReranker::new("http://127.0.0.1:8899/v1/rerank").endpoint(),
            "http://127.0.0.1:8899/v1/rerank"
        );
        assert_eq!(
            RemoteReranker::new("http://127.0.0.1:8899")
                .with_model("nvidia/nv-rerankqa-mistral-4b-v3")
                .endpoint(),
            "http://127.0.0.1:8899/nv-rerankqa-mistral-4b-v3/reranking"
        );
    }

    #[tokio::test]
    async fn reranker_scores_all_documents_sorts_and_truncates_top_k() {
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
                    axum::Json(serde_json::json!({
                        "results": [
                            {"index": 0, "relevance_score": 0.2},
                            {"index": 1, "relevance_score": 0.9},
                            {"index": 2, "relevance_score": 0.5},
                            {"index": 3, "relevance_score": 0.7}
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
        let documents = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "gamma".to_string(),
            "delta".to_string(),
        ];
        let ranked = RemoteReranker::new(&format!("http://{addr}"))
            .rerank("question", &documents, 2)
            .await
            .unwrap();
        assert_eq!(ranked, vec![(1, 0.9), (3, 0.7)]);
        let requests = captured.lock().await;
        assert_eq!(requests[0]["top_n"], 4);
        assert_eq!(requests[0]["documents"].as_array().unwrap().len(), 4);
    }

    #[tokio::test]
    async fn reranker_normalizes_out_of_range_scores_like_ragflow() {
        use axum::{Router, routing::post};
        let app = Router::new().route(
            "/v1/rerank",
            post(|| async move {
                axum::Json(serde_json::json!({
                    "results": [
                        {"index": 0, "relevance_score": 10.0},
                        {"index": 1, "relevance_score": -5.0},
                        {"index": 2, "relevance_score": 2.5},
                        {"index": 3, "relevance_score": 0.0}
                    ]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let documents = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let ranked = RemoteReranker::new(&format!("http://{addr}"))
            .rerank("q", &documents, 4)
            .await
            .unwrap();
        assert_eq!(ranked[0].0, 0);
        assert!((ranked[0].1 - 1.0).abs() < 1e-6, "{ranked:?}");
        assert_eq!(ranked[1].0, 2);
        assert!((ranked[1].1 - 0.5).abs() < 1e-6, "{ranked:?}");
        assert_eq!(ranked[2].0, 3);
        assert!((ranked[2].1 - 1.0 / 3.0).abs() < 1e-6, "{ranked:?}");
        assert_eq!(ranked[3], (1, 0.0));
    }

    #[tokio::test]
    async fn nvidia_reranker_uses_passages_rankings_and_logits() {
        use axum::{Router, routing::post};
        let captured: std::sync::Arc<tokio::sync::Mutex<Vec<serde_json::Value>>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let sink = captured.clone();
        let app = Router::new().route(
            "/reranking",
            post(move |axum::Json(body): axum::Json<serde_json::Value>| {
                let sink = sink.clone();
                async move {
                    sink.lock().await.push(body);
                    axum::Json(serde_json::json!({
                        "rankings": [
                            {"index": 0, "logit": 5.0},
                            {"index": 1, "logit": -1.0}
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
        let documents = vec!["first".to_string(), "second".to_string()];
        let ranked = RemoteReranker::new(&format!("http://{addr}"))
            .with_model("nvidia/rerank-qa-mistral-4b")
            .rerank("question", &documents, 1)
            .await
            .unwrap();
        assert_eq!(ranked, vec![(0, 1.0)]);
        let requests = captured.lock().await;
        assert_eq!(requests[0]["model"], "nv-rerank-qa-mistral-4b:1");
        assert_eq!(requests[0]["query"]["text"], "question");
        assert_eq!(requests[0]["passages"][1]["text"], "second");
        assert_eq!(requests[0]["top_n"], 2);
    }

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
        assert_eq!(requests[0]["top_n"], 2);
        assert!(
            requests[0].get("top_k").is_none(),
            "RAGFlow asks for every document with top_n, not top_k: {}",
            requests[0]
        );
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

    #[tokio::test]
    async fn remote_reranker_reports_http_and_json_failures_with_guidance() {
        use axum::{Router, body::Body, http::Response, routing::post};
        for (status, body, expected) in [
            (503, "<html>proxy unavailable</html>", "not JSON"),
            (200, "<html>wrong endpoint</html>", "not JSON"),
            (
                401,
                r#"{"error":{"message":"model unavailable"}}"#,
                "model unavailable",
            ),
        ] {
            let app = Router::new().route(
                "/v1/rerank",
                post(move || async move {
                    Response::builder()
                        .status(status)
                        .body(Body::from(body))
                        .unwrap()
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let base_url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let error = RemoteReranker::new(&base_url)
                .rerank("q", &["doc".into()], 1)
                .await
                .unwrap_err()
                .to_string();
            server.abort();
            let status_text = status.to_string();
            for detail in [
                status_text.as_str(),
                expected,
                "API base URL",
                "credentials",
                "model name",
                "重试检索",
            ] {
                assert!(error.contains(detail), "{error}");
            }
        }
    }

    #[tokio::test]
    async fn remote_reranker_refuses_oversized_response_before_json_decoding() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let limit = crate::common::cmd_timeout::body_limit_bytes();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).await.unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                limit as u64 + 1
            );
            socket.write_all(headers.as_bytes()).await.unwrap();
        });
        let error = RemoteReranker::new(&base_url)
            .rerank("q", &["doc".into()], 1)
            .await
            .unwrap_err()
            .to_string();
        server.await.unwrap();
        for detail in ["Reranker API", "HTTP 200", "exceeds", "limit", "重试检索"] {
            assert!(error.contains(detail), "{error}");
        }
    }

    #[tokio::test]
    async fn remote_reranker_body_errors_do_not_expose_url_credentials() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{")
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
        });
        let base_url =
            format!("http://user-fixture:password-fixture@{address}/?token=token-fixture");
        let error = RemoteReranker::new(&base_url)
            .rerank("q", &["doc".into()], 1)
            .await
            .unwrap_err()
            .to_string();
        server.await.unwrap();
        assert!(error.contains("body read failed"), "{error}");
        assert!(error.contains("HTTP 200"), "{error}");
        for secret in ["user-fixture", "password-fixture", "token-fixture"] {
            assert!(!error.contains(secret), "{error}");
        }
    }

    #[tokio::test]
    async fn remote_reranker_request_errors_do_not_expose_url_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let base_url =
            format!("http://user-fixture:password-fixture@{address}/?token=token-fixture");
        let error = RemoteReranker::new(&base_url)
            .rerank("q", &["doc".into()], 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("request failed"), "{error}");
        assert!(error.contains("credentials"), "{error}");
        for secret in ["user-fixture", "password-fixture", "token-fixture"] {
            assert!(!error.contains(secret), "{error}");
        }
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
