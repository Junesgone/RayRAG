//! Provider-aware model discovery and credential normalization.
//!
//! This module follows the observable model-list behavior of RAGFlow
//! `rag/llm/model_meta.py` while keeping inference clients in Rust. Providers
//! without a special discovery dialect use the OpenAI-compatible `/v1/models`
//! contract, which covers hosted gateways and self-hosted vLLM-style servers.

use crate::Result;
use crate::api::tenant_models::ModelCapability;
use reqwest::{Client, Url};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

const DEFAULT_MAX_TOKENS: u64 = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderDialect {
    VolcEngine,
    Ollama,
    LocalAi,
    Xinference,
    OpenRouter,
    Vllm,
    LmStudio,
    NewApi,
    Replicate,
    OpenAiCompatible,
}

impl ProviderDialect {
    pub fn from_provider(provider_id: &str, provider_name: &str) -> Self {
        // The provider catalog (mirroring RAGFlow conf/llm_factories.json) knows
        // the exact dialect of every factory; fall back to name/id heuristics.
        if let Some(preset) = crate::providers::provider_preset(provider_id) {
            return preset.dialect;
        }
        if let Some(preset) = crate::providers::provider_preset_by_name(provider_name) {
            return preset.dialect;
        }
        Self::from_provider_heuristic(provider_id, provider_name)
    }

    /// Legacy heuristic path, kept for providers outside the catalog.
    fn from_provider_heuristic(provider_id: &str, provider_name: &str) -> Self {
        let normalized_id = normalize_provider_name(provider_id);
        let normalized_name = normalize_provider_name(provider_name);
        match normalized_id.as_str() {
            "volcengine" | "doubao" => Self::VolcEngine,
            "ollama" => Self::Ollama,
            "localai" => Self::LocalAi,
            "xinference" => Self::Xinference,
            "openrouter" => Self::OpenRouter,
            "vllm" => Self::Vllm,
            "lmstudio" => Self::LmStudio,
            "newapi" => Self::NewApi,
            "replicate" => Self::Replicate,
            _ => match normalized_name.as_str() {
                "volcengine" | "doubao" => Self::VolcEngine,
                "ollama" => Self::Ollama,
                "localai" => Self::LocalAi,
                "xinference" => Self::Xinference,
                "openrouter" => Self::OpenRouter,
                "vllm" => Self::Vllm,
                "lmstudio" => Self::LmStudio,
                "newapi" => Self::NewApi,
                "replicate" => Self::Replicate,
                _ => Self::OpenAiCompatible,
            },
        }
    }

    /// Append `/v1` to an origin-style base for dialects whose compatible
    /// routes live under `/v1`. Preserves explicit paths (e.g. VolcEngine's
    /// `/api/v3` or an OpenAI-compatible base that already ends in `/v1`).
    pub fn apply_v1_suffix(self, base: &str) -> String {
        let trimmed = base.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return String::new();
        }
        match self {
            Self::Ollama
            | Self::LocalAi
            | Self::Xinference
            | Self::Vllm
            | Self::LmStudio
            | Self::OpenAiCompatible => {
                let Ok(mut url) = reqwest::Url::parse(trimmed) else {
                    return trimmed.to_string();
                };
                if matches!(url.path(), "" | "/") {
                    url.set_path("/v1");
                    url.to_string().trim_end_matches('/').to_string()
                } else if !url.path().ends_with("/v1") && !url.path().contains("/v1/") {
                    // OpenAI-compatible aggregate bases may carry their own path
                    // (e.g. Azure deployments); leave those untouched.
                    trimmed.to_string()
                } else {
                    trimmed.to_string()
                }
            }
            _ => trimmed.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscoveredModel {
    pub name: String,
    pub model_types: Vec<ModelCapability>,
    pub features: Vec<String>,
    pub max_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

impl DiscoveredModel {
    fn inferred(name: String) -> Self {
        Self {
            model_types: infer_openai_model_types(&name),
            name,
            features: Vec::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
            status: None,
        }
    }
}

pub fn normalize_provider_api_key(
    provider_id: &str,
    provider_name: &str,
    api_key: Option<&str>,
) -> Option<String> {
    // Explicitly configured key wins; otherwise consult the provider's
    // recommended environment variable (e.g. ZHIPU_API_KEY, DEEPSEEK_API_KEY).
    let key = match api_key {
        Some(key) if !key.trim().is_empty() => key.to_string(),
        _ => {
            let env_name = crate::providers::provider_api_key_env(provider_id, provider_name)?;
            std::env::var(env_name)
                .ok()
                .filter(|key| !key.trim().is_empty())?
        }
    };
    let dialect = ProviderDialect::from_provider(provider_id, provider_name);
    let field = match dialect {
        ProviderDialect::VolcEngine => "ark_api_key",
        ProviderDialect::OpenRouter | ProviderDialect::NewApi | ProviderDialect::Replicate => {
            "api_key"
        }
        _ => return Some(key),
    };
    normalize_json_secret(&key, field)
}

pub fn deserialize_optional_api_key<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(Value::Object(value)) => serde_json::to_string(&value)
            .map(Some)
            .map_err(serde::de::Error::custom),
        Some(_) => Err(serde::de::Error::custom(
            "API key must be a string, object, or null",
        )),
    }
}

/// Normalize a provider base for the OpenAI-compatible inference clients.
///
/// Ollama, LocalAI, Xinference, vLLM and LM Studio publish their compatible
/// inference routes under `/v1` when the configured base is only an origin.
/// When the configured base is empty, the provider catalog preset supplies a
/// domestic-first default endpoint.
pub fn normalize_inference_base(provider_id: &str, provider_name: &str, api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        if let Some(default) = crate::providers::provider_default_base(provider_id, provider_name) {
            return crate::providers::provider_dialect(provider_id, provider_name)
                .apply_v1_suffix(default)
                .to_string();
        }
        return String::new();
    }
    let dialect = ProviderDialect::from_provider(provider_id, provider_name);
    if !matches!(
        dialect,
        ProviderDialect::Ollama
            | ProviderDialect::LocalAi
            | ProviderDialect::Xinference
            | ProviderDialect::Vllm
            | ProviderDialect::LmStudio
    ) {
        return trimmed.to_string();
    }
    let Ok(mut url) = Url::parse(trimmed) else {
        return trimmed.to_string();
    };
    if !matches!(url.path(), "" | "/") {
        return trimmed.to_string();
    }
    url.set_path("/v1");
    url.to_string().trim_end_matches('/').to_string()
}

pub async fn discover_provider_models(
    provider_id: &str,
    provider_name: &str,
    api_base: &str,
    api_key: Option<&str>,
    static_models: &[String],
) -> Result<Vec<DiscoveredModel>> {
    let dialect = ProviderDialect::from_provider(provider_id, provider_name);
    let normalized_key = normalize_provider_api_key(provider_id, provider_name, api_key);
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()?;

    let remote = match dialect {
        ProviderDialect::Ollama | ProviderDialect::LocalAi => {
            discover_ollama_models(
                &client,
                api_base,
                normalized_key.as_deref(),
                dialect == ProviderDialect::LocalAi,
            )
            .await?
        }
        ProviderDialect::Replicate => Vec::new(),
        _ => discover_http_models(&client, dialect, api_base, normalized_key.as_deref()).await?,
    };

    let mut merged = BTreeMap::new();
    for name in static_models {
        let name = name.trim();
        if !name.is_empty() {
            merged.insert(
                name.to_string(),
                DiscoveredModel::inferred(name.to_string()),
            );
        }
    }
    for model in remote {
        merged.insert(model.name.clone(), model);
    }
    Ok(merged.into_values().collect())
}

