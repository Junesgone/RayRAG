//! DeepSeek application tool APIs (Search, Fetch, OCR, ASR, TTS).
//!
//! These are not OpenAI-compatible endpoints: DeepSeek exposes them under
//! /v1/tools/* with its own JSON or multipart shapes. The chat application
//! tools (deepseek_web_search / deepseek_read_url) are declared in the chat
//! request instead and live in crate::llm.

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

/// True when the configured base belongs to DeepSeek.
pub fn is_deepseek_base(api_base: &str) -> bool {
    api_base.to_ascii_lowercase().contains("api.deepseek.com")
}

/// DeepSeek endpoints are documented at https://api.deepseek.com/v1/tools/*.
/// Provider instances commonly store the OpenAI-compatible base with a /v1
/// suffix, so normalize to the origin before appending the tool path.
pub fn origin(api_base: &str) -> String {
    let trimmed = api_base.trim().trim_end_matches('/');
    let trimmed = trimmed
        .strip_suffix("/v1")
        .or_else(|| trimmed.strip_suffix("/V1"))
        .unwrap_or(trimmed);
    trimmed.trim_end_matches('/').to_string()
}

/// One normalized Search API hit.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub summary: String,
    pub content: String,
    pub score: f64,
}

/// One fetched page.
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedPage {
    pub title: String,
    pub content: String,
}

fn parse_search_results(value: &Value) -> Vec<SearchHit> {
    value
        .get("results")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let url = row.get("url").and_then(Value::as_str).unwrap_or("");
                    if url.is_empty() {
                        return None;
                    }
                    let score = row
                        .get("rerank_score")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                        / 3.0;
                    Some(SearchHit {
                        title: row
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        url: url.to_string(),
                        snippet: row
                            .get("snippet")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        summary: row
                            .get("summary")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        content: row
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        score: score.clamp(0.0, 1.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_ocr_pages(value: &Value) -> Result<String> {
    if let Some(error) = value.get("error") {
        return Err(anyhow!("DeepSeek OCR error: {error}"));
    }
    let pages = value
        .get("pages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("DeepSeek OCR response has no pages array: {value}"))?;
    let text = pages
        .iter()
        .filter_map(|page| page.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(text.trim().to_string())
}

fn deepseek_voice(voice: &str) -> &'static str {
    match voice.trim().to_ascii_lowercase().as_str() {
        "tide" | "male" => "tide",
        // OpenAI/other-provider names have no DeepSeek equivalent; Stella is
        // the documented default.
        _ => "stella",
    }
}

fn error_message(status: reqwest::StatusCode, body: &str) -> String {
    let detail = body.chars().take(400).collect::<String>();
    format!("DeepSeek tool API error ({status}): {detail}")
}

/// Search the web through the DeepSeek Search API.
pub async fn search(
    api_base: &str,
    api_key: &str,
    query: &str,
    count: u32,
    need_content: bool,
) -> Result<Vec<SearchHit>> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("DeepSeek Search requires DEEPSEEK_API_KEY"));
    }
    let url = format!("{}/v1/tools/search", origin(api_base));
    let body = serde_json::json!({
        "query": query,
        "count": count.clamp(1, 50),
        "need_content": need_content,
    });
    let response = crate::common::cmd_timeout::model_client_for(&origin(api_base))
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("DeepSeek Search request failed: {url}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(error_message(status, &text)));
    }
    let value: Value =
        serde_json::from_str(&text).with_context(|| "DeepSeek Search response is not JSON")?;
    Ok(parse_search_results(&value))
}

/// Fetch one URL as Markdown through the DeepSeek Fetch API.
pub async fn fetch_page(api_base: &str, api_key: &str, page_url: &str) -> Result<FetchedPage> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("DeepSeek Fetch requires DEEPSEEK_API_KEY"));
    }
    let url = format!("{}/v1/tools/fetch", origin(api_base));
    let response = crate::common::cmd_timeout::model_client_for(&origin(api_base))
        .post(&url)
        .bearer_auth(api_key)
        .json(&serde_json::json!({"url": page_url}))
        .send()
        .await
        .with_context(|| format!("DeepSeek Fetch request failed: {url}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(error_message(status, &text)));
    }
    let value: Value =
        serde_json::from_str(&text).with_context(|| "DeepSeek Fetch response is not JSON")?;
    Ok(FetchedPage {
        title: value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        content: value
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

/// Recognize a PDF, Office document or image page by page.
pub async fn ocr(
    api_base: &str,
    api_key: &str,
    filename: &str,
    data: &[u8],
    rich_layout: bool,
) -> Result<String> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("DeepSeek OCR requires DEEPSEEK_API_KEY"));
    }
    let url = format!("{}/v1/tools/ocr", origin(api_base));
    let purpose = if rich_layout {
        "rich_layout_recognition"
    } else {
        "text_recognition"
    };
    // The purpose field must precede the file field for DeepSeek documents.
    let form = reqwest::multipart::Form::new()
        .text("purpose", purpose)
        .part(
            "file",
            reqwest::multipart::Part::bytes(data.to_vec()).file_name(filename.to_string()),
        );
    let response = crate::common::cmd_timeout::model_client()
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .with_context(|| format!("DeepSeek OCR request failed: {url}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(error_message(status, &text)));
    }
    let value: Value =
        serde_json::from_str(text.trim()).with_context(|| "DeepSeek OCR response is not JSON")?;
    parse_ocr_pages(&value)
}

/// Transcribe a WAV or Ogg Opus file.
pub async fn asr(api_base: &str, api_key: &str, filename: &str, audio: &[u8]) -> Result<String> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("DeepSeek ASR requires DEEPSEEK_API_KEY"));
    }
    let url = format!("{}/v1/tools/asr", origin(api_base));
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(audio.to_vec()).file_name(filename.to_string()),
    );
    let response = crate::common::cmd_timeout::model_client()
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .with_context(|| format!("DeepSeek ASR request failed: {url}"))?;
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(error_message(status, &text)));
    }
    let value: Value =
        serde_json::from_str(&text).with_context(|| "DeepSeek ASR response is not JSON")?;
    value
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("DeepSeek ASR response has no text field"))
}

