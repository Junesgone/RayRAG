//! The two audio endpoints of the chat surface, `POST /api/v1/chat/audio/speech` (text to speech) and
//! `POST /api/v1/chat/audio/transcription` (speech to text).
//!
//! Both are thin over the OpenAI-compatible clients in [`crate::audio`], which is exactly what
//! upstream does through `LLMBundle`: the endpoint only decides *which* model to use, so a deployment
//! that has configured no speech model must say so rather than silently return nothing.
//!
//! The failure wording is upstream's, because clients match on it:
//!
//! | situation | answer |
//! |---|---|
//! | no TTS endpoint configured | `102 No default TTS model is set` |
//! | no ASR endpoint configured | `102 No default ASR model is set` |
//! | no `file` part | `102 Missing 'file' in multipart form-data` |
//! | a suffix outside the allowlist | `102 Unsupported audio format: .mp4. Allowed: .aac, .flac, .m4a, .mp3, .ogg, .opus, .wav, .webm, .wma` |
//!
//! Two deliberate differences from upstream, both in the direction of not hiding failures:
//!
//! * speech synthesizes before answering, so a provider error becomes an explicit `105` with the
//!   provider's message instead of a truncated audio stream whose headers already said `200 OK`;
//! * transcription's `stream=true` emits one `partial` event carrying the whole transcript and then
//!   `[DONE]`, because the transcription call underneath is not itself streaming. A client that reads
//!   partial events still gets the text, and the deviation is documented rather than faked with
//!   invented partials.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequest, Multipart, Request, State};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;

use crate::server::{AppState, AuthContext, api_error_code, code};

/// The suffixes upstream accepts for transcription, sorted the way upstream sorts them in the message.
pub(crate) const ALLOWED_AUDIO_EXTENSIONS: [&str; 9] = [
    ".aac", ".flac", ".m4a", ".mp3", ".ogg", ".opus", ".wav", ".webm", ".wma",
];

/// The suffix of `filename`, lowercased, or an empty string when it has none.
pub(crate) fn audio_extension(filename: &str) -> String {
    match filename.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => format!(".{}", extension.to_lowercase()),
        _ => String::new(),
    }
}

/// The documented refusal for a suffix outside the allowlist.
pub(crate) fn unsupported_format_message(suffix: &str) -> String {
    format!(
        "Unsupported audio format: {suffix}. Allowed: {}",
        ALLOWED_AUDIO_EXTENSIONS.join(", ")
    )
}