/// Probe the provider's authenticated model-list endpoint for the Model
/// providers `Verify` button. RAGFlow probes one live model per capability;
/// this Rust slice uses the provider-native catalog endpoint so verification
/// is read-only while still rejecting bad credentials and unreachable bases.
pub async fn verify_provider_connection(
    provider_id: &str,
    provider_name: &str,
    api_base: &str,
    api_key: Option<&str>,
) -> Result<()> {
    let dialect = ProviderDialect::from_provider(provider_id, provider_name);
    if dialect == ProviderDialect::Replicate {
        anyhow::bail!("Provider connection verification is not supported for Replicate yet");
    }
    let base = normalize_inference_base(provider_id, provider_name, api_base);
    let url = if matches!(dialect, ProviderDialect::Ollama | ProviderDialect::LocalAi) {
        parse_http_base(&base)?.join("api/tags")?
    } else {
        model_list_url(dialect, &base)?
    };
    let timeout = std::env::var("LLM_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10)
        .clamp(1, 120);
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5.min(timeout)))
        .timeout(Duration::from_secs(timeout))
        .build()?;
    let normalized_key = normalize_provider_api_key(provider_id, provider_name, api_key);
    let mut request = client.get(url);
    if let Some(key) = normalized_key.as_deref().filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        anyhow::bail!("Provider verification returned HTTP {}", response.status());
    }
    let _: Value = response.json().await?;
    Ok(())
}

/// Upstream `verify_api_key` capability inference: probe real chat /
/// embedding / rerank requests for the selected models instead of relying on
/// a model-list GET. Only OpenAI-compatible-style transports can be probed
/// generically; provider-specific REST protocols (Baidu/Tencent/XunFei/Google
/// and OCR/TTS/ASR models) are intentionally skipped and documented as
/// partial. An empty `models` slice falls back to the model-list probe.
pub async fn verify_provider_capabilities(
    provider_id: &str,
    provider_name: &str,
    api_base: &str,
    api_key: Option<&str>,
    models: &[(String, Vec<ModelCapability>)],
) -> Result<()> {
    if models.is_empty() {
        return verify_provider_connection(provider_id, provider_name, api_base, api_key).await;
    }
    let dialect = ProviderDialect::from_provider(provider_id, provider_name);
    let chat_ok = matches!(
        dialect,
        ProviderDialect::OpenAiCompatible
            | ProviderDialect::Vllm
            | ProviderDialect::LmStudio
            | ProviderDialect::NewApi
            | ProviderDialect::Xinference
            | ProviderDialect::OpenRouter
            | ProviderDialect::VolcEngine
    );
    let embed_ok = matches!(
        dialect,
        ProviderDialect::OpenAiCompatible
            | ProviderDialect::Vllm
            | ProviderDialect::LmStudio
            | ProviderDialect::NewApi
            | ProviderDialect::Xinference
            | ProviderDialect::OpenRouter
    );
    let rerank_ok = matches!(
        dialect,
        ProviderDialect::OpenAiCompatible
            | ProviderDialect::Vllm
            | ProviderDialect::LmStudio
            | ProviderDialect::NewApi
            | ProviderDialect::Xinference
    );
    if !chat_ok {
        return Ok(());
    }
    let base = normalize_inference_base(provider_id, provider_name, api_base);
    let timeout = std::env::var("LLM_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10)
        .clamp(1, 120);
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5.min(timeout)))
        .timeout(Duration::from_secs(timeout))
        .build()?;
    let normalized_key = normalize_provider_api_key(provider_id, provider_name, api_key);
    let bearer = normalized_key.as_deref().filter(|key| !key.is_empty());
    let mut probed = false;
    let mut errors = Vec::new();
    for (model_name, capabilities) in models {
        for capability in capabilities {
            let result = match capability {
                ModelCapability::Chat | ModelCapability::ImageToText => {
                    probed = true;
                    probe_chat_model(&client, dialect, &base, model_name, bearer).await
                }
                ModelCapability::Embedding if embed_ok => {
                    probed = true;
                    probe_embedding_model(&client, &base, model_name, bearer).await
                }
                ModelCapability::Rerank if rerank_ok => {
                    probed = true;
                    probe_rerank_model(&client, &base, model_name, bearer).await
                }
                ModelCapability::Embedding | ModelCapability::Rerank => Ok(()),
                ModelCapability::Ocr
                | ModelCapability::SpeechToText
                | ModelCapability::TextToSpeech => Ok(()),
            };
            if let Err(error) = result {
                errors.push(format!(
                    "Fail to access model({provider_name}/{model_name}): {error}"
                ));
            }
        }
    }
    if !probed {
        return Ok(());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("{}", errors.join("\n"))
    }
}

async fn probe_chat_model(
    client: &Client,
    dialect: ProviderDialect,
    api_base: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<()> {
    let url = match dialect {
        ProviderDialect::VolcEngine => {
            let mut origin = parse_http_base(api_base)?;
            origin.set_path("/api/v3/chat/completions");
            origin.set_query(None);
            origin
        }
        ProviderDialect::OpenRouter => {
            let mut origin = parse_http_base(api_base)?;
            origin.set_path("/api/v1/chat/completions");
            origin.set_query(None);
            origin
        }
        _ => openai_v1_base(parse_http_base(api_base)?).join("chat/completions")?,
    };
    let mut request = client.post(url).json(&serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Hi" }],
        "max_tokens": 8,
        "stream": false,
    }));
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        anyhow::bail!("chat probe returned HTTP {}", response.status());
    }
    let _: Value = response.json().await?;
    Ok(())
}

async fn probe_embedding_model(
    client: &Client,
    api_base: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<()> {
    let url = openai_v1_base(parse_http_base(api_base)?).join("embeddings")?;
    let mut request = client.post(url).json(&serde_json::json!({
        "model": model,
        "input": ["Test if the api key is available"],
    }));
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        anyhow::bail!("embedding probe returned HTTP {}", response.status());
    }
    let payload: Value = response.json().await?;
    let non_empty = payload
        .get("data")
        .and_then(Value::as_array)
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("embedding"))
        .and_then(Value::as_array)
        .is_some_and(|embedding| !embedding.is_empty());
    if !non_empty {
        anyhow::bail!("embedding probe returned an empty vector");
    }
    Ok(())
}

async fn probe_rerank_model(
    client: &Client,
    api_base: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<()> {
    let url = openai_v1_base(parse_http_base(api_base)?).join("rerank")?;
    let mut request = client.post(url).json(&serde_json::json!({
        "model": model,
        "query": "What's the weather?",
        "documents": ["Is it sunny today?"],
    }));
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        anyhow::bail!("rerank probe returned HTTP {}", response.status());
    }
    let payload: Value = response.json().await?;
    let non_empty = payload
        .get("results")
        .and_then(Value::as_array)
        .is_some_and(|results| !results.is_empty());
    if !non_empty {
        anyhow::bail!("rerank probe returned no results");
    }
    Ok(())
}

async fn discover_http_models(
    client: &Client,
    dialect: ProviderDialect,
    api_base: &str,
    api_key: Option<&str>,
) -> Result<Vec<DiscoveredModel>> {
    let url = model_list_url(dialect, api_base)?;
    let mut request = client.get(url);
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Ok(Vec::new());
    }
    let raw: Value = response.json().await?;
    Ok(match dialect {
        ProviderDialect::VolcEngine => parse_volcengine_models(&raw),
        ProviderDialect::OpenRouter => parse_openrouter_models(&raw),
        ProviderDialect::Xinference => parse_xinference_models(&raw),
        _ => parse_openai_compatible_models(&raw),
    })
}

async fn discover_ollama_models(
    client: &Client,
    api_base: &str,
    api_key: Option<&str>,
    local_ai: bool,
) -> Result<Vec<DiscoveredModel>> {
    let base = parse_http_base(api_base)?;
    let tags_url = base.join("api/tags")?;
    let show_url = base.join("api/show")?;
    let mut request = client.get(tags_url);
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    if !response.status().is_success() {
        return Ok(Vec::new());
    }
    let tags: Value = response.json().await?;
    let Some(models) = tags.get("models").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    let mut discovered = Vec::new();
    for model in models {
        let Some(name) = model.get("name").and_then(Value::as_str) else {
            continue;
        };
        let mut request = client.post(show_url.clone()).json(&serde_json::json!({
            "model": name
        }));
        if let Some(key) = api_key.filter(|key| !key.is_empty()) {
            request = request.bearer_auth(key);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            continue;
        }
        let detail: Value = response.json().await?;
        if let Some(model) = parse_ollama_detail(name, &detail, local_ai) {
            discovered.push(model);
        }
    }
    Ok(discovered)
}

