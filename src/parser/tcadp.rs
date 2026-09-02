//! TCADP 解析器 — RAGFlow `deepdoc/parser/tcadp_parser.py` 的 Rust 实现
//!
//! 腾讯云 LKEAP（v20240522）ReconstructDocument 文档重构 API：
//!   1. TC3-HMAC-SHA256 签名 POST（SSE 流式响应）
//!   2. 进度 100% 时取 DocumentRecognizeResultUrl 下载 zip
//!   3. 安全解压（拒绝对称加密/符号链接/绝对路径/.. 穿越）→ 收集 .json/.md
//!   4. 按 item.type 渲染 sections（text/table/...）
//! 凭据：节点配置或 env TENCENT_SECRET_ID / TENCENT_SECRET_KEY / TENCENT_REGION。

use crate::parser::{Document, new_document};
use anyhow::{Result, bail};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

const TCADP_HOST: &str = "lkeap.tencentcloudapi.com";
const TCADP_VERSION: &str = "2024-05-22";
const TCADP_ACTION: &str = "ReconstructDocument";
const TCADP_SERVICE: &str = "lkeap";
const TCADP_REGION_DEFAULT: &str = "ap-guangzhou";

/// 扩展名 → FileType（对齐腾讯云 ReconstructDocument 取值）。
fn file_type_from_name(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "pdf" => "PDF",
        "png" => "PNG",
        "jpg" | "jpeg" => "JPG",
        "tif" | "tiff" => "TIFF",
        "doc" | "docx" => "WORD",
        "ppt" | "pptx" => "PPT",
        "xls" | "xlsx" => "EXCEL",
        _ => "PDF",
    }
}

/// TCADP 配置。
#[derive(Debug, Clone, Default)]
pub struct TcadpConfig {
    pub secret_id: String,
    pub secret_key: String,
    pub region: String,
}

impl TcadpConfig {
    /// 从节点配置 / env 解析（对齐上游 get_base_config + 参数）。
    pub fn from_params(
        secret_id: Option<&str>,
        secret_key: Option<&str>,
        region: Option<&str>,
    ) -> Result<Self> {
        let read = |param: Option<&str>, env: &str| -> String {
            param
                .map(str::to_string)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| std::env::var(env).unwrap_or_default())
        };
        let config = Self {
            secret_id: read(secret_id, "TENCENT_SECRET_ID"),
            secret_key: read(secret_key, "TENCENT_SECRET_KEY"),
            region: region
                .map(str::to_string)
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    std::env::var("TENCENT_REGION")
                        .unwrap_or_else(|_| TCADP_REGION_DEFAULT.to_string())
                }),
        };
        if config.secret_id.is_empty() || config.secret_key.is_empty() {
            bail!("TCADP requires TENCENT_SECRET_ID and TENCENT_SECRET_KEY");
        }
        Ok(config)
    }
}

/// TCADP 解析器。
#[derive(Default)]
pub struct TcadpParser;

/// 同步降级：无凭据/离线时按原始文本处理（云端完整能力走 parse_with_config）。
impl crate::parser::Parse for TcadpParser {
    fn parse(&self, name: &str, data: &[u8]) -> Result<Document> {
        let content = String::from_utf8_lossy(data).to_string();
        Ok(new_document(
            name,
            content,
            "application/octet-stream",
            data.len(),
        ))
    }
}

impl TcadpParser {
    pub fn new() -> Self {
        Self
    }

    /// 解析文档（对齐上游 TCADPParser.__call__ 全流程）。
    pub async fn parse_with_config(
        &self,
        name: &str,
        data: &[u8],
        config: &TcadpConfig,
    ) -> Result<Document> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30 * 60))
            .build()?;
        let file_type = file_type_from_name(name);
        let file_base64 = base64_encode(data);
        let payload = serde_json::json!({
            "FileType": file_type,
            "FileBase64": file_base64,
            "FileStartPageNumber": 1,
            "FileEndPageNumber": 1000,
        });

        let url = format!("https://{TCADP_HOST}");
        let response = client
            .post(&url)
            .headers(sign_headers(
                &payload,
                &config.region,
                &config.secret_id,
                &config.secret_key,
            ))
            .json(&payload)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("TCADP request failed: {error}"))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("TCADP HTTP {status}: {body}");
        }
        let body = response.text().await?;
        let download_url = extract_download_url_from_sse(&body)?;

        // 下载结果 zip
        let zip_bytes = client
            .get(&download_url)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("TCADP result download failed: {error}"))?
            .bytes()
            .await?;

        let content = extract_content_from_zip(&zip_bytes)?;
        let sections = parse_content_to_sections(&content);
        let text = sections.join("\n");
        Ok(new_document(
            name,
            text,
            "application/octet-stream",
            data.len(),
        ))
    }
}

