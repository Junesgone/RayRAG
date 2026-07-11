//! OpenAI-compatible embedding API client.
//!
//! Supports any OpenAI-compatible endpoint (OpenAI, MiniMax, local vLLM, etc.).
//! Ported from RAGFlow's `rag/llm/embedding_model.py`.

use super::Embedder;
use crate::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct OpenAIEmbedder {
    client: Client,
    api_base: String,
    api_key: String,
    model: String,
}

#[derive(Serialize)]
struct EmbedRequest {
    model: String,
    input: Vec<String>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

impl OpenAIEmbedder {
    pub fn new(api_base: &str, api_key: &str, model: &str) -> Self {
        Self {
            client: Client::new(),
            api_base: api_base.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl Embedder for OpenAIEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.api_base);
        let req = EmbedRequest {
            model: self.model.clone(),
            input: texts.iter().map(|t| t.to_string()).collect(),
        };

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&req)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!(
                "Embedding API error ({}): {}",
                status,
                body
            ));
        }

        let data: EmbedResponse = resp.json().await?;
        let embeddings: Vec<Vec<f32>> = data.data.into_iter().map(|d| d.embedding).collect();

        Ok(embeddings)
    }
}