fn model_list_url(dialect: ProviderDialect, api_base: &str) -> Result<Url> {
    let base = parse_http_base(api_base)?;
    match dialect {
        ProviderDialect::VolcEngine => {
            let mut origin = base;
            origin.set_path("/api/v3/models");
            origin.set_query(None);
            Ok(origin)
        }
        ProviderDialect::OpenRouter => {
            let mut origin = base;
            origin.set_path("/api/v1/models");
            origin.set_query(Some("output_modalities=all"));
            Ok(origin)
        }
        ProviderDialect::Vllm => {
            let base = ensure_v1_url(base);
            Ok(base.join("models")?)
        }
        _ => {
            let base = openai_v1_base(base);
            Ok(base.join("models")?)
        }
    }
}

fn parse_http_base(api_base: &str) -> Result<Url> {
    let trimmed = api_base.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Provider API base is required for model discovery");
    }
    let mut url = Url::parse(trimmed)?;
    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!("Provider API base must use http or https");
    }
    url.set_query(None);
    url.set_fragment(None);
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn ensure_v1_url(mut url: Url) -> Url {
    let path = url.path().trim_end_matches('/');
    if !path.ends_with("/v1") {
        url.set_path(&format!("{path}/v1/"));
    }
    url
}

fn openai_v1_base(mut url: Url) -> Url {
    let path = url.path();
    if let Some(offset) = path.find("/v1") {
        url.set_path(&format!("{}/", &path[..offset + 3]));
    } else {
        let path = path.trim_end_matches('/');
        url.set_path(&format!("{path}/v1/"));
    }
    url
}

fn parse_openai_compatible_models(raw: &Value) -> Vec<DiscoveredModel> {
    let Some(models) = model_array(raw) else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|model| {
            let name = model
                .get("id")
                .or_else(|| model.get("name"))
                .and_then(Value::as_str)?
                .to_string();
            Some(DiscoveredModel {
                model_types: infer_openai_model_types(&name),
                max_tokens: first_u64(
                    model,
                    &[
                        "max_tokens",
                        "max_completion_tokens",
                        "context_length",
                        "max_model_len",
                    ],
                )
                .unwrap_or(DEFAULT_MAX_TOKENS),
                name,
                features: Vec::new(),
                status: None,
            })
        })
        .collect()
}

fn parse_xinference_models(raw: &Value) -> Vec<DiscoveredModel> {
    let Some(models) = raw.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|model| {
            let name = model.get("id").and_then(Value::as_str)?.to_string();
            let model_type = match model
                .get("model_type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "embedding" => ModelCapability::Embedding,
                "rerank" => ModelCapability::Rerank,
                "image" => ModelCapability::ImageToText,
                "TTS" => ModelCapability::TextToSpeech,
                "speech2text" => ModelCapability::SpeechToText,
                "ocr" => ModelCapability::Ocr,
                _ => ModelCapability::Chat,
            };
            Some(DiscoveredModel {
                name,
                model_types: vec![model_type],
                features: Vec::new(),
                max_tokens: first_u64(model, &["context_length", "max_tokens"])
                    .unwrap_or(DEFAULT_MAX_TOKENS),
                status: None,
            })
        })
        .collect()
}

fn parse_volcengine_models(raw: &Value) -> Vec<DiscoveredModel> {
    let Some(models) = raw.get("data").and_then(Value::as_array) else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|model| {
            let status = model
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if status == "Shutdown" {
                return None;
            }
            let name = model.get("id").and_then(Value::as_str)?.to_string();
            let mut model_types = Vec::new();
            if model.get("domain").and_then(Value::as_str) == Some("Embedding")
                || string_array(model.get("task_type"))
                    .iter()
                    .any(|value| matches!(*value, "TextEmbedding" | "ImageEmbedding"))
            {
                model_types.push(ModelCapability::Embedding);
            } else {
                let modalities = model.get("modalities").unwrap_or(&Value::Null);
                let input = string_array(modalities.get("input_modalities"));
                let output = string_array(modalities.get("output_modalities"));
                if output.contains(&"text") {
                    model_types.push(ModelCapability::Chat);
                }
                if output.contains(&"embeddings") {
                    model_types.push(ModelCapability::Embedding);
                }
                if input.contains(&"image") && output.contains(&"text") {
                    model_types.push(ModelCapability::ImageToText);
                }
                if input.contains(&"audio") && output.contains(&"text") {
                    model_types.push(ModelCapability::SpeechToText);
                }
                if output.contains(&"audio") {
                    model_types.push(ModelCapability::TextToSpeech);
                }
            }
            deduplicate_capabilities(&mut model_types);
            if model_types.is_empty() {
                return None;
            }
            let mut features = Vec::new();
            if model
                .pointer("/features/tools/function_calling")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                features.push("is_tools".to_string());
            }
            if model
                .pointer("/token_limits/max_reasoning_token_length")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                > 0
            {
                features.push("thinking".to_string());
            }
            Some(DiscoveredModel {
                name,
                model_types,
                features,
                max_tokens: model
                    .pointer("/token_limits/max_input_token_length")
                    .and_then(Value::as_u64)
                    .unwrap_or(DEFAULT_MAX_TOKENS),
                status: (!status.is_empty()).then(|| status.to_string()),
            })
        })
        .collect()
}

fn parse_openrouter_models(raw: &Value) -> Vec<DiscoveredModel> {
    let Some(models) = model_array(raw) else {
        return Vec::new();
    };
    models
        .iter()
        .filter_map(|model| {
            let name = model
                .get("id")
                .or_else(|| model.get("name"))
                .or_else(|| model.get("canonical_slug"))
                .and_then(Value::as_str)?
                .to_string();
            let architecture = model.get("architecture").unwrap_or(&Value::Null);
            let input = string_array(architecture.get("input_modalities"));
            let output = string_array(architecture.get("output_modalities"));
            let parameters = string_array(model.get("supported_parameters"));
            let mut model_types = Vec::new();
            if output.contains(&"text") {
                model_types.push(ModelCapability::Chat);
            }
            if output.contains(&"embeddings") {
                model_types.push(ModelCapability::Embedding);
            }
            if input.contains(&"image") && output.contains(&"text") {
                model_types.push(ModelCapability::ImageToText);
            }
            if input.contains(&"audio") && output.contains(&"text") {
                model_types.push(ModelCapability::SpeechToText);
            }
            if output.contains(&"audio") {
                model_types.push(ModelCapability::TextToSpeech);
            }
            deduplicate_capabilities(&mut model_types);
            let mut features = Vec::new();
            if parameters.contains(&"tools") {
                features.push("is_tools".to_string());
            }
            if parameters
                .iter()
                .any(|value| matches!(*value, "reasoning" | "include_reasoning"))
            {
                features.push("thinking".to_string());
            }
            Some(DiscoveredModel {
                name,
                model_types,
                features,
                max_tokens: model
                    .pointer("/top_provider/max_completion_tokens")
                    .and_then(Value::as_u64)
                    .or_else(|| model.get("context_length").and_then(Value::as_u64))
                    .or_else(|| {
                        model
                            .pointer("/top_provider/context_length")
                            .and_then(Value::as_u64)
                    })
                    .unwrap_or(DEFAULT_MAX_TOKENS),
                status: None,
            })
        })
        .collect()
}

fn parse_ollama_detail(name: &str, detail: &Value, local_ai: bool) -> Option<DiscoveredModel> {
    let capabilities = string_array(detail.get("capabilities"));
    let mut model_types = Vec::new();
    if capabilities.contains(&"completion") {
        model_types.push(ModelCapability::Chat);
    }
    if capabilities.contains(&"vision") {
        model_types.push(ModelCapability::ImageToText);
    }
    if capabilities.contains(&"embedding") {
        model_types.push(ModelCapability::Embedding);
    }
    deduplicate_capabilities(&mut model_types);
    let mut features = Vec::new();
    if capabilities.contains(&"thinking") {
        features.push("thinking".to_string());
    }
    if capabilities.contains(&"tools") {
        features.push("is_tools".to_string());
    }
    let model_info = detail.get("model_info")?;
    let max_tokens = if local_ai {
        model_info
            .get("general.context_length")
            .and_then(Value::as_u64)
    } else {
        let family = detail
            .pointer("/details/family")
            .and_then(Value::as_str)
            .unwrap_or_default();
        model_info
            .get(format!("{family}.context_length"))
            .and_then(Value::as_u64)
    }
    .unwrap_or(DEFAULT_MAX_TOKENS);
    let name = if local_ai {
        name.rsplit_once(':').map_or(name, |(base, _)| base)
    } else {
        name
    };
    Some(DiscoveredModel {
        name: name.to_string(),
        model_types,
        features,
        max_tokens,
        status: None,
    })
}