/// TC3-HMAC-SHA256 请求头（对齐腾讯云 API v3 签名）。
fn sign_headers(
    payload: &serde_json::Value,
    region: &str,
    secret_id: &str,
    secret_key: &str,
) -> reqwest::header::HeaderMap {
    use reqwest::header::{CONTENT_TYPE, HOST, HeaderMap, HeaderValue};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let date = {
        // UTC YYYY-MM-DD
        let days = timestamp / 86400;
        let (year, month, day) = civil_from_days(days as i64);
        format!("{year:04}-{month:02}-{day:02}")
    };

    let payload_bytes = payload.to_string();
    let hashed_payload = hex::encode(Sha256::digest(payload_bytes.as_bytes()));
    let canonical_headers = format!(
        "content-type:application/json; charset=utf-8\nhost:{TCADP_HOST}\nx-tc-action:reconstructdocument\n"
    );
    let signed_headers = "content-type;host;x-tc-action";
    let canonical_request =
        format!("POST\n/\n\n{canonical_headers}\n{signed_headers}\n{hashed_payload}");
    let credential_scope = format!("{date}/{TCADP_SERVICE}/tc3_request");
    let string_to_sign = format!(
        "TC3-HMAC-SHA256\n{timestamp}\n{credential_scope}\n{}",
        hex::encode(Sha256::digest(canonical_request.as_bytes()))
    );

    let secret_date = hmac_sha256(format!("TC3{secret_key}").as_bytes(), date.as_bytes());
    let secret_service = hmac_sha256(&secret_date, TCADP_SERVICE.as_bytes());
    let secret_signing = hmac_sha256(&secret_service, b"tc3_request");
    let signature = hex::encode(hmac_sha256(&secret_signing, string_to_sign.as_bytes()));

    let authorization = format!(
        "TC3-HMAC-SHA256 Credential={secret_id}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}"
    );

    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    headers.insert(HOST, HeaderValue::from_static(TCADP_HOST));
    headers.insert(
        "X-TC-Action",
        HeaderValue::from_static("ReconstructDocument"),
    );
    headers.insert("X-TC-Version", HeaderValue::from_static(TCADP_VERSION));
    headers.insert(
        "X-TC-Timestamp",
        HeaderValue::from_str(&timestamp.to_string()).unwrap(),
    );
    headers.insert("X-TC-Region", HeaderValue::from_str(region).unwrap());
    headers.insert(
        "Authorization",
        HeaderValue::from_str(&authorization).unwrap(),
    );
    headers
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("hmac key");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// 天数 → 公历日期（Unix epoch 起点）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

/// 从 SSE 响应提取 DocumentRecognizeResultUrl（对齐上游事件循环）。
fn extract_download_url_from_sse(body: &str) -> Result<String> {
    let mut task_progress: Option<String> = None;
    let mut download_url: Option<String> = None;
    for line in body.lines() {
        let line = line.trim();
        if !line.starts_with("data:") {
            continue;
        }
        let data = line[5..].trim();
        if data.is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
            if let Some(progress) = value.get("Progress").and_then(serde_json::Value::as_str) {
                task_progress = Some(progress.to_string());
            }
            if let Some(url) = value
                .get("DocumentRecognizeResultUrl")
                .and_then(serde_json::Value::as_str)
            {
                download_url = Some(url.to_string());
            }
            // 上游仅记录 Message；失败页信息单独在 FailedPages 中（此处忽略）
        }
    }
    match (task_progress, download_url) {
        (Some(progress), Some(url)) if progress == "100" => Ok(url),
        (Some(progress), _) => bail!("TCADP task not finished (progress {progress}%)"),
        (None, _) => bail!("TCADP no SSE progress event received"),
    }
}

/// 安全解压并收集 .json/.md 内容（对齐上游 _extract_content_from_zip 防御）。
fn extract_content_from_zip(zip_bytes: &[u8]) -> Result<Vec<serde_json::Value>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))?;
    let mut results = Vec::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index)?;
        let name = member.name().replace('\\', "/");
        if member.is_dir() {
            continue;
        }
        // 防御：加密 / 符号链接 / 绝对路径 / 穿越
        if member.encrypted() {
            bail!("[TCADP] Encrypted zip entry not supported: {name}");
        }
        if member
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            bail!("[TCADP] Symlink zip entry not supported: {name}");
        }
        if name.starts_with('/')
            || name.starts_with("//")
            || name.len() > 2 && name.as_bytes()[1] == b':'
        {
            bail!("[TCADP] Unsafe zip path (absolute): {name}");
        }
        let parts: Vec<&str> = name
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect();
        if parts.contains(&"..") {
            bail!("[TCADP] Unsafe zip path (traversal): {name}");
        }
        if !(name.ends_with(".json") || name.ends_with(".md")) {
            continue;
        }
        let mut buffer = Vec::new();
        member.read_to_end(&mut buffer)?;
        if name.ends_with(".json") {
            let value: serde_json::Value = serde_json::from_slice(&buffer).map_err(|error| {
                anyhow::anyhow!("[TCADP] JSON parse failed for {name}: {error}")
            })?;
            match value {
                serde_json::Value::Array(items) => results.extend(items),
                other => results.push(other),
            }
        } else {
            let content = String::from_utf8_lossy(&buffer).to_string();
            results.push(serde_json::json!({"type": "text", "content": content, "file": name}));
        }
    }
    if results.is_empty() {
        bail!("[TCADP] No parseable content in result zip");
    }
    Ok(results)
}

