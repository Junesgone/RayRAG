//! 本地代码沙箱 — RAGFlow `agent/sandbox/providers/local.py` 的 Rust 实现
//!
//! 子进程隔离执行（对齐上游 LocalProvider）：
//!   - Python/JS wrapper：`main(**args)` → `__RAGFLOW_RESULT__:` base64 JSON 结构化结果
//!   - 临时实例目录作 cwd + env 隔离（HOME/TMPDIR/MPLBACKEND/PYTHONUNBUFFERED）
//!   - 超时 kill（进程组）+ 输出大小上限 + 最大内存/CPU 软限制（ulimit 命令）
//!   - 结构化结果提取：从 stdout 移除标记行并解析（对齐 extract_structured_result）

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
use std::time::Duration;
use tokio::process::Command;

/// 结构化结果标记前缀（对齐 RESULT_MARKER_PREFIX）。
pub const RESULT_MARKER_PREFIX: &str = "__RAGFLOW_RESULT__:";

/// 沙箱限制（对齐 LocalProvider 配置项）。
#[derive(Debug, Clone)]
pub struct SandboxLimits {
    /// 单次执行最大秒数。
    pub timeout_seconds: u64,
    /// 最大内存 MB（RLIMIT_AS）。
    pub max_memory_mb: u64,
    /// 最大输出字节。
    pub max_output_bytes: usize,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            timeout_seconds: 10,
            max_memory_mb: 512,
            max_output_bytes: 1 << 20,
        }
    }
}

/// 本地沙箱执行结果。
#[derive(Debug, Clone, Default)]
pub struct SandboxExecution {
    pub stdout: String,
    pub stderr: String,
    /// 提取出的结构化结果（wrapper 输出 `{"present":true,"value":...}`）。
    pub structured: Option<Value>,
}

/// 本地代码沙箱。
#[derive(Debug, Clone)]
pub struct LocalSandbox {
    pub python_bin: String,
    pub node_bin: String,
    pub limits: SandboxLimits,
}

impl Default for LocalSandbox {
    fn default() -> Self {
        Self {
            python_bin: std::env::var("RAYRAG_SANDBOX_PYTHON")
                .unwrap_or_else(|_| "python3".to_string()),
            node_bin: std::env::var("RAYRAG_SANDBOX_NODE").unwrap_or_else(|_| "node".to_string()),
            limits: SandboxLimits::default(),
        }
    }
}

impl LocalSandbox {
    pub fn new(limits: SandboxLimits) -> Self {
        Self {
            python_bin: std::env::var("RAYRAG_SANDBOX_PYTHON").unwrap_or_else(|_| "python3".to_string()),
            node_bin: std::env::var("RAYRAG_SANDBOX_NODE").unwrap_or_else(|_| "node".to_string()),
            limits,
        }
    }

    /// 执行代码（对齐 LocalProvider.execute_code）。
    pub async fn execute(
        &self,
        code: &str,
        language: &str,
        arguments: &Map<String, Value>,
    ) -> Result<SandboxExecution> {
        let normalized = normalize_language(language);
        if self.limits.timeout_seconds == 0 {
            bail!("Execution timeout must be greater than 0 seconds");
        }
        // 实例目录（对齐 instance_dir：临时目录，退出后由调用方清理）
        let instance_dir = tempfile::Builder::new()
            .prefix("rayrag-sandbox-")
            .tempdir()
            .map_err(|error| anyhow::anyhow!("Failed to create instance dir: {error}"))?;
        let dir = instance_dir.path();

        // 写 args.json + wrapper 脚本
        let args_json = serde_json::to_string(arguments)?;
        let (interpreter, script_file) = match normalized.as_str() {
            "python" => {
                let script = build_python_wrapper(code, &args_json);
                let script_path = dir.join("main.py");
                std::fs::write(&script_path, script)?;
                (self.python_bin.clone(), script_path)
            }
            "javascript" | "nodejs" => {
                let script = build_javascript_wrapper(code, &args_json);
                let script_path = dir.join("main.js");
                std::fs::write(&script_path, script)?;
                (self.node_bin.clone(), script_path)
            }
            other => bail!("Unsupported language for local provider: {other}"),
        };

        // 单 bash -c：ulimit 资源限制（对齐 preexec_fn）+ exec 解释器脚本
        let limit_prefix = format!(
            "ulimit -t {} -v {} -f {} -n 64; ",
            self.limits.timeout_seconds + 1,
            self.limits.max_memory_mb * 1024,
            self.limits.max_output_bytes / 1024,
        );
        let shell_cmd = format!(
            "{}exec {} '{}'",
            limit_prefix,
            shell_quote(&interpreter),
            script_file.display()
        );
        let mut wrapped = Command::new("bash");
        wrapped
            .arg("-c")
            .arg(&shell_cmd)
            .current_dir(dir)
            .env("HOME", dir)
            .env("TMPDIR", dir)
            .env("MPLBACKEND", "Agg")
            .env("PYTHONUNBUFFERED", "1")
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // 进程组：超时 kill 整个组（对齐 killpg）
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            wrapped.process_group(0);
        }