fn infer_openai_model_types(name: &str) -> Vec<ModelCapability> {
    let name = name.to_ascii_lowercase();
    if contains_hint(&name, &["ocr"]) {
        return vec![ModelCapability::Ocr, ModelCapability::ImageToText];
    }
    if contains_hint(&name, &["rerank", "reranker"]) {
        return vec![ModelCapability::Rerank];
    }
    if contains_hint(&name, &["embed", "embedding", "bge"]) {
        return vec![ModelCapability::Embedding];
    }
    if contains_hint(
        &name,
        &["asr", "stt", "transcribe", "transcriber", "whisper"],
    ) {
        return vec![ModelCapability::SpeechToText];
    }
    if contains_hint(&name, &["tts", "text-to-speech"]) {
        return vec![ModelCapability::TextToSpeech];
    }
    let mut model_types = vec![ModelCapability::Chat];
    if contains_hint(
        &name,
        &[
            "vl",
            "vision",
            "llava",
            "internvl",
            "minicpm-v",
            "gpt-4o",
            "glm-4v",
            "qvq",
            "qwen-vl",
            "pixtral",
        ],
    ) {
        model_types.push(ModelCapability::ImageToText);
    }
    model_types
}

fn normalize_provider_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalize_json_secret(raw: &str, field: &str) -> Option<String> {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Some(raw.to_string());
    };
    let Some(value) = value.as_object().and_then(|object| object.get(field)) else {
        return Some(raw.to_string());
    };
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        value => Some(value.to_string()),
    }
}

fn contains_hint(value: &str, hints: &[&str]) -> bool {
    hints.iter().any(|hint| value.contains(hint))
}

fn model_array(raw: &Value) -> Option<&Vec<Value>> {
    raw.get("data")
        .and_then(Value::as_array)
        .or_else(|| raw.as_array())
}

fn string_array(value: Option<&Value>) -> Vec<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn first_u64(value: &Value, fields: &[&str]) -> Option<u64> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_u64))
}

fn deduplicate_capabilities(capabilities: &mut Vec<ModelCapability>) {
    let mut seen = Vec::new();
    capabilities.retain(|capability| {
        if seen.contains(capability) {
            false
        } else {
            seen.push(*capability);
            true
        }
    });
}

// ── Provider model catalog (seeded from RAGFlow conf/models/*.json) ────────
//
// The 20 provider fixtures under `src/api/fixtures/models/` mirror RAGFlow's
// `conf/models/<factory>.json` verbatim (name / url{default,singapore,us,global}
// / url_suffix{chat,embedding,rerank,...} / models / features). They are embedded
// at compile time and exposed for the providers page model browser and for
// callers that need the static catalog of a factory without a live API call.

/// One model entry from a provider catalog fixture.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub max_tokens: u64,
    /// Canonical capability tags: `chat` / `embedding` / `rerank` /
    /// `image2text` / `speech2text` / `tts` / `ocr` (RAGFlow's `vision` and
    /// `asr` are normalized onto the RayRAG capability names).
    pub model_types: Vec<String>,
    /// True when the fixture marks this model as thinking-capable.
    pub thinking: bool,
}

/// A seeded provider catalog fixture (mirrors RAGFlow conf/models/<factory>.json).
#[derive(Debug, Clone, Serialize)]
pub struct ProviderModels {
    /// Fixture key, e.g. `"deepseek"`, `"openai"`, `"zhipu-ai"`.
    pub factory: String,
    /// Display name, e.g. `"DeepSeek"`.
    pub name: String,
    /// Region -> base URL (`default` / `singapore` / `us` / `global` ...).
    pub url: BTreeMap<String, String>,
    /// Endpoint kind -> URL suffix (`chat` / `embedding` / `rerank` / ...).
    pub url_suffix: BTreeMap<String, String>,
    /// Seeded model list (empty for dynamic providers such as Ollama/vLLM).
    pub models: Vec<ModelInfo>,
    /// Free-form `features` object preserved verbatim from the fixture.
    pub features: Option<Value>,
}

/// All 61 RAGFlow provider fixtures, embedded at compile time.
const PROVIDER_MODEL_FIXTURES: &[(&str, &str)] = &[
    ("aliyun", include_str!("api/fixtures/models/aliyun.json")),
    ("baidu", include_str!("api/fixtures/models/baidu.json")),
    ("cohere", include_str!("api/fixtures/models/cohere.json")),
    (
        "deepseek",
        include_str!("api/fixtures/models/deepseek.json"),
    ),
    ("deerapi", include_str!("api/fixtures/models/deerapi.json")),
    (
        "fishaudio",
        include_str!("api/fixtures/models/fishaudio.json"),
    ),
    ("gitee", include_str!("api/fixtures/models/gitee.json")),
    ("google", include_str!("api/fixtures/models/google.json")),
    (
        "huggingface",
        include_str!("api/fixtures/models/huggingface.json"),
    ),
    (
        "lmstudio",
        include_str!("api/fixtures/models/lmstudio.json"),
    ),
    ("minimax", include_str!("api/fixtures/models/minimax.json")),
    (
        "moonshot",
        include_str!("api/fixtures/models/moonshot.json"),
    ),
    ("nvidia", include_str!("api/fixtures/models/nvidia.json")),
    ("ollama", include_str!("api/fixtures/models/ollama.json")),
    ("openai", include_str!("api/fixtures/models/openai.json")),
    (
        "openrouter",
        include_str!("api/fixtures/models/openrouter.json"),
    ),
    (
        "siliconflow",
        include_str!("api/fixtures/models/siliconflow.json"),
    ),
    ("vllm", include_str!("api/fixtures/models/vllm.json")),
    (
        "volcengine",
        include_str!("api/fixtures/models/volcengine.json"),
    ),
    ("xai", include_str!("api/fixtures/models/xai.json")),
    (
        "zhipu-ai",
        include_str!("api/fixtures/models/zhipu-ai.json"),
    ),
    ("302ai", include_str!("api/fixtures/models/302ai.json")),
    (
        "anthropic",
        include_str!("api/fixtures/models/anthropic.json"),
    ),
    (
        "astraflow",
        include_str!("api/fixtures/models/astraflow.json"),
    ),
    ("avian", include_str!("api/fixtures/models/avian.json")),
    (
        "azure-openai",
        include_str!("api/fixtures/models/azure-openai.json"),
    ),
    (
        "baichuan",
        include_str!("api/fixtures/models/baichuan.json"),
    ),
    ("bedrock", include_str!("api/fixtures/models/bedrock.json")),
    (
        "cometapi",
        include_str!("api/fixtures/models/cometapi.json"),
    ),
    (
        "deepinfra",
        include_str!("api/fixtures/models/deepinfra.json"),
    ),
    (
        "futurmix",
        include_str!("api/fixtures/models/futurmix.json"),
    ),
    (
        "gpustack",
        include_str!("api/fixtures/models/gpustack.json"),
    ),
    ("groq", include_str!("api/fixtures/models/groq.json")),
    (
        "huaweicloud",
        include_str!("api/fixtures/models/huaweicloud.json"),
    ),
    ("hunyuan", include_str!("api/fixtures/models/hunyuan.json")),
    (
        "jiekouai",
        include_str!("api/fixtures/models/jiekouai.json"),
    ),
    ("jina", include_str!("api/fixtures/models/jina.json")),
    ("localai", include_str!("api/fixtures/models/localai.json")),
    ("longcat", include_str!("api/fixtures/models/longcat.json")),
    ("mineru", include_str!("api/fixtures/models/mineru.json")),
    (
        "mineru_local",
        include_str!("api/fixtures/models/mineru_local.json"),
    ),
    ("mistral", include_str!("api/fixtures/models/mistral.json")),
    (
        "modelscope",
        include_str!("api/fixtures/models/modelscope.json"),
    ),
    ("n1n", include_str!("api/fixtures/models/n1n.json")),
    ("novita", include_str!("api/fixtures/models/novita.json")),
    (
        "orcarouter",
        include_str!("api/fixtures/models/orcarouter.json"),
    ),
    (
        "paddleocr",
        include_str!("api/fixtures/models/paddleocr.json"),
    ),
    (
        "paddleocr_local",
        include_str!("api/fixtures/models/paddleocr_local.json"),
    ),
    (
        "perplexity",
        include_str!("api/fixtures/models/perplexity.json"),
    ),
    ("ppio", include_str!("api/fixtures/models/ppio.json")),
    ("qiniu", include_str!("api/fixtures/models/qiniu.json")),
    (
        "replicate",
        include_str!("api/fixtures/models/replicate.json"),
    ),
    ("stepfun", include_str!("api/fixtures/models/stepfun.json")),
    (
        "togetherai",
        include_str!("api/fixtures/models/togetherai.json"),
    ),
    (
        "tokenhub",
        include_str!("api/fixtures/models/tokenhub.json"),
    ),
    (
        "tokenpony",
        include_str!("api/fixtures/models/tokenpony.json"),
    ),
    ("upstage", include_str!("api/fixtures/models/upstage.json")),
    ("voyage", include_str!("api/fixtures/models/voyage.json")),
    ("xiaomi", include_str!("api/fixtures/models/xiaomi.json")),
    (
        "xinference",
        include_str!("api/fixtures/models/xinference.json"),
    ),
    ("xunfei", include_str!("api/fixtures/models/xunfei.json")),
];