/// Synthesize WAV audio. OpenAI voice names other than tide map to stella.
pub async fn tts(api_base: &str, api_key: &str, text: &str, voice: &str) -> Result<Vec<u8>> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("DeepSeek TTS requires DEEPSEEK_API_KEY"));
    }
    let url = format!("{}/v1/tools/tts", origin(api_base));
    let body = serde_json::json!({
        "text": text,
        "voice_id": deepseek_voice(voice),
        "format": "wav",
        "filter_markdown": true,
    });
    let response = crate::common::cmd_timeout::model_client_for(&origin(api_base))
        .post(&url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("DeepSeek TTS request failed: {url}"))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(error_message(status, &body)));
    }
    Ok(response.bytes().await?.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_normalizes_openai_compatible_base() {
        assert_eq!(
            origin("https://api.deepseek.com/v1"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            origin("https://api.deepseek.com/v1/"),
            "https://api.deepseek.com"
        );
        assert_eq!(
            origin("https://api.deepseek.com"),
            "https://api.deepseek.com"
        );
    }

    #[test]
    fn search_results_map_scores_and_content() {
        let hits = parse_search_results(&serde_json::json!({
            "results": [{
                "title": "T",
                "url": "https://example.com/a",
                "snippet": "S",
                "summary": "Long",
                "content": "Body",
                "rerank_score": 2.4
            }]
        }));
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/a");
        assert!((hits[0].score - 0.8).abs() < 1e-9);
    }

    #[test]
    fn ocr_pages_join_and_errors_are_explicit() {
        let text = parse_ocr_pages(&serde_json::json!({
            "pages": [{"text": "one"}, {"text": "two"}]
        }))
        .unwrap();
        assert_eq!(text, "one\n\ntwo");
        assert!(
            parse_ocr_pages(&serde_json::json!({
                "error": {"message": "blocked"}
            }))
            .is_err()
        );
    }

    #[test]
    fn voice_names_never_forward_openai_defaults() {
        assert_eq!(deepseek_voice("alloy"), "stella");
        assert_eq!(deepseek_voice("tide"), "tide");
        assert_eq!(deepseek_voice("TIDE"), "tide");
    }
}
