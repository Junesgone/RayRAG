//! Embedding pipeline — hybrid: cloud API + local Candle fallback.
//!
//! Three modes:
//! 1. `openai` — cloud API (OpenAI/MiniMax/Qwen/vLLM)
//! 2. `candle`  — local model (offline/privacy)
//! 3. `hybrid`  — try cloud first, fall back to local

pub mod candle;
pub mod openai;

pub use candle::{CandleConfig, CandleEmbedder, PoolingStrategy};
pub use openai::OpenAIEmbedder;

use crate::{Chunk, Result};
use std::sync::Arc;

/// Embedding model trait.
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    /// Generate embeddings for a batch of text chunks.
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// Embed chunks in-place, setting their `embedding` field.
    async fn embed_chunks(&self, chunks: &mut [Chunk]) -> Result<()> {
        let texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
        let embeddings = self.embed(&texts).await?;
        for (chunk, embedding) in chunks.iter_mut().zip(embeddings) {
            chunk.embedding = Some(embedding);
        }
        Ok(())
    }
}

/// Shared runtime embedder used by the HTTP server.
pub type SharedEmbedder = Arc<dyn Embedder>;

/// Hybrid embedder: tries cloud API first, falls back to local Candle.
pub struct HybridEmbedder {
    primary: Arc<dyn Embedder>,
    fallback: Arc<dyn Embedder>,
}

impl HybridEmbedder {
    /// Create a hybrid embedder with primary (cloud) and fallback (local).
    pub fn new(primary: Box<dyn Embedder>, fallback: Box<dyn Embedder>) -> Self {
        Self {
            primary: Arc::from(primary),
            fallback: Arc::from(fallback),
        }
    }

    /// Try embedding with primary; if it fails, use fallback.
    async fn embed_or_fallback(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        match self.primary.embed(texts).await {
            Ok(embeddings) => {
                tracing::debug!("Hybrid: cloud API success ({} texts)", texts.len());
                Ok(embeddings)
            }
            Err(e) => {
                tracing::warn!(
                    "Hybrid: cloud API failed ({:?}), falling back to local Candle",
                    e
                );
                self.fallback.embed(texts).await
            }
        }
    }
}

#[async_trait::async_trait]
impl Embedder for HybridEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed_or_fallback(texts).await
    }
}

/// Convenience: create an OpenAI embedder pointing to MiniMax (your RAGFlow setup).
pub fn minimax_embedder(api_key: &str) -> OpenAIEmbedder {
    OpenAIEmbedder::new("https://api.minimaxi.com/v1", api_key, "MiniMax-M3")
}

/// Convenience: create an OpenAI-compatible embedder for any endpoint.
pub fn openai_compatible_embedder(api_base: &str, api_key: &str, model: &str) -> OpenAIEmbedder {
    OpenAIEmbedder::new(api_base, api_key, model)
}

/// Default embedder: Candle with all-MiniLM-L6-v2.
///
/// ⚠️  This WILL download the model from HuggingFace (with mirror fallback).
///     Only call this from the UI download trigger, never from background/pipeline code.
///     For pipeline use, prefer `local_embedder(path)` instead.
#[deprecated(
    since = "0.1.0",
    note = "Use local_embedder() instead — models should be downloaded explicitly via UI"
)]
pub async fn default_embedder() -> Result<Box<dyn Embedder>> {
    let embedder = CandleEmbedder::load(CandleConfig::mini()).await?;
    Ok(Box::new(embedder))
}

/// Candle embedder with local model (no download).
pub async fn local_embedder(path: &str) -> Result<Box<dyn Embedder>> {
    let embedder = CandleEmbedder::load(CandleConfig::from_local(path)).await?;
    Ok(Box::new(embedder))
}

/// Hybrid embedder: MiniMax cloud + local Candle fallback.
///
/// ⚠️  The fallback Candle model WILL be downloaded from HuggingFace.
///     Only call from UI paths, never from background pipelines.
#[deprecated(
    since = "0.1.0",
    note = "Use local_embedder() for pipeline code; models should be pre-downloaded via UI"
)]
pub async fn hybrid_embedder(minimax_api_key: &str) -> Result<HybridEmbedder> {
    let cloud = Box::new(minimax_embedder(minimax_api_key));
    // Note: auto-downloads if model not cached — UI path only
    let local = Box::new(CandleEmbedder::load(CandleConfig::mini()).await?);
    Ok(HybridEmbedder::new(cloud, local))
}