/// 渲染 sections（对齐上游 _parse_content_to_sections）。
fn parse_content_to_sections(content_data: &[serde_json::Value]) -> Vec<String> {
    let mut sections = Vec::new();
    for item in content_data {
        let content_type = item
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("text");
        let content = item
            .get("content")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        if content.trim().is_empty() {
            continue;
        }
        match content_type {
            "table" => {
                sections.push(format!("[表格]\n{content}"));
            }
            "image" => {
                sections.push(format!("[图片]\n{content}"));
            }
            _ => sections.push(content.to_string()),
        }
    }
    sections
}

fn base64_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_type_maps_extensions() {
        assert_eq!(file_type_from_name("a.pdf"), "PDF");
        assert_eq!(file_type_from_name("a.PNG"), "PNG");
        assert_eq!(file_type_from_name("a.docx"), "WORD");
        assert_eq!(file_type_from_name("a.xlsx"), "EXCEL");
        assert_eq!(file_type_from_name("noext"), "PDF");
    }

    #[test]
    fn civil_from_days_known_epoch() {
        // 1970-01-01 = day 0
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-08-04 ≈ day 20669
        assert_eq!(civil_from_days(20669), (2026, 8, 4));
    }

    #[test]
    fn sse_extracts_download_url_at_100_percent() {
        let body = r#"data: {"Progress": "10", "Message": "处理中"}

data: {"Progress": "100", "TaskId": "t1", "SuccessPageNum": 3, "FailPageNum": 0, "DocumentRecognizeResultUrl": "https://example.com/result.zip"}

"#;
        let url = extract_download_url_from_sse(body).unwrap();
        assert_eq!(url, "https://example.com/result.zip");
    }

    #[test]
    fn sse_rejects_unfinished_task() {
        let body = r#"data: {"Progress": "42"}"#;
        let error = extract_download_url_from_sse(body).unwrap_err().to_string();
        assert!(error.contains("not finished"));
    }

    #[test]
    fn zip_extraction_collects_json_and_md() {
        use std::io::Write;
        let mut zip_buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip_buffer);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("result.json", options).unwrap();
            writer
                .write_all(
                    r#"[{"type":"text","content":"第一段"},{"type":"table","content":"|a|b|"}]"#
                        .as_bytes(),
                )
                .unwrap();
            writer.start_file("notes.md", options).unwrap();
            writer.write_all("# 标题".as_bytes()).unwrap();
            writer.finish().unwrap();
        }
        let data = zip_buffer.into_inner();
        let content = extract_content_from_zip(&data).unwrap();
        assert_eq!(content.len(), 3);
        let sections = parse_content_to_sections(&content);
        assert!(sections[0].contains("第一段"));
        assert!(sections[1].contains("[表格]"));
        assert!(sections[2].contains("# 标题"));
    }

    #[test]
    fn zip_rejects_traversal_paths() {
        use std::io::Write;
        let mut zip_buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip_buffer);
            writer
                .start_file("../evil.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            writer.write_all(b"{}").unwrap();
            writer.finish().unwrap();
        }
        let data = zip_buffer.into_inner();
        let error = extract_content_from_zip(&data).unwrap_err().to_string();
        assert!(error.contains("traversal"));
    }

    #[test]
    fn config_requires_secret_pair() {
        let error = TcadpConfig::from_params(None, None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("TENCENT_SECRET_ID"));
        let config =
            TcadpConfig::from_params(Some("id"), Some("key"), Some("ap-shanghai")).unwrap();
        assert_eq!(config.region, "ap-shanghai");
        assert_eq!(config.secret_id, "id");
    }

    #[test]
    fn sign_headers_include_tc3_authorization() {
        let headers = sign_headers(
            &json!({"FileType": "PDF"}),
            "ap-guangzhou",
            "test-secret-id",
            "test-secret-key",
        );
        let auth = headers.get("Authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("TC3-HMAC-SHA256 Credential=test-secret-id/"));
        assert!(auth.contains("SignedHeaders=content-type;host;x-tc-action"));
        assert!(auth.contains("Signature="));
        assert_eq!(
            headers.get("X-TC-Version").unwrap().to_str().unwrap(),
            "2024-05-22"
        );
    }
}