/// Parse every embedded fixture into a `ProviderModels` entry.
pub fn provider_model_catalog() -> Vec<ProviderModels> {
    PROVIDER_MODEL_FIXTURES
        .iter()
        .filter_map(|(factory, raw)| parse_provider_models(factory, raw))
        .collect()
}

/// Static model list for a factory key (e.g. `"deepseek"`, `"zhipu-ai"`).
///
/// Returns an empty vector when the factory is unknown or carries no static
/// model list (dynamic providers like Ollama/vLLM discover models at runtime).
pub fn list_models_for(factory: &str) -> Vec<ModelInfo> {
    provider_model_catalog()
        .into_iter()
        .find(|provider| provider.factory == factory)
        .map(|provider| provider.models)
        .unwrap_or_default()
}

/// Resolve a RayRAG provider (id + display name) onto a catalog fixture key.
pub fn resolve_model_factory(provider_id: &str, provider_name: &str) -> Option<String> {
    let normalized_id = normalize_provider_name(provider_id);
    let normalized_name = normalize_provider_name(provider_name);
    let aliases: &[(&str, &str)] = &[
        ("zhipu", "zhipu-ai"),
        ("zhipuai", "zhipu-ai"),
        ("bigmodel", "zhipu-ai"),
        ("glm", "zhipu-ai"),
        ("qwen", "aliyun"),
        ("dashscope", "aliyun"),
        ("kimi", "moonshot"),
        ("doubao", "volcengine"),
        ("ark", "volcengine"),
        ("grok", "xai"),
        ("gemini", "google"),
        ("qianfan", "baidu"),
        ("claude", "anthropic"),
        ("anthropic", "anthropic"),
        ("hunyuan", "hunyuan"),
        ("tencent", "hunyuan"),
        ("xunfei", "xunfei"),
        ("spark", "xunfei"),
        ("baichuan", "baichuan"),
        ("stepfun", "stepfun"),
        ("together", "togetherai"),
        ("togetherai", "togetherai"),
        ("voyage", "voyage"),
        ("jina", "jina"),
        ("perplexity", "perplexity"),
        ("modelscope", "modelscope"),
        ("localai", "localai"),
        ("novita", "novita"),
        ("upstage", "upstage"),
        ("replicate", "replicate"),
        ("azure", "azure-openai"),
        ("azure-openai", "azure-openai"),
        ("bedrock", "bedrock"),
        ("gpustack", "gpustack"),
        ("deepinfra", "deepinfra"),
        ("ppio", "ppio"),
        ("qiniu", "qiniu"),
        ("xiaomi", "xiaomi"),
        ("n1n", "n1n"),
        ("302ai", "302ai"),
        ("tokenpony", "tokenpony"),
        ("cometapi", "cometapi"),
        ("longcat", "longcat"),
        ("jiekouai", "jiekouai"),
        ("astraflow", "astraflow"),
        ("astraflow-cn", "astraflow"),
        ("deerapi", "deerapi"),
        ("avian", "avian"),
        ("futurmix", "futurmix"),
        ("orcarouter", "orcarouter"),
        ("tokenhub", "tokenhub"),
        ("huawei", "huaweicloud"),
        ("huaweicloud", "huaweicloud"),
        ("xinference", "xinference"),
        ("mineru", "mineru"),
        ("paddleocr", "paddleocr"),
    ];
    for (needle, factory) in aliases {
        if normalized_id == *needle || normalized_name == *needle {
            return Some((*factory).to_string());
        }
    }
    for (factory, _) in PROVIDER_MODEL_FIXTURES {
        let key = normalize_provider_name(factory);
        if normalized_id == key || normalized_name == key {
            return Some((*factory).to_string());
        }
    }
    None
}

/// Compact payload for the providers page model browser:
/// `{ "<factory>": {"name": "...", "models": [{"n": "...", "t": ["chat", ...]}]} }`.
pub fn provider_models_payload() -> Value {
    let mut catalog = serde_json::Map::new();
    for provider in provider_model_catalog() {
        let models = provider
            .models
            .iter()
            .map(|model| {
                let mut tags = model.model_types.clone();
                if model.thinking && !tags.iter().any(|tag| tag == "thinking") {
                    tags.push("thinking".to_string());
                }
                serde_json::json!({ "n": model.name, "t": tags })
            })
            .collect::<Vec<_>>();
        catalog.insert(
            provider.factory.clone(),
            serde_json::json!({ "name": provider.name, "models": models }),
        );
    }
    Value::Object(catalog)
}

/// `(factory, display name, static model count)` for every seeded provider.
pub fn provider_catalog_summary() -> Vec<(String, String, usize)> {
    provider_model_catalog()
        .into_iter()
        .map(|provider| (provider.factory, provider.name, provider.models.len()))
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════════
// RAGFlow conf/ coverage — llm_factories.json factory mapping.
//
// RAGFlow keeps two overlapping catalogs under `conf/`:
//   - `conf/models/<factory>.json`  — the "all models" catalog (name / url
//     regions / url_suffix / models[{name, max_tokens, model_types}]).
//     These 20 files are embedded verbatim in [`PROVIDER_MODEL_FIXTURES`].
//   - `conf/llm_factories.json`     — `{"factory_llm_infos": [...]}`, the
//     factory registry used by `common/settings.py` to build
//     `FACTORY_LLM_INFOS` / `LLM_FACTORIES` (factory name → endpoint +
//     hoisted `llm` model rows stamped with `fid`).
//
// The fixtures below rebuild the `factory_llm_infos` shape from the embedded
// conf/models catalog: every factory exposes its default endpoint and its
// model list with `llm_name` / `model_type` / `max_tokens` / `is_tools`.
// Per-model `is_tools` nuances of llm_factories.json are approximated by
// `model_types.contains("chat")` — the conf/models catalog does not carry a
// per-model tools flag. The DB-level `init_llm_factory()` normalization of
// llm_factories.json lives in `api::joint_services` (LlmFactoryRecord /
// `normalize_factory_llm_infos`).
// ═══════════════════════════════════════════════════════════════════════════

/// One model row of the factory mapping (llm_factories.json `llm` entry).
#[derive(Debug, Clone, Serialize)]
pub struct FactoryModelInfo {
    pub llm_name: String,
    /// Canonical capability: `chat` / `embedding` / `rerank` / `image2text`
    /// / `speech2text` / `tts` / `ocr` (normalized like [`ModelInfo`]).
    pub model_type: String,
    pub max_tokens: u64,
    /// True when the model carries the `chat` capability (best-effort
    /// approximation of llm_factories.json `is_tools`).
    pub is_tools: bool,
}

/// One factory mapping row (llm_factories.json `factory_llm_infos` entry):
/// factory name → default endpoint + model list.
#[derive(Debug, Clone, Serialize)]
pub struct FactoryLlmInfo {
    pub name: String,
    /// Default endpoint: fixture `url.default` wins, then the provider
    /// catalog preset base, then the empty string (self-hosted engines).
    pub url: String,
    pub llm: Vec<FactoryModelInfo>,
}

/// Rebuild the `factory_llm_infos` mapping for every embedded fixture.
pub fn factory_llm_infos() -> Vec<FactoryLlmInfo> {
    provider_model_catalog()
        .into_iter()
        .map(|provider| FactoryLlmInfo {
            name: provider.name,
            url: factory_endpoint(&provider.factory).unwrap_or_default(),
            llm: provider
                .models
                .iter()
                .map(|model| FactoryModelInfo {
                    llm_name: model.name.clone(),
                    model_type: model
                        .model_types
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "chat".to_string()),
                    max_tokens: model.max_tokens,
                    is_tools: model.model_types.iter().any(|t| t == "chat"),
                })
                .collect(),
        })
        .collect()
}

