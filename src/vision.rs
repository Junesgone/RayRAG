//! Vision (image-to-text) model client — OpenAI-compatible VLM endpoints.
//!
//! Mirrors RAGFlow's `rag/llm/cv_model.py` (GptV4 and its subclasses):
//! every vision provider is an OpenAI-compatible chat completion with an
//! `image_url` content part, so a single client covers OpenAI GPT-4o,
//! xAI grok-vision, Qwen-VL, Hunyuan-Vision, Zhipu 4V, StepFun, VolcEngine,
//! LM Studio, Together AI, Yi-VL, SiliconFlow and local llama.cpp servers
//! (e.g. a Qwen3-VL GGUF with `--mmproj`).
//!
//! The default `describe()` prompt is RAGFlow's
//! `vision_llm_describe_prompt.md` (PDF page image -> clean Markdown).

use crate::Result;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use reqwest::Client;
use serde_json::{Value, json};

/// Default transcription prompt, ported from
/// `rag/prompts/vision_llm_describe_prompt.md`.
pub const VISION_LLM_DESCRIBE_PROMPT: &str = r#"## INSTRUCTION
Transcribe the content from the provided PDF page image into clean Markdown format.

- Only output the content transcribed from the image.
- Do NOT output this instruction or any other explanation.
- If the content is missing or you do not understand the input, return an empty string.

## RULES
1. Do NOT generate examples, demonstrations, or templates.
2. Do NOT output any extra text such as 'Example', 'Example Output', or similar.
3. Do NOT generate any tables, headings, or content that is not explicitly present in the image.
4. Transcribe content word-for-word. Do NOT modify, translate, or omit any content.
5. Do NOT explain Markdown or mention that you are using Markdown.
6. Do NOT wrap the output in ```markdown or ``` blocks.
7. Only apply Markdown structure to headings, paragraphs, lists, and tables, strictly based on the layout of the image. Do NOT create tables unless an actual table exists in the image.
8. Preserve the original language, information, and order exactly as shown in the image.

> If you do not detect valid content in the image, return an empty string.
"#;

/// Vision model client configuration.
#[derive(Debug, Clone)]
pub struct VisionConfig {
    /// API base URL (OpenAI-compatible, e.g. http://127.0.0.1:8088/v1).
    pub api_base: String,
    /// API key (may be empty for local servers).
    pub api_key: String,
    /// Model name (e.g. "Qwen3.5-9B-Q4_K_M.gguf" or "gpt-4o").
    pub model: String,
    /// Response language hint used by some providers (default "Chinese").
    pub lang: String,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "gpt-4o".into(),
            lang: "Chinese".into(),
        }
    }
}

/// OpenAI-compatible vision (image-to-text) client.
pub struct VisionClient {
    config: VisionConfig,
    client: Client,
}

/// Result of a vision describe call: the model's text plus token usage.
#[derive(Debug, Clone, PartialEq)]
pub struct VisionDescription {
    pub content: String,
    pub total_tokens: u64,
}

impl VisionClient {
    /// Create a new vision client.
    pub fn new(config: VisionConfig) -> Self {
        let client = crate::common::cmd_timeout::model_client_for(&config.api_base);
        Self { config, client }
    }

    /// Normalize an image input into a base64 data URL.
    ///
    /// Accepts (mirroring RAGFlow `Base._normalize_image` / `image2base64`):
    /// - a local file path
    /// - raw bytes
    /// - an already-encoded `data:image/...;base64,...` URL (passed through)
    /// - a plain base64 string (wrapped as image/png data URL)
    pub fn normalize_image(image: &[u8], mime: &str) -> String {
        format!("data:{};base64,{}", mime, BASE64.encode(image))
    }

    /// Build the messages payload for an image with an optional custom prompt.
    ///
    /// Mirrors RAGFlow `Base._image_prompt` + `GptV4.describe_with_prompt`:
    /// content is an array of `{type:"text"}` and `{type:"image_url"}` parts.
    fn vision_messages(&self, image: &str, prompt: Option<&str>) -> Value {
        json!([{
            "role": "user",
            "content": [
                {"type": "text", "text": prompt.unwrap_or(VISION_LLM_DESCRIBE_PROMPT)},
                {"type": "image_url", "image_url": {"url": image}}
            ]
        }])
    }

    async fn describe_inner(&self, image: &str, prompt: Option<&str>) -> Result<VisionDescription> {
        let url = format!(
            "{}/chat/completions",
            self.config.api_base.trim_end_matches('/')
        );
        let body = json!({
            "model": self.config.model,
            "messages": self.vision_messages(image, prompt),
            "max_tokens": 2048,
        });

        let mut req = self.client.post(&url).json(&body);
        if !self.config.api_key.is_empty() {
            req = req.bearer_auth(&self.config.api_key);
        }

        let resp = req.send().await.map_err(|error| {
            anyhow::anyhow!(
                "Vision request failed: {}. Check the image-to-text model's API base URL, connectivity and API key", error.without_url()
            )
        })?;
        let status = resp.status();
        let json: Value = crate::common::cmd_timeout::read_json_limited(
            resp,
            crate::common::cmd_timeout::body_limit_bytes(),
            "Vision API",
        )
        .await.map_err(|error| anyhow::anyhow!(
            "{error}. Check the image-to-text model's API base URL and OpenAI-compatible response format"
        ))?;

        if !status.is_success() {
            let err_msg = json
                .pointer("/error/message")
                .and_then(Value::as_str)
                .or_else(|| json["error"].as_str())
                .or_else(|| json["message"].as_str())
                .unwrap_or("Unknown error");
            anyhow::bail!(
                "Vision API error ({}): {}. Check the image-to-text model's API base URL, API key and vision support",
                status,
                err_msg
            );
        }

        if json["choices"].as_array().is_none_or(Vec::is_empty) {
            anyhow::bail!(
                "Vision API returned HTTP {status} with an empty response (no choices). Check the image-to-text model's API base URL and OpenAI-compatible response format"
            );
        }
        if !json["choices"][0]["message"]["content"].is_string() {
            anyhow::bail!(
                "Vision API returned HTTP {status} without text content. Check the image-to-text model's API base URL, vision support and OpenAI-compatible response format"
            );
        }
        let content = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_string();
        let total_tokens = json["usage"]["total_tokens"].as_u64().unwrap_or(0);

        Ok(VisionDescription {
            content,
            total_tokens,
        })
    }

    /// Describe an image with the default PDF-transcription prompt.
    ///
    /// Mirrors RAGFlow `GptV4.describe(image)`.
    pub async fn describe(&self, image: &str) -> Result<VisionDescription> {
        self.describe_inner(image, None).await
    }

    /// Describe an image with a custom prompt.
    ///
    /// Mirrors RAGFlow `GptV4.describe_with_prompt(image, prompt)`.
    pub async fn describe_with_prompt(
        &self,
        image: &str,
        prompt: &str,
    ) -> Result<VisionDescription> {
        self.describe_inner(image, Some(prompt)).await
    }
}


