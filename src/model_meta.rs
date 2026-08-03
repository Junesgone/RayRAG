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
            std::env::var(env_name).ok().filter(|key| !key.trim().is_empty())?
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
        if let Some(default) = crate::providers::provider_default_base(provider_id, provider_name)
        {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