        let started = wrapped.spawn().map_err(|error| anyhow::anyhow!("Spawn failed: {error}"))?;
        let timeout = Duration::from_secs(self.limits.timeout_seconds);
        let output = tokio::time::timeout(timeout, started.wait_with_output())
            .await
            .map_err(|_| anyhow::anyhow!("Execution timed out after {} seconds", self.limits.timeout_seconds))?
            .map_err(|error| anyhow::anyhow!("Execution failed: {error}"))?;

        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        validate_output_size(&stdout, &stderr, self.limits.max_output_bytes)?;

        // 结构化结果提取（对齐 extract_structured_result）
        let (cleaned, structured) = extract_structured_result(&stdout);
        stdout = cleaned;

        Ok(SandboxExecution {
            stdout,
            stderr,
            structured,
        })
    }
}

/// 语言规范化（对齐 _normalize_language）。
fn normalize_language(language: &str) -> String {
    let low = language.trim().to_lowercase();
    match low.as_str() {
        "py" | "python3" | "python3.11" | "python3.12" => "python".to_string(),
        "js" | "javascript" | "nodejs" | "node" => "javascript".to_string(),
        other => other.to_string(),
    }
}

/// Python wrapper（对齐 build_python_wrapper）。
fn build_python_wrapper(code: &str, args_json: &str) -> String {
    format!(
        "{code}\n\nif __name__ == \"__main__\":\n    import base64\n    import json\n\n    result = main(**{args_json})\n    payload = json.dumps({{\"present\": True, \"value\": result, \"type\": \"json\"}}, ensure_ascii=False, separators=(\",\", \":\"))\n    print(\"{RESULT_MARKER_PREFIX}\" + base64.b64encode(payload.encode(\"utf-8\")).decode(\"ascii\"))\n"
    )
}

/// JavaScript wrapper（对齐 build_javascript_wrapper）。
fn build_javascript_wrapper(code: &str, args_json: &str) -> String {
    format!(
        "{code}\n\nconst __ragflowArgs = {args_json};\n\n(async () => {{\n  const __ragflowMain = typeof main !== 'undefined' ? main : module.exports && module.exports.main;\n  if (typeof __ragflowMain !== 'function') {{\n    throw new Error('main() must be defined or exported.');\n  }}\n  const output = await Promise.resolve(__ragflowMain(__ragflowArgs));\n  if (typeof output === 'undefined') {{\n    throw new Error('main() must return a value. Use null for an empty result.');\n  }}\n  const payload = JSON.stringify({{ present: true, value: output, type: 'json' }});\n  console.log('{RESULT_MARKER_PREFIX}' + Buffer.from(payload, 'utf8').toString('base64'));\n}})().catch((error) => {{\n  console.error(error);\n  process.exit(1);\n}});\n"
    )
}

