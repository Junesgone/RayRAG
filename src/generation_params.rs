//! RAGFlow-compatible generation parameter extraction and validation.

use serde::{Deserialize, Serialize};

pub const GENERATION_CONFIG_KEYS: [&str; 6] = [
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "max_tokens",
    "reasoning",
];

/// Effective parameters forwarded to an OpenAI-compatible completion call.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GenerationParams {
    pub temperature: f32,
    pub top_p: f32,
    pub frequency_penalty: f32,
    pub presence_penalty: f32,
    pub max_tokens: u32,
    /// Thinking / chain-of-thought mode (RAGFlow prompt_config.reasoning).
    #[serde(default)]
    pub reasoning: bool,
}

impl Default for GenerationParams {
    fn default() -> Self {
        // RAGFlow v0.26.4 Dialog.llm_setting defaults.
        Self {
            temperature: 0.1,
            top_p: 0.3,
            frequency_penalty: 0.7,
            presence_penalty: 0.4,
            max_tokens: 512,
            reasoning: false,
        }
    }
}

/// Optional request overrides. Unknown request fields are intentionally ignored.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GenerationParamsPatch {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub max_tokens: Option<u32>,
    pub reasoning: Option<bool>,
}

impl<'de> Deserialize<'de> for GenerationParamsPatch {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        Self::from_request(&value).map_err(serde::de::Error::custom)
    }
}

impl GenerationParams {
    pub fn merged(self, patch: GenerationParamsPatch) -> Self {
        Self {
            temperature: patch.temperature.unwrap_or(self.temperature),
            top_p: patch.top_p.unwrap_or(self.top_p),
            frequency_penalty: patch.frequency_penalty.unwrap_or(self.frequency_penalty),
            presence_penalty: patch.presence_penalty.unwrap_or(self.presence_penalty),
            max_tokens: patch.max_tokens.unwrap_or(self.max_tokens),
            reasoning: patch.reasoning.unwrap_or(self.reasoning),
        }
    }

    pub fn with_max_tokens(self, max_tokens: u32) -> Self {
        Self { max_tokens, ..self }
    }
}

impl GenerationParamsPatch {
    /// Mirrors `_generation_params.extract_generation_config`: only five named,
    /// non-null top-level fields are read. RayRAG adds an explicit HTTP-safe
    /// type/range contract instead of deferring malformed values to a provider.
    pub fn from_request(value: &serde_json::Value) -> anyhow::Result<Self> {
        let object = value
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Request body must be a JSON object"))?;
        Ok(Self {
            temperature: optional_float(object, "temperature", 0.0, 1.0)?,
            top_p: optional_float(object, "top_p", 0.0, 1.0)?,
            frequency_penalty: optional_float(object, "frequency_penalty", 0.0, 1.0)?,
            presence_penalty: optional_float(object, "presence_penalty", 0.0, 1.0)?,
            max_tokens: optional_u32(object, "max_tokens", 1, 128_000)?,
            reasoning: object.get("reasoning").and_then(|v| v.as_bool()),
        })
    }
}

fn optional_float(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    min: f32,
    max: f32,
) -> anyhow::Result<Option<f32>> {
    let Some(value) = object.get(key).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let value = value
        .as_f64()
        .filter(|value| value.is_finite())
        .ok_or_else(|| anyhow::anyhow!("`{key}` must be a finite number"))?;
    if !(f64::from(min)..=f64::from(max)).contains(&value) {
        anyhow::bail!("`{key}` must be in range [{min}, {max}]");
    }
    Ok(Some(value as f32))
}

fn optional_u32(
    object: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    min: u32,
    max: u32,
) -> anyhow::Result<Option<u32>> {
    let Some(value) = object.get(key).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let value = value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| anyhow::anyhow!("`{key}` must be an integer"))?;
    if !(min..=max).contains(&value) {
        anyhow::bail!("`{key}` must be in range [{min}, {max}]");
    }
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_only_non_null_generation_keys_and_merges_defaults() {
        let patch = GenerationParamsPatch::from_request(&serde_json::json!({
            "temperature": 0.8,
            "top_p": null,
            "max_tokens": 2048,
            "stream": true,
            "unknown": "ignored"
        }))
        .unwrap();
        let params = GenerationParams::default().merged(patch);
        assert_eq!(params.temperature, 0.8);
        assert_eq!(params.top_p, 0.3);
        assert_eq!(params.max_tokens, 2048);
        assert_eq!(params.frequency_penalty, 0.7);
    }

    #[test]
    fn rejects_wrong_types_and_out_of_range_values() {
        let type_error = GenerationParamsPatch::from_request(&serde_json::json!({
            "temperature": "0.5"
        }))
        .unwrap_err();
        assert!(type_error.to_string().contains("finite number"));
        let range_error = GenerationParamsPatch::from_request(&serde_json::json!({
            "max_tokens": 0
        }))
        .unwrap_err();
        assert!(range_error.to_string().contains("[1, 128000]"));
    }

    #[test]
    fn effective_params_round_trip() {
        let params = GenerationParams::default();
        let json = serde_json::to_value(params).unwrap();
        assert_eq!(
            serde_json::from_value::<GenerationParams>(json).unwrap(),
            params
        );
    }
}
