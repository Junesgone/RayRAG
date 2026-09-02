//! 百度翻译连接器 — DeepL 的中国大陆替代
//!
//! RAGFlow 的 `deepl` 工具依赖外部服务且需要 auth_key，在中国大陆
//! 无法直连。本模块提供两个国内直连的提供方：
//!
//! 1. **开放平台 API**（`fanyi-api.baidu.com`）：需要
//!    `BAIDU_TRANSLATE_APPID` + `BAIDU_TRANSLATE_KEY`，免费额度，
//!    段落级翻译，稳定可靠（DeepL 的正式替代）。
//! 2. **免 key 免费端点**（`fanyi.baidu.com/sug`）：无需任何凭据，
//!    国内直连，适合词/短语级翻译；未配置 APPID/KEY 时自动降级。
//!
//! 语言代码兼容 RAGFlow DeepL 风格（ZH/EN-GB/JA…），内部映射为
//! 百度翻译代码（zh/en/jp…）。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use md5::{Digest, Md5};
use reqwest::Client;
use serde_json::Value;

/// 百度翻译开放平台端点（正式 API，需签名）。
const VIP_ENDPOINT: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";
/// 百度翻译免 key 免费端点（仅词/短语级）。
const SUG_ENDPOINT: &str = "https://fanyi.baidu.com/sug";
/// 开放平台免费版默认 QPS/字符限制（单次最大 6000 字节）。
const MAX_TEXT_BYTES: usize = 6000;

/// DeepL 风格语言代码 → 百度翻译语言代码。
fn map_lang(code: &str) -> &'static str {
    let normalized = code.to_ascii_uppercase();
    let base = normalized.split('-').next().unwrap_or(&normalized);
    match base {
        "ZH" => "zh",
        "EN" => "en",
        "JA" => "jp",
        "KO" => "kor",
        "FR" => "fra",
        "DE" => "de",
        "ES" => "spa",
        "RU" => "ru",
        "PT" => "pt",
        "IT" => "it",
        "NL" => "nl",
        "PL" => "pl",
        "AR" => "ara",
        "TR" => "tr",
        "VI" => "vie",
        "TH" => "th",
        "ID" => "id",
        "MS" => "ms",
        _ => "auto",
    }
}

/// 翻译请求。
#[derive(Debug, Clone)]
pub struct TranslateRequest {
    /// 待翻译文本。
    pub text: String,
    /// DeepL 风格源语言（ZH/EN/JA…；auto 表示自动检测）。
    pub source_lang: String,
    /// DeepL 风格目标语言（EN-GB/EN-US/ZH…）。
    pub target_lang: String,
}

/// 百度翻译客户端。
#[derive(Debug, Clone)]
pub struct BaiduTranslateClient {
    client: Client,
    appid: Option<String>,
    secret_key: Option<String>,
}

impl Default for BaiduTranslateClient {
    fn default() -> Self {
        Self::from_env()
    }
}

impl BaiduTranslateClient {
    /// 从环境变量构造客户端（`BAIDU_TRANSLATE_APPID` / `BAIDU_TRANSLATE_KEY`）。
    pub fn from_env() -> Self {
        Self {
            client: Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .expect("build Baidu translate HTTP client"),
            appid: std::env::var("BAIDU_TRANSLATE_APPID").ok(),
            secret_key: std::env::var("BAIDU_TRANSLATE_KEY").ok(),
        }
    }

    /// 是否配置了开放平台凭据。
    pub fn has_credentials(&self) -> bool {
        self.appid.is_some() && self.secret_key.is_some()
    }

    /// 翻译文本：有凭据走开放平台 API，否则降级到免 key 免费端点。
    pub async fn translate(&self, request: &TranslateRequest) -> Result<String> {
        if request.text.trim().is_empty() {
            return Ok(String::new());
        }
        if self.has_credentials() {
            self.translate_vip(request).await
        } else {
            self.translate_sug(request).await
        }
    }

    /// 开放平台 API（MD5 签名，段落级）。
    async fn translate_vip(&self, request: &TranslateRequest) -> Result<String> {
        self.translate_vip_at(request, VIP_ENDPOINT).await
    }

