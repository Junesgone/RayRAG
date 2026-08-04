//! CodeExec 连接器 — RAGFlow `code_exec.py` 的 Rust 实现
//!
//! 上游语义：把代码发送到独立沙箱服务（HTTP POST :9385/run），执行后按契约处理结果：
//!   1. 类型推断（Null/Boolean/Number/String/Object/Array<T>）
//!   2. expected_type 递归校验（含 Array<T> 展开）
//!   3. 业务输出选择（保留键排除后必须恰好一个）
//!   4. canonical content 渲染（String 原样，Object/Array → 排序缩进 JSON）
//!   5. artifact 附件段落（image/pdf/csv/json/html 类型规范化）
//! 沙箱服务地址由 RAYRAG_SANDBOX_HOST 配置（默认 127.0.0.1:9385，可指向 Docker 沙箱）。

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;

/// 系统保留输出键（对齐上游 SYSTEM_OUTPUT_KEYS）。
const SYSTEM_OUTPUT_KEYS: &[&str] = &[
    "content",
    "actual_type",
    "attachments",
    "_ERROR",
    "_ARTIFACTS",
    "_ATTACHMENT_CONTENT",
    "raw_result",
    "_created_time",
    "_elapsed_time",
];

/// 沙箱执行结果（对齐 :9385/run 响应体）。
#[derive(Debug, Clone, Default)]
pub struct SandboxResult {
    pub stdout: String,
    pub stderr: Option<String>,
    pub artifacts: Vec<Value>,
    /// 结构化结果元数据（result_present / result_value）。
    pub metadata: Map<String, Value>,
}

/// 代码执行请求。
#[derive(Debug, Clone)]
pub struct CodeRequest {
    pub language: String,
    pub code: String,
    pub arguments: Map<String, Value>,
}

/// 沙箱客户端。
#[derive(Debug, Clone)]
pub struct SandboxClient {
    base_url: String,
    http: reqwest::Client,
}

impl Default for SandboxClient {
    fn default() -> Self {
        let host = std::env::var("RAYRAG_SANDBOX_HOST")
            .unwrap_or_else(|_| "http://127.0.0.1:9385".to_string());
        Self::new(&host)
    }
}

impl SandboxClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(10 * 60))
                .build()
                .expect("sandbox http client"),
        }
    }

    /// 调用沙箱执行代码（对齐上游 POST :9385/run）。
    pub async fn run(&self, request: &CodeRequest) -> Result<SandboxResult> {
        let payload = json!({
            "code": request.code,
            "lang": request.language,
            "arguments": request.arguments,
        });
        let response = self
            .http
            .post(format!("{}/run", self.base_url))
            .json(&payload)
            .send()
            .await
            .map_err(|error| anyhow::anyhow!("Exception executing code: {error}"))?;
        if !response.status().is_success() {
            bail!("Sandbox HTTP {}", response.status());
        }
        let body: Value = response.json().await?;
        Ok(SandboxResult {
            stdout: body.get("stdout").and_then(Value::as_str).unwrap_or("").to_string(),
            stderr: body.get("stderr").and_then(Value::as_str).map(str::to_string),
            artifacts: body
                .get("artifacts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            metadata: Map::new(),
        })
    }
}

// ─────────────────────────── 契约层（对齐上游纯函数） ───────────────────────────

/// 推断值的实际类型（对齐 infer_actual_type）。
pub fn infer_actual_type(value: &Value) -> String {
    match value {
        Value::Null => "Null".to_string(),
        Value::Bool(_) => "Boolean".to_string(),
        Value::Number(_) => "Number".to_string(),
        Value::String(_) => "String".to_string(),
        Value::Object(_) => "Object".to_string(),
        Value::Array(items) => {
            if items.is_empty() {
                return "Array<Any>".to_string();
            }
            let mut inferred = std::collections::BTreeSet::new();
            for item in items {
                inferred.insert(infer_actual_type(item));
            }
            if inferred.len() == 1 {
                format!("Array<{}>", inferred.iter().next().unwrap())
            } else {
                "Array<Any>".to_string()
            }
        }
    }
}

