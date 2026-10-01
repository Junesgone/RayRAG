//! OpenAI-compatible embedding API client.
//!
//! Supports any OpenAI-compatible endpoint (OpenAI, MiniMax, local vLLM,
//! llama.cpp, etc.). Ported from RAGFlow's `rag/llm/embedding_model.py`.
//!
//! Two response shapes are accepted so a base URL with or without the `/v1`
//! prefix works against llama.cpp / vLLM / TEI-style servers:
//! 1. Standard OpenAI: `{"object":"list","data":[{"embedding":[f32,...]}]}`
//! 2. llama.cpp bare `/embeddings`: `[{"index":0,"embedding":[[f32,...]]}]`

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
    #[serde(default)]
    embedding: serde_json::Value,
}

impl OpenAIEmbedder {
    pub fn new(api_base: &str, api_key: &str, model: &str) -> Self {
        Self {
            // Model calls get a short, explicit timeout instead of the process-wide
            // command budget: a stalled embedding server must not hold a request (and
            // its buffer) for two hours.
            client: crate::common::cmd_timeout::model_client(),
            api_base: api_base.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }
}

/// Normalize one `embedding` field into a flat `Vec<f32>`.
///
/// The standard OpenAI shape is a flat array `[f32, ...]`; llama.cpp's bare
/// `/embeddings` endpoint wraps each vector in a one-element outer array
/// (`[[f32, ...]]`) for backwards compatibility with its old Python client.
fn flatten_embedding(value: &serde_json::Value) -> Option<Vec<f32>> {
    match value {
        serde_json::Value::Array(outer) if outer.len() == 1 && outer[0].is_array() => {
            outer[0].as_array().map(|inner| parse_f32_array(inner))
        }
        serde_json::Value::Array(flat) => Some(parse_f32_array(flat)),
        _ => None,
    }
}

fn parse_f32_array(values: &[serde_json::Value]) -> Vec<f32> {
    values
        .iter()
        .filter_map(|value| value.as_f64().map(|n| n as f32))
        .collect()
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

        // Bounded read: the endpoint decides how much it sends, the limit decides how
        // much this process ever holds.
        let body: serde_json::Value = crate::common::cmd_timeout::read_json_limited(
            resp,
            crate::common::cmd_timeout::body_limit_bytes(),
            "Embedding API",
        )
        .await?;
        let embeddings: Vec<Vec<f32>> =
            if let Some(data) = body.get("data").and_then(|d| d.as_array()) {
                // Standard OpenAI shape.
                data.iter()
                    .filter_map(|item| flatten_embedding(item.get("embedding")?))
                    .collect()
            } else if let Some(items) = body.as_array() {
                // llama.cpp bare /embeddings shape: [{index, embedding:[[...]]}].
                items
                    .iter()
                    .filter_map(|item| flatten_embedding(item.get("embedding")?))
                    .collect()
            } else {
                return Err(anyhow::anyhow!(
                    "Embedding API response has unexpected shape: {}",
                    body
                ));
            };

        if embeddings.len() != texts.len() {
            return Err(anyhow::anyhow!(
                "Embedding API returned {} vectors for {} texts",
                embeddings.len(),
                texts.len()
            ));
        }
        Ok(embeddings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_standard_openai_embedding() {
        let value = serde_json::json!([0.1, 0.2, 0.3]);
        assert_eq!(flatten_embedding(&value), Some(vec![0.1, 0.2, 0.3]));
    }

    #[test]
    fn flattens_llama_cpp_nested_embedding() {
        let value = serde_json::json!([[0.1, 0.2, 0.3]]);
        assert_eq!(flatten_embedding(&value), Some(vec![0.1, 0.2, 0.3]));
    }

    #[test]
    fn rejects_non_array_embedding() {
        assert_eq!(flatten_embedding(&serde_json::json!("nope")), None);
        assert_eq!(flatten_embedding(&serde_json::json!(42)), None);
        // Nested-but-empty resolves to Some(empty); embedding vector itself
        // must be an array of numbers to be usable.
        assert_eq!(
            flatten_embedding(&serde_json::json!([[[]]])),
            Some(Vec::<f32>::new())
        );
    }

    #[test]
    fn parses_standard_openai_response_shape() {
        let body = serde_json::json!({
            "object": "list",
            "data": [
                {"embedding": [1.0, 2.0], "index": 0},
                {"embedding": [3.0, 4.0], "index": 1}
            ]
        });
        let parsed: Vec<Vec<f32>> = body
            .get("data")
            .and_then(|d| d.as_array())
            .map(|data| {
                data.iter()
                    .filter_map(|item| flatten_embedding(item.get("embedding")?))
                    .collect()
            })
            .unwrap();
        assert_eq!(parsed, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    }

    #[test]
    fn parses_llama_cpp_bare_array_shape() {
        let body = serde_json::json!([
            {"index": 0, "embedding": [[1.0, 2.0]]},
            {"index": 1, "embedding": [[3.0, 4.0]]}
        ]);
        let parsed: Vec<Vec<f32>> = body
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| flatten_embedding(item.get("embedding")?))
                    .collect()
            })
            .unwrap();
        assert_eq!(parsed, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
    }
}