    /// 开放平台 API 的可注入端点变体（供单元测试 mock HTTP）。
    async fn translate_vip_at(&self, request: &TranslateRequest, endpoint: &str) -> Result<String> {
        let appid = self.appid.as_deref().expect("appid present");
        let secret_key = self.secret_key.as_deref().expect("secret key present");
        if request.text.len() > MAX_TEXT_BYTES {
            bail!("BaiduTranslate text exceeds {MAX_TEXT_BYTES} bytes");
        }
        let query = request.text.clone();
        let salt = format!(
            "{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .context("clock before epoch")?
                .as_millis()
        );
        let sign = format!("{appid}{query}{salt}{secret_key}");
        let mut hasher = Md5::new();
        hasher.update(sign.as_bytes());
        let sign = hex::encode(hasher.finalize());

        let from = map_lang(&request.source_lang);
        let to = map_lang(&request.target_lang);
        let response = self
            .client
            .post(endpoint)
            .form(&[
                ("q", query.as_str()),
                ("from", from),
                ("to", to),
                ("appid", appid),
                ("salt", salt.as_str()),
                ("sign", sign.as_str()),
            ])
            .send()
            .await
            .context("request Baidu translate VIP API")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("read Baidu translate VIP response")?;
        if !status.is_success() {
            bail!(
                "BaiduTranslate VIP API HTTP {status}: {}",
                truncate(&body, 300)
            );
        }
        let json: Value = serde_json::from_str(&body).context("parse Baidu translate VIP JSON")?;
        if let Some(code) = json.get("error_code").and_then(Value::as_str)
            && code != "0" {
                let msg = json
                    .get("error_msg")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                bail!("BaiduTranslate VIP error {code}: {msg}");
            }
        let results = json
            .get("trans_result")
            .and_then(Value::as_array)
            .context("BaiduTranslate VIP missing trans_result")?;
        let mut parts = Vec::new();
        for item in results {
            if let Some(dst) = item.get("dst").and_then(Value::as_str) {
                parts.push(dst.to_string());
            }
        }
        Ok(parts.join("\n"))
    }

    /// 免 key 免费端点（词/短语级；超长时截断）。
    async fn translate_sug(&self, request: &TranslateRequest) -> Result<String> {
        self.translate_sug_at(request, SUG_ENDPOINT).await
    }

    /// 免 key 免费端点的可注入端点变体（供单元测试 mock HTTP）。
    async fn translate_sug_at(&self, request: &TranslateRequest, endpoint: &str) -> Result<String> {
        let mut text = request.text.clone();
        if text.len() > MAX_TEXT_BYTES {
            text.truncate(MAX_TEXT_BYTES);
        }
        let response = self
            .client
            .post(endpoint)
            .form(&[("kw", text.as_str())])
            .header("User-Agent", "Mozilla/5.0 (compatible; RayRAG/1.0)")
            .send()
            .await
            .context("request Baidu translate sug endpoint")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("read Baidu translate sug response")?;
        if !status.is_success() {
            bail!("BaiduTranslate sug HTTP {status}: {}", truncate(&body, 300));
        }
        let json: Value = serde_json::from_str(&body).context("parse Baidu translate sug JSON")?;
        let results = json
            .get("data")
            .and_then(Value::as_array)
            .context("BaiduTranslate sug missing data")?;
        let mut parts = Vec::new();
        for item in results {
            if let Some(value) = item.get("v").and_then(Value::as_str) {
                parts.push(value.to_string());
            }
        }
        if parts.is_empty() {
            bail!("BaiduTranslate sug returned no translations");
        }
        Ok(parts.join("\n"))
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let mut out = String::new();
    for _ in 0..max {
        match chars.next() {
            Some(ch) => out.push(ch),
            None => break,
        }
    }
    out
}

/// 语言代码快速校验（DeepL 风格列表，与 RAGFlow 对齐）。
pub fn validate_lang_code(code: &str, target: bool) -> Result<()> {
    let normalized = code.to_ascii_uppercase();
    let base = normalized.split('-').next().unwrap_or(&normalized);
    let valid = [
        "AR", "BG", "CS", "DA", "DE", "EL", "EN", "ES", "ET", "FI", "FR", "HU", "ID", "IT", "JA",
        "KO", "LT", "LV", "NB", "NL", "PL", "PT", "RO", "RU", "SK", "SL", "SV", "TR", "UK", "ZH",
    ];
    if !valid.contains(&base) {
        bail!(
            "translate source language '{code}' must be one of: {}",
            valid.join(", ")
        );
    }
    if target {
        let targets = ["EN-GB", "EN-US", "PT-BR", "PT-PT"];
        let _ = targets;
    }
    Ok(())
}

/// 中文文档注释保留（供 rustdoc）。
#[allow(dead_code)]
fn _doc_note() -> HashMap<&'static str, &'static str> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_deepl_lang_codes_to_baidu() {
        assert_eq!(map_lang("ZH"), "zh");
        assert_eq!(map_lang("EN-GB"), "en");
        assert_eq!(map_lang("ja"), "jp");
        assert_eq!(map_lang("ko"), "kor");
        assert_eq!(map_lang("XX"), "auto");
    }

