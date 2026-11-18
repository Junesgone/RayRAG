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
use serde::Serialize;
use serde_json::Value;

const GUIDANCE: &str = "Check the embedding provider API URL, API key and model in Models and dataset Configuration, then retry.";

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

impl OpenAIEmbedder {
    pub fn new(api_base: &str, api_key: &str, model: &str) -> Self {
        Self {
            // Model calls get a short, explicit timeout instead of the process-wide
            // command budget: a stalled embedding server must not hold a request (and
            // its buffer) for two hours.
            client: crate::common::cmd_timeout::model_client_for(api_base),
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
fn flatten_embedding(value: &Value) -> Option<Vec<f32>> {
    let outer = value.as_array()?;
    let values = if outer.len() == 1 && outer[0].is_array() {
        outer[0].as_array()?
    } else {
        outer
    };
    if values.is_empty() {
        return None;
    }
    values
        .iter()
        .map(|value| {
            let number = value.as_f64()? as f32;
            number.is_finite().then_some(number)
        })
        .collect()
}

fn parse_embeddings(body: &Value, count: usize) -> Result<Vec<Vec<f32>>> {
    use anyhow::{Context, bail};
    let items = body
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| body.as_array())
        .context("Embedding API response has unexpected shape")?;
    if items.len() != count {
        bail!(
            "Embedding API returned {} items for {count} texts",
            items.len()
        );
    }
    // Positional mode is only unambiguous when every item omits its index.
    let indexed = items.iter().any(|item| item.get("index").is_some());
    let mut ordered = vec![Vec::new(); count];
    let mut dimension = None;
    for (position, item) in items.iter().enumerate() {
        let index = if indexed {
            item.get("index")
                .and_then(Value::as_u64)
                .and_then(|index| usize::try_from(index).ok())
                .with_context(|| {
                    format!("Embedding API item {position} has a missing or invalid index")
                })?
        } else {
            position
        };
        let slot = ordered
            .get_mut(index)
            .with_context(|| format!("Embedding API index {index} is out of range"))?;
        if !slot.is_empty() {
            bail!("Embedding API returned duplicate index {index}");
        }
        let vector = item
            .get("embedding")
            .and_then(flatten_embedding)
            .with_context(|| {
                format!("Embedding API item {position} has an invalid or empty vector")
            })?;
        if vector.len() != *dimension.get_or_insert(vector.len()) {
            bail!("Embedding API returned inconsistent vector dimensions at item {position}");
        }
        *slot = vector;
    }
    Ok(ordered)
}

#[async_trait::async_trait]
impl Embedder for OpenAIEmbedder {
    async fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
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
            .await
            .map_err(|error| {
                anyhow::anyhow!(
                    "Embedding API request failed: {}. {GUIDANCE}",
                    error.without_url()
                )
            })?;

        if !resp.status().is_success() {
            anyhow::bail!("Embedding API returned HTTP {}. {GUIDANCE}", resp.status());
        }

        // Bounded read: the endpoint decides how much it sends, the limit decides how
        // much this process ever holds.
        let body: serde_json::Value = crate::common::cmd_timeout::read_json_limited(
            resp,
            crate::common::cmd_timeout::body_limit_bytes(),
            "Embedding API",
        )
        .await
        .map_err(|error| {
            anyhow::anyhow!("Embedding API response could not be read as JSON: {error}. {GUIDANCE}")
        })?;
        parse_embeddings(&body, texts.len()).map_err(|error| anyhow::anyhow!("{error}. {GUIDANCE}"))
    }
}