/// 规范化期望类型（对齐 _normalize_expected_type：递归 Array<T>）。
pub fn normalize_expected_type(expected: &str) -> String {
    let low = expected.trim().to_lowercase();
    let simple = match low.as_str() {
        "string" => "String",
        "number" => "Number",
        "boolean" => "Boolean",
        "object" => "Object",
        "null" => "Null",
        "any" => "Any",
        _ => "",
    };
    if !simple.is_empty() {
        return simple.to_string();
    }
    if low.starts_with("array<") && low.ends_with('>') {
        let start = expected.find('<').unwrap() + 1;
        let end = expected.rfind('>').unwrap();
        let inner = expected[start..end].trim();
        if inner.is_empty() {
            return expected.trim().to_string();
        }
        return format!("Array<{}>", normalize_expected_type(inner));
    }
    expected.trim().to_string()
}

/// 递归校验值是否匹配期望类型（对齐 _validate_expected_type）。
pub fn validate_expected_type(expected: &str, value: &Value) -> Result<()> {
    let etype = normalize_expected_type(expected);
    if etype.is_empty() || etype.eq_ignore_ascii_case("any") {
        return Ok(());
    }
    if etype.starts_with("Array<") && etype.ends_with('>') {
        let inner = &etype[6..etype.len() - 1];
        let items = value.as_array().ok_or_else(|| {
            anyhow::anyhow!(
                "CodeExec contract mismatch: expected type {etype}, got {}",
                infer_actual_type(value)
            )
        })?;
        for item in items {
            validate_expected_type(inner, item)?;
        }
        return Ok(());
    }
    let actual = infer_actual_type(value);
    if actual != etype {
        bail!(
            "CodeExec contract mismatch: expected type {etype}, got {actual}"
        );
    }
    Ok(())
}

/// 校验业务输出名（对齐 _validate_business_output_name）。
fn validate_business_output_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        bail!("CodeExec business output name must not be empty");
    }
    if SYSTEM_OUTPUT_KEYS.contains(&name) {
        bail!("CodeExec reserved output name is not allowed: {name}");
    }
    if name.contains('.') {
        bail!("CodeExec business output name must not contain '.': {name}");
    }
    Ok(())
}

/// 从输出映射中选择业务输出（对齐 select_business_output）。
pub fn select_business_output(outputs: &Map<String, Value>) -> Result<(String, Value)> {
    if outputs.len() == 1 {
        let (name, value) = outputs.iter().next().unwrap();
        validate_business_output_name(name)?;
        return Ok((name.clone(), value.clone()));
    }
    let business: Vec<(&String, &Value)> = outputs
        .iter()
        .filter(|(name, _)| !SYSTEM_OUTPUT_KEYS.contains(&name.as_str()))
        .collect();
    if business.len() != 1 {
        bail!(
            "CodeExec contract must contain exactly one business output, got {}",
            business.len()
        );
    }
    validate_business_output_name(business[0].0)?;
    Ok((business[0].0.clone(), business[0].1.clone()))
}

/// canonical content 渲染（对齐 render_canonical_content）。
pub fn render_canonical_content(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Object(_) | Value::Array(_) => {
            // 排序键 + 2 空格缩进（对齐 json.dumps(sort_keys=True, indent=2)）
            render_sorted_json(value)
        }
        other => other.to_string(),
    }
}

/// 排序键 JSON 渲染（Object 按键排序，Array 保持顺序）。
fn render_sorted_json(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let sorted: BTreeMap<String, Value> = map
                    .iter()
                    .map(|(key, val)| (key.clone(), sorted(val)))
                    .collect();
                Value::Object(Map::from_iter(sorted.into_iter()))
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string_pretty(&sorted(value)).unwrap_or_else(|_| value.to_string())
}

