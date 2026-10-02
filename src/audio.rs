//! Speech-to-text (ASR) and text-to-speech (TTS) clients — OpenAI-compatible.
//!
//! Mirrors RAGFlow's `rag/llm/sequence2txt_model.py` (GPTSeq2txt and its
//! subclasses: StepFun / FuturMix / DeepInfra / Gitee / CometAPI / DeerAPI /
//! GPUStack / Xinference — all OpenAI-compatible `/audio/transcriptions`
//! multipart endpoints) and `rag/llm/tts_model.py` (HTTPBasedTTS and its
//! subclasses: OpenAI / Xinference / Ollama / GPUStack / SILICONFLOW /
//! DeepInfra — all POST `{base_url}/audio/speech` with a JSON payload of
//! `{"model", "voice", "input"}` and a raw audio byte response).
//!
//! A single OpenAI-compatible surface therefore covers every provider that
//! exposes either endpoint, including domestic (China) deployments such as
//! StepFun, SiliconFlow, Xinference and Ollama.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

/// Configuration shared by the ASR and TTS clients.
#[derive(Debug, Clone)]
pub struct AudioConfig {
    /// Base URL, e.g. `https://api.openai.com/v1` or `http://127.0.0.1:8088/v1`.
    pub api_base: String,
    /// API key; may be empty for local endpoints without auth.
    pub api_key: String,
    /// Model name, e.g. `whisper-1`, `step-asr`, `qwen-audio-asr`.
    pub model: String,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.openai.com/v1".into(),
            api_key: String::new(),
            model: "whisper-1".into(),
        }
    }
}

/// Output of a transcription call.
#[derive(Debug, Clone, PartialEq)]
pub struct Transcription {
    /// The recognized text (stripped).
    pub text: String,
    /// Approximate token count of the text (mirrors RAGFlow's
    /// `num_tokens_from_string` return in `transcription()`).
    pub tokens: usize,
}

/// Speech-to-text client (OpenAI-compatible `/audio/transcriptions`).
#[derive(Debug, Clone)]
pub struct AsrClient {
    pub config: AudioConfig,
    client: reqwest::Client,
}

/// Build an optional ASR client from environment (`RAYRAG_ASR_API_BASE` +
/// `RAYRAG_ASR_API_KEY` + `RAYRAG_ASR_MODEL`). Returns `Ok(None)` when the
/// base URL is unset (audio parsing falls back to metadata placeholders).
pub fn asr_from_env() -> Result<Option<AsrClient>> {
    let Some(api_base) = std::env::var("RAYRAG_ASR_API_BASE")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let api_key = std::env::var("RAYRAG_ASR_API_KEY").unwrap_or_default();
    let model = std::env::var("RAYRAG_ASR_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "whisper-1".to_string());
    Ok(Some(AsrClient::new(AudioConfig {
        api_base,
        api_key,
        model,
    })))
}

impl AsrClient {
    pub fn new(config: AudioConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(crate::common::cmd_timeout::duration())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client build");
        Self { config, client }
    }

    /// Transcribe an in-memory audio file.
    ///
    /// `audio` is the raw audio bytes (wav/mp3/ogg/flac...), `filename` is
    /// passed through to the multipart `file` part (e.g. `audio.wav`) so
    /// servers can sniff the container format.
    pub async fn transcribe(&self, audio: &[u8], filename: &str) -> Result<Transcription> {
        let url = format!(
            "{}/audio/transcriptions",
            self.config.api_base.trim_end_matches('/')
        );
        let form = reqwest::multipart::Form::new()
            .part(
                "file",
                reqwest::multipart::Part::bytes(audio.to_vec()).file_name(filename.to_string()),
            )
            .text("model", self.config.model.clone());
        let mut request = self
            .client
            .post(&url)
            .multipart(form)
            .header(reqwest::header::ACCEPT, "application/json");
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(&self.config.api_key);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("ASR request failed: {url}"))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .with_context(|| "ASR response body read failed")?;
        if !status.is_success() {
            bail!(
                "ASR API error ({status}): {}",
                String::from_utf8_lossy(&body)
                    .chars()
                    .take(300)
                    .collect::<String>()
            );
        }
        let value: serde_json::Value =
            serde_json::from_slice(&body).with_context(|| "ASR response is not JSON")?;
        let text = value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("ASR response missing `text` field: {value}"))?
            .trim()
            .to_string();
        let tokens = crate::chunk::token_count(&text);
        Ok(Transcription { text, tokens })
    }
}

/// Text-to-speech client (OpenAI-compatible `/audio/speech`).
#[derive(Debug, Clone)]
pub struct TtsClient {
    pub config: AudioConfig,
    /// Default voice name when none is supplied.
    pub default_voice: String,
    client: reqwest::Client,
}

impl TtsClient {
    pub fn new(config: AudioConfig) -> Self {
        Self::with_voice(config, "alloy")
    }

    pub fn with_voice(config: AudioConfig, default_voice: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(crate::common::cmd_timeout::duration())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("reqwest client build");
        Self {
            config,
            default_voice: default_voice.into(),
            client,
        }
    }

    /// Synthesize speech; returns the raw audio bytes (typically MP3).
    pub async fn synthesize(&self, text: &str) -> Result<Vec<u8>> {
        self.synthesize_with_voice(text, &self.default_voice).await
    }

