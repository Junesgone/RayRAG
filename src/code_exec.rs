//! CodeExec 连接器 — RAGFlow `code_exec.py` 的 Rust 实现
//!
//! 上游语义：把代码发送到独立沙箱服务（HTTP POST :9385/run），执行后按契约处理结果：
//!   1. 类型推断（Null/Boolean/Number/String/Object/Array<T>）
//!   2. expected_type 递归校验（含 Array<T> 展开）
//!   3. 业务输出选择（保留键排除后必须恰好一个）
//!   4. canonical content 渲染（String 原样，Object/Array → 排序缩进 JSON）
//!   5. artifact 附件段落（image/pdf/csv/json/html 类型规范化）
//! 沙箱服务地址由 RAYRAG_EXECUTOR_MANAGER_URL（优先）或 RAYRAG_SANDBOX_HOST 配置
//! （默认 http://127.0.0.1:9385，即 RAGFlow executor_manager 服务）；请求契约对齐
//! RAGFlow `agent/sandbox/providers/self_managed.py`：
//!   - POST /run 请求体 {code_b64, language, arguments}（code 需 base64 编码）
//!   - 响应体 CodeExecutionResult：status/stdout/stderr/exit_code/detail/
//!     time_used_ms/memory_used_kb/artifacts/result{present,value,type}
//!   - GET /healthz 健康检查；单次执行超时由 RAYRAG_SANDBOX_TIMEOUT 配置（默认 30s）。
//! /run 为同步接口（executor_manager 内部维护容器池），无需轮询/创建实例。

use anyhow::{Result, bail};
use base64::Engine;
use serde_json::{Map, Value, json};
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

/// 沙箱执行结果（对齐 executor_manager CodeExecutionResult 响应体）。
#[derive(Debug, Clone, Default)]
pub struct SandboxResult {
    pub stdout: String,
    pub stderr: Option<String>,
    /// 进程退出码（默认 0）。
    pub exit_code: i64,
    /// 执行状态（success/program_error/resource_limit_exceeded/...）。
    pub status: Option<String>,
    /// 附加错误详情。
    pub detail: Option<String>,
    /// 执行耗时毫秒。
    pub time_used_ms: Option<f64>,
    /// 内存占用 KB。
    pub memory_used_kb: Option<f64>,
    pub artifacts: Vec<Value>,
    /// 结构化结果元数据（result_present / result_value / result_type）。
    pub metadata: Map<String, Value>,
}

/// 代码执行请求。
#[derive(Debug, Clone)]
pub struct CodeRequest {
    pub language: String,
    pub code: String,
    pub arguments: Map<String, Value>,
}

/// 沙箱客户端（executor_manager 远程客户端）。
#[derive(Debug, Clone)]
pub struct SandboxClient {
    base_url: String,
    http: reqwest::Client,
    /// 单次 /run 请求超时（对齐 RAGFlow timeout 配置，默认 30s）。
    timeout: Duration,
}