/// Default endpoint for a factory key (e.g. `"deepseek"`):
/// fixture `url.default` → provider-catalog preset base → `None`.
pub fn factory_endpoint(factory: &str) -> Option<String> {
    provider_model_catalog()
        .into_iter()
        .find(|provider| provider.factory == factory)
        .and_then(|provider| provider.url.get("default").cloned())
        .or_else(|| crate::providers::provider_default_base(factory, factory).map(str::to_owned))
}

/// `(factory, default endpoint, static model count)` — compact summary of the
/// llm_factories.json mapping for the providers page / diagnostics.
pub fn factory_mapping_summary() -> Vec<(String, String, usize)> {
    factory_llm_infos()
        .into_iter()
        .map(|info| (info.name, info.url, info.llm.len()))
        .collect()
}

fn parse_provider_models(factory: &str, raw: &str) -> Option<ProviderModels> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let name = fixture_str(&value, &["name", "Name"])
        .unwrap_or(factory)
        .to_string();
    let url = fixture_object(&value, &["url"])
        .map(|object| {
            object
                .iter()
                .filter_map(|(region, url)| {
                    url.as_str().map(|url| (region.clone(), url.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let url_suffix = fixture_object(&value, &["url_suffix", "url-suffix"])
        .map(|object| {
            object
                .iter()
                .filter_map(|(kind, suffix)| {
                    suffix
                        .as_str()
                        .map(|suffix| (kind.clone(), suffix.to_string()))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_model_info)
        .collect();
    let features = value.get("features").cloned();
    Some(ProviderModels {
        factory: factory.to_string(),
        name,
        url,
        url_suffix,
        models,
        features,
    })
}

fn parse_model_info(model: &Value) -> Option<ModelInfo> {
    let name = fixture_str(model, &["name", "Name"])?.to_string();
    let max_tokens = model
        .get("max_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_TOKENS);
    let model_types = model
        .get("model_types")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(normalize_model_type)
        .collect::<Vec<_>>();
    let thinking = model
        .pointer("/thinking/default_value")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(ModelInfo {
        name,
        max_tokens,
        model_types,
        thinking,
    })
}

/// Map RAGFlow model_type labels onto RayRAG capability names.
fn normalize_model_type(raw: &str) -> String {
    match raw {
        "vision" | "image2text" | "image_to_text" => "image2text".to_string(),
        "asr" | "speech2text" | "speech_to_text" => "speech2text".to_string(),
        "tts" | "text_to_speech" => "tts".to_string(),
        "embedding" | "embeddings" => "embedding".to_string(),
        other => other.to_string(),
    }
}

fn fixture_str<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
}

fn fixture_object<'a>(
    value: &'a Value,
    keys: &[&str],
) -> Option<&'a serde_json::Map<String, Value>> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_object))
}

// ═══════════════════════════════════════════════════════════════════════════
// RAGFlow conf/all_models.json — flat model alias index. The internal Go SDK
// (internal/entity/models/model.go ProviderManager) builds `alias2ModelIndex`
// from this file (name + display aliases → canonical entry); the Python API
// and the React UI do not consume it. RayRAG ports the same alias-resolution
// semantics plus a UI-autocomplete search.
// ═══════════════════════════════════════════════════════════════════════════

/// One entry of the flat all-models catalog (`name` + display `alias` list).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AllModelEntry {
    pub name: String,
    #[serde(default)]
    pub alias: Vec<String>,
    #[serde(default)]
    pub model_types: Vec<String>,
}

static ALL_MODELS: std::sync::OnceLock<Vec<AllModelEntry>> = std::sync::OnceLock::new();
static ALL_MODEL_ALIASES: std::sync::OnceLock<BTreeMap<String, usize>> = std::sync::OnceLock::new();

fn all_models() -> &'static Vec<AllModelEntry> {
    ALL_MODELS.get_or_init(|| {
        serde_json::from_str::<serde_json::Value>(include_str!(
            "api/fixtures/models/all_models.json"
        ))
        .ok()
        .and_then(|value| value.get("models").cloned())
        .and_then(|models| serde_json::from_value::<Vec<AllModelEntry>>(models).ok())
        .unwrap_or_default()
    })
}

/// Lowercased alias → entry index. The Go SDK rejects duplicate aliases across
/// models; RayRAG keeps the first occurrence deterministically instead.
fn alias_index() -> &'static BTreeMap<String, usize> {
    ALL_MODEL_ALIASES.get_or_init(|| {
        let mut map = BTreeMap::new();
        for (idx, model) in all_models().iter().enumerate() {
            for alias in std::iter::once(&model.name).chain(model.alias.iter()) {
                let alias = alias.trim();
                if alias.is_empty() {
                    continue;
                }
                map.entry(alias.to_lowercase()).or_insert(idx);
            }
        }
        map
    })
}

/// Resolve a model name or display alias (case-insensitive) onto its canonical
/// flat-catalog entry, mirroring the Go SDK `alias2ModelIndex`.
pub fn resolve_model_alias(name: &str) -> Option<&'static AllModelEntry> {
    let idx = *alias_index().get(&name.trim().to_lowercase())?;
    all_models().get(idx)
}

/// Case-insensitive substring search over model names + aliases for the
/// Add-model autocomplete. Returns at most `limit` distinct entries.
pub fn search_model_aliases(query: &str, limit: usize) -> Vec<&'static AllModelEntry> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Vec::new();
    }
    let mut seen: BTreeMap<usize, &'static AllModelEntry> = BTreeMap::new();
    for (idx, model) in all_models().iter().enumerate() {
        let matched = std::iter::once(&model.name)
            .chain(model.alias.iter())
            .any(|alias| alias.to_lowercase().contains(&query));
        if matched {
            seen.entry(idx).or_insert(model);
        }
        if seen.len() >= limit {
            break;
        }
    }
    seen.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capability_verification_probes_chat_embedding_and_rerank() {
        use axum::routing::post;
        let app = axum::Router::new()
            .route(
                "/v1/chat/completions",
                post(|| async {
                    axum::Json(serde_json::json!({
                        "choices": [{ "message": { "content": "Hi" } }]
                    }))
                }),
            )
            .route(
                "/v1/embeddings",
                post(|| async {
                    axum::Json(serde_json::json!({
                        "data": [{ "embedding": [0.1, 0.2] }]
                    }))
                }),
            )
            .route(
                "/v1/rerank",
                post(|| async {
                    axum::Json(serde_json::json!({
                        "results": [{ "index": 0, "relevance_score": 0.9 }]
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        verify_provider_capabilities(
            "openai",
            "OpenAI",
            &format!("http://{address}/v1"),
            None,
            &[
                ("gpt-audit".to_string(), vec![ModelCapability::Chat]),
                ("emb-audit".to_string(), vec![ModelCapability::Embedding]),
                ("rer-audit".to_string(), vec![ModelCapability::Rerank]),
            ],
        )
        .await
        .unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn capability_verification_fails_closed_on_chat_4xx() {
        let app = axum::Router::new().route(
            "/v1/chat/completions",
            axum::routing::post(|| async { axum::http::StatusCode::UNAUTHORIZED }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = verify_provider_capabilities(
            "openai",
            "OpenAI",
            &format!("http://{address}/v1"),
            None,
            &[("gpt-audit".to_string(), vec![ModelCapability::Chat])],
        )
        .await;
        server.abort();
        assert!(result.is_err(), "non-2xx chat probe must fail closed");
    }

    #[test]
    fn provider_key_normalization_matches_ragflow_json_credentials() {
        assert_eq!(
            normalize_provider_api_key(
                "replicate",
                "Replicate",
                Some(r#"{"api_key":"r8_test","extra":true}"#)
            )
            .as_deref(),
            Some("r8_test")
        );
        assert_eq!(
            normalize_provider_api_key(
                "volcengine",
                "VolcEngine",
                Some(r#"{"ark_api_key":"ark_test"}"#)
            )
            .as_deref(),
            Some("ark_test")
        );
        assert_eq!(
            normalize_provider_api_key("openrouter", "OpenRouter", Some("plain-key")).as_deref(),
            Some("plain-key")
        );
        assert_eq!(
            normalize_provider_api_key(
                "new-api",
                "New API",
                Some(r#"{"not_api_key":"preserved"}"#)
            )
            .as_deref(),
            Some(r#"{"not_api_key":"preserved"}"#)
        );
        assert_eq!(
            normalize_provider_api_key("replicate", "Replicate", Some(r#"{"api_key":null}"#)),
            None
        );
    }

    #[test]
    fn api_key_deserializer_accepts_string_object_and_null_only() {
        #[derive(Deserialize)]
        struct Secret {
            #[serde(default, deserialize_with = "deserialize_optional_api_key")]
            api_key: Option<String>,
        }

        assert_eq!(
            serde_json::from_str::<Secret>(r#"{"api_key":"plain"}"#)
                .unwrap()
                .api_key
                .as_deref(),
            Some("plain")
        );
        assert_eq!(
            serde_json::from_str::<Secret>(r#"{"api_key":{"api_key":"nested"}}"#)
                .unwrap()
                .api_key
                .as_deref(),
            Some(r#"{"api_key":"nested"}"#)
        );
        assert!(
            serde_json::from_str::<Secret>(r#"{"api_key":42}"#).is_err(),
            "numeric credentials must not be silently stringified"
        );
        assert_eq!(
            serde_json::from_str::<Secret>(r#"{"api_key":null}"#)
                .unwrap()
                .api_key,
            None
        );
    }

    #[test]
    fn inference_base_adds_v1_only_for_origin_based_local_providers() {
        assert_eq!(
            normalize_inference_base("ollama", "Ollama", "http://127.0.0.1:11434/"),
            "http://127.0.0.1:11434/v1"
        );
        assert_eq!(
            normalize_inference_base("vllm", "VLLM", "http://127.0.0.1:8000/custom"),
            "http://127.0.0.1:8000/custom"
        );
        assert_eq!(
            normalize_inference_base("openrouter", "OpenRouter", "https://openrouter.ai/api/v1/"),
            "https://openrouter.ai/api/v1"
        );
    }

    #[tokio::test]
    async fn provider_connection_probe_is_read_only_and_requires_success_json() {
        use axum::{Json, Router, http::HeaderMap, routing::get};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/v1/models",
            get(|headers: HeaderMap| async move {
                assert_eq!(
                    headers
                        .get(reqwest::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer probe-key")
                );
                Json(serde_json::json!({ "data": [{ "id": "probe-model" }] }))
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        verify_provider_connection(
            "openai",
            "OpenAI",
            &format!("http://{address}/v1"),
            Some("probe-key"),
        )
        .await
        .unwrap();
    }

    #[test]
    fn openai_compatible_inference_covers_all_ragflow_hints() {
        let raw = serde_json::json!({
            "data": [
                {"id": "Qwen3-Embedding-4B", "max_model_len": 32768},
                {"id": "mxbai-rerank-large-v2"},
                {"id": "Qwen2.5-VL-7B", "context_length": 16384},
                {"id": "whisper-large-v3"},
                {"id": "kokoro-tts"}
            ]
        });
        let models = parse_openai_compatible_models(&raw);
        assert_eq!(models[0].model_types, vec![ModelCapability::Embedding]);
        assert_eq!(models[0].max_tokens, 32768);
        assert_eq!(models[1].model_types, vec![ModelCapability::Rerank]);
        assert_eq!(
            models[2].model_types,
            vec![ModelCapability::Chat, ModelCapability::ImageToText]
        );
        assert_eq!(models[3].model_types, vec![ModelCapability::SpeechToText]);
        assert_eq!(models[4].model_types, vec![ModelCapability::TextToSpeech]);
    }

    #[test]
    fn openrouter_uses_modalities_features_and_provider_limits() {
        let raw = serde_json::json!({
            "data": [{
                "id": "vendor/multimodal",
                "architecture": {
                    "input_modalities": ["text", "image", "audio"],
                    "output_modalities": ["text"]
                },
                "supported_parameters": ["tools", "reasoning"],
                "top_provider": {"max_completion_tokens": 65536}
            }]
        });
        let models = parse_openrouter_models(&raw);
        assert_eq!(
            models[0].model_types,
            vec![
                ModelCapability::Chat,
                ModelCapability::ImageToText,
                ModelCapability::SpeechToText
            ]
        );
        assert_eq!(models[0].features, vec!["is_tools", "thinking"]);
        assert_eq!(models[0].max_tokens, 65536);
    }

    #[test]
    fn volcengine_filters_shutdown_and_derives_modalities() {
        let raw = serde_json::json!({
            "data": [
                {"id": "gone", "status": "Shutdown", "domain": "Embedding"},
                {
                    "id": "doubao-vl",
                    "status": "Running",
                    "modalities": {
                        "input_modalities": ["image"],
                        "output_modalities": ["text"]
                    },
                    "features": {"tools": {"function_calling": true}},
                    "token_limits": {
                        "max_reasoning_token_length": 1,
                        "max_input_token_length": 131072
                    }
                }
            ]
        });
        let models = parse_volcengine_models(&raw);
        assert_eq!(models.len(), 1);
        assert_eq!(
            models[0].model_types,
            vec![ModelCapability::Chat, ModelCapability::ImageToText]
        );
        assert_eq!(models[0].features, vec!["is_tools", "thinking"]);
        assert_eq!(models[0].status.as_deref(), Some("Running"));
    }

    #[test]
    fn ollama_and_localai_details_keep_their_distinct_names_and_context_keys() {
        let ollama = serde_json::json!({
            "capabilities": ["completion", "vision", "tools"],
            "details": {"family": "qwen2"},
            "model_info": {"qwen2.context_length": 32768}
        });
        let local = serde_json::json!({
            "capabilities": ["embedding"],
            "model_info": {"general.context_length": 4096}
        });
        let ollama = parse_ollama_detail("qwen2.5:7b", &ollama, false).unwrap();
        let local = parse_ollama_detail("bge-m3:latest", &local, true).unwrap();
        assert_eq!(ollama.name, "qwen2.5:7b");
        assert_eq!(ollama.max_tokens, 32768);
        assert_eq!(local.name, "bge-m3");
        assert_eq!(local.model_types, vec![ModelCapability::Embedding]);
    }

    #[test]
    fn model_urls_match_provider_specific_discovery_contracts() {
        assert_eq!(
            model_list_url(
                ProviderDialect::OpenAiCompatible,
                "http://localhost:8000/custom/v1/chat"
            )
            .unwrap()
            .as_str(),
            "http://localhost:8000/custom/v1/models"
        );
        assert_eq!(
            model_list_url(ProviderDialect::OpenRouter, "https://openrouter.ai/api/v1")
                .unwrap()
                .as_str(),
            "https://openrouter.ai/api/v1/models?output_modalities=all"
        );
        assert_eq!(
            model_list_url(
                ProviderDialect::VolcEngine,
                "https://ark.cn-beijing.volces.com/api/v3"
            )
            .unwrap()
            .as_str(),
            "https://ark.cn-beijing.volces.com/api/v3/models"
        );
    }

    #[test]
    fn seeded_provider_catalog_covers_all_ragflow_fixtures() {
        let catalog = provider_model_catalog();
        // All 61 RAGFlow conf/models fixtures parse into catalog entries
        // (20 baseline + 40 added 2026-08-06 from the live v0.26.2 container,
        // plus deerapi added 2026-08-13 from llm_factories.json).
        assert_eq!(catalog.len(), 61);
        let total_models: usize = catalog.iter().map(|p| p.models.len()).sum();
        assert!(
            total_models > 500,
            "expected 500+ seeded models, got {total_models}"
        );
        // Multi-region URL maps survive (aliyun: default/singapore/us).
        let aliyun = catalog.iter().find(|p| p.factory == "aliyun").unwrap();
        assert!(aliyun.url.contains_key("singapore"));
        assert!(aliyun.url.contains_key("us"));
        assert!(aliyun.url_suffix.contains_key("rerank"));
        // Heterogeneous fixtures parse leniently (baidu "Name", huggingface "url-suffix").
        let baidu = catalog.iter().find(|p| p.factory == "baidu").unwrap();
        assert_eq!(baidu.name, "Baidu");
        assert!(!baidu.models.is_empty());
        let hf = catalog.iter().find(|p| p.factory == "huggingface").unwrap();
        assert_eq!(hf.name, "HuggingFace");
        assert_eq!(hf.models.len(), 1);
        // Dynamic providers carry an empty static list.
        assert!(
            catalog
                .iter()
                .find(|p| p.factory == "ollama")
                .unwrap()
                .models
                .is_empty()
        );
        // list_models_for resolves by factory key; vision/asr normalize onto RayRAG names.
        assert_eq!(list_models_for("deepseek").len(), 2);
        let moonshot = list_models_for("moonshot");
        assert!(
            moonshot
                .iter()
                .any(|m| m.model_types.contains(&"image2text".to_string()))
        );
        assert!(moonshot.iter().any(|m| m.thinking));
        // Provider id/name resolution maps aliases onto fixtures.
        assert_eq!(
            resolve_model_factory("zhipu-ai", "Zhipu AI").as_deref(),
            Some("zhipu-ai")
        );
        assert_eq!(
            resolve_model_factory("qwen", "Aliyun").as_deref(),
            Some("aliyun")
        );
        assert_eq!(
            resolve_model_factory("volcengine", "VolcEngine").as_deref(),
            Some("volcengine")
        );
        assert_eq!(resolve_model_factory("unknown", "Nope").as_deref(), None);
    }

    #[test]
    fn conf_models_catalog_covers_newest_ragflow_models() {
        // Spot-check models added in recent RAGFlow conf/models catalogs —
        // regression guard: when conf/models/*.json grows, the embedded
        // fixtures must be refreshed (they are currently verbatim copies).
        let catalog = provider_model_catalog();
        let has = |factory: &str, model: &str| {
            catalog
                .iter()
                .find(|p| p.factory == factory)
                .is_some_and(|p| p.models.iter().any(|m| m.name == model))
        };
        assert!(has("openai", "gpt-5.2-pro"));
        assert!(has("openai", "gpt-5.2"));
        assert!(has("openai", "whisper-1"));
        assert!(has("openai", "tts-1"));
        assert!(has("deepseek", "deepseek-v4-flash"));
        assert!(has("deepseek", "deepseek-v4-pro"));
        assert!(has("zhipu-ai", "glm-5"));
        assert!(has("zhipu-ai", "glm-5-turbo"));
        assert!(has("zhipu-ai", "embedding-3"));
        assert!(has("zhipu-ai", "glm-asr-2512"));
        assert!(has("zhipu-ai", "glm-tts"));
        assert!(has("zhipu-ai", "glm-ocr"));
        assert!(has("zhipu-ai", "rerank"));
        assert!(has("moonshot", "kimi-k2.6"));
        assert!(has("minimax", "minimax-m2.7"));
        assert!(has("xai", "grok-4"));
        assert!(has("volcengine", "doubao-seed-2-0-pro-260215"));
        assert!(has("volcengine", "doubao-embedding-vision-251215"));
        assert!(has("aliyun", "qwen-flash"));
        assert!(has("aliyun", "text-embedding-v4"));
        assert!(has("baidu", "ernie-5.0"));
        assert!(has("cohere", "rerank-v4.0-pro"));
        assert!(has("cohere", "embed-v4.0"));
        assert!(has("gitee", "BAAI/bge-m3"));
        assert!(has("nvidia", "nvidia/nv-embedqa-mistral-7b-v2"));
        assert!(has("huggingface", "openai/gpt-oss-120b:fastest"));
        assert!(has("deerapi", "gpt-5-chat-latest"));
        // The nvidia catalog is the largest fixture (46 models).
        let nvidia = catalog.iter().find(|p| p.factory == "nvidia").unwrap();
        assert!(nvidia.models.len() >= 46);
    }

    #[test]
    fn factory_llm_infos_maps_factory_to_endpoint_and_models() {
        let infos = factory_llm_infos();
        assert_eq!(infos.len(), 61);
        // Most factories carry a default endpoint (fixture url or preset);
        // self-hosted engines (ollama/vllm/lmstudio/localai/...) legitimately
        // resolve to empty until the user configures them.
        assert!(
            infos.iter().filter(|info| !info.url.is_empty()).count() >= 50,
            "expected 50+ factories to resolve a default endpoint"
        );
        // OpenAI: chat models are tools-capable, embeddings are not.
        let openai = infos.iter().find(|info| info.name == "OpenAI").unwrap();
        let gpt = openai
            .llm
            .iter()
            .find(|m| m.llm_name == "gpt-5.2-pro")
            .unwrap();
        assert_eq!(gpt.model_type, "chat");
        assert!(gpt.is_tools);
        assert_eq!(gpt.max_tokens, 400000);
        let embedding = openai
            .llm
            .iter()
            .find(|m| m.llm_name == "text-embedding-3-large")
            .unwrap();
        assert_eq!(embedding.model_type, "embedding");
        assert!(!embedding.is_tools);
        // Zhipu keeps a rerank row with an empty max_tokens default (8192).
        let zhipu = infos.iter().find(|info| info.name == "ZHIPU-AI").unwrap();
        let rerank = zhipu.llm.iter().find(|m| m.llm_name == "rerank").unwrap();
        assert_eq!(rerank.model_type, "rerank");
        assert_eq!(rerank.max_tokens, 8192);
        // Endpoints come from the fixture url.default.
        assert_eq!(
            factory_endpoint("deepseek").as_deref(),
            Some("https://api.deepseek.com")
        );
        assert_eq!(
            factory_endpoint("openai").as_deref(),
            Some("https://api.openai.com/v1")
        );
        // Summary rows are (factory, endpoint, model count).
        let summary = factory_mapping_summary();
        assert_eq!(summary.len(), 61);
        let (name, url, count) = summary.iter().find(|(n, _, _)| n == "Nvidia").unwrap();
        assert_eq!(url, "https://integrate.api.nvidia.com/v1");
        assert!(*count >= 46);
        // The DeerAPI relay resolves to its public OpenAI-compatible base.
        let (_, deer_url, deer_count) = summary.iter().find(|(n, _, _)| n == "DeerAPI").unwrap();
        assert_eq!(deer_url, "https://api.deerapi.com/v1");
        assert!(*deer_count >= 37);
    }
    #[test]
    fn all_models_alias_index_resolves_names_and_aliases() {
        // Case-insensitive name resolution.
        let entry = resolve_model_alias("gpt-4o").expect("gpt-4o is in all_models.json");
        assert_eq!(entry.name, "gpt-4o");
        assert!(entry.model_types.iter().any(|t| t == "chat"));
        // Case-insensitive.
        assert_eq!(
            resolve_model_alias("GPT-4O").map(|e| &e.name),
            Some(&"gpt-4o".to_string())
        );
        // Unknown alias resolves to nothing.
        assert!(resolve_model_alias("definitely-not-a-real-model-xyz").is_none());
    }

    #[test]
    fn all_models_search_finds_substrings_and_respects_limit() {
        let hits = search_model_aliases("deepseek", 10);
        assert!(!hits.is_empty(), "deepseek models must be searchable");
        assert!(hits.iter().all(|entry| {
            std::iter::once(&entry.name)
                .chain(entry.alias.iter())
                .any(|a| a.to_lowercase().contains("deepseek"))
        }));
        assert!(hits.len() <= 10);
        // Empty query short-circuits.
        assert!(search_model_aliases("", 10).is_empty());
        // Limit is honored.
        assert!(search_model_aliases("gpt", 3).len() <= 3);
    }
}