/// 附件类型规范化（对齐 _normalize_attachment_type）。
pub fn normalize_attachment_type(name: &str, mime_type: &str) -> String {
    let mime = mime_type.trim().to_lowercase();
    if mime.starts_with("image/") {
        return "image".to_string();
    }
    match mime.as_str() {
        "application/pdf" => return "pdf".to_string(),
        "text/csv" => return "csv".to_string(),
        "application/json" => return "json".to_string(),
        "text/html" => return "html".to_string(),
        _ => {}
    }
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    if ext.is_empty() || ext == name {
        "file".to_string()
    } else {
        ext
    }
}

/// 附件段落渲染（对齐 _format_attachment_section）。
pub fn format_attachment_section(key: &str, attachment_type: &str, name: &str, parsed: &str) -> String {
    let title = if name.is_empty() {
        format!("{key} ({attachment_type})")
    } else {
        format!("{key} ({attachment_type}): {name}")
    };
    format!("{title}\n{parsed}").trim().to_string()
}

// ─────────────────────────── 顶层结果处理 ───────────────────────────

/// 处理沙箱执行结果（对齐 _process_execution_result 的核心分支）。
pub fn process_result(result: &SandboxResult) -> Map<String, Value> {
    let mut outputs = Map::new();

    // 结构化结果优先；否则 stdout 反序列化（对齐 _resolve_execution_result_value）
    let resolved = if result.metadata.get("result_present").and_then(Value::as_bool) == Some(true) {
        result
            .metadata
            .get("result_value")
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        serde_json::from_str::<Value>(&result.stdout).unwrap_or_else(|_| Value::String(result.stdout.clone()))
    };

    // stderr-only 且无结果/工件 → 错误（对齐上游分支）
    let has_artifacts = !result.artifacts.is_empty();
    let stdout_blank = result.stdout.trim().is_empty();
    if let Some(stderr) = &result.stderr {
        if !stderr.trim().is_empty() && !has_artifacts && stdout_blank {
            outputs.insert("_ERROR".into(), Value::String(stderr.clone()));
            outputs.insert("content".into(), Value::String(String::new()));
            return outputs;
        }
    }

    // 业务输出选择 + canonical 渲染
    let base_content = match &resolved {
        Value::Object(map) => match select_business_output(map) {
            Ok((_name, value)) => render_canonical_content(&value),
            Err(error) => {
                outputs.insert("_ERROR".into(), Value::String(error.to_string()));
                return outputs;
            }
        },
        _ => render_canonical_content(&resolved),
    };

    let mut content_parts = Vec::new();
    if !base_content.is_empty() {
        content_parts.push(base_content);
    }

    // artifact 附件段落（对齐 _format_attachment_section 循环）
    if has_artifacts {
        let mut sections = Vec::new();
        for (index, artifact) in result.artifacts.iter().enumerate() {
            let name = artifact
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let content_b64 = artifact
                .get("content_b64")
                .and_then(Value::as_str)
                .unwrap_or("");
            let mime_type = artifact
                .get("mime_type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if name.is_empty() || content_b64.is_empty() {
                continue;
            }
            let attachment_type = normalize_attachment_type(&name, &mime_type);
            let parsed = match base64_decode(content_b64) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                Err(_) => "Artifact generated but parse failed.".to_string(),
            };
            sections.push(format_attachment_section(
                &format!("attachment_{}", index + 1),
                &attachment_type,
                &name,
                &parsed,
            ));
        }
        if sections.is_empty() {
            content_parts.push("attachment_count: 0".to_string());
        } else {
            content_parts.push(format!(
                "attachment_count: {}\n\n{}",
                sections.len(),
                sections.join("\n\n")
            ));
        }
    }

    let content = content_parts.join("\n\n");
    outputs.insert("content".into(), Value::String(content));
    outputs.insert("actual_type".into(), Value::String(infer_actual_type(&resolved)));
    outputs
}