    #[test]
    fn validates_deepl_style_lang_codes() {
        assert!(validate_lang_code("ZH", false).is_ok());
        assert!(validate_lang_code("EN-GB", true).is_ok());
        assert!(validate_lang_code("XX", false).is_err());
    }

    #[tokio::test]
    async fn empty_text_returns_empty() {
        let client = BaiduTranslateClient::default();
        let request = TranslateRequest {
            text: "   ".to_string(),
            source_lang: "ZH".to_string(),
            target_lang: "EN".to_string(),
        };
        assert_eq!(client.translate(&request).await.unwrap(), "");
    }

    #[tokio::test]
    async fn sug_endpoint_parses_translations() {
        use axum::extract::Form;
        use axum::{Router, routing::post};
        let app = Router::new().route(
            "/sug",
            post(|Form(form): Form<HashMap<String, String>>| async move {
                assert_eq!(form.get("kw").map(String::as_str), Some("hello"));
                r#"{"errno":0,"data":[{"k":"hello","v":"你好"},{"k":"hello world","v":"你好，世界"}]}"#
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = BaiduTranslateClient {
            client: reqwest::Client::new(),
            appid: None,
            secret_key: None,
        };
        let request = TranslateRequest {
            text: "hello".to_string(),
            source_lang: "EN".to_string(),
            target_lang: "ZH".to_string(),
        };
        let translated = client
            .translate_sug_at(&request, &format!("http://{addr}/sug"))
            .await
            .unwrap();
        assert_eq!(translated, "你好\n你好，世界");
    }

    #[tokio::test]
    async fn vip_endpoint_parses_trans_result_and_signs_form() {
        use axum::extract::Form;
        use axum::{Router, routing::post};
        let app = Router::new().route(
            "/vip",
            post(|Form(form): Form<HashMap<String, String>>| async move {
                assert_eq!(form.get("appid").map(String::as_str), Some("test_appid"));
                assert_eq!(form.get("from").map(String::as_str), Some("zh"));
                assert_eq!(form.get("to").map(String::as_str), Some("en"));
                let sign = form.get("sign").map(String::as_str).unwrap_or_default();
                assert_eq!(sign.len(), 32);
                assert!(sign.chars().all(|ch| ch.is_ascii_hexdigit()));
                r#"{"from":"zh","to":"en","trans_result":[{"src":"你好","dst":"Hello"}]}"#
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = BaiduTranslateClient {
            client: reqwest::Client::new(),
            appid: Some("test_appid".to_string()),
            secret_key: Some("test_secret".to_string()),
        };
        let request = TranslateRequest {
            text: "你好".to_string(),
            source_lang: "ZH".to_string(),
            target_lang: "EN".to_string(),
        };
        let translated = client
            .translate_vip_at(&request, &format!("http://{addr}/vip"))
            .await
            .unwrap();
        assert_eq!(translated, "Hello");
    }

    #[tokio::test]
    async fn vip_endpoint_surfaces_api_error_code() {
        use axum::{Router, routing::post};
        let app = Router::new().route(
            "/vip",
            post(|| async { r#"{"error_code":"54003","error_msg":"Invalid Access"}"# }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = BaiduTranslateClient {
            client: reqwest::Client::new(),
            appid: Some("test_appid".to_string()),
            secret_key: Some("test_secret".to_string()),
        };
        let request = TranslateRequest {
            text: "你好".to_string(),
            source_lang: "ZH".to_string(),
            target_lang: "EN".to_string(),
        };
        let error = client
            .translate_vip_at(&request, &format!("http://{addr}/vip"))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("54003"), "{error}");
    }
}