/// Upstream answers these through `get_data_error_result`, i.e. HTTP 200 carrying the business code.
fn data_error(message: &str) -> Response {
    Json(serde_json::json!({ "code": code::INVALID_OR_MISSING_DATA, "data": null, "message": message }))
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct SpeechRequest {
    #[serde(default)]
    pub text: String,
}

/// `POST /api/v1/chat/audio/speech`.
pub async fn speech(
    State(_state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
    Json(body): Json<SpeechRequest>,
) -> Response {
    let text = body.text.trim();
    if text.is_empty() {
        // Upstream indexes `req["text"]` and would raise; naming the field is more useful and is the
        // same choice the rest of RayRAG makes for a missing argument.
        return api_error_code(
            axum::http::StatusCode::BAD_REQUEST,
            code::INVALID_ARGUMENT,
            "required argument are missing: text",
        );
    }
    let client = match crate::audio::tts_from_env() {
        Ok(Some(client)) => client,
        Ok(None) => return data_error("No default TTS model is set"),
        Err(error) => return data_error(&error.to_string()),
    };
    match client.synthesize(text).await {
        Ok(bytes) if !bytes.is_empty() => Response::builder()
            .status(axum::http::StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "audio/mpeg")
            .header(axum::http::header::CACHE_CONTROL, "no-cache")
            .header(axum::http::header::CONNECTION, "keep-alive")
            .header("X-Accel-Buffering", "no")
            .body(Body::from(bytes))
            .unwrap_or_else(|error| {
                api_error_code(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    code::OPERATION_ERROR,
                    &error.to_string(),
                )
            }),
        Ok(_) => data_error("The TTS provider returned an empty response"),
        Err(error) => api_error_code(
            axum::http::StatusCode::BAD_GATEWAY,
            code::CONNECTION_ERROR,
            &error.to_string(),
        ),
    }
}

/// `POST /api/v1/chat/audio/transcription`.
pub async fn transcription(
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
    request: Request,
) -> Response {
    // A raw request rather than an extractor argument: axum refuses a multipart body on a route
    // whose extractor set does not itself announce it, and the failure mode is opaque.
    let multipart = match Multipart::from_request(request, &state).await {
        Ok(multipart) => multipart,
        Err(error) => {
            return data_error(&format!("Missing 'file' in multipart form-data ({error})"));
        }
    };
    let (audio, filename, stream) = match read_multipart(multipart).await {
        Ok(parsed) => parsed,
        Err(message) => return data_error(&message),
    };
    let suffix = audio_extension(&filename);
    if !ALLOWED_AUDIO_EXTENSIONS.contains(&suffix.as_str()) {
        return data_error(&unsupported_format_message(&suffix));
    }
    let client = match crate::audio::asr_from_env() {
        Ok(Some(client)) => client,
        Ok(None) => return data_error("No default ASR model is set"),
        Err(error) => return data_error(&error.to_string()),
    };
    // The provider is given a name carrying the suffix it must see: the extension is how these APIs
    // tell a wav from an mp3.
    let name = if filename.is_empty() {
        format!("audio{suffix}")
    } else {
        filename.clone()
    };
    let transcription = match client.transcribe(&audio, &name).await {
        Ok(transcription) => transcription,
        Err(error) => {
            return api_error_code(
                axum::http::StatusCode::BAD_GATEWAY,
                code::CONNECTION_ERROR,
                &error.to_string(),
            );
        }
    };
    if stream {
        let event = serde_json::json!({ "event": "partial", "text": transcription.text });
        let body = format!("data: {event}\n\ndata: [DONE]\n\n");
        return Response::builder()
            .status(axum::http::StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
            .header(axum::http::header::CACHE_CONTROL, "no-cache")
            .header("X-Accel-Buffering", "no")
            .body(Body::from(body))
            .unwrap_or_else(|error| {
                api_error_code(
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    code::OPERATION_ERROR,
                    &error.to_string(),
                )
            });
    }
    Json(serde_json::json!({
        "code": 0,
        "data": { "text": transcription.text, "tokens": transcription.tokens },
        "message": "success",
    }))
    .into_response()
}

/// Pull `file` and `stream` out of the multipart body.
#[allow(clippy::type_complexity)]
async fn read_multipart(mut multipart: Multipart) -> Result<(Vec<u8>, String, bool), String> {
    let mut audio: Option<Vec<u8>> = None;
    let mut filename = String::new();
    let mut stream = false;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| format!("Missing 'file' in multipart form-data ({error})"))?
    {
        match field.name().unwrap_or_default() {
            "file" => {
                filename = field.file_name().unwrap_or_default().to_string();
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|error| format!("Could not read the uploaded audio: {error}"))?;
                audio = Some(bytes.to_vec());
            }
            "stream" => {
                let value = field.text().await.unwrap_or_default();
                stream = value.trim().eq_ignore_ascii_case("true");
            }
            _ => {}
        }
    }
    match audio {
        Some(bytes) if !bytes.is_empty() => Ok((bytes, filename, stream)),
        Some(_) => Err("The uploaded audio is empty".to_string()),
        None => Err("Missing 'file' in multipart form-data".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suffix_is_read_lowercased_and_only_from_a_real_extension() {
        assert_eq!(audio_extension("recording.WAV"), ".wav");
        assert_eq!(audio_extension("a.b.mp3"), ".mp3");
        assert_eq!(audio_extension("noextension"), "");
        assert_eq!(audio_extension(".hidden"), "");
        assert!(ALLOWED_AUDIO_EXTENSIONS.contains(&audio_extension("x.M4A").as_str()));
    }

    #[test]
    fn the_refusal_names_the_suffix_and_every_allowed_one() {
        let message = unsupported_format_message(".mp4");
        assert_eq!(
            message,
            "Unsupported audio format: .mp4. Allowed: .aac, .flac, .m4a, .mp3, .ogg, .opus, .wav, .webm, .wma"
        );
        // The same sentence the guide documents, character for character.
        assert!(message.ends_with(".wma"));
    }

    #[test]
    fn the_allowlist_is_exactly_the_documented_set() {
        assert_eq!(
            ALLOWED_AUDIO_EXTENSIONS.to_vec(),
            vec![
                ".aac", ".flac", ".m4a", ".mp3", ".ogg", ".opus", ".wav", ".webm", ".wma"
            ]
        );
        // `.mp4` is a video container: the guide uses it as the counterexample.
        assert!(!ALLOWED_AUDIO_EXTENSIONS.contains(&".mp4"));
    }
}