/// Base64 解码（宽松：忽略空白）。
fn base64_decode(input: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(input.trim())
        .map_err(|error| anyhow::anyhow!("base64 decode: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_types_cover_all_variants() {
        assert_eq!(infer_actual_type(&Value::Null), "Null");
        assert_eq!(infer_actual_type(&json!(true)), "Boolean");
        assert_eq!(infer_actual_type(&json!(42)), "Number");
        assert_eq!(infer_actual_type(&json!("text")), "String");
        assert_eq!(infer_actual_type(&json!({"a": 1})), "Object");
        assert_eq!(infer_actual_type(&json!([])), "Array<Any>");
        assert_eq!(infer_actual_type(&json!([1, 2])), "Array<Number>");
        assert_eq!(infer_actual_type(&json!([1, "a"])), "Array<Any>");
    }

    #[test]
    fn normalize_expected_type_handles_nested_arrays() {
        assert_eq!(normalize_expected_type("string"), "String");
        assert_eq!(normalize_expected_type(" array< number > "), "Array<Number>");
        assert_eq!(normalize_expected_type("Array<Array<String>>"), "Array<Array<String>>");
        assert_eq!(normalize_expected_type("any"), "Any");
    }

    #[test]
    fn validate_expected_type_accepts_and_rejects() {
        assert!(validate_expected_type("Array<Number>", &json!([1, 2, 3])).is_ok());
        assert!(validate_expected_type("Array<String>", &json!([1, 2])).is_err());
        assert!(validate_expected_type("String", &json!("x")).is_ok());
        assert!(validate_expected_type("Number", &json!("x")).is_err());
    }

    #[test]
    fn select_business_output_requires_exactly_one() {
        let map = Map::from_iter([
            ("content".into(), json!("sys")),
            ("answer".into(), json!("business")),
        ]);
        let (name, value) = select_business_output(&map).unwrap();
        assert_eq!(name, "answer");
        assert_eq!(value, json!("business"));

        // 两个业务输出 → 错误
        let bad = Map::from_iter([
            ("a".into(), json!(1)),
            ("b".into(), json!(2)),
        ]);
        assert!(select_business_output(&bad).is_err());

        // 保留键名 → 错误
        let reserved = Map::from_iter([("content".into(), json!(1))]);
        assert!(select_business_output(&reserved).is_err());
    }

    #[test]
    fn canonical_content_sorts_object_keys() {
        let value = json!({"b": 2, "a": {"d": 1, "c": 3}});
        let rendered = render_canonical_content(&value);
        assert!(rendered.contains("\"a\"") && rendered.contains("\"b\""));
        let a_pos = rendered.find("\"a\"").unwrap();
        let b_pos = rendered.find("\"b\"").unwrap();
        assert!(a_pos < b_pos, "keys must be sorted: {rendered}");
    }

    #[test]
    fn attachment_type_normalization() {
        assert_eq!(normalize_attachment_type("x.png", "image/png"), "image");
        assert_eq!(normalize_attachment_type("r.pdf", "application/pdf"), "pdf");
        assert_eq!(normalize_attachment_type("d.csv", "text/csv"), "csv");
        assert_eq!(normalize_attachment_type("f.json", ""), "json");
        assert_eq!(normalize_attachment_type("noext", ""), "file");
    }

    #[test]
    fn stderr_only_result_becomes_error() {
        let result = SandboxResult {
            stdout: String::new(),
            stderr: Some("Traceback: boom".into()),
            artifacts: vec![],
            metadata: Map::new(),
        };
        let outputs = process_result(&result);
        assert!(outputs.get("_ERROR").unwrap().as_str().unwrap().contains("boom"));
    }

    #[test]
    fn business_output_and_artifacts_render() {
        let result = SandboxResult {
            stdout: r#"{"answer": {"price": 1.5}}"#.into(),
            stderr: None,
            artifacts: vec![json!({
                "name": "chart.csv",
                "content_b64": "YTJiYwo=", // "a2b\n"
                "mime_type": "text/csv",
            })],
            metadata: Map::new(),
        };
        let outputs = process_result(&result);
        let content = outputs.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("\"price\": 1.5"));
        assert!(content.contains("attachment_count: 1"));
        assert!(content.contains("attachment_1 (csv): chart.csv"));
        assert_eq!(outputs.get("actual_type").unwrap().as_str().unwrap(), "Object");
    }
}