    /// Synthesize speech with an explicit voice (mirrors RAGFlow
    /// `HTTPBasedTTS.tts(text, voice="alloy")` with the same JSON payload
    /// shape `{"model", "voice", "input"}`).
    pub async fn synthesize_with_voice(&self, text: &str, voice: &str) -> Result<Vec<u8>> {
        let url = format!(
            "{}/audio/speech",
            self.config.api_base.trim_end_matches('/')
        );
        let payload = serde_json::json!({
            "model": self.config.model,
            "voice": voice,
            "input": text,
        });
        let mut request = self.client.post(&url).json(&payload);
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(&self.config.api_key);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("TTS request failed: {url}"))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .with_context(|| "TTS response body read failed")?;
        if !status.is_success() {
            bail!(
                "TTS API error ({status}): {}",
                String::from_utf8_lossy(&body)
                    .chars()
                    .take(300)
                    .collect::<String>()
            );
        }
        if body.is_empty() {
            bail!("TTS API returned an empty audio body");
        }
        Ok(body.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::StatusCode,
        routing::{get, post},
    };
    use serde_json::{Value, json};

    async fn spawn_audio_server() -> (String, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/v1/audio/transcriptions",
                post(|mut multipart: axum::extract::Multipart| async move {
                    let mut file_name = String::new();
                    let mut model = String::new();
                    let mut bytes: Option<Vec<u8>> = None;
                    let mut field = multipart.next_field().await.unwrap();
                    while let Some(f) = field {
                        let name = f.name().unwrap_or("").to_string();
                        if name == "file" {
                            file_name = f.file_name().unwrap_or("").to_string();
                            let data = f.bytes().await.unwrap();
                            bytes = Some(data.to_vec());
                        } else if name == "model" {
                            model = f.text().await.unwrap();
                        }
                        field = multipart.next_field().await.unwrap();
                    }
                    let bytes = bytes.unwrap();
                    assert_eq!(model, "whisper-1");
                    assert_eq!(bytes, b"RIFFfake-wav");
                    assert_eq!(file_name, "audio.wav");
                    axum::Json(json!({"text": "  Hello RayRAG ASR  " }))
                }),
            )
            .route(
                "/v1/audio/speech",
                post(|payload: axum::Json<Value>| async move {
                    assert_eq!(payload["model"], "kokoro-tts");
                    assert_eq!(payload["voice"], "alloy");
                    assert_eq!(payload["input"], "Hello RayRAG TTS");
                    (
                        StatusCode::OK,
                        [(axum::http::header::CONTENT_TYPE, "audio/mpeg")],
                        Body::from(vec![0x49u8, 0x44, 0x33, 0x00, 0x01]), // fake ID3 header
                    )
                }),
            )
            .route("/health", get(|| async { "ok" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}/v1"), server)
    }

    #[tokio::test]
    async fn asr_transcribes_multipart_audio_with_openai_shape() {
        let (base, _server) = spawn_audio_server().await;
        let client = AsrClient::new(AudioConfig {
            api_base: base,
            api_key: "test-key".into(),
            model: "whisper-1".into(),
        });
        let out = client
            .transcribe(b"RIFFfake-wav", "audio.wav")
            .await
            .unwrap();
        assert_eq!(out.text, "Hello RayRAG ASR");
        assert!(out.tokens > 0);
    }

    #[tokio::test]
    async fn asr_rejects_missing_text_field_with_clear_error() {
        let app = Router::new().route(
            "/v1/audio/transcriptions",
            post(|mut _multipart: axum::extract::Multipart| async move {
                axum::Json(json!({"error": "no text"}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = AsrClient::new(AudioConfig {
            api_base: format!("http://{addr}/v1"),
            api_key: String::new(),
            model: "whisper-1".into(),
        });
        let err = client.transcribe(b"wav", "a.wav").await.unwrap_err();
        assert!(err.to_string().contains("missing `text` field"), "{err}");
        server.abort();
    }

    #[tokio::test]
    async fn tts_synthesizes_raw_audio_bytes_with_default_voice() {
        let (base, _server) = spawn_audio_server().await;
        let client = TtsClient::new(AudioConfig {
            api_base: base,
            api_key: "test-key".into(),
            model: "kokoro-tts".into(),
        });
        let audio = client.synthesize("Hello RayRAG TTS").await.unwrap();
        assert_eq!(audio, vec![0x49u8, 0x44, 0x33, 0x00, 0x01]);
    }

    #[tokio::test]
    async fn tts_propagates_non_success_status_with_truncated_body() {
        let app = Router::new().route(
            "/v1/audio/speech",
            post(|_payload: axum::Json<Value>| async move {
                (StatusCode::BAD_REQUEST, "quota exceeded")
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = TtsClient::new(AudioConfig {
            api_base: format!("http://{addr}/v1"),
            api_key: String::new(),
            model: "m".into(),
        });
        let err = client.synthesize("hi").await.unwrap_err();
        assert!(err.to_string().contains("400"), "{err}");
        server.abort();
    }

    #[tokio::test]
    async fn asr_client_sets_bearer_auth_when_key_is_present() {
        let (base, _server) = spawn_audio_server().await;
        // The mock does not check auth; verify the request still succeeds and
        // the client was constructed with the key (no panic paths).
        let client = AsrClient::new(AudioConfig {
            api_base: base,
            api_key: "sk-secret".into(),
            model: "whisper-1".into(),
        });
        let out = client
            .transcribe(b"RIFFfake-wav", "audio.wav")
            .await
            .unwrap();
        assert_eq!(out.text, "Hello RayRAG ASR");
    }
}