/// 从 stdout 提取结构化结果（对齐 extract_structured_result）。
pub fn extract_structured_result(stdout: &str) -> (String, Option<Value>) {
    use base64::Engine;
    let mut cleaned_lines = Vec::new();
    let mut structured = None;
    for line in stdout.lines() {
        if let Some(payload_b64) = line.strip_prefix(RESULT_MARKER_PREFIX) {
            let payload_b64 = payload_b64.trim();
            if payload_b64.is_empty() {
                cleaned_lines.push(line);
                continue;
            }
            match base64::engine::general_purpose::STANDARD
                .decode(payload_b64)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            {
                Some(value) => structured = Some(value),
                None => cleaned_lines.push(line),
            }
            continue;
        }
        cleaned_lines.push(line);
    }
    let mut cleaned = cleaned_lines.join("\n");
    if stdout.ends_with('\n') && !cleaned.is_empty() && !cleaned.ends_with('\n') {
        cleaned.push('\n');
    }
    (cleaned, structured)
}

/// 输出大小校验（对齐 _validate_output_size）。
fn validate_output_size(stdout: &str, stderr: &str, max: usize) -> Result<()> {
    if stdout.len() + stderr.len() > max {
        bail!(
            "Output exceeds the maximum allowed size of {max} bytes (got {})",
            stdout.len() + stderr.len()
        );
    }
    Ok(())
}

/// 辅助：单引号 shell 转义（路径安全）。
fn shell_quote(input: &str) -> String {
    format!("'{}'", input.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn python_executes_main_and_returns_structured_result() {
        let sandbox = LocalSandbox::default();
        let code = "def main(name):\n    return {\"greeting\": f\"hello {name}\"}\n";
        let result = sandbox
            .execute(code, "python3", &Map::from_iter([("name".into(), json!("世界"))]))
            .await
            .unwrap();
        eprintln!("PROBE stdout={:?} stderr={:?}", result.stdout, result.stderr);
        // wrapper 输出被提取
        assert!(!result.stdout.contains(RESULT_MARKER_PREFIX));
        let structured = result.structured.expect("structured result");
        assert_eq!(structured["present"], json!(true));
        assert_eq!(structured["value"]["greeting"], json!("hello 世界"));
    }

    #[tokio::test]
    async fn python_stdout_stays_visible() {
        let sandbox = LocalSandbox::default();
        let code = "def main():\n    print('plain output')\n    return 42\n";
        let result = sandbox.execute(code, "python", &Map::new()).await.unwrap();
        assert!(result.stdout.contains("plain output"));
        assert_eq!(result.structured.unwrap()["value"], json!(42));
    }

    #[tokio::test]
    async fn timeout_kills_hanging_script() {
        let limits = SandboxLimits {
            timeout_seconds: 2,
            ..Default::default()
        };
        let sandbox = LocalSandbox::new(limits);
        let code = "def main():\n    import time\n    time.sleep(30)\n    return 1\n";
        let error = sandbox.execute(code, "python", &Map::new()).await.unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }

    #[tokio::test]
    async fn stderr_captured_on_error() {
        let sandbox = LocalSandbox::default();
        let code = "def main():\n    raise ValueError('boom')\n";
        let result = sandbox.execute(code, "python", &Map::new()).await.unwrap();
        assert!(result.stderr.contains("boom"));
    }

    #[tokio::test]
    async fn unsupported_language_rejected() {
        let sandbox = LocalSandbox::default();
        let error = sandbox
            .execute("code", "ruby", &Map::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("Unsupported language"));
    }

    #[test]
    fn structured_result_extraction_removes_marker_lines() {
        let stdout = "hello\n__RAGFLOW_RESULT__:eyJwcmVzZW50Ijp0cnVlLCJ2YWx1ZSI6MX0=\nworld\n";
        let (cleaned, structured) = extract_structured_result(stdout);
        assert!(!cleaned.contains(RESULT_MARKER_PREFIX));
        assert!(cleaned.contains("hello") && cleaned.contains("world"));
        assert_eq!(structured.unwrap()["value"], json!(1));
    }

    #[test]
    fn language_normalization() {
        assert_eq!(normalize_language("python3.12"), "python");
        assert_eq!(normalize_language("nodejs"), "javascript");
        assert_eq!(normalize_language("bash"), "bash");
    }

    #[test]
    fn output_size_validation() {
        assert!(validate_output_size("a", "b", 10).is_ok());
        assert!(validate_output_size(&"x".repeat(100), "", 10).is_err());
    }
}