impl Default for SandboxClient {
    fn default() -> Self {
        let setting = crate::api::runtime_config::system_settings_get("sandbox.self_managed")
            .and_then(|value| value.as_object().cloned());
        // Existing RayRAG environment overrides stay authoritative; the
        // durable RAGFlow system setting is the production fallback.
        let url = std::env::var("RAYRAG_EXECUTOR_MANAGER_URL")
            .or_else(|_| std::env::var("RAYRAG_SANDBOX_HOST"))
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                setting.as_ref().and_then(|config| {
                    config
                        .get("endpoint")
                        .or_else(|| config.get("EXECUTOR_MANAGER_URL"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
            })
            .unwrap_or_else(|| "http://127.0.0.1:9385".to_string());
        let timeout = std::env::var("RAYRAG_SANDBOX_TIMEOUT")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .or_else(|| {
                setting.as_ref().and_then(|config| {
                    config
                        .get("timeout")
                        .or_else(|| config.get("EXECUTOR_MANAGER_TIMEOUT"))
                        .and_then(json_u64)
                })
            })
            .unwrap_or(crate::common::cmd_timeout::seconds());
        Self::with_timeout(&url, timeout)
    }
}

impl SandboxClient {
    pub fn new(base_url: &str) -> Self {
        let timeout_secs = std::env::var("RAYRAG_SANDBOX_TIMEOUT")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(crate::common::cmd_timeout::seconds());
        Self::with_timeout(base_url, timeout_secs)
    }

    fn with_timeout(base_url: &str, timeout_secs: u64) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::builder()
                .timeout(crate::common::cmd_timeout::duration())
                .build()
                .expect("sandbox http client"),
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// 健康检查（对齐上游 GET /healthz）。
    pub async fn health_check(&self) -> bool {
        match self
            .http
            .get(format!("{}/healthz", self.base_url))
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// 调用沙箱执行代码（对齐 executor_manager POST /run 契约）。
    pub async fn run(&self, request: &CodeRequest) -> Result<SandboxResult> {
        // code 需 base64 编码，语言需规范化为 python/nodejs（对齐 CodeExecutionRequest）
        let code_b64 = base64::engine::general_purpose::STANDARD.encode(request.code.as_bytes());
        let payload = json!({
            "code_b64": code_b64,
            "language": normalize_sandbox_language(&request.language),
            "arguments": request.arguments,
        });
        let response = self
            .http
            .post(format!("{}/run", self.base_url))
            .timeout(self.timeout)
            .json(&payload)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    anyhow::anyhow!(
                        "Sandbox execution timed out after {} seconds",
                        self.timeout.as_secs()
                    )
                } else {
                    anyhow::anyhow!("Exception executing code: {error}")
                }
            })?;
        if !response.status().is_success() {
            bail!("Sandbox HTTP {}", response.status());
        }
        let body: Value = response.json().await?;
        Ok(parse_run_response(&body))
    }
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

/// 语言规范化（对齐 SelfManagedProvider._normalize_language：python/python3 → python，
/// javascript/nodejs → nodejs）。
pub fn normalize_sandbox_language(language: &str) -> String {
    let low = language.trim().to_lowercase();
    match low.as_str() {
        "python3" | "py" | "python3.11" | "python3.12" => "python".to_string(),
        "javascript" | "js" | "node" => "nodejs".to_string(),
        other => other.to_string(),
    }
}

/// 解析 /run 响应体（对齐 CodeExecutionResult + result 结构化字段）。
pub fn parse_run_response(body: &Value) -> SandboxResult {
    let mut metadata = Map::new();
    if let Some(result) = body.get("result") {
        metadata.insert(
            "result_present".into(),
            result.get("present").cloned().unwrap_or(Value::Bool(false)),
        );
        metadata.insert(
            "result_value".into(),
            result.get("value").cloned().unwrap_or(Value::Null),
        );
        metadata.insert(
            "result_type".into(),
            result
                .get("type")
                .cloned()
                .unwrap_or_else(|| Value::String("json".into())),
        );
    } else {
        // 缺省 result → 无结构化结果
        metadata.insert("result_present".into(), Value::Bool(false));
        metadata.insert("result_value".into(), Value::Null);
        metadata.insert("result_type".into(), Value::String("json".into()));
    }
    SandboxResult {
        stdout: body
            .get("stdout")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        stderr: body
            .get("stderr")
            .and_then(Value::as_str)
            .map(str::to_string),
        exit_code: body.get("exit_code").and_then(Value::as_i64).unwrap_or(0),
        status: body
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string),
        detail: body
            .get("detail")
            .and_then(Value::as_str)
            .map(str::to_string),
        time_used_ms: body.get("time_used_ms").and_then(Value::as_f64),
        memory_used_kb: body.get("memory_used_kb").and_then(Value::as_f64),
        artifacts: body
            .get("artifacts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        metadata,
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
        bail!("CodeExec contract mismatch: expected type {etype}, got {actual}");
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
                Value::Object(Map::from_iter(sorted))
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
pub fn format_attachment_section(
    key: &str,
    attachment_type: &str,
    name: &str,
    parsed: &str,
) -> String {
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
    let resolved = if result
        .metadata
        .get("result_present")
        .and_then(Value::as_bool)
        == Some(true)
    {
        result
            .metadata
            .get("result_value")
            .cloned()
            .unwrap_or(Value::Null)
    } else {
        serde_json::from_str::<Value>(&result.stdout)
            .unwrap_or_else(|_| Value::String(result.stdout.clone()))
    };

    // stderr-only 且无结果/工件 → 错误（对齐上游分支）
    let has_artifacts = !result.artifacts.is_empty();
    let stdout_blank = result.stdout.trim().is_empty();
    if let Some(stderr) = &result.stderr
        && !stderr.trim().is_empty() && !has_artifacts && stdout_blank {
            outputs.insert("_ERROR".into(), Value::String(stderr.clone()));
            outputs.insert("content".into(), Value::String(String::new()));
            return outputs;
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
    outputs.insert(
        "actual_type".into(),
        Value::String(infer_actual_type(&resolved)),
    );
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
        assert_eq!(
            normalize_expected_type(" array< number > "),
            "Array<Number>"
        );
        assert_eq!(
            normalize_expected_type("Array<Array<String>>"),
            "Array<Array<String>>"
        );
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
        let bad = Map::from_iter([("a".into(), json!(1)), ("b".into(), json!(2))]);
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
            ..Default::default()
        };
        let outputs = process_result(&result);
        assert!(
            outputs
                .get("_ERROR")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("boom")
        );
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
            ..Default::default()
        };
        let outputs = process_result(&result);
        let content = outputs.get("content").unwrap().as_str().unwrap();
        assert!(content.contains("\"price\": 1.5"));
        assert!(content.contains("attachment_count: 1"));
        assert!(content.contains("attachment_1 (csv): chart.csv"));
        assert_eq!(
            outputs.get("actual_type").unwrap().as_str().unwrap(),
            "Object"
        );
    }

    // ── executor_manager 远程客户端契约测试（mock，不发真实网络） ──────────

    #[test]
    fn sandbox_language_normalization_matches_executor_manager() {
        assert_eq!(normalize_sandbox_language("python3"), "python");
        assert_eq!(normalize_sandbox_language("python3.12"), "python");
        assert_eq!(normalize_sandbox_language("javascript"), "nodejs");
        assert_eq!(normalize_sandbox_language("node"), "nodejs");
        assert_eq!(normalize_sandbox_language("bash"), "bash");
    }

    #[test]
    fn run_response_parses_full_executor_manager_contract() {
        let body = json!({
            "status": "success",
            "stdout": "hello\n",
            "stderr": "",
            "exit_code": 0,
            "detail": null,
            "time_used_ms": 123.4,
            "memory_used_kb": 5120.0,
            "artifacts": [ { "name": "a.png", "mime_type": "image/png", "size": 10, "content_b64": "eA==" } ],
            "result": { "present": true, "value": { "answer": 42 }, "type": "json" }
        });
        let result = parse_run_response(&body);
        assert_eq!(result.stdout, "hello\n");
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.status.as_deref(), Some("success"));
        assert_eq!(result.time_used_ms, Some(123.4));
        assert_eq!(result.memory_used_kb, Some(5120.0));
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.metadata["result_present"], json!(true));
        assert_eq!(result.metadata["result_value"]["answer"], json!(42));
        assert_eq!(result.metadata["result_type"], json!("json"));
        // 结构化结果可直接驱动 process_result（select_business_output 挑出 answer）
        let outputs = process_result(&result);
        assert!(outputs["content"].as_str().unwrap().contains("42"));
    }

    #[test]
    fn run_response_defaults_missing_fields() {
        let body = json!({ "stdout": "x" });
        let result = parse_run_response(&body);
        assert_eq!(result.exit_code, 0);
        assert!(result.status.is_none());
        assert!(result.detail.is_none());
        assert!(result.time_used_ms.is_none());
        assert_eq!(
            result
                .metadata
                .get("result_present")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(result.metadata.get("result_value"), Some(&Value::Null));
    }

    #[tokio::test]
    async fn run_sends_code_b64_language_and_arguments() {
        use axum::{Json, Router, routing::post};
        let app = Router::new().route("/run", post(|body: Json<Value>| async move {
            // 请求契约：code_b64 + language + arguments（对齐 CodeExecutionRequest）
            assert!(body["code_b64"].as_str().unwrap().starts_with("ZGVmIG1haW4"));
            assert_eq!(body["language"], "python");
            assert_eq!(body["arguments"]["x"], 1);
            Json(json!({
                "status": "success",
                "stdout": "\n__RAGFLOW_RESULT__:eyJwcmVzZW50Ijp0cnVlLCJ2YWx1ZSI6MTMsInR5cGUiOiJqc29uIn0=",
                "stderr": "",
                "exit_code": 0,
                "result": { "present": true, "value": 13, "type": "json" }
            }))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = SandboxClient::new(&format!("http://{addr}"));
        let result = client
            .run(&CodeRequest {
                language: "python3".into(),
                code: "def main():\n    return 13\n".into(),
                arguments: Map::from_iter([("x".into(), json!(1))]),
            })
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.metadata["result_value"], json!(13));
        assert_eq!(result.status.as_deref(), Some("success"));
    }

    #[tokio::test]
    async fn run_surfaces_http_errors() {
        use axum::{Router, routing::post};
        let app = Router::new().route(
            "/run",
            post(|| async { axum::http::StatusCode::BAD_REQUEST }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = SandboxClient::new(&format!("http://{addr}"));
        let error = client
            .run(&CodeRequest {
                language: "python".into(),
                code: "x".into(),
                arguments: Map::new(),
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Sandbox HTTP 400"), "{error}");
    }

    #[tokio::test]
    async fn health_check_hits_healthz() {
        use axum::{Router, routing::get};
        let app = Router::new().route("/healthz", get(|| async { "{\"status\":\"ok\"}" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = SandboxClient::new(&format!("http://{addr}"));
        assert!(client.health_check().await);
        // 未监听端口 → false
        let dead = SandboxClient::new("http://127.0.0.1:1");
        assert!(!dead.health_check().await);
    }
}
